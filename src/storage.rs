//! Pulse-owned persistence. Every file Pulse writes lives under `PULSE_HOME`
//! (default `~/.pulse-analytics`), grouped by purpose; provider homes stay
//! read-only discovery roots.
//!
//! ```text
//! PULSE_HOME/
//!   config/    settings: app, provider, presence and integration files
//!   data/      the analytics database
//!   accounts/  Pulse-managed account profiles
//!   cache/     derived state, safe to delete
//!   state/     locks and migration receipts
//!   logs/      the capped daemon log
//!   webview/   the embedded browser profile
//! ```
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use fs2::FileExt;

mod migrate;

pub fn home() -> PathBuf {
    std::env::var_os("PULSE_HOME")
        .filter(|value| !value.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .expect("user home is unavailable")
                .join(".pulse-analytics")
        })
}

/// A settings file, for example `config_file("provider.json")`.
pub fn config_file(name: &str) -> PathBuf {
    home().join("config").join(name)
}

/// Derived state that Pulse rebuilds when it is missing.
pub fn cache_path(name: &str) -> PathBuf {
    home().join("cache").join(name)
}

/// Lock files and one-time receipts.
pub fn state_file(name: &str) -> PathBuf {
    home().join("state").join(name)
}

pub fn log_file(name: &str) -> PathBuf {
    home().join("logs").join(name)
}

pub fn database_path() -> PathBuf {
    database_path_in(&home())
}

fn database_path_in(home: &Path) -> PathBuf {
    home.join("data").join("pulse-analytics.db")
}

pub fn accounts_dir() -> PathBuf {
    home().join("accounts")
}

/// The embedded browser profile (local storage and HTTP cache).
pub fn webview_dir() -> PathBuf {
    home().join("webview")
}

/// Isolated `CODEX_HOME` reused by every Codex quota read.
pub fn codex_probe_home() -> PathBuf {
    cache_path("codex-probe")
}

pub fn initialize() -> Result<()> {
    let home = home();
    if !home.is_absolute() {
        bail!("PULSE_HOME must be an absolute path");
    }
    let existed = home.exists();
    for folder in ["config", "data", "cache", "state", "logs"] {
        fs::create_dir_all(home.join(folder))?;
    }
    #[cfg(unix)]
    if !existed {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = existed;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(home.join("state").join("migration.lock"))?;
    lock.lock_exclusive()?;
    migrate::relocate_flat_layout(&home)?;
    migrate::import_legacy(
        &home,
        &crate::config::claude_home(),
        &crate::codex::config::codex_home(),
    )?;
    sweep_stale_atomic_temps(&home, STALE_TEMP_AGE);
    Ok(())
}

const STALE_TEMP_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// `NamedTempFile::new_in` names its file `.tmp` plus six alphanumerics. A
/// crash between create and persist leaves that file behind forever.
fn is_atomic_temp_name(name: &str) -> bool {
    name.strip_prefix(".tmp").is_some_and(|suffix| {
        suffix.len() == 6 && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

fn sweep_stale_atomic_temps(root: &Path, min_age: std::time::Duration) {
    let now = std::time::SystemTime::now();
    let folders = ["", "config", "data", "cache", "state"];
    for dir in folders.map(|folder| root.join(folder)) {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let stale = metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= min_age);
            if metadata.is_file()
                && stale
                && is_atomic_temp_name(&entry.file_name().to_string_lossy())
                && let Err(error) = fs::remove_file(entry.path())
            {
                tracing::warn!(path = %entry.path().display(), %error, "failed to remove stale temp file");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweep_removes_only_old_atomic_temp_files() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("cache")).unwrap();
        let stale = root.path().join(".tmpfaVwJJ");
        let nested = root.path().join("cache").join(".tmpAb12Cd");
        let other = root.path().join(".tmp-not-ours.json");
        let config = root.path().join("config.json");
        let dir = root.path().join(".tmpDir123");
        for path in [&stale, &nested, &other, &config] {
            fs::write(path, b"x").unwrap();
        }
        fs::create_dir(&dir).unwrap();

        sweep_stale_atomic_temps(root.path(), std::time::Duration::from_secs(3600));
        assert!(
            stale.exists(),
            "a fresh temp may belong to an in-flight write"
        );

        sweep_stale_atomic_temps(root.path(), std::time::Duration::ZERO);
        assert!(!stale.exists());
        assert!(!nested.exists());
        assert!(other.exists());
        assert!(config.exists());
        assert!(dir.exists());
    }
}
