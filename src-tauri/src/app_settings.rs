//! Pulse application preferences persisted outside the analytics database.
//!
//! The window-close handler must read `close_to_tray` synchronously (it runs on
//! the UI event thread with no async runtime), so the value is mirrored in an
//! atomic that is loaded once at startup and updated whenever the setting
//! changes. The JSON file next to the other Pulse state
//! (`~/.pulse-analytics/pulse-app-settings.json`, matching `pulse-provider.json`) is the
//! durable copy; adding it here avoids a database schema migration.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    /// When true (default) closing the window hides Pulse to the system tray;
    /// when false the window close quits the app entirely.
    #[serde(default = "default_close_to_tray")]
    pub close_to_tray: bool,
}

fn default_close_to_tray() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            close_to_tray: default_close_to_tray(),
        }
    }
}

/// Synchronous mirror of `close_to_tray` for the window-close hook.
static CLOSE_TO_TRAY: AtomicBool = AtomicBool::new(true);

fn state_path() -> PathBuf {
    cc_discord_presence::storage::config_file("app.json")
}

/// Load persisted settings into the in-memory mirror. Call once at startup,
/// before the window can emit a close event.
pub fn init() {
    CLOSE_TO_TRAY.store(load().close_to_tray, Ordering::Relaxed);
}

pub fn load() -> AppSettings {
    let Ok(raw) = fs::read_to_string(state_path()) else {
        return AppSettings::default();
    };
    serde_json::from_str::<AppSettings>(&raw).unwrap_or_default()
}

/// Synchronous read of the close-to-tray preference for the close handler.
pub fn close_to_tray_enabled() -> bool {
    CLOSE_TO_TRAY.load(Ordering::Relaxed)
}

pub fn set_close_to_tray(enabled: bool) -> Result<AppSettings> {
    CLOSE_TO_TRAY.store(enabled, Ordering::Relaxed);
    let settings = AppSettings {
        close_to_tray: enabled,
    };
    save(&settings)?;
    Ok(settings)
}

fn save(settings: &AppSettings) -> Result<()> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create settings dir {}", parent.display()))?;
    }
    let payload = serde_json::to_string_pretty(settings)?;
    fs::write(&path, payload)
        .with_context(|| format!("failed to write app settings {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_defaults_to_close_to_tray_on() {
        assert!(AppSettings::default().close_to_tray);
    }

    #[test]
    fn deserializes_partial_json_with_default() {
        let parsed: AppSettings = serde_json::from_str("{}").expect("parse empty object");
        assert!(parsed.close_to_tray);
        let off: AppSettings =
            serde_json::from_str(r#"{"close_to_tray":false}"#).expect("parse explicit");
        assert!(!off.close_to_tray);
    }

    #[test]
    fn mirror_reflects_last_store() {
        CLOSE_TO_TRAY.store(false, Ordering::Relaxed);
        assert!(!close_to_tray_enabled());
        CLOSE_TO_TRAY.store(true, Ordering::Relaxed);
        assert!(close_to_tray_enabled());
    }
}
