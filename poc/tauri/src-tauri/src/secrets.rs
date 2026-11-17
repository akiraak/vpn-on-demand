//! OS の資格情報ストア（Windows の資格情報マネージャー、macOS のキーチェーン）にキーを保存する（試作）。

use keyring::{Entry, Error};
use serde::Serialize;

const SERVICE: &str = "vpn-on-demand-poc";

fn entry(name: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, name).map_err(|e| format!("資格情報ストアを開けません: {e}"))
}

/// 全文は画面に出さない。先頭 4 文字と末尾 2 文字だけを見せる。
fn mask(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len());
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 2..].iter().collect();
    format!("{head}{}{tail}", "*".repeat(chars.len() - 6))
}

#[derive(Serialize)]
pub struct Saved {
    pub name: String,
    /// 保存されていれば伏せた値、無ければ None
    pub masked: Option<String>,
}

pub fn save(name: &str, value: &str) -> Result<(), String> {
    entry(name)?
        .set_password(value)
        .map_err(|e| format!("保存できません: {e}"))
}

pub fn load(name: &str) -> Result<Saved, String> {
    match entry(name)?.get_password() {
        Ok(v) => Ok(Saved {
            name: name.to_string(),
            masked: Some(mask(&v)),
        }),
        Err(Error::NoEntry) => Ok(Saved {
            name: name.to_string(),
            masked: None,
        }),
        Err(e) => Err(format!("読めません: {e}")),
    }
}

pub fn delete(name: &str) -> Result<(), String> {
    match entry(name)?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("消せません: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::mask;

    #[test]
    fn masks_long_and_short_values() {
        assert_eq!(mask("abcd1234wxyz"), "abcd******yz");
        assert_eq!(mask("short"), "*****");
    }
}
