//! Development tool: measures how fast a Fly.io Machine can become this PC's Tailscale exit node.
//!
//! Creates a Machine running Tailscale's official image in a region (default nrt), then times:
//! 1. creating a new Machine,
//! 2. starting a stopped Machine (its root filesystem is reset, so Tailscale joins as a new
//!    node each time),
//! 3. starting a suspended Machine (memory is restored, so the same Tailscale node resumes).
//!
//! Each cycle: start → Tailscale peer online and usable as exit node → switch this PC to it →
//! public IP confirmed through it (ipinfo.io). Also records the IP's country/org and a download
//! speed. At the end the Machine is destroyed and the app is checked for leftovers.
//! **This runs (billed) Machines and switches this PC's exit node.**
//!
//! ```bash
//! set -a; . ../../.env; set +a      # FLY_API_TOKEN, TAILSCALE_AUTH_KEY
//! cargo run --example measure_fly -- --app <unique-app-name> [--region nrt] [--cycles 3]
//! cargo run --example measure_fly -- --app <name> --up     # connect and leave it running
//! cargo run --example measure_fly -- --app <name> --down   # exit node off, destroy all machines
//! ```

use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use vpn_on_demand_lib::tailscale::Tailscale;
use vpn_on_demand_lib::vpn::{IpInfoProbe, Mesh, Peer, Probe};

const API: &str = "https://api.machines.dev/v1";
const IMAGE: &str = "tailscale/tailscale:stable";
const SPEED_URL: &str = "https://speed.cloudflare.com/__down?bytes=50000000";

static CANCEL: AtomicBool = AtomicBool::new(false);

// ---------- timeline ----------

struct Rec {
    t0: Instant,
    events: Mutex<Vec<(f64, String, String)>>,
}

impl Rec {
    fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    fn add(&self, source: &str, what: impl Into<String>) {
        let t = self.now();
        let what = redact(&what.into());
        println!("[{t:>7.1}s] {source:<9} {what}");
        self.events
            .lock()
            .unwrap()
            .push((t, source.to_string(), what));
    }
}

/// Replaces `tskey-...` and `fm2_...` with a placeholder.
fn redact(s: &str) -> String {
    let mut out = s.to_string();
    for marker in ["tskey-", "fm2_"] {
        let mut res = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(i) = rest.find(marker) {
            res.push_str(&rest[..i]);
            res.push_str(marker);
            res.push_str("REDACTED");
            let tail = &rest[i + marker.len()..];
            let end = tail
                .find(|c: char| !(c.is_ascii_alphanumeric() || "-_+/=".contains(c)))
                .unwrap_or(tail.len());
            rest = &tail[end..];
        }
        res.push_str(rest);
        out = res;
    }
    out
}

fn epoch_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// `YYYY-MM-DDTHH:MM:SSZ` for seconds since the Unix epoch.
fn fmt_utc(t: u64) -> String {
    let days = (t / 86_400) as i64;
    let rem = t % 86_400;
    // Civil from days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn check_cancel() -> Result<(), String> {
    if CANCEL.load(Ordering::SeqCst) {
        Err("cancelled".into())
    } else {
        Ok(())
    }
}

// ---------- Fly Machines API ----------

struct Fly {
    http: reqwest::Client,
    auth: String,
    app: String,
}

impl Fly {
    fn new(token: &str, app: &str) -> Self {
        let token = token.trim();
        // Tokens from `fly tokens create` / the dashboard start with "FlyV1 " and are sent as is.
        let auth = if token.starts_with("FlyV1 ") {
            token.to_string()
        } else {
            format!("Bearer {token}")
        };
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(90))
                .connect_timeout(Duration::from_secs(5))
                .pool_max_idle_per_host(0)
                .build()
                .unwrap(),
            auth,
            app: app.to_string(),
        }
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value), String> {
        let mut last = String::new();
        // Retry while the network is unreachable (right after switching the exit node).
        for attempt in 0..8u64 {
            let mut req = self
                .http
                .request(method.clone(), format!("{API}{path}"))
                .header("Authorization", &self.auth);
            if let Some(b) = &body {
                req = req.json(b);
            }
            match req.send().await {
                Ok(res) => {
                    let status = res.status().as_u16();
                    let text = res.text().await.unwrap_or_default();
                    let v = serde_json::from_str(&text).unwrap_or(Value::String(text));
                    return Ok((status, v));
                }
                Err(e) if e.is_connect() || e.is_timeout() => {
                    last = e.to_string();
                    tokio::time::sleep(Duration::from_secs(attempt.min(3) + 1)).await;
                }
                Err(e) => return Err(format!("Fly API: {e}")),
            }
        }
        Err(format!("Fly API unreachable: {last}"))
    }

    async fn ok(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let (status, v) = self.call(method, path, body).await?;
        if (200..300).contains(&status) {
            Ok(v)
        } else {
            Err(format!(
                "Fly API {path}: HTTP {status}: {}",
                redact(&v.to_string())
            ))
        }
    }

    async fn ensure_app(&self, org: &str) -> Result<(), String> {
        let (status, _) = self
            .call(reqwest::Method::GET, &format!("/apps/{}", self.app), None)
            .await?;
        if status == 200 {
            return Ok(());
        }
        self.ok(
            reqwest::Method::POST,
            "/apps",
            Some(json!({"app_name": self.app, "org_slug": org})),
        )
        .await
        .map(|_| ())
    }

    async fn machines(&self) -> Result<Vec<Value>, String> {
        let v = self
            .ok(
                reqwest::Method::GET,
                &format!("/apps/{}/machines", self.app),
                None,
            )
            .await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    async fn create(
        &self,
        region: &str,
        host_name: &str,
        auth_key: &str,
    ) -> Result<String, String> {
        let body = json!({
            "name": host_name,
            "region": region,
            "config": {
                "image": IMAGE,
                "env": {
                    "TS_AUTHKEY": auth_key,
                    "TS_HOSTNAME": host_name,
                    "TS_EXTRA_ARGS": "--advertise-exit-node",
                    "TS_USERSPACE": "false",
                    "TS_ACCEPT_DNS": "false",
                },
                // Exit nodes need IP forwarding; then hand over to the image's own entrypoint.
                "init": {"exec": [
                    "/bin/sh", "-c",
                    "sysctl -w net.ipv4.ip_forward=1 net.ipv6.conf.all.forwarding=1; exec /usr/local/bin/containerboot"
                ]},
                "guest": {"cpu_kind": "shared", "cpus": 1, "memory_mb": 256},
                "restart": {"policy": "no"},
                "auto_destroy": false,
            },
        });
        let v = self
            .ok(
                reqwest::Method::POST,
                &format!("/apps/{}/machines", self.app),
                Some(body),
            )
            .await?;
        v.get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("no machine id in {v}"))
    }

    async fn action(&self, id: &str, action: &str) -> Result<Value, String> {
        self.ok(
            reqwest::Method::POST,
            &format!("/apps/{}/machines/{id}/{action}", self.app),
            Some(json!({})),
        )
        .await
    }

    async fn get(&self, id: &str) -> Result<Value, String> {
        self.ok(
            reqwest::Method::GET,
            &format!("/apps/{}/machines/{id}", self.app),
            None,
        )
        .await
    }

    async fn wait(&self, id: &str, state: &str) -> Result<(), String> {
        let mut path = format!(
            "/apps/{}/machines/{id}/wait?state={state}&timeout=60",
            self.app
        );
        if state != "started" {
            let m = self.get(id).await?;
            if let Some(i) = m.get("instance_id").and_then(Value::as_str) {
                path.push_str(&format!("&instance_id={i}"));
            }
        }
        self.ok(reqwest::Method::GET, &path, None).await.map(|_| ())
    }

    async fn destroy(&self, id: &str) -> Result<(), String> {
        let (status, v) = self
            .call(
                reqwest::Method::DELETE,
                &format!("/apps/{}/machines/{id}?force=true", self.app),
                None,
            )
            .await?;
        if (200..300).contains(&status) || status == 404 {
            Ok(())
        } else {
            Err(format!("destroy: HTTP {status}: {v}"))
        }
    }
}

// ---------- Tailscale side ----------

/// Waits until the peer (joined at or after `since`) is online and usable as an exit node.
async fn wait_peer(host: &str, since: &str, rec: &Rec) -> Result<Peer, String> {
    let start = Instant::now();
    let mut seen = false;
    loop {
        check_cancel()?;
        if let Ok(st) = Tailscale.status().await
            && let Some(p) = st.peer(host, since)
        {
            if !seen {
                rec.add("tailscale", format!("peer seen (online={})", p.online));
                seen = true;
            }
            if p.online && p.exit_node_option {
                return Ok(p.clone());
            }
        }
        if start.elapsed() > Duration::from_secs(300) {
            return Err("the peer didn't come online within 5 minutes".into());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Switches to the peer and waits until the public IP can be read through it.
async fn switch_and_check(peer: &Peer, rec: &Rec) -> Result<Value, String> {
    let ip = peer.ipv4().ok_or("peer has no IP")?;
    Tailscale
        .set_exit_node(Some(ip))
        .await
        .map_err(|e| e.to_string())?;
    rec.add("tailscale", "exit node on");
    let start = Instant::now();
    loop {
        check_cancel()?;
        if let Ok(info) = IpInfoProbe.public_ip().await {
            rec.add(
                "ipinfo",
                format!("{} {} ({})", info.country, info.city, info.org),
            );
            return Ok(
                json!({"ip": info.ip, "country": info.country, "city": info.city, "org": info.org}),
            );
        }
        if start.elapsed() > Duration::from_secs(60) {
            return Err("the public IP couldn't be read through the exit node within 60 s".into());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn exit_off(rec: &Rec) {
    match Tailscale.set_exit_node(None).await {
        Ok(()) => rec.add("tailscale", "exit node off"),
        Err(e) => rec.add("tailscale", format!("exit node off FAILED: {e}")),
    }
}

async fn speed_test(rec: &Rec) -> Option<f64> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .ok()?;
    let start = Instant::now();
    let bytes = client
        .get(SPEED_URL)
        .send()
        .await
        .ok()?
        .bytes()
        .await
        .ok()?;
    let secs = start.elapsed().as_secs_f64();
    let mbps = bytes.len() as f64 * 8.0 / secs / 1e6;
    rec.add(
        "speed",
        format!(
            "{} MB in {secs:.1}s = {mbps:.0} Mbit/s",
            bytes.len() / 1_000_000
        ),
    );
    Some(mbps)
}

// ---------- cycles ----------

#[derive(Default, Clone)]
struct Cycle {
    name: String,
    api: f64,   // API call → Machine started
    join: f64,  // started → peer online and approved as exit node
    route: f64, // switch → public IP read through it
    total: f64,
    ip: Value,
}

async fn connect_cycle(
    name: &str,
    rec: &Rec,
    begin: impl std::future::Future<Output = Result<(), String>>,
    host: &str,
    since: &str,
) -> Result<Cycle, String> {
    let t0 = rec.now();
    rec.add("cycle", format!("== {name}"));
    begin.await?;
    let t_started = rec.now();
    rec.add("fly", "machine started");
    let peer = wait_peer(host, since, rec).await?;
    let t_peer = rec.now();
    rec.add("tailscale", "peer online and usable as exit node");
    let ip = switch_and_check(&peer, rec).await?;
    let t_ip = rec.now();
    Ok(Cycle {
        name: name.to_string(),
        api: t_started - t0,
        join: t_peer - t_started,
        route: t_ip - t_peer,
        total: t_ip - t0,
        ip,
    })
}

struct Ctx<'a> {
    fly: &'a Fly,
    rec: &'a Rec,
    region: String,
    host: String,
    auth_key: String,
    cycles: usize,
}

async fn run(
    ctx: &Ctx<'_>,
    id_slot: &Mutex<Option<String>>,
    out: &mut Vec<Cycle>,
) -> Result<Option<f64>, String> {
    let (fly, rec, host) = (ctx.fly, ctx.rec, ctx.host.as_str());

    // 1. Create.
    let since = fmt_utc(epoch_now() - 5);
    let c = connect_cycle(
        "create",
        rec,
        async {
            let id = fly.create(&ctx.region, host, &ctx.auth_key).await?;
            rec.add("fly", format!("machine created ({id})"));
            *id_slot.lock().unwrap() = Some(id.clone());
            fly.wait(&id, "started").await
        },
        host,
        &since,
    )
    .await?;
    out.push(c);
    let speed = speed_test(rec).await;
    let id = id_slot.lock().unwrap().clone().unwrap();

    // 2. Stop / start: a fresh boot, Tailscale joins as a new node each time.
    for i in 1..=ctx.cycles {
        exit_off(rec).await;
        fly.action(&id, "stop").await?;
        fly.wait(&id, "stopped").await?;
        rec.add("fly", "machine stopped");
        check_cancel()?;
        let since = fmt_utc(epoch_now() - 5);
        let c = connect_cycle(
            &format!("stop/start {i}"),
            rec,
            async {
                let r = fly.action(&id, "start").await?;
                rec.add("fly", format!("start: {r}"));
                fly.wait(&id, "started").await
            },
            host,
            &since,
        )
        .await?;
        out.push(c);
    }

    // 3. Suspend / start: memory is restored, the same node resumes.
    let since = fmt_utc(epoch_now() - 600);
    for i in 1..=ctx.cycles {
        exit_off(rec).await;
        fly.action(&id, "suspend").await?;
        fly.wait(&id, "suspended").await?;
        rec.add("fly", "machine suspended; waiting 30 s");
        tokio::time::sleep(Duration::from_secs(30)).await;
        check_cancel()?;
        let c = connect_cycle(
            &format!("suspend/start {i}"),
            rec,
            async {
                let r = fly.action(&id, "start").await?;
                rec.add("fly", format!("start: {r}"));
                fly.wait(&id, "started").await
            },
            host,
            &since,
        )
        .await?;
        out.push(c);
    }
    Ok(speed)
}

/// `--up`: creates a Machine, switches this PC to it and leaves it running (for trying streaming
/// sites by hand). Undo with `--down`.
async fn up(fly: &Fly, region: &str, auth_key: &str) -> i32 {
    let rec = Rec {
        t0: Instant::now(),
        events: Mutex::new(Vec::new()),
    };
    let host = format!("vpn-on-demand-fly-{region}");
    let id_slot = Mutex::new(None);
    let since = fmt_utc(epoch_now() - 5);
    let r = connect_cycle(
        "up",
        &rec,
        async {
            let id = fly.create(region, &host, auth_key).await?;
            rec.add("fly", format!("machine created ({id})"));
            *id_slot.lock().unwrap() = Some(id.clone());
            fly.wait(&id, "started").await
        },
        &host,
        &since,
    )
    .await;
    match r {
        Ok(c) => {
            println!(
                "Connected in {:.1}s. This PC now uses {host} as its exit node. Run --down when done.",
                c.total
            );
            0
        }
        Err(e) => {
            rec.add("measure", format!("FAILED: {e}"));
            down(fly).await;
            1
        }
    }
}

/// `--down`: turns the exit node off and destroys every Machine in the app.
async fn down(fly: &Fly) -> i32 {
    let rec = Rec {
        t0: Instant::now(),
        events: Mutex::new(Vec::new()),
    };
    exit_off(&rec).await;
    let machines = match fly.machines().await {
        Ok(m) => m,
        Err(e) => {
            rec.add("fly", format!("can't list machines: {e}"));
            return 1;
        }
    };
    for m in &machines {
        if let Some(id) = m.get("id").and_then(Value::as_str) {
            match fly.destroy(id).await {
                Ok(()) => rec.add("fly", format!("machine destroyed ({id})")),
                Err(e) => rec.add("fly", format!("destroy FAILED: {e}")),
            }
        }
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    match fly.machines().await {
        Ok(m)
            if m.iter()
                .all(|m| m.get("state").and_then(Value::as_str) == Some("destroyed")) =>
        {
            rec.add("check", "no machine left");
            0
        }
        Ok(m) => {
            rec.add(
                "check",
                format!("WARNING: {} machine(s) still exist!", m.len()),
            );
            1
        }
        Err(e) => {
            rec.add("check", format!("WARNING: can't check: {e}"));
            1
        }
    }
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        eprintln!("{name} is not set (put it in .env and `set -a; . ../../.env; set +a`).");
        std::process::exit(2);
    })
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
    };
    let Some(app) = arg("--app") else {
        eprintln!(
            "usage: measure_fly --app <unique-app-name> [--region nrt] [--cycles 3] [--org personal]"
        );
        std::process::exit(2);
    };
    let region = arg("--region").unwrap_or_else(|| "nrt".into());
    let cycles: usize = arg("--cycles").and_then(|c| c.parse().ok()).unwrap_or(3);
    let org = arg("--org").unwrap_or_else(|| "personal".into());
    let fly = Fly::new(&env("FLY_API_TOKEN"), &app);
    let auth_key = env("TAILSCALE_AUTH_KEY");

    if args.iter().any(|a| a == "--down") {
        std::process::exit(down(&fly).await);
    }
    if let Err(e) = fly.ensure_app(&org).await {
        eprintln!("{e}");
        std::process::exit(1);
    }
    match fly.machines().await {
        Ok(m) if m.is_empty() => {}
        Ok(m) => {
            eprintln!(
                "The app already has {} machine(s). Destroy them first.",
                m.len()
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
    if args.iter().any(|a| a == "--up") {
        std::process::exit(up(&fly, &region, &auth_key).await);
    }
    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\nCancelling (the machine will be destroyed)...");
            CANCEL.store(true, Ordering::SeqCst);
        }
    });

    let rec = Rec {
        t0: Instant::now(),
        events: Mutex::new(Vec::new()),
    };
    let ctx = Ctx {
        fly: &fly,
        rec: &rec,
        host: format!("vpn-on-demand-fly-{region}"),
        region,
        auth_key,
        cycles,
    };
    let id_slot = Mutex::new(None);
    let mut cycles_out = Vec::new();
    let result = run(&ctx, &id_slot, &mut cycles_out).await;
    if let Err(e) = &result {
        rec.add("measure", format!("FAILED: {e}"));
    }

    // Always clean up.
    exit_off(&rec).await;
    let id = id_slot.lock().unwrap().clone();
    if let Some(id) = id {
        match fly.destroy(&id).await {
            Ok(()) => rec.add("fly", "machine destroyed"),
            Err(e) => rec.add("fly", format!("destroy FAILED: {e}")),
        }
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    let left = fly.machines().await;
    let left_msg = match &left {
        Ok(m)
            if m.iter()
                .all(|m| m.get("state").and_then(Value::as_str) == Some("destroyed")) =>
        {
            "no machine left".to_string()
        }
        Ok(m) => format!("WARNING: {} machine(s) still exist!", m.len()),
        Err(e) => format!("WARNING: can't check: {e}"),
    };
    rec.add("check", left_msg.clone());

    println!("\n===== cycles (seconds) =====");
    println!(
        "{:<16}{:>8}{:>8}{:>8}{:>8}  ip",
        "cycle", "api", "join", "route", "total"
    );
    for c in &cycles_out {
        println!(
            "{:<16}{:>8.1}{:>8.1}{:>8.1}{:>8.1}  {} {}",
            c.name,
            c.api,
            c.join,
            c.route,
            c.total,
            c.ip.get("country").and_then(Value::as_str).unwrap_or("?"),
            c.ip.get("org").and_then(Value::as_str).unwrap_or("?"),
        );
    }
    println!("{left_msg}");

    let dir = PathBuf::from("measure-out").join(format!("fly-{}-{}", ctx.region, epoch_now()));
    std::fs::create_dir_all(&dir).ok();
    let events = rec.events.lock().unwrap().clone();
    let summary = json!({
        "region": ctx.region,
        "ok": result.is_ok(),
        "error": result.as_ref().err(),
        "speed_mbps": result.as_ref().ok().cloned().flatten(),
        "left": left_msg,
        "cycles": cycles_out.iter().map(|c| json!({
            "name": c.name, "api": c.api, "join": c.join, "route": c.route, "total": c.total, "ip": c.ip,
        })).collect::<Vec<_>>(),
        "events": events.iter().map(|(t, s, w)| json!({"t": t, "source": s, "what": w})).collect::<Vec<_>>(),
    });
    std::fs::write(
        dir.join("result.json"),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .ok();
    println!("saved to {}", dir.display());
    if !left_msg.starts_with("no machine") {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc() {
        assert_eq!(fmt_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(fmt_utc(1_794_971_027), "2026-11-18T03:03:47Z");
        assert_eq!(fmt_utc(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn redacts_tokens() {
        assert_eq!(
            redact("FlyV1 fm2_abc+/=,x tskey-auth-1"),
            "FlyV1 fm2_REDACTED,x tskey-REDACTED"
        );
    }
}
