//! cloud-init user data that turns a fresh Ubuntu server into a Tailscale exit node.

use crate::error::{Result, VpnError};
use base64::Engine;

/// Tailscale auth keys look like `tskey-auth-XXXX-YYYY`. Only allow characters that are safe
/// inside a single-quoted shell string.
pub fn validate_auth_key(key: &str) -> Result<()> {
    if key.is_empty() {
        return Err(VpnError::Setup(
            "The Tailscale auth key is not set. Add it in Settings.".into(),
        ));
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(VpnError::Setup(
            "The Tailscale auth key contains unexpected characters. Paste it again in Settings."
                .into(),
        ));
    }
    if !key.starts_with("tskey-") {
        return Err(VpnError::Setup(
            "The Tailscale auth key should start with \"tskey-\". Check it in Settings.".into(),
        ));
    }
    Ok(())
}

fn validate_hostname(hostname: &str) -> Result<()> {
    if hostname.is_empty()
        || !hostname
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(VpnError::Other(format!("Invalid hostname: {hostname}")));
    }
    Ok(())
}

/// The bash script run once on first boot.
pub fn script(auth_key: &str, hostname: &str) -> Result<String> {
    validate_auth_key(auth_key)?;
    validate_hostname(hostname)?;
    Ok(format!(
        r#"#!/bin/bash
set -eux
cat > /etc/sysctl.d/99-tailscale.conf <<'EOF'
net.ipv4.ip_forward = 1
net.ipv6.conf.all.forwarding = 1
EOF
sysctl -p /etc/sysctl.d/99-tailscale.conf
curl -fsSL https://tailscale.com/install.sh | sh
tailscale up --auth-key='{auth_key}' --hostname={hostname} --advertise-exit-node --ssh
if command -v ufw >/dev/null && ufw status | grep -q active; then
  ufw allow in on tailscale0
  ufw allow 41641/udp
  ufw delete allow 22/tcp || ufw delete allow 22 || true
  ufw reload
fi
"#
    ))
}

/// Base64 of [`script`], as the Vultr API expects.
pub fn user_data(auth_key: &str, hostname: &str) -> Result<String> {
    Ok(base64::engine::general_purpose::STANDARD.encode(script(auth_key, hostname)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_script_with_key_and_hostname() {
        let s = script("tskey-auth-abc123-XYZ", "vpn-on-demand-nrt").unwrap();
        assert!(s.contains("--auth-key='tskey-auth-abc123-XYZ'"));
        assert!(s.contains("--hostname=vpn-on-demand-nrt --advertise-exit-node"));
        assert!(s.contains("net.ipv4.ip_forward = 1"));
        assert!(s.contains("ufw delete allow 22/tcp"));
    }

    #[test]
    fn user_data_is_base64_of_script() {
        let ud = user_data("tskey-auth-a", "vpn-on-demand-nrt").unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(ud)
            .unwrap();
        assert!(
            String::from_utf8(decoded)
                .unwrap()
                .starts_with("#!/bin/bash")
        );
    }

    #[test]
    fn rejects_bad_keys() {
        assert!(script("", "h").is_err());
        assert!(script("tskey-auth-a'; rm -rf /", "h").is_err());
        assert!(script("abc", "h").is_err());
        assert!(script("tskey-auth-a", "Bad Host").is_err());
    }
}
