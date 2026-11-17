//! Keys in the OS credential store (Windows Credential Manager, macOS Keychain).

use keyring::{Entry, Error};
use serde::Serialize;

const SERVICE: &str = "vpn-on-demand";
pub const VULTR_API_KEY: &str = "vultr_api_key";
pub const TAILSCALE_AUTH_KEY: &str = "tailscale_auth_key";
pub const NAMES: [&str; 2] = [VULTR_API_KEY, TAILSCALE_AUTH_KEY];

fn entry(name: &str) -> Result<Entry, String> {
    if !NAMES.contains(&name) {
        return Err(format!("Unknown key: {name}"));
    }
    Entry::new(SERVICE, name).map_err(|e| format!("Can't open the credential store: {e}"))
}

/// Never show a whole key. Keep the first 4 and last 2 characters.
pub fn mask(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len());
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 2..].iter().collect();
    format!("{head}{}{tail}", "*".repeat((chars.len() - 6).min(16)))
}

#[derive(Serialize)]
pub struct Saved {
    pub name: String,
    /// Masked value if saved.
    pub masked: Option<String>,
}

pub fn save(name: &str, value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("The key is empty.".into());
    }
    entry(name)?
        .set_password(value)
        .map_err(|e| format!("Can't save the key: {e}"))
}

pub fn get(name: &str) -> Result<Option<String>, String> {
    match entry(name)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Can't read the key: {e}")),
    }
}

pub fn load(name: &str) -> Result<Saved, String> {
    Ok(Saved {
        name: name.to_string(),
        masked: get(name)?.map(|v| mask(&v)),
    })
}

pub fn delete(name: &str) -> Result<(), String> {
    match entry(name)?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Can't remove the key: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::mask;

    #[test]
    fn masks_long_and_short_values() {
        assert_eq!(mask("abcd1234wxyz"), "abcd******yz");
        assert_eq!(mask("short"), "*****");
        assert_eq!(mask(&"x".repeat(60)).len(), 4 + 16 + 2);
    }
}
