//! Non-secret settings, saved as JSON in the app's config folder. Keys are not stored here.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Disconnect automatically after this many hours. `None` = never.
    pub auto_disconnect_hours: Option<u32>,
    /// Last region the user picked (Vultr region id).
    pub last_region: String,
    /// Country of `last_region` (ISO code).
    pub last_country: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_disconnect_hours: Some(4),
            last_region: "nrt".into(),
            last_country: "JP".into(),
        }
    }
}

impl Settings {
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("settings.json")
    }

    pub fn load(config_dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(config_dir))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, config_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(config_dir).map_err(|e| format!("Can't save settings: {e}"))?;
        let json = serde_json::to_string_pretty(self).expect("settings JSON");
        std::fs::write(Self::path(config_dir), json)
            .map_err(|e| format!("Can't save settings: {e}"))
    }

    pub fn normalized(mut self) -> Self {
        self.auto_disconnect_hours = self.auto_disconnect_hours.map(|h| h.clamp(1, 24));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_defaults() {
        let dir = std::env::temp_dir().join(format!("vod-settings-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Settings::load(&dir), Settings::default());
        let s = Settings {
            auto_disconnect_hours: None,
            last_region: "ewr".into(),
            last_country: "US".into(),
        };
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), s);
        std::fs::write(Settings::path(&dir), r#"{"last_region":"fra"}"#).unwrap();
        let l = Settings::load(&dir);
        assert_eq!(l.last_region, "fra");
        assert_eq!(l.auto_disconnect_hours, Some(4));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            Settings {
                auto_disconnect_hours: Some(99),
                ..Default::default()
            }
            .normalized()
            .auto_disconnect_hours,
            Some(24)
        );
    }
}
