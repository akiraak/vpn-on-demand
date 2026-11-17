//! Tailscale の CLI を呼んで状態を読む（試作。出口ノードは切り替えない）。

use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

/// CLI の場所の候補（CLAUDE.md の「設計の方針」の順）。
fn candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    #[cfg(target_os = "windows")]
    {
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
        // 開発用（WSL から Windows の Tailscale を呼ぶ）
        v.push(PathBuf::from("/usr/bin/tailscale"));
        v.push(PathBuf::from("/mnt/c/Program Files/Tailscale/tailscale.exe"));
    }
    v
}

pub fn find_cli() -> Option<PathBuf> {
    candidates().into_iter().find(|p| p.is_file())
}

fn command(path: &PathBuf) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(path);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[derive(Serialize)]
pub struct Peer {
    pub host_name: String,
    pub os: String,
    pub online: bool,
}

#[derive(Serialize)]
pub struct Status {
    pub cli_path: String,
    pub version: String,
    pub backend_state: String,
    pub tailnet: String,
    pub self_host_name: String,
    pub self_dns_name: String,
    pub self_online: bool,
    /// いま使っている出口ノード（無ければ None）
    pub exit_node: Option<Peer>,
    /// 出口ノードとして選べるピア
    pub exit_node_options: Vec<Peer>,
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
        os: s(v, "OS"),
        online: b(v, "Online"),
    }
}

pub fn status() -> Result<Status, String> {
    let path = find_cli().ok_or_else(|| {
        "Tailscale の CLI が見つかりません。Tailscale のアプリを入れてください。".to_string()
    })?;
    let out = command(&path)
        .args(["status", "--json"])
        .output()
        .map_err(|e| format!("Tailscale の CLI を実行できません: {e}"))?;
    let json: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        format!(
            "tailscale status --json の出力を読めません（終了コード {:?}）: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        )
    })?;

    let self_node = json.get("Self").cloned().unwrap_or(Value::Null);
    let peers: Vec<&Value> = json
        .get("Peer")
        .and_then(Value::as_object)
        .map(|m| m.values().collect())
        .unwrap_or_default();

    Ok(Status {
        cli_path: path.display().to_string(),
        version: s(&json, "Version"),
        backend_state: s(&json, "BackendState"),
        tailnet: json
            .get("CurrentTailnet")
            .map(|t| s(t, "Name"))
            .unwrap_or_default(),
        self_host_name: s(&self_node, "HostName"),
        self_dns_name: s(&self_node, "DNSName"),
        self_online: b(&self_node, "Online"),
        exit_node: peers.iter().find(|p| b(p, "ExitNode")).map(|p| peer(p)),
        exit_node_options: peers
            .iter()
            .filter(|p| b(p, "ExitNodeOption"))
            .map(|p| peer(p))
            .collect(),
    })
}
