use std::fmt;

/// Errors shown to the user. Messages never contain secrets.
#[derive(Debug, Clone, PartialEq)]
pub enum VpnError {
    /// The user pressed Cancel.
    Cancelled,
    /// Waited too long for something (server boot, exit node approval, ...).
    Timeout(String),
    /// Vultr API failed.
    Cloud(String),
    /// Tailscale is missing, not running, or a CLI call failed.
    Mesh(String),
    /// Missing keys or other setup problems.
    Setup(String),
    Other(String),
}

impl fmt::Display for VpnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VpnError::Cancelled => write!(f, "Cancelled."),
            VpnError::Timeout(m)
            | VpnError::Cloud(m)
            | VpnError::Mesh(m)
            | VpnError::Setup(m)
            | VpnError::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for VpnError {}

pub type Result<T> = std::result::Result<T, VpnError>;
