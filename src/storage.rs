//! Pulse-owned persistence. Provider homes remain read-only discovery roots.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use rusqlite::{Connection, OpenFlags};

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

pub fn database_path() -> PathBuf {
    home().join("pulse-analytics.db")
}

/// Isolated `CODEX_HOME` reused by every read-only Codex quota probe.
pub fn codex_probe_home() -> PathBuf {
    home().join("codex").join("probe-home")
}

pub fn initialize() -> Result<()> {
    if !home().is_absolute() {
        bail!("PULSE_HOME must be an absolute path");
    }
    migrate(
        &home(),
        &crate::config::claude_home(),
        &crate::codex::config::codex_home(),
    )?;
    sweep_stale_atomic_temps(&home(), STALE_TEMP_AGE);
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
    for dir in [root.to_path_buf(), root.join("claude"), root.join("codex")] {
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

/// Copy once, never move or overwrite. SQLite creates a consistent snapshot,
/// including committed WAL pages. Keep legacy files available for rollback.
fn migrate(destination: &Path, claude: &Path, codex: &Path) -> Result<()> {
    let existed = destination.exists();
    fs::create_dir_all(destination)?;
    #[cfg(unix)]
    if !existed {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(destination, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = existed;
    for provider in ["claude", "codex"] {
        fs::create_dir_all(destination.join(provider))?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(destination.join("migration.lock"))?;
    lock.lock_exclusive()?;
    let marker = destination.join("legacy-migration-v1.json");
    if marker.exists() {
        return Ok(());
    }
    for name in [
        "pulse-provider.json",
        "pulse-app-settings.json",
        "pulse-opencode.json",
        "pulse-last-app-snapshot.json",
    ] {
        copy_file(&claude.join(name), &destination.join(name), true)?;
    }
    for (root, provider, names) in [
        (
            claude,
            "claude",
            &[
                "discord-presence-config.json",
                "discord-presence-usage-cache.json",
                "discord-presence-metrics.json",
                "discord-presence-metrics.md",
                "cc-discord-presence-debug.log",
            ][..],
        ),
        (
            codex,
            "codex",
            &[
                "discord-presence-config.json",
                "discord-presence-plan-cache.json",
            ][..],
        ),
    ] {
        for name in names {
            copy_file(
                &root.join(name),
                &destination.join(provider).join(name),
                name.ends_with(".json"),
            )?;
        }
    }
    let source = claude.join("pulse-analytics.db");
    let target = destination.join("pulse-analytics.db");
    if source.exists() && !target.exists() {
        let connection = Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .context("cannot read legacy analytics; original files were retained")?;
        connection.busy_timeout(std::time::Duration::from_secs(10))?;
        let temporary = tempfile::NamedTempFile::new_in(destination)?;
        connection.execute(
            "VACUUM INTO ?1",
            [temporary.path().to_str().context("invalid database path")?],
        )?;
        // FTS5 integrity checks need a writable connection to the private copy.
        // The legacy source connection remains read-only.
        let check = Connection::open(temporary.path())?;
        let integrity: String = check.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            bail!("legacy database copy failed integrity check: {integrity}");
        }
        drop(check);
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(&target)
            .context("cannot install analytics copy; original was retained")?;
    }
    let mut receipt = tempfile::NamedTempFile::new_in(destination)?;
    receipt.write_all(b"{\"version\":1,\"legacy_files_retained\":true}\n")?;
    receipt.as_file().sync_all()?;
    receipt.persist_noclobber(marker)?;
    Ok(())
}

fn copy_file(source: &Path, target: &Path, json: bool) -> Result<()> {
    if !source.exists() || target.exists() {
        return Ok(());
    }
    let bytes = fs::read(source).with_context(|| format!("cannot read {}", source.display()))?;
    if json {
        serde_json::from_slice::<serde_json::Value>(&bytes).with_context(|| {
            format!(
                "invalid legacy JSON {}; original retained",
                source.display()
            )
        })?;
    }
    let parent = target.parent().context("missing destination directory")?;
    fs::create_dir_all(parent)?;
    let mut copy = tempfile::NamedTempFile::new_in(parent)?;
    copy.write_all(&bytes)?;
    copy.as_file().sync_all()?;
    if fs::read(copy.path())? != bytes {
        bail!("copy verification failed");
    }
    copy.persist_noclobber(target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_preserves_wal_history_settings_and_provider_sources() {
        let root = tempfile::tempdir().unwrap();
        let claude = root.path().join("legacy");
        let codex = root.path().join("codex");
        let target = root.path().join("pulse");
        fs::create_dir_all(&claude).unwrap();
        fs::create_dir_all(&codex).unwrap();
        fs::write(
            claude.join("pulse-provider.json"),
            r#"{"active_provider":"opencode"}"#,
        )
        .unwrap();
        fs::write(claude.join(".credentials.json"), "private").unwrap();
        fs::write(codex.join("discord-presence-config.json"), "{}").unwrap();
        let source = Connection::open(claude.join("pulse-analytics.db")).unwrap();
        source.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; PRAGMA user_version=6; CREATE TABLE sessions(id TEXT); INSERT INTO sessions VALUES('retained'); CREATE VIRTUAL TABLE sessions_fts USING fts5(id); INSERT INTO sessions_fts VALUES('retained');").unwrap();
        migrate(&target, &claude, &codex).unwrap();
        let copied = Connection::open(target.join("pulse-analytics.db")).unwrap();
        assert_eq!(
            copied
                .query_row("SELECT id FROM sessions", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "retained"
        );
        assert_eq!(
            copied
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            6
        );
        assert!(claude.join("pulse-analytics.db").exists());
        assert!(!target.join(".credentials.json").exists());
        assert!(target.join("codex/discord-presence-config.json").exists());
        fs::write(target.join("pulse-provider.json"), "{}").unwrap();
        migrate(&target, &claude, &codex).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("pulse-provider.json")).unwrap(),
            "{}"
        );
    }

    #[test]
    fn failed_copy_is_retryable_and_never_overwrites_existing_state() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(source.join("pulse-provider.json"), "invalid").unwrap();
        assert!(migrate(&target, &source, &source).is_err());
        assert!(!target.join("legacy-migration-v1.json").exists());
        fs::write(target.join("pulse-provider.json"), "{}").unwrap();
        migrate(&target, &source, &source).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("pulse-provider.json")).unwrap(),
            "{}"
        );
        assert_eq!(
            fs::read_to_string(source.join("pulse-provider.json")).unwrap(),
            "invalid"
        );
    }

    #[test]
    fn sweep_removes_only_old_atomic_temp_files() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("codex")).unwrap();
        let stale = root.path().join(".tmpfaVwJJ");
        let nested = root.path().join("codex").join(".tmpAb12Cd");
        let other = root.path().join(".tmp-not-ours.json");
        let config = root.path().join("pulse-provider.json");
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

    #[test]
    fn corrupt_database_does_not_publish_a_copy_or_receipt() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("legacy");
        let target = root.path().join("pulse");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("pulse-analytics.db"), b"not a database").unwrap();
        assert!(migrate(&target, &source, &source).is_err());
        assert!(!target.join("pulse-analytics.db").exists());
        assert!(!target.join("legacy-migration-v1.json").exists());
        assert_eq!(
            fs::read(source.join("pulse-analytics.db")).unwrap(),
            b"not a database"
        );
    }
}
