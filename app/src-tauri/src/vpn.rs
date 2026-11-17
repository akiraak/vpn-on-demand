//! Connect / disconnect flows. Vultr, Tailscale and the public-IP check are behind traits so the
//! flows can be tested with fakes.

use crate::cloud_init;
use crate::error::{Result, VpnError};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Hostnames (and Vultr labels) of the servers this app creates start with this.
pub const HOST_PREFIX: &str = "vpn-on-demand-";
pub const ADMIN_MACHINES_URL: &str = "https://login.tailscale.com/admin/machines";

pub fn hostname_for(region: &str) -> String {
    format!("{HOST_PREFIX}{}", region.to_ascii_lowercase())
}

// ---------- types ----------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Server {
    pub id: String,
    pub label: String,
    pub region: String,
    pub plan: String,
    pub main_ip: String,
    pub status: String,
    /// ISO 8601, as Vultr returns it. Billing starts here.
    pub created: String,
    pub tags: Vec<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Peer {
    pub host_name: String,
    pub dns_name: String,
    pub ips: Vec<String>,
    pub online: bool,
    /// This peer is the exit node in use.
    pub exit_node: bool,
    /// This peer can be used as an exit node (it advertises one and it is approved).
    pub exit_node_option: bool,
}

impl Peer {
    pub fn ipv4(&self) -> Option<&str> {
        self.ips
            .iter()
            .find(|ip| !ip.contains(':'))
            .or(self.ips.first())
            .map(String::as_str)
    }
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct MeshStatus {
    pub backend_state: String,
    pub self_host_name: String,
    pub tailnet: String,
    pub peers: Vec<Peer>,
}

impl MeshStatus {
    pub fn running(&self) -> bool {
        self.backend_state == "Running"
    }

    /// The exit node in use, if any.
    pub fn exit_node(&self) -> Option<&Peer> {
        self.peers.iter().find(|p| p.exit_node)
    }

    /// The exit node in use, if it is one of ours.
    pub fn our_exit_node(&self) -> Option<&Peer> {
        self.exit_node()
            .filter(|p| p.host_name.starts_with(HOST_PREFIX))
    }

    /// The peer for a hostname. A deleted ephemeral node with the same name can linger for a
    /// while, so prefer the one that is online.
    pub fn peer(&self, host_name: &str) -> Option<&Peer> {
        let mut found: Vec<&Peer> = self
            .peers
            .iter()
            .filter(|p| p.host_name == host_name)
            .collect();
        found.sort_by_key(|p| (!p.online, !p.exit_node_option));
        found.into_iter().next()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct IpInfo {
    pub ip: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub org: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Connection {
    pub server: Server,
    pub host_name: String,
    pub hourly_cost: Option<f64>,
    pub public_ip: Option<IpInfo>,
    /// The country the user picked (ISO code), to compare with `public_ip.country`.
    pub expected_country: String,
}

impl Connection {
    pub fn country_matches(&self) -> Option<bool> {
        self.public_ip
            .as_ref()
            .map(|ip| ip.country.eq_ignore_ascii_case(&self.expected_country))
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case", tag = "step")]
pub enum Step {
    Checking,
    RemovingOther { region: String },
    CreatingServer { region: String },
    UsingExisting { region: String },
    WaitingForServer { seconds: u64 },
    WaitingForApproval { host_name: String, seconds: u64 },
    Switching,
    CheckingIp,
    TurningOffExitNode,
    DeletingServer { region: String },
    Verifying,
}

// ---------- traits ----------

pub trait Cloud: Send + Sync {
    fn list_servers(&self) -> impl Future<Output = Result<Vec<Server>>> + Send;
    fn create_server(
        &self,
        region: &str,
        hostname: &str,
        user_data: &str,
    ) -> impl Future<Output = Result<Server>> + Send;
    fn delete_server(&self, id: &str) -> impl Future<Output = Result<()>> + Send;
    fn hourly_cost(&self, plan: &str, region: &str) -> impl Future<Output = Option<f64>> + Send;
}

pub trait Mesh: Send + Sync {
    fn status(&self) -> impl Future<Output = Result<MeshStatus>> + Send;
    /// `None` turns the exit node off.
    fn set_exit_node(&self, ip: Option<&str>) -> impl Future<Output = Result<()>> + Send;
}

pub trait Probe: Send + Sync {
    fn public_ip(&self) -> impl Future<Output = Result<IpInfo>> + Send;
}

/// Public IP address and country through ipinfo.io.
pub struct IpInfoProbe;

impl Probe for IpInfoProbe {
    async fn public_ip(&self) -> Result<IpInfo> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|e| VpnError::Other(e.to_string()))?;
        let res = client
            .get("https://ipinfo.io/json")
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| VpnError::Other(format!("Can't check the public IP address: {e}")))?;
        res.json::<IpInfo>()
            .await
            .map_err(|e| VpnError::Other(format!("Can't check the public IP address: {e}")))
    }
}

// ---------- flows ----------

#[derive(Clone, Debug)]
pub struct Timing {
    pub poll: Duration,
    pub online_timeout: Duration,
    pub approval_timeout: Duration,
    pub retry_delay: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(5),
            online_timeout: Duration::from_secs(10 * 60),
            approval_timeout: Duration::from_secs(15 * 60),
            retry_delay: Duration::from_secs(3),
        }
    }
}

pub struct Vpn<C, M, P> {
    pub cloud: C,
    pub mesh: M,
    pub probe: P,
    pub timing: Timing,
}

pub struct ConnectRequest<'a> {
    pub region: &'a str,
    pub country: &'a str,
    pub auth_key: &'a str,
}

impl<C: Cloud, M: Mesh, P: Probe> Vpn<C, M, P> {
    async fn mesh_running(&self) -> Result<MeshStatus> {
        let st = self.mesh.status().await?;
        if !st.running() {
            return Err(VpnError::Mesh(format!(
                "Tailscale is not connected (state: {}). Open Tailscale and sign in.",
                if st.backend_state.is_empty() {
                    "unknown"
                } else {
                    &st.backend_state
                }
            )));
        }
        Ok(st)
    }

    /// Creates (or reuses) a server in the region, waits until it is an approved exit node,
    /// and switches this PC to it. If anything fails after a server exists, the server is
    /// deleted again so it doesn't keep costing money.
    pub async fn connect(
        &self,
        req: &ConnectRequest<'_>,
        report: &(dyn Fn(Step) + Send + Sync),
        cancel: &AtomicBool,
    ) -> Result<Connection> {
        report(Step::Checking);
        cloud_init::validate_auth_key(req.auth_key)?;
        self.mesh_running().await?;
        check_cancel(cancel)?;
        let servers = self.cloud.list_servers().await?;

        match self.connect_inner(req, servers, report, cancel).await {
            Ok(c) => Ok(c),
            Err(e) => {
                // Don't leave a server running (and don't leave the PC pointed at a dead exit node).
                match self.disconnect(report).await {
                    Ok(()) => Err(e),
                    Err(e2) => Err(VpnError::Other(format!(
                        "{e} Deleting the server also failed: {e2} Press Disconnect to try again."
                    ))),
                }
            }
        }
    }

    async fn connect_inner(
        &self,
        req: &ConnectRequest<'_>,
        servers: Vec<Server>,
        report: &(dyn Fn(Step) + Send + Sync),
        cancel: &AtomicBool,
    ) -> Result<Connection> {
        let host_name = hostname_for(req.region);
        let (others, mut same): (Vec<Server>, Vec<Server>) =
            servers.into_iter().partition(|s| s.region != req.region);

        if !others.is_empty() {
            // Switching countries: stop using the old exit node first, then delete the servers.
            let st = self.mesh.status().await?;
            if st.our_exit_node().is_some() {
                report(Step::TurningOffExitNode);
                self.mesh.set_exit_node(None).await?;
            }
            for s in &others {
                report(Step::RemovingOther {
                    region: s.region.clone(),
                });
                self.cloud.delete_server(&s.id).await?;
            }
        }
        // Extra servers in the same region (shouldn't happen) are deleted too.
        while same.len() > 1 {
            let s = same.pop().unwrap();
            self.cloud.delete_server(&s.id).await?;
        }
        check_cancel(cancel)?;

        let server = match same.pop() {
            Some(s) => {
                report(Step::UsingExisting {
                    region: s.region.clone(),
                });
                s
            }
            None => {
                report(Step::CreatingServer {
                    region: req.region.to_string(),
                });
                let ud = cloud_init::user_data(req.auth_key, &host_name)?;
                self.cloud
                    .create_server(req.region, &host_name, &ud)
                    .await?
            }
        };

        // Wait until the server has joined the tailnet.
        let start = Instant::now();
        loop {
            check_cancel(cancel)?;
            let st = self.mesh_running().await?;
            if st.peer(&host_name).is_some_and(|p| p.online) {
                break;
            }
            if start.elapsed() > self.timing.online_timeout {
                return Err(VpnError::Timeout(format!(
                    "The server didn't join your tailnet within {} minutes. Check that the \
                     Tailscale auth key is valid (reusable, not expired) and try again.",
                    self.timing.online_timeout.as_secs() / 60
                )));
            }
            report(Step::WaitingForServer {
                seconds: start.elapsed().as_secs(),
            });
            tokio::time::sleep(self.timing.poll).await;
        }

        // Wait until it is approved as an exit node.
        let start = Instant::now();
        let peer = loop {
            check_cancel(cancel)?;
            let st = self.mesh_running().await?;
            if let Some(p) = st
                .peer(&host_name)
                .filter(|p| p.online && p.exit_node_option)
            {
                break p.clone();
            }
            if start.elapsed() > self.timing.approval_timeout {
                return Err(VpnError::Timeout(format!(
                    "{host_name} wasn't approved as an exit node within {} minutes. Approve it at \
                     {ADMIN_MACHINES_URL}, or add autoApprovers to your tailnet policy (see \
                     Settings), then connect again.",
                    self.timing.approval_timeout.as_secs() / 60
                )));
            }
            report(Step::WaitingForApproval {
                host_name: host_name.clone(),
                seconds: start.elapsed().as_secs(),
            });
            tokio::time::sleep(self.timing.poll).await;
        };

        check_cancel(cancel)?;
        report(Step::Switching);
        let ip = peer
            .ipv4()
            .ok_or_else(|| VpnError::Mesh(format!("{host_name} has no Tailscale IP address.")))?;
        self.mesh.set_exit_node(Some(ip)).await?;

        report(Step::CheckingIp);
        let public_ip = self.public_ip_with_retry().await;
        let hourly_cost = self.cloud.hourly_cost(&server.plan, &server.region).await;
        Ok(Connection {
            server,
            host_name,
            hourly_cost,
            public_ip,
            expected_country: req.country.to_string(),
        })
    }

    /// The traffic may drop for a few seconds right after switching.
    pub async fn public_ip_with_retry(&self) -> Option<IpInfo> {
        for i in 0..4 {
            if let Ok(ip) = self.probe.public_ip().await {
                return Some(ip);
            }
            if i < 3 {
                tokio::time::sleep(self.timing.retry_delay).await;
            }
        }
        None
    }

    /// Turns off our exit node and deletes every server with the app's tag, then checks that
    /// they are gone. Safe to run any number of times.
    pub async fn disconnect(&self, report: &(dyn Fn(Step) + Send + Sync)) -> Result<()> {
        // If Tailscale isn't running there is no exit node in effect; still delete the servers.
        if let Ok(st) = self.mesh.status().await
            && st.our_exit_node().is_some()
        {
            report(Step::TurningOffExitNode);
            self.mesh.set_exit_node(None).await?;
        }
        for s in self.cloud.list_servers().await? {
            report(Step::DeletingServer {
                region: s.region.clone(),
            });
            self.cloud.delete_server(&s.id).await?;
        }
        report(Step::Verifying);
        for i in 0..3 {
            let left = self.cloud.list_servers().await?;
            if left.is_empty() {
                return Ok(());
            }
            if i == 2 {
                let ids: Vec<&str> = left.iter().map(|s| s.label.as_str()).collect();
                return Err(VpnError::Cloud(format!(
                    "These servers still exist: {}. Press Disconnect again, or delete them on \
                     the Vultr website.",
                    ids.join(", ")
                )));
            }
            tokio::time::sleep(self.timing.retry_delay).await;
        }
        Ok(())
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        Err(VpnError::Cancelled)
    } else {
        Ok(())
    }
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeCloud {
        servers: Mutex<Vec<Server>>,
        created: Mutex<Vec<(String, String)>>,
        deleted: Mutex<Vec<String>>,
        fail_create: bool,
        /// Number of delete calls that fail before deletes succeed.
        fail_deletes: Mutex<u32>,
        next_id: Mutex<u32>,
    }

    fn server(id: &str, region: &str) -> Server {
        Server {
            id: id.into(),
            label: hostname_for(region),
            region: region.into(),
            plan: "vc2-1c-1gb".into(),
            main_ip: "203.0.113.1".into(),
            status: "active".into(),
            created: "2026-11-16T00:00:00+00:00".into(),
            tags: vec!["vpn-on-demand".into()],
        }
    }

    impl Cloud for &FakeCloud {
        async fn list_servers(&self) -> Result<Vec<Server>> {
            Ok(self.servers.lock().unwrap().clone())
        }
        async fn create_server(
            &self,
            region: &str,
            hostname: &str,
            user_data: &str,
        ) -> Result<Server> {
            if self.fail_create {
                return Err(VpnError::Cloud("Out of stock".into()));
            }
            assert!(!user_data.is_empty());
            let mut n = self.next_id.lock().unwrap();
            *n += 1;
            let s = server(&format!("new{n}"), region);
            self.created
                .lock()
                .unwrap()
                .push((region.into(), hostname.into()));
            self.servers.lock().unwrap().push(s.clone());
            Ok(s)
        }
        async fn delete_server(&self, id: &str) -> Result<()> {
            let mut f = self.fail_deletes.lock().unwrap();
            if *f > 0 {
                *f -= 1;
                return Err(VpnError::Cloud("Can't reach the Vultr API".into()));
            }
            self.deleted.lock().unwrap().push(id.into());
            self.servers.lock().unwrap().retain(|s| s.id != id);
            Ok(())
        }
        async fn hourly_cost(&self, _plan: &str, _region: &str) -> Option<f64> {
            Some(0.007)
        }
    }

    /// A tailnet where servers join after `join_after` status calls and get approved after
    /// `approve_after` more (or never).
    struct FakeMesh<'a> {
        cloud: &'a FakeCloud,
        state: Mutex<FakeMeshState>,
        backend: &'static str,
    }

    #[derive(Default)]
    struct FakeMeshState {
        calls: u32,
        join_after: Option<u32>,
        approve_after: Option<u32>,
        exit_ip: Option<String>,
        set_calls: Vec<Option<String>>,
        /// An exit node that isn't ours (the user's own).
        foreign_exit: bool,
    }

    impl<'a> FakeMesh<'a> {
        fn new(cloud: &'a FakeCloud, join_after: Option<u32>, approve_after: Option<u32>) -> Self {
            Self {
                cloud,
                state: Mutex::new(FakeMeshState {
                    join_after,
                    approve_after,
                    ..Default::default()
                }),
                backend: "Running",
            }
        }
    }

    impl Mesh for FakeMesh<'_> {
        async fn status(&self) -> Result<MeshStatus> {
            let mut st = self.state.lock().unwrap();
            st.calls += 1;
            let mut peers = Vec::new();
            for (i, s) in self.cloud.servers.lock().unwrap().iter().enumerate() {
                let ip = format!("100.64.0.{}", i + 10);
                let joined = st.join_after.is_some_and(|n| st.calls > n);
                let approved = joined
                    && st
                        .approve_after
                        .is_some_and(|n| st.calls > st.join_after.unwrap() + n);
                peers.push(Peer {
                    host_name: s.label.clone(),
                    dns_name: format!("{}.ts.net.", s.label),
                    ips: vec![ip.clone(), "fd7a::1".into()],
                    online: joined,
                    exit_node: st.exit_ip.as_deref() == Some(ip.as_str()),
                    exit_node_option: approved,
                });
            }
            if st.foreign_exit {
                peers.push(Peer {
                    host_name: "home-router".into(),
                    online: true,
                    exit_node: true,
                    exit_node_option: true,
                    ..Default::default()
                });
            }
            Ok(MeshStatus {
                backend_state: self.backend.into(),
                peers,
                ..Default::default()
            })
        }
        async fn set_exit_node(&self, ip: Option<&str>) -> Result<()> {
            let mut st = self.state.lock().unwrap();
            st.set_calls.push(ip.map(str::to_string));
            st.exit_ip = ip.map(str::to_string);
            if ip.is_none() {
                st.foreign_exit = false;
            }
            Ok(())
        }
    }

    struct FakeProbe(&'static str);

    impl Probe for FakeProbe {
        async fn public_ip(&self) -> Result<IpInfo> {
            Ok(IpInfo {
                ip: "203.0.113.1".into(),
                country: self.0.into(),
                city: "Tokyo".into(),
                org: "AS20473".into(),
            })
        }
    }

    fn fast() -> Timing {
        Timing {
            poll: Duration::from_millis(1),
            online_timeout: Duration::from_millis(200),
            approval_timeout: Duration::from_millis(200),
            retry_delay: Duration::from_millis(1),
        }
    }

    fn vpn<'a>(
        cloud: &'a FakeCloud,
        mesh: FakeMesh<'a>,
        country: &'static str,
    ) -> Vpn<&'a FakeCloud, FakeMesh<'a>, FakeProbe> {
        Vpn {
            cloud,
            mesh,
            probe: FakeProbe(country),
            timing: fast(),
        }
    }

    const REQ: ConnectRequest = ConnectRequest {
        region: "nrt",
        country: "JP",
        auth_key: "tskey-auth-test",
    };

    fn steps() -> (Mutex<Vec<Step>>, impl Fn(&Mutex<Vec<Step>>, Step)) {
        (Mutex::new(Vec::new()), |v: &Mutex<Vec<Step>>, s| {
            v.lock().unwrap().push(s)
        })
    }

    #[tokio::test]
    async fn connects_by_creating_a_server() {
        let cloud = FakeCloud::default();
        let v = vpn(&cloud, FakeMesh::new(&cloud, Some(2), Some(2)), "JP");
        let (log, push) = steps();
        let c = v
            .connect(&REQ, &|s| push(&log, s), &AtomicBool::new(false))
            .await
            .unwrap();
        assert_eq!(c.host_name, "vpn-on-demand-nrt");
        assert_eq!(c.country_matches(), Some(true));
        assert_eq!(c.hourly_cost, Some(0.007));
        assert_eq!(
            cloud.created.lock().unwrap().as_slice(),
            [("nrt".to_string(), "vpn-on-demand-nrt".to_string())]
        );
        assert_eq!(
            v.mesh.state.lock().unwrap().set_calls,
            [Some("100.64.0.10".to_string())]
        );
        let log = log.lock().unwrap();
        assert!(log.contains(&Step::CreatingServer {
            region: "nrt".into()
        }));
        assert!(
            log.iter()
                .any(|s| matches!(s, Step::WaitingForApproval { .. }))
        );
    }

    #[tokio::test]
    async fn reuses_an_existing_server_in_the_same_region() {
        let cloud = FakeCloud::default();
        cloud.servers.lock().unwrap().push(server("old", "nrt"));
        let v = vpn(&cloud, FakeMesh::new(&cloud, Some(0), Some(0)), "JP");
        let c = v
            .connect(&REQ, &|_| {}, &AtomicBool::new(false))
            .await
            .unwrap();
        assert_eq!(c.server.id, "old");
        assert!(cloud.created.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn switching_country_deletes_the_old_server_first() {
        let cloud = FakeCloud::default();
        cloud.servers.lock().unwrap().push(server("us", "ewr"));
        let mesh = FakeMesh::new(&cloud, Some(0), Some(0));
        mesh.state.lock().unwrap().exit_ip = Some("100.64.0.10".into());
        let v = vpn(&cloud, mesh, "JP");
        v.connect(&REQ, &|_| {}, &AtomicBool::new(false))
            .await
            .unwrap();
        assert_eq!(cloud.deleted.lock().unwrap().as_slice(), ["us"]);
        let st = v.mesh.state.lock().unwrap();
        assert_eq!(
            st.set_calls[0], None,
            "exit node must be turned off before deleting"
        );
        assert_eq!(cloud.servers.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn fails_when_tailscale_is_not_running() {
        let cloud = FakeCloud::default();
        let mut mesh = FakeMesh::new(&cloud, Some(0), Some(0));
        mesh.backend = "NeedsLogin";
        let v = vpn(&cloud, mesh, "JP");
        let e = v
            .connect(&REQ, &|_| {}, &AtomicBool::new(false))
            .await
            .unwrap_err();
        assert!(matches!(e, VpnError::Mesh(_)));
        assert!(e.to_string().contains("NeedsLogin"));
        assert!(cloud.created.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn rejects_a_bad_auth_key_before_creating_anything() {
        let cloud = FakeCloud::default();
        let v = vpn(&cloud, FakeMesh::new(&cloud, Some(0), Some(0)), "JP");
        let req = ConnectRequest {
            auth_key: "",
            ..REQ
        };
        assert!(matches!(
            v.connect(&req, &|_| {}, &AtomicBool::new(false)).await,
            Err(VpnError::Setup(_))
        ));
        assert!(cloud.created.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_failure_is_reported() {
        let cloud = FakeCloud {
            fail_create: true,
            ..Default::default()
        };
        let v = vpn(&cloud, FakeMesh::new(&cloud, Some(0), Some(0)), "JP");
        let e = v
            .connect(&REQ, &|_| {}, &AtomicBool::new(false))
            .await
            .unwrap_err();
        assert_eq!(e, VpnError::Cloud("Out of stock".into()));
    }

    #[tokio::test]
    async fn join_timeout_deletes_the_server() {
        let cloud = FakeCloud::default();
        let v = vpn(&cloud, FakeMesh::new(&cloud, None, None), "JP");
        let e = v
            .connect(&REQ, &|_| {}, &AtomicBool::new(false))
            .await
            .unwrap_err();
        assert!(matches!(e, VpnError::Timeout(ref m) if m.contains("didn't join")));
        assert!(cloud.servers.lock().unwrap().is_empty());
        assert_eq!(cloud.deleted.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn approval_timeout_deletes_the_server() {
        let cloud = FakeCloud::default();
        let v = vpn(&cloud, FakeMesh::new(&cloud, Some(0), None), "JP");
        let e = v
            .connect(&REQ, &|_| {}, &AtomicBool::new(false))
            .await
            .unwrap_err();
        assert!(matches!(e, VpnError::Timeout(ref m) if m.contains("autoApprovers")));
        assert!(cloud.servers.lock().unwrap().is_empty());
        assert!(v.mesh.state.lock().unwrap().set_calls.is_empty());
    }

    #[tokio::test]
    async fn cancel_deletes_the_server() {
        let cloud = FakeCloud::default();
        let v = vpn(&cloud, FakeMesh::new(&cloud, None, None), "JP");
        let cancel = AtomicBool::new(false);
        let push_cancel = |s: Step| {
            if matches!(s, Step::WaitingForServer { .. }) {
                cancel.store(true, Ordering::SeqCst);
            }
        };
        let e = v.connect(&REQ, &push_cancel, &cancel).await.unwrap_err();
        assert_eq!(e, VpnError::Cancelled);
        assert!(cloud.servers.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn warns_when_country_differs() {
        let cloud = FakeCloud::default();
        let v = vpn(&cloud, FakeMesh::new(&cloud, Some(0), Some(0)), "US");
        let c = v
            .connect(&REQ, &|_| {}, &AtomicBool::new(false))
            .await
            .unwrap();
        assert_eq!(c.country_matches(), Some(false));
    }

    #[tokio::test]
    async fn disconnect_turns_off_exit_node_and_deletes() {
        let cloud = FakeCloud::default();
        cloud.servers.lock().unwrap().push(server("a", "nrt"));
        let mesh = FakeMesh::new(&cloud, Some(0), Some(0));
        mesh.state.lock().unwrap().exit_ip = Some("100.64.0.10".into());
        let v = vpn(&cloud, mesh, "JP");
        v.disconnect(&|_| {}).await.unwrap();
        assert!(cloud.servers.lock().unwrap().is_empty());
        assert_eq!(v.mesh.state.lock().unwrap().set_calls, [None]);
    }

    #[tokio::test]
    async fn disconnect_is_idempotent_and_keeps_foreign_exit_node() {
        let cloud = FakeCloud::default();
        let mesh = FakeMesh::new(&cloud, Some(0), Some(0));
        mesh.state.lock().unwrap().foreign_exit = true;
        let v = vpn(&cloud, mesh, "JP");
        v.disconnect(&|_| {}).await.unwrap();
        v.disconnect(&|_| {}).await.unwrap();
        assert!(
            v.mesh.state.lock().unwrap().set_calls.is_empty(),
            "the user's own exit node is left alone"
        );
    }

    #[tokio::test]
    async fn failed_delete_can_be_retried() {
        let cloud = FakeCloud::default();
        cloud.servers.lock().unwrap().push(server("a", "nrt"));
        *cloud.fail_deletes.lock().unwrap() = 1;
        let v = vpn(&cloud, FakeMesh::new(&cloud, Some(0), Some(0)), "JP");
        assert!(v.disconnect(&|_| {}).await.is_err());
        assert_eq!(cloud.servers.lock().unwrap().len(), 1);
        v.disconnect(&|_| {}).await.unwrap();
        assert!(cloud.servers.lock().unwrap().is_empty());
    }

    #[test]
    fn prefers_online_peer_with_same_name() {
        let st = MeshStatus {
            peers: vec![
                Peer {
                    host_name: "vpn-on-demand-nrt".into(),
                    ips: vec!["100.1.1.1".into()],
                    online: false,
                    ..Default::default()
                },
                Peer {
                    host_name: "vpn-on-demand-nrt".into(),
                    ips: vec!["100.2.2.2".into()],
                    online: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            st.peer("vpn-on-demand-nrt").unwrap().ipv4(),
            Some("100.2.2.2")
        );
    }
}
