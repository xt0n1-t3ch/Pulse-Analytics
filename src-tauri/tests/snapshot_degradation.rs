//! Public snapshot degradation contract against the same JSON DTO consumed by
//! the frontend. Fault fixtures redirect both provider homes so no user config
//! is read or modified.

use serde_json::Value;
use std::sync::Mutex;

static HOME_ENV_LOCK: Mutex<()> = Mutex::new(());

/// Pulse, Claude and Codex homes in one scratch folder removed on drop.
struct IsolatedHomes {
    _guard: std::sync::MutexGuard<'static, ()>,
    root: tempfile::TempDir,
}

fn isolated_homes() -> IsolatedHomes {
    let guard = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let root = tempfile::tempdir().expect("fixture root");
    let claude_home = root.path().join("claude-home");
    let codex_home = root.path().join("codex-home");
    std::fs::create_dir_all(&claude_home).expect("Claude fixture home");
    std::fs::create_dir_all(&codex_home).expect("Codex fixture home");
    unsafe {
        std::env::set_var("CLAUDE_HOME", &claude_home);
        std::env::set_var("CODEX_HOME", &codex_home);
        std::env::set_var("PULSE_HOME", root.path().join("pulse"));
    }
    IsolatedHomes {
        _guard: guard,
        root,
    }
}

fn assert_core_payload(snapshot: &Value) {
    assert_eq!(snapshot["revision"], 1);
    assert_eq!(snapshot["sync_state"], "live");
    assert!(snapshot["snapshot_captured_at"].is_string());
    assert!(snapshot["health"].is_object());
    assert!(snapshot["metrics"].is_object());
    assert!(snapshot["sessions"].is_array());
    assert!(snapshot["access"].is_object());
    assert!(snapshot["plan"].is_object());
}

#[test]
fn malformed_discord_config_degrades_settings_and_preview_without_dropping_analytics() {
    let homes = isolated_homes();
    pulse::commands::set_active_provider("claude".to_string()).expect("select Claude");
    let config_path = homes.root.path().join("pulse/config/claude-presence.json");
    std::fs::create_dir_all(config_path.parent().expect("config folder")).expect("config folder");
    std::fs::write(&config_path, b"{ malformed json").expect("write malformed config");

    let direct_error = pulse::commands::get_discord_settings()
        .expect_err("the dedicated Discord settings read must report the fault");
    assert!(direct_error.contains("invalid JSON"));
    let direct_preview_error = pulse::commands::get_discord_preview()
        .expect_err("the dedicated Discord preview read must report the same config fault");
    assert!(direct_preview_error.contains("invalid JSON"));

    let degraded = pulse::commands::get_app_snapshot()
        .expect("an ancillary Discord config failure must preserve analytics");
    assert_core_payload(&degraded);
    assert!(degraded["discord_settings"].is_null());
    assert!(
        degraded["discord_preview"].is_null(),
        "a config-dependent preview must not be fabricated from defaults"
    );
    assert!(
        degraded["discord_settings_error"]
            .as_str()
            .is_some_and(|error| error.contains("invalid JSON"))
    );

    std::fs::remove_file(&config_path).expect("repair malformed config");
    let recovered = pulse::commands::get_app_snapshot().expect("recovered snapshot");
    assert!(recovered["discord_settings"].is_object());
    assert!(recovered["discord_preview"].is_object());
    assert!(recovered["discord_settings_error"].is_null());
}
