//! Moves older Pulse layouts into the current one: the flat 1.9 layout under
//! `PULSE_HOME`, and the files that releases before 1.9 kept in `~/.claude`.
use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags};

/// Flat 1.9 file and its place in the current layout, both relative to `PULSE_HOME`.
const RELOCATIONS: &[(&str, &str)] = &[
    ("pulse-provider.json", "config/provider.json"),
    ("pulse-app-settings.json", "config/app.json"),
    ("pulse-opencode.json", "config/opencode.json"),
    ("pulse-orion.json", "config/orion.json"),
    ("pulse-commandcode.json", "config/commandcode.json"),
    (
        "claude/discord-presence-config.json",
        "config/claude-presence.json",
    ),
    (
        "codex/discord-presence-config.json",
        "config/codex-presence.json",
    ),
    ("pulse-last-app-snapshot.json", "cache/app-snapshot.json"),
    (
        "claude/discord-presence-usage-cache.json",
        "cache/claude-usage.json",
    ),
    (
        "codex/discord-presence-plan-cache.json",
        "cache/codex-plan.json",
    ),
    ("claude/cc-discord-presence-debug.log", "logs/daemon.log"),
    ("legacy-migration-v1.json", "state/legacy-migration-v1.json"),
];

/// Flat 1.9 leftovers with no successor: lock files, rebuilt caches and the
/// metrics files nothing reads.
const OBSOLETE: &[&str] = &[
    "migration.lock",
    "pulse-commandcode.lock",
    "pulse-orion.lock",
    "pulse-opencode.lock",
    "claude/discord-presence-metrics.json",
    "claude/discord-presence-metrics.md",
    "claude/discord-presence-metrics.json.tmp",
    "claude/discord-presence-metrics.md.tmp",
    "claude/cc-discord-presence-debug.log.old",
    "claude/session-checkpoints",
    "codex/probe-home",
];

/// Settings that releases before 1.9 kept in `~/.claude`, and their place now.
const LEGACY_CLAUDE: &[(&str, &str)] = &[
    ("pulse-provider.json", "config/provider.json"),
    ("pulse-app-settings.json", "config/app.json"),
    ("pulse-opencode.json", "config/opencode.json"),
    (
        "discord-presence-config.json",
        "config/claude-presence.json",
    ),
];

/// Pulse-owned files in `~/.claude` removed once the import succeeded.
const LEGACY_CLAUDE_LEFTOVERS: &[&str] = &[
    "pulse-last-app-snapshot.json",
    "discord-presence-usage-cache.json",
    "discord-presence-metrics.json",
    "discord-presence-metrics.md",
    "cc-discord-presence-debug.log",
    "cc-discord-presence-debug.log.old",
    "cc-discord-presence.lock",
    "cc-discord-presence.instance.json",
    "pulse-session-checkpoints",
];

const LEGACY_RECEIPT: &str = "state/legacy-migration-v1.json";

/// How long the import waits for a legacy writer still holding the database.
const LEGACY_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub(super) fn relocate_flat_layout(home: &Path) -> Result<()> {
    for (old, new) in RELOCATIONS {
        move_file(&home.join(old), &home.join(new))?;
    }
    relocate_database(home)?;
    for name in OBSOLETE {
        remove_path(&home.join(name));
    }
    for dir in ["claude", "codex"] {
        // Only succeeds when empty; anything unknown stays for the user.
        let _ = fs::remove_dir(home.join(dir));
    }
    Ok(())
}

/// The current layout wins: an old copy next to a newer file is a stale duplicate.
fn move_file(old: &Path, new: &Path) -> Result<()> {
    if !old.is_file() {
        return Ok(());
    }
    if new.exists() {
        fs::remove_file(old)?;
        return Ok(());
    }
    fs::create_dir_all(new.parent().context("missing destination directory")?)?;
    fs::rename(old, new).with_context(|| format!("cannot move {}", old.display()))
}

/// Folds the WAL into the main file first, so one rename moves every committed page.
fn relocate_database(home: &Path) -> Result<()> {
    let old = home.join("pulse-analytics.db");
    let new = crate::storage::database_path_in(home);
    if !old.is_file() {
        return Ok(());
    }
    if new.exists() {
        tracing::warn!(path = %old.display(), "kept an older analytics database next to the current one");
        return Ok(());
    }
    {
        let connection =
            Connection::open(&old).with_context(|| format!("cannot open {}", old.display()))?;
        let busy: i64 =
            connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
        if busy != 0 {
            bail!("the analytics database is in use; close every other Pulse window and retry");
        }
        let mode: String =
            connection.query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("delete") {
            bail!("the analytics database is in use; close every other Pulse window and retry");
        }
    }
    fs::create_dir_all(new.parent().context("missing database directory")?)?;
    fs::rename(&old, &new).with_context(|| format!("cannot move {}", old.display()))?;
    for suffix in ["-wal", "-shm"] {
        remove_path(&home.join(format!("pulse-analytics.db{suffix}")));
    }
    Ok(())
}

fn remove_path(path: &Path) {
    let result = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    if let Err(error) = result
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), %error, "failed to remove an obsolete Pulse file");
    }
}

/// Imports settings and history from releases before 1.9, then removes the
/// Pulse files they left in `~/.claude`. The Codex presence config belongs to
/// Codex Discord Rich Presence as well, so it is copied and never removed.
pub(super) fn import_legacy(home: &Path, claude: &Path, codex: &Path) -> Result<()> {
    let receipt = home.join(LEGACY_RECEIPT);
    if receipt.exists() {
        return Ok(());
    }
    for (name, target) in LEGACY_CLAUDE {
        copy_file(&claude.join(name), &home.join(target))?;
    }
    copy_file(
        &codex.join("discord-presence-config.json"),
        &home.join("config/codex-presence.json"),
    )?;
    import_legacy_database(
        &claude.join("pulse-analytics.db"),
        &crate::storage::database_path_in(home),
    )?;
    for name in LEGACY_CLAUDE
        .iter()
        .map(|(name, _)| *name)
        .chain(LEGACY_CLAUDE_LEFTOVERS.iter().copied())
    {
        remove_path(&claude.join(name));
    }
    for suffix in ["", "-wal", "-shm"] {
        remove_path(&claude.join(format!("pulse-analytics.db{suffix}")));
    }
    let parent = receipt.parent().context("missing state directory")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(b"{\"version\":1,\"legacy_files_retained\":false}\n")?;
    file.as_file().sync_all()?;
    file.persist_noclobber(receipt)?;
    Ok(())
}

/// SQLite writes a consistent snapshot, including committed WAL pages.
fn import_legacy_database(source: &Path, target: &Path) -> Result<()> {
    if !source.exists() || target.exists() {
        return Ok(());
    }
    let connection = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("cannot read legacy analytics; original files were retained")?;
    connection.busy_timeout(LEGACY_BUSY_TIMEOUT)?;
    let parent = target.parent().context("missing database directory")?;
    fs::create_dir_all(parent)?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    connection.execute(
        "VACUUM INTO ?1",
        [temporary.path().to_str().context("invalid database path")?],
    )?;
    // FTS5 integrity checks need a writable connection to the private copy.
    let check = Connection::open(temporary.path())?;
    let integrity: String = check.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        bail!("legacy database copy failed integrity check: {integrity}");
    }
    drop(check);
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(target)
        .context("cannot install analytics copy; original was retained")?;
    Ok(())
}

fn copy_file(source: &Path, target: &Path) -> Result<()> {
    if !source.exists() || target.exists() {
        return Ok(());
    }
    let bytes = fs::read(source).with_context(|| format!("cannot read {}", source.display()))?;
    serde_json::from_slice::<serde_json::Value>(&bytes).with_context(|| {
        format!(
            "invalid legacy JSON {}; original retained",
            source.display()
        )
    })?;
    let parent = target.parent().context("missing destination directory")?;
    fs::create_dir_all(parent)?;
    let mut copy = tempfile::NamedTempFile::new_in(parent)?;
    copy.write_all(&bytes)?;
    copy.as_file().sync_all()?;
    copy.persist_noclobber(target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_database(path: &Path) {
        let source = Connection::open(path).unwrap();
        source.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; PRAGMA user_version=6; CREATE TABLE sessions(id TEXT); INSERT INTO sessions VALUES('retained'); CREATE VIRTUAL TABLE sessions_fts USING fts5(id); INSERT INTO sessions_fts VALUES('retained');").unwrap();
    }

    fn session_ids(path: &Path) -> Vec<String> {
        let connection = Connection::open(path).unwrap();
        let mut statement = connection.prepare("SELECT id FROM sessions").unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn legacy_import_copies_history_and_settings_then_removes_pulse_files_from_claude() {
        let root = tempfile::tempdir().unwrap();
        let claude = root.path().join("claude-home");
        let codex = root.path().join("codex-home");
        let home = root.path().join("pulse");
        fs::create_dir_all(&claude).unwrap();
        fs::create_dir_all(&codex).unwrap();
        fs::write(
            claude.join("pulse-provider.json"),
            r#"{"active_provider":"opencode"}"#,
        )
        .unwrap();
        fs::write(claude.join("cc-discord-presence.lock"), "").unwrap();
        fs::write(claude.join(".credentials.json"), "private").unwrap();
        fs::write(claude.join("discord-presence-data.json"), "{}").unwrap();
        fs::write(codex.join("discord-presence-config.json"), "{}").unwrap();
        legacy_database(&claude.join("pulse-analytics.db"));

        import_legacy(&home, &claude, &codex).unwrap();

        assert_eq!(
            session_ids(&home.join("data/pulse-analytics.db")),
            ["retained"]
        );
        assert_eq!(
            fs::read_to_string(home.join("config/provider.json")).unwrap(),
            r#"{"active_provider":"opencode"}"#
        );
        assert!(home.join("config/codex-presence.json").exists());
        for gone in [
            "pulse-provider.json",
            "cc-discord-presence.lock",
            "pulse-analytics.db",
            "pulse-analytics.db-wal",
        ] {
            assert!(
                !claude.join(gone).exists(),
                "{gone} stayed in the Claude home"
            );
        }
        // Claude's own files and the statusline input are never touched.
        assert!(claude.join(".credentials.json").exists());
        assert!(claude.join("discord-presence-data.json").exists());
        assert!(codex.join("discord-presence-config.json").exists());
        assert!(!home.join(".credentials.json").exists());
    }

    #[test]
    fn legacy_import_never_overwrites_current_settings_and_runs_once() {
        let root = tempfile::tempdir().unwrap();
        let claude = root.path().join("claude-home");
        let home = root.path().join("pulse");
        fs::create_dir_all(&claude).unwrap();
        fs::create_dir_all(home.join("config")).unwrap();
        fs::write(home.join("config/provider.json"), "{}").unwrap();
        fs::write(
            claude.join("pulse-provider.json"),
            r#"{"active_provider":"codex"}"#,
        )
        .unwrap();

        import_legacy(&home, &claude, &claude).unwrap();
        assert_eq!(
            fs::read_to_string(home.join("config/provider.json")).unwrap(),
            "{}"
        );

        fs::write(claude.join("pulse-provider.json"), "{}").unwrap();
        import_legacy(&home, &claude, &claude).unwrap();
        assert!(
            claude.join("pulse-provider.json").exists(),
            "the receipt stops a second import"
        );
    }

    #[test]
    fn invalid_legacy_settings_stop_the_import_and_keep_the_original() {
        let root = tempfile::tempdir().unwrap();
        let claude = root.path().join("claude-home");
        let home = root.path().join("pulse");
        fs::create_dir_all(&claude).unwrap();
        fs::write(claude.join("pulse-provider.json"), "invalid").unwrap();

        assert!(import_legacy(&home, &claude, &claude).is_err());
        assert!(!home.join(LEGACY_RECEIPT).exists());
        assert_eq!(
            fs::read_to_string(claude.join("pulse-provider.json")).unwrap(),
            "invalid"
        );
    }

    #[test]
    fn corrupt_legacy_database_publishes_no_copy_and_keeps_the_original() {
        let root = tempfile::tempdir().unwrap();
        let claude = root.path().join("claude-home");
        let home = root.path().join("pulse");
        fs::create_dir(&claude).unwrap();
        fs::write(claude.join("pulse-analytics.db"), b"not a database").unwrap();

        assert!(import_legacy(&home, &claude, &claude).is_err());
        assert!(!home.join("data/pulse-analytics.db").exists());
        assert!(!home.join(LEGACY_RECEIPT).exists());
        assert_eq!(
            fs::read(claude.join("pulse-analytics.db")).unwrap(),
            b"not a database"
        );
    }

    #[test]
    fn flat_layout_moves_into_purpose_folders_and_leaves_no_provider_folders() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        fs::create_dir_all(home.join("claude/session-checkpoints/v3")).unwrap();
        fs::create_dir_all(home.join("codex/probe-home")).unwrap();
        fs::write(
            home.join("pulse-provider.json"),
            r#"{"active_provider":"claude"}"#,
        )
        .unwrap();
        fs::write(home.join("claude/discord-presence-config.json"), "{}").unwrap();
        fs::write(home.join("codex/discord-presence-plan-cache.json"), "{}").unwrap();
        fs::write(home.join("claude/discord-presence-metrics.md"), "x").unwrap();
        fs::write(home.join("pulse-orion.lock"), "").unwrap();
        legacy_database(&home.join("pulse-analytics.db"));

        relocate_flat_layout(home).unwrap();

        assert_eq!(
            fs::read_to_string(home.join("config/provider.json")).unwrap(),
            r#"{"active_provider":"claude"}"#
        );
        assert!(home.join("config/claude-presence.json").exists());
        assert!(home.join("cache/codex-plan.json").exists());
        assert_eq!(
            session_ids(&home.join("data/pulse-analytics.db")),
            ["retained"]
        );
        let mut top: Vec<String> = fs::read_dir(home)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        top.sort();
        assert_eq!(top, ["cache", "config", "data"]);
    }

    #[test]
    fn flat_layout_relocation_is_idempotent_and_keeps_the_newer_file() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        fs::create_dir_all(home.join("config")).unwrap();
        fs::write(home.join("config/provider.json"), "new").unwrap();
        fs::write(home.join("pulse-provider.json"), "old").unwrap();

        relocate_flat_layout(home).unwrap();
        relocate_flat_layout(home).unwrap();

        assert_eq!(
            fs::read_to_string(home.join("config/provider.json")).unwrap(),
            "new"
        );
        assert!(!home.join("pulse-provider.json").exists());
    }
}
