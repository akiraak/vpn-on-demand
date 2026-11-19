//! Development tool: measures where the time goes while connecting.
//!
//! Runs the app's own connect flow (`Vpn::connect`) against the real Vultr and Tailscale, records
//! a timeline, reads timing files from inside the server, then disconnects and checks that no
//! server is left. **This creates a server (billed) and switches this PC's exit node.**
//!
//! ```bash
//! set -a; . ../../.env; set +a      # VULTR_API_KEY, TAILSCALE_AUTH_KEY
//! cargo run --example measure -- nrt JP [--out DIR] [--os "Debian 13 x64 (trixie)"] [--plan vhp-1c-1gb-amd]
//! cargo run --example measure -- --print-user-data   # show the script, create nothing
//! ```
//!
//! Only for measuring: the user data gets extra lines that write timestamps to
//! `/var/lib/vod-measure/` and serve that directory on the server's Tailscale IP (port 8099)
//! after cloud-init has finished. Auth keys are redacted from everything that is saved.

use base64::Engine;
use serde_json::{Value, json};
use std::mem::Discriminant;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use vpn_on_demand_lib::error::Result;
use vpn_on_demand_lib::tailscale::Tailscale;
use vpn_on_demand_lib::vpn::{
    self, Cloud, ConnectRequest, IpInfoProbe, Mesh, MeshStatus, Server, Step, Timing, Vpn,
};
use vpn_on_demand_lib::vultr::Vultr;

const VM_DIR: &str = "/var/lib/vod-measure";
const VM_PORT: u16 = 8099;
const VM_FILES: &[&str] = &[
    "marks",
    "install.log",
    "cloud-init-output.txt",
    "analyze.txt",
    "analyze-blame.txt",
    "systemd.txt",
    "apt-history.txt",
    "journal.txt",
];

// ---------- timeline ----------

struct Rec {
    t0: Instant,
    t0_epoch: f64,
    events: Mutex<Vec<(f64, String, String)>>,
}

fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

impl Rec {
    fn new() -> Self {
        Self {
            t0: Instant::now(),
            t0_epoch: epoch_now(),
            events: Mutex::new(Vec::new()),
        }
    }

    fn add(&self, source: &str, what: impl Into<String>) {
        self.add_at(self.t0.elapsed().as_secs_f64(), source, what);
    }

    /// An event at a wall-clock time (seconds since the Unix epoch).
    fn add_epoch(&self, epoch: f64, source: &str, what: impl Into<String>) {
        self.add_at(epoch - self.t0_epoch, source, what);
    }

    fn add_at(&self, t: f64, source: &str, what: impl Into<String>) {
        let what = redact(&what.into());
        println!("[{t:>7.1}s] {source:<9} {what}");
        self.events
            .lock()
            .unwrap()
            .push((t, source.to_string(), what));
    }

    fn sorted(&self) -> Vec<(f64, String, String)> {
        let mut v = self.events.lock().unwrap().clone();
        v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        v
    }
}

/// Replaces `tskey-...` with `tskey-REDACTED`.
fn redact(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("tskey-") {
        out.push_str(&rest[..i]);
        out.push_str("tskey-REDACTED");
        let tail = &rest[i + "tskey-".len()..];
        let end = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Seconds since the Unix epoch of a UTC RFC 3339 time (`...Z` or `...+00:00`).
fn parse_utc(t: &str) -> Option<f64> {
    let n = |r: std::ops::Range<usize>| t.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, s) = (
        n(0..4)?,
        n(5..7)?,
        n(8..10)?,
        n(11..13)?,
        n(14..16)?,
        n(17..19)?,
    );
    let mut frac = 0.0;
    if t.as_bytes().get(19) == Some(&b'.') {
        let digits: String = t[20..].chars().take_while(char::is_ascii_digit).collect();
        frac = format!("0.{digits}").parse().unwrap_or(0.0);
    }
    // Days from civil (Howard Hinnant).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + h * 3600 + mi * 60 + s) as f64 + frac)
}

// ---------- user data with timing marks ----------

/// Adds timestamp marks to the app's user data script. Works on any bash script whose first
/// lines are `#!/bin/bash` and `set -eux`; known lines get extra marks around them: the line
/// that installs Tailscale (it mentions the official installer, possibly as a fallback) and
/// `tailscale up`.
fn instrument(script: &str) -> String {
    let mut out = String::new();
    let mut started = false;
    for line in script.lines() {
        if !line.starts_with('#') && line.contains("https://tailscale.com/install.sh | sh") {
            out.push_str("vod_mark install-start\nset +x -o pipefail\n");
            out.push_str(&format!(
                "{{ {line}; }} 2>&1 | while IFS= read -r l; do printf '%s %s\\n' \"$(date +%T.%3N)\" \"$l\"; done | tee {VM_DIR}/install.log\n"
            ));
            out.push_str("set -x +o pipefail\nvod_mark install-done\n");
            continue;
        }
        if line.starts_with("tailscale up ") {
            out.push_str(&format!(
                "vod_mark tailscale-up-start\n{line}\nvod_mark tailscale-up-done\n"
            ));
            continue;
        }
        out.push_str(line);
        out.push('\n');
        // Only the script's own first one (heredocs inside may have their own).
        if line == "set -eux" && !started {
            started = true;
            out.push_str(&format!(
                r#"mkdir -p {VM_DIR}
vod_mark() {{ echo "$(date +%s.%3N) $1" >> {VM_DIR}/marks; }}
vod_mark script-start
awk -v now="$(date +%s.%N)" '{{printf "%.3f kernel-boot\n", now-$1}}' /proc/uptime >> {VM_DIR}/marks
PS4='+ $(date +%T.%3N) '
"#
            ));
        }
    }
    out.push_str(&format!(
        r#"vod_mark script-done
cat > {VM_DIR}/collect.sh <<'VODEOF'
#!/bin/bash
d={VM_DIR}
cloud-init status --wait >/dev/null 2>&1 || true
echo "$(date +%s.%3N) cloud-init-done" >> $d/marks
cloud-init analyze show > $d/analyze.txt 2>&1 || true
cloud-init analyze blame > $d/analyze-blame.txt 2>&1 || true
{{ systemd-analyze; echo; systemd-analyze critical-chain; echo; systemd-analyze blame | head -40; }} > $d/systemd.txt 2>&1 || true
cp /var/log/apt/history.log $d/apt-history.txt 2>/dev/null || true
journalctl -b -o short-iso-precise --no-pager -u apt-daily.service -u apt-daily-upgrade.service -u unattended-upgrades.service -u tailscaled.service > $d/journal.txt 2>&1 || true
cp /var/log/cloud-init-output.log $d/cloud-init-output.txt
sed -i -E 's/tskey-[A-Za-z0-9_-]+/tskey-REDACTED/g' $d/*.txt $d/*.log $d/marks
touch $d/done
VODEOF
chmod +x {VM_DIR}/collect.sh
vod_ip=$(tailscale ip -4 | head -n1)
setsid -f bash -c "{VM_DIR}/collect.sh; cd {VM_DIR} && exec python3 -m http.server {VM_PORT} --bind $vod_ip" >/dev/null 2>&1 </dev/null
"#
    ));
    out
}

// ---------- Vultr wrapper ----------

struct MeasuringCloud {
    inner: Arc<Vultr>,
    rec: Arc<Rec>,
    server: Arc<Mutex<Option<Server>>>,
}

impl Cloud for MeasuringCloud {
    async fn list_servers(&self) -> Result<Vec<Server>> {
        self.rec.add("api", "Vultr list_servers start");
        let r = self.inner.list_servers().await;
        self.rec
            .add("api", format!("Vultr list_servers done (ok={})", r.is_ok()));
        r
    }

    async fn create_server(&self, region: &str, hostname: &str, user_data: &str) -> Result<Server> {
        let b64 = base64::engine::general_purpose::STANDARD;
        let script = String::from_utf8(b64.decode(user_data).expect("base64")).expect("utf-8");
        let ud = b64.encode(instrument(&script));
        self.rec.add(
            "app",
            "create: reading plans/availability/OS, then POST /instances",
        );
        let s = self.inner.create_server(region, hostname, &ud).await?;
        self.rec.add(
            "app",
            format!("create: API returned (plan {}, id {})", s.plan, s.id),
        );
        if let Some(t) = parse_utc(&s.created) {
            self.rec.add_epoch(t, "vultr", "date_created");
        }
        *self.server.lock().unwrap() = Some(s.clone());
        Ok(s)
    }

    async fn delete_server(&self, id: &str) -> Result<()> {
        self.rec.add("app", "delete: DELETE /instances");
        self.inner.delete_server(id).await
    }

    async fn hourly_cost(&self, plan: &str, region: &str) -> Option<f64> {
        self.inner.hourly_cost(plan, region).await
    }
}

// ---------- Tailscale wrapper ----------

struct MeasuringMesh {
    rec: Arc<Rec>,
}

impl Mesh for MeasuringMesh {
    async fn status(&self) -> Result<MeshStatus> {
        Tailscale.status().await
    }

    async fn set_exit_node(&self, ip: Option<&str>) -> Result<()> {
        let what = if ip.is_some() { "on" } else { "off" };
        self.rec
            .add("api", format!("tailscale set exit node {what} start"));
        let r = Tailscale.set_exit_node(ip).await;
        self.rec.add(
            "api",
            format!("tailscale set exit node {what} done (ok={})", r.is_ok()),
        );
        r
    }
}

// ---------- polling Vultr and Tailscale in parallel ----------

async fn watch(
    vultr: Arc<Vultr>,
    rec: Arc<Rec>,
    server: Arc<Mutex<Option<Server>>>,
    host_name: String,
    stop: Arc<AtomicBool>,
) {
    let mut last_vultr = String::new();
    let mut last_peer = String::new();
    let mut peer_created_seen = false;
    while !stop.load(Ordering::SeqCst) {
        let s = server.lock().unwrap().clone();
        if let Some(s) = s {
            if let Ok(Some(i)) = vultr.instance(&s.id).await {
                let f = |k: &str| i.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                let now = format!(
                    "status={} power={} server_status={} ip={}",
                    f("status"),
                    f("power_status"),
                    f("server_status"),
                    if f("main_ip") == "0.0.0.0" {
                        "none"
                    } else {
                        "assigned"
                    }
                );
                if now != last_vultr {
                    rec.add("vultr", now.clone());
                    last_vultr = now;
                }
            }
            if let Ok(st) = Tailscale.status().await
                && let Some(p) = st.peer(&host_name, &s.created)
            {
                let now = format!(
                    "peer seen online={} exit_node_option={}",
                    p.online, p.exit_node_option
                );
                if now != last_peer {
                    rec.add("tailscale", now.clone());
                    last_peer = now;
                }
                if !peer_created_seen && let Some(t) = parse_utc(&p.created) {
                    rec.add_epoch(t, "tailscale", "peer Created (joined the tailnet)");
                    peer_created_seen = true;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

// ---------- reading files from the server ----------

async fn http_get(url: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    match client.get(url).send().await {
        Ok(r) if r.status().is_success() => r.text().await.ok(),
        Ok(_) => None,
        // From WSL the tailnet route may be missing; fall back to Windows' curl.exe.
        Err(_) => {
            let out = std::process::Command::new("/mnt/c/Windows/System32/curl.exe")
                .args(["-sf", "-m", "10", url])
                .current_dir("/mnt/c")
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
        }
    }
}

async fn fetch_vm_files(ip: &str, dir: &Path, rec: &Rec, cancel: &AtomicBool) {
    let base = format!("http://{ip}:{VM_PORT}");
    let start = Instant::now();
    loop {
        if http_get(&format!("{base}/done")).await.is_some() {
            break;
        }
        if start.elapsed() > Duration::from_secs(180) || cancel.load(Ordering::SeqCst) {
            rec.add("measure", "gave up waiting for the server's timing files");
            return;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    for f in VM_FILES {
        let Some(body) = http_get(&format!("{base}/{f}")).await else {
            continue;
        };
        let body = redact(&body);
        std::fs::write(dir.join(format!("vm-{f}")), &body).ok();
        if *f == "marks" {
            for line in body.lines() {
                if let Some((t, what)) = line.split_once(' ')
                    && let Ok(t) = t.parse::<f64>()
                {
                    rec.add_epoch(t, "vm", what);
                }
            }
        }
    }
}

// ---------- main ----------

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        eprintln!("{name} is not set (put it in .env and `set -a; . ../../.env; set +a`).");
        std::process::exit(2);
    })
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut pos = Vec::new();
    let mut out_root = PathBuf::from("measure-out");
    let mut os: Option<String> = None;
    let mut plan: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--out" {
            out_root = PathBuf::from(it.next().expect("--out DIR"));
        } else if a == "--os" {
            os = Some(it.next().expect("--os NAME").clone());
        } else if a == "--plan" {
            plan = Some(it.next().expect("--plan ID").clone());
        } else {
            pos.push(a.clone());
        }
    }
    if args.iter().any(|a| a == "--print-user-data") {
        // For checking the script (with a dummy key); creates nothing.
        let s =
            vpn_on_demand_lib::cloud_init::script("tskey-auth-dummy", "vpn-on-demand-nrt").unwrap();
        print!("{}", instrument(&s));
        return;
    }
    let region = pos.first().cloned().unwrap_or_else(|| "nrt".into());
    let country = pos.get(1).cloned().unwrap_or_else(|| "JP".into());
    let host_name = vpn::hostname_for(&region);

    let mut vultr = Vultr::new(env("VULTR_API_KEY")).expect("Vultr key");
    if let Some(os) = &os {
        vultr = vultr.with_os(os);
    }
    if let Some(plan) = &plan {
        vultr = vultr.with_plan(plan);
    }
    let vultr = Arc::new(vultr);
    let auth_key = env("TAILSCALE_AUTH_KEY");

    // Never interfere with a real connection.
    match vultr.list_servers().await {
        Ok(s) if s.is_empty() => {}
        Ok(s) => {
            eprintln!(
                "{} server(s) with the app's tag already exist. Disconnect first.",
                s.len()
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Vultr: {e}");
            std::process::exit(1);
        }
    }

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let dir = out_root.join(format!("{region}-{stamp}"));
    std::fs::create_dir_all(&dir).expect("output dir");

    let rec = Arc::new(Rec::new());
    let server = Arc::new(Mutex::new(None));
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("\nCancelling (the server will be deleted)...");
                cancel.store(true, Ordering::SeqCst);
            }
        });
    }

    let v = Vpn {
        cloud: MeasuringCloud {
            inner: vultr.clone(),
            rec: rec.clone(),
            server: server.clone(),
        },
        mesh: MeasuringMesh { rec: rec.clone() },
        probe: IpInfoProbe,
        timing: Timing::default(),
    };

    let last_step: Mutex<Option<Discriminant<Step>>> = Mutex::new(None);
    let report_rec = rec.clone();
    let report = move |step: Step| {
        let d = std::mem::discriminant(&step);
        let mut last = last_step.lock().unwrap();
        if *last != Some(d) {
            *last = Some(d);
            report_rec.add("step", format!("{step:?}"));
        }
    };

    let stop = Arc::new(AtomicBool::new(false));
    let watcher = tokio::spawn(watch(
        vultr.clone(),
        rec.clone(),
        server.clone(),
        host_name.clone(),
        stop.clone(),
    ));

    rec.add(
        "app",
        format!(
            "Connect pressed ({region}, os={}, plan={})",
            os.as_deref().unwrap_or("default"),
            plan.as_deref().unwrap_or("default")
        ),
    );
    let req = ConnectRequest {
        region: &region,
        country: &country,
        auth_key: &auth_key,
    };
    let result = v.connect(&req, &report, &cancel).await;
    let connected_at = rec.t0.elapsed().as_secs_f64();
    match &result {
        Ok(c) => rec.add(
            "app",
            format!(
                "Connected ({}) country={}",
                c.host_name,
                c.public_ip
                    .as_ref()
                    .map(|i| i.country.as_str())
                    .unwrap_or("?")
            ),
        ),
        Err(e) => rec.add("app", format!("connect failed: {e}")),
    }
    stop.store(true, Ordering::SeqCst);
    watcher.await.ok();

    if result.is_ok() {
        let created = server.lock().unwrap().as_ref().map(|s| s.created.clone());
        let ip = match (Tailscale.status().await, created) {
            (Ok(st), Some(c)) => st
                .peer(&host_name, &c)
                .and_then(|p| p.ipv4().map(str::to_string)),
            _ => None,
        };
        if let Some(ip) = ip {
            fetch_vm_files(&ip, &dir, &rec, &cancel).await;
        }
    }

    let dis_start = rec.t0.elapsed().as_secs_f64();
    rec.add("app", "Disconnect");
    let dis = v.disconnect(&report).await;
    let dis_secs = rec.t0.elapsed().as_secs_f64() - dis_start;
    match &dis {
        Ok(()) => rec.add("app", format!("Disconnected ({dis_secs:.1}s)")),
        Err(e) => rec.add("app", format!("disconnect failed: {e}")),
    }
    let left = vultr.list_servers().await;
    let left_msg = match &left {
        Ok(s) if s.is_empty() => "no server left".to_string(),
        Ok(s) => format!("WARNING: {} server(s) still exist!", s.len()),
        Err(e) => format!("WARNING: can't check: {e}"),
    };
    rec.add("check", left_msg.clone());

    let events = rec.sorted();
    let timeline: String = events
        .iter()
        .map(|(t, s, w)| format!("{t:>7.1}  {s:<9} {w}\n"))
        .collect();
    std::fs::write(dir.join("timeline.txt"), &timeline).ok();
    let summary = json!({
        "region": region,
        "os": os,
        "plan": plan,
        "ok": result.is_ok(),
        "connect_seconds": connected_at,
        "disconnect_seconds": dis_secs,
        "left": left_msg,
        "events": events.iter().map(|(t, s, w)| json!({"t": t, "source": s, "what": w})).collect::<Vec<_>>(),
    });
    std::fs::write(
        dir.join("events.json"),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .ok();

    println!("\n===== timeline (seconds from Connect) =====\n{timeline}");
    println!("connect: {connected_at:.1}s, disconnect: {dis_secs:.1}s, {left_msg}");
    println!("saved to {}", dir.display());
    if !matches!(left, Ok(ref s) if s.is_empty()) || dis.is_err() {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_keys() {
        assert_eq!(
            redact("--auth-key='tskey-auth-abc_1-XYZ' x tskey-b"),
            "--auth-key='tskey-REDACTED' x tskey-REDACTED"
        );
    }

    #[test]
    fn parses_utc_times() {
        assert_eq!(parse_utc("1970-01-01T00:00:10Z"), Some(10.0));
        assert_eq!(
            parse_utc("2026-11-18T03:03:47+00:00"),
            Some(1_794_971_027.0)
        );
        let t = parse_utc("2026-11-18T03:03:47.5Z").unwrap();
        assert!((t - 1_794_971_027.5).abs() < 1e-6);
    }

    #[test]
    fn instruments_the_app_script() {
        let s = vpn_on_demand_lib::cloud_init::script("tskey-auth-a", "vpn-on-demand-nrt").unwrap();
        let i = instrument(&s);
        assert!(i.contains("vod_mark script-start"));
        assert!(i.contains("vod_mark install-start"));
        assert!(i.contains("vod_mark tailscale-up-done"));
        assert!(i.contains("python3 -m http.server 8099"));
        assert!(i.starts_with("#!/bin/bash\nset -eux\nmkdir -p"));
    }
}
