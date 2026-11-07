//! Tests for the Codex access probe gate in the command layer.

use super::{ConfigReload, codex_probe_requested, request_codex_usage, snapshot_save_due};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[test]
fn no_codex_process_starts_without_a_user_refresh() {
    let cli_checked = std::cell::Cell::new(false);
    assert!(!codex_probe_requested(false, || {
        cli_checked.set(true);
        true
    }));
    assert!(
        !cli_checked.get(),
        "the CLI lookup may itself launch a process"
    );
}

#[test]
fn a_user_refresh_reads_once_when_the_cli_exists() {
    assert!(codex_probe_requested(true, || true));
    assert!(!codex_probe_requested(true, || false));

    let (trigger, requests) = std::sync::mpsc::sync_channel::<()>(1);
    let force = AtomicBool::new(false);
    // About ten minutes of five-second ticks with one click at the start.
    for tick in 0..120 {
        request_codex_usage(&trigger, &force, codex_probe_requested(tick == 0, || true));
    }
    assert_eq!(requests.try_iter().count(), 1);
    assert!(
        force.load(Ordering::Acquire),
        "a click bypasses the quota cache"
    );
}

#[test]
fn config_reloads_only_when_the_file_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, "1").unwrap();
    let mut reload = ConfigReload::observed(&path);
    let mut loads = 0;
    let mut load = || -> anyhow::Result<u32> {
        loads += 1;
        Ok(std::fs::read_to_string(&path)?.parse()?)
    };
    for _ in 0..20 {
        assert_eq!(reload.reload_if_changed(&path, &mut load), None);
    }
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    std::io::Write::write_all(&mut &file, b"2").unwrap();
    file.set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    drop(file);
    assert_eq!(reload.reload_if_changed(&path, &mut load), Some(2));
    assert_eq!(reload.reload_if_changed(&path, &mut load), None);
    assert_eq!(loads, 1);
}

#[test]
fn the_launch_snapshot_is_saved_at_most_once_a_minute() {
    let start = std::time::Instant::now();
    assert!(
        snapshot_save_due(None, start),
        "the first snapshot is saved"
    );
    let saves = (1..=120)
        .map(|tick| start + Duration::from_secs(5 * tick))
        .scan(Some(start), |last, now| {
            let due = snapshot_save_due(*last, now);
            if due {
                *last = Some(now);
            }
            Some(due)
        })
        .filter(|due| *due)
        .count();
    assert_eq!(
        saves, 10,
        "ten minutes of five-second changes save ten times"
    );
}
