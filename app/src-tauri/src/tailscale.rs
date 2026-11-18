//! Tailscale through its documented CLI (`tailscale status --json`, `tailscale set`).

use crate::error::{Result, VpnError};
use crate::vpn::{Mesh, MeshStatus, Peer};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

pub const DOWNLOAD_URL: &str = "https://tailscale.com/download";

fn candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    #[cfg(target_os = "windows")]
    {
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            v.push(PathBuf::from(pf).join("Tailscale").join("tailscale.exe"));
        }
        v.push(PathBuf::from(r"C:\Program Files\Tailscale\tailscale.exe"));
    }
    #[cfg(target_os = "macos")]
    {
        v.push(PathBuf::from("/usr/local/bin/tailscale"));
        v.push(PathBuf::from(
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        v.push(PathBuf::from("/usr/bin/tailscale"));
        // Development from WSL: the Windows client.
        v.push(PathBuf::from(
            "/mnt/c/Program Files/Tailscale/tailscale.exe",
        ));
    }
    v
}

pub fn find_cli() -> Option<PathBuf> {
    candidates().into_iter().find(|p| p.is_file())
}

fn not_installed() -> VpnError {
    VpnError::Mesh(format!(
        "Tailscale is not installed. Install it from {DOWNLOAD_URL} and sign in."
    ))
}

fn run(args: &[&str]) -> Result<std::process::Output> {
    let path = find_cli().ok_or_else(not_installed)?;
    #[allow(unused_mut)]
    let mut cmd = Command::new(&path);
    cmd.args(args);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output()
        .map_err(|e| VpnError::Mesh(format!("Can't run the Tailscale CLI: {e}")))
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn b(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn peer(v: &Value) -> Peer {
    Peer {
        host_name: s(v, "HostName"),
        dns_name: s(v, "DNSName"),
        ips: v
            .get("TailscaleIPs")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        online: b(v, "Online"),
        exit_node: b(v, "ExitNode"),
        exit_node_option: b(v, "ExitNodeOption"),
        created: s(v, "Created"),
    }
}

/// Parses `tailscale status --json`.
pub fn parse_status(json: &Value) -> MeshStatus {
    let self_node = json.get("Self").cloned().unwrap_or(Value::Null);
    MeshStatus {
        backend_state: s(json, "BackendState"),
        self_host_name: s(&self_node, "HostName"),
        tailnet: json
            .get("CurrentTailnet")
            .map(|t| s(t, "Name"))
            .unwrap_or_default(),
        peers: json
            .get("Peer")
            .and_then(Value::as_object)
            .map(|m| m.values().map(peer).collect())
            .unwrap_or_default(),
    }
}

pub struct Tailscale;

fn status_blocking() -> Result<MeshStatus> {
    let out = run(&["status", "--json"])?;
    let json: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        VpnError::Mesh(format!(
            "Can't read the Tailscale status. Is Tailscale running? ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    })?;
    Ok(parse_status(&json))
}

fn set_exit_node_blocking(ip: Option<String>) -> Result<()> {
    let out = match &ip {
        Some(ip) => run(&[
            "set",
            &format!("--exit-node={ip}"),
            "--exit-node-allow-lan-access=true",
        ])?,
        None => run(&["set", "--exit-node="])?,
    };
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        let msg = if msg.trim().is_empty() {
            String::from_utf8_lossy(&out.stdout)
        } else {
            msg
        };
        return Err(VpnError::Mesh(format!(
            "Tailscale couldn't {} the exit node: {}",
            if ip.is_some() {
                "switch to"
            } else {
                "turn off"
            },
            msg.trim()
        )));
    }
    Ok(())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| VpnError::Other(format!("Internal error: {e}")))?
}

impl Mesh for Tailscale {
    async fn status(&self) -> Result<MeshStatus> {
        blocking(status_blocking).await
    }

    async fn set_exit_node(&self, ip: Option<&str>) -> Result<()> {
        let ip = ip.map(str::to_string);
        blocking(move || set_exit_node_blocking(ip)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_status() {
        let j = json!({
            "BackendState": "Running",
            "Self": {"HostName": "pc", "Online": true},
            "CurrentTailnet": {"Name": "example.com"},
            "Peer": {
                "k1": {"HostName": "vpn-on-demand-nrt", "DNSName": "vpn-on-demand-nrt.ts.net.",
                       "TailscaleIPs": ["100.64.0.5", "fd7a::5"], "Online": true,
                       "ExitNode": true, "ExitNodeOption": true,
                       "Created": "2026-11-18T03:03:47.761338815Z"}
            }
        });
        let st = parse_status(&j);
        assert_eq!(st.backend_state, "Running");
        assert_eq!(st.self_host_name, "pc");
        assert_eq!(st.tailnet, "example.com");
        assert_eq!(st.peers.len(), 1);
        let p = &st.peers[0];
        assert!(p.online && p.exit_node && p.exit_node_option);
        assert_eq!(p.ips[0], "100.64.0.5");
        assert_eq!(p.created, "2026-11-18T03:03:47.761338815Z");
    }
}
