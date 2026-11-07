//! Unit tests for the command layer: snapshots, access routes, Discord settings and cost totals.

use super::{
    REPORTS_BUNDLE_CACHE, aggregate_cost_totals, build_reports_bundle_for_scope_from_roots,
    cached_reports_bundle,
};
use super::{
    active_access_route, active_presence_presentation, api_probe_route,
    build_claude_context_breakdown, build_claude_session_infos, build_codex_discord_preview,
    build_codex_discord_preview_with_access, build_codex_session_infos, claude_route_from_usage,
    codex_fallback_surface, codex_plan_key_from_tier, codex_session_surface,
    codex_total_input_tokens, cost_availability, daily_cost_series, dismiss_notification,
    extract_discord_user, get_notifications, get_unread_notification_count,
    mark_all_notifications_read, mark_notification_read, merge_access_routes,
    plan_key_from_override, route_for_provider, route_limits_for_claude_presence,
    route_limits_for_presence, scan_discord_leveldb_dirs, semantic_snapshot_fingerprint,
};
use crate::access::{
    AccessAvailability, AccessFreshness, AccessProof, AccessProvenance, AccessRouteSnapshot,
    AccessSnapshot, AccessSourceKind, AccessWindow, AuthMethod, access_route_from_usage,
    subscription_source,
};
use crate::notifications::NotificationStore;
use cc_discord_presence::codex::config::{
    DesktopPresenceDesign, DisplayConfig, OpenAiPlanMode,
    PresenceConfig as TestCodexPresenceConfig, PresenceSurface as TestPresenceSurface,
    PrivacyConfig,
};
use cc_discord_presence::codex::cost::{
    CostAttribution, PricingSource, PricingStatus, TokenCostBreakdown,
};
use cc_discord_presence::codex::model::{SessionSpeed, SpeedMode, SpeedSource};
use cc_discord_presence::codex::session::{
    CodexSessionSnapshot, ContextWindowSnapshot, ContextWindowSource,
};
use cc_discord_presence::codex::telemetry::limits::RateLimits;
use cc_discord_presence::codex::telemetry::plan::DetectedPlanTier;
use cc_discord_presence::codex::telemetry::plan::PlanDetector;
use cc_discord_presence::codex::telemetry::service_tier::resolve_service_tier;
use cc_discord_presence::config::PresenceConfig as TestClaudePresenceConfig;
use cc_discord_presence::cost;
use cc_discord_presence::provider::Provider;

use cc_discord_presence::session::{ClaudeSessionSnapshot, DataSource, ReasoningEffort, Speed};
use cc_discord_presence::usage::{ExtraUsage, UsageData, UsageWindow};
use codex_presence_core::{
    QuotaScope, QuotaWindow, RateLimitScope, UsageSignal, UsageSnapshot, UsageSource,
};
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

static REPORTS_BUNDLE_CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

fn quota_usage(provider: &str, signal: UsageSignal, windows: Vec<QuotaWindow>) -> UsageSnapshot {
    UsageSnapshot {
        source: UsageSource::new(format!("{provider}-subscription:default"), [signal]),
        scopes: vec![QuotaScope {
            id: Some(provider.to_string()),
            name: None,
            kind: RateLimitScope::GlobalAccount,
            windows,
        }],
        credits: None,
        observed_at: Some(chrono::Utc::now()),
        provenance_source: "deterministic fixture".to_string(),
    }
}

fn discord_user_record(
    user_id: &str,
    username: &str,
    global_name: Option<&str>,
    avatar_hash: &str,
    padding: usize,
) -> Vec<u8> {
    let global_name = global_name
        .map(|name| format!(r#", "global_name":"{name}""#))
        .unwrap_or_default();
    format!(
        r#"{{"id":"{user_id}","username":"{username}"{}{},"discriminator":"0","avatar":"{avatar_hash}"}}{}"#,
        " ".repeat(padding),
        global_name,
        " ".repeat(120)
    )
    .into_bytes()
}

fn discord_user_fixture(global_name: Option<&str>, padding: usize) -> Vec<u8> {
    discord_user_record(
        "123456789012345678",
        "pulse-user",
        global_name,
        "avatar-hash",
        padding,
    )
}

fn discord_test_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pulse-discord-user-{tag}-{}-{:?}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create discord test dir");
    dir
}

#[test]
fn discord_user_extracts_global_name_from_bounded_record_region() {
    let data = discord_user_fixture(Some("Pulse Display"), 700);

    let user = extract_discord_user(&data).expect("discord user");

    assert_eq!(user.global_name.as_deref(), Some("Pulse Display"));
}

#[test]
fn discord_user_treats_empty_or_missing_global_name_as_absent() {
    let empty = extract_discord_user(&discord_user_fixture(Some(""), 0)).expect("discord user");
    let missing = extract_discord_user(&discord_user_fixture(None, 0)).expect("discord user");

    assert_eq!(empty.global_name, None);
    assert_eq!(missing.global_name, None);
}

#[test]
fn discord_user_scan_skips_unreadable_entry_and_uses_later_file() {
    let dir = discord_test_dir("unreadable");
    std::fs::create_dir(dir.join("newest.ldb")).expect("create unreadable directory entry");
    std::fs::write(dir.join("older.log"), discord_user_fixture(None, 0))
        .expect("write valid discord record");

    let user = scan_discord_leveldb_dirs(std::slice::from_ref(&dir)).expect("discord user");

    assert_eq!(user.username, "pulse-user");
    std::fs::remove_dir_all(dir).expect("remove discord test dir");
}

#[test]
fn discord_user_scan_falls_back_to_second_candidate_directory() {
    let first = discord_test_dir("first-empty");
    let second = discord_test_dir("second-valid");
    std::fs::write(first.join("000001.log"), b"not a discord user").expect("write empty candidate");
    std::fs::write(second.join("000002.ldb"), discord_user_fixture(None, 0))
        .expect("write valid candidate");

    let user = scan_discord_leveldb_dirs(&[first.clone(), second.clone()])
        .expect("discord user from second candidate");

    assert_eq!(user.user_id, "123456789012345678");
    std::fs::remove_dir_all(first).expect("remove first discord test dir");
    std::fs::remove_dir_all(second).expect("remove second discord test dir");
}

#[test]
fn discord_user_scan_uses_newest_file_across_candidate_directories() {
    let first = discord_test_dir("first-older");
    let second = discord_test_dir("second-newer");
    let older_path = first.join("000001.ldb");
    let newer_path = second.join("000002.ldb");
    std::fs::write(
        &older_path,
        discord_user_record("111111111111111111", "old-user", None, "old-avatar", 0),
    )
    .expect("write older discord record");
    std::fs::write(
        &newer_path,
        discord_user_record("222222222222222222", "current-user", None, "new-avatar", 0),
    )
    .expect("write newer discord record");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&older_path)
        .expect("open older discord record")
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        .expect("set older modification time");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&newer_path)
        .expect("open newer discord record")
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_100))
        .expect("set newer modification time");

    let user = scan_discord_leveldb_dirs(&[first.clone(), second.clone()])
        .expect("discord user from globally newest file");

    assert_eq!(user.user_id, "222222222222222222");
    assert_eq!(user.username, "current-user");
    std::fs::remove_dir_all(first).expect("remove first discord test dir");
    std::fs::remove_dir_all(second).expect("remove second discord test dir");
}

#[test]
fn discord_user_extract_prefers_richer_later_record() {
    let mut data = discord_user_record("111111111111111111", "bare-user", None, "", 0);
    data.extend_from_slice(&discord_user_record(
        "222222222222222222",
        "current-user",
        Some("Current Display"),
        "current-avatar",
        0,
    ));

    let user = extract_discord_user(&data).expect("best discord user");

    assert_eq!(user.user_id, "222222222222222222");
    assert_eq!(user.global_name.as_deref(), Some("Current Display"));
    assert_eq!(user.avatar_hash, "current-avatar");
}

#[test]
fn discord_user_extract_resolves_single_clean_record() {
    let user = extract_discord_user(&discord_user_fixture(Some("Pulse Display"), 0))
        .expect("single discord user");

    assert_eq!(user.user_id, "123456789012345678");
    assert_eq!(user.username, "pulse-user");
}

#[test]
fn discord_user_never_mixes_fields_from_a_following_profile() {
    // Trailing filler mirrors real LevelDB bulk: the extractor's outer loop
    // only keeps scanning while a record-sized lookahead remains.
    let mut data = br#"{"id":"123456789012345678"}"#.to_vec();
    data.extend_from_slice(
        br#"{"id":"987654321098765432","username":"next-account","avatar":"hashB"}"#,
    );
    data.extend_from_slice(b" ".repeat(120).as_slice());

    let user = extract_discord_user(&data).expect("second profile resolves");

    assert_eq!(user.user_id, "987654321098765432");
    assert_eq!(user.username, "next-account");
    assert_eq!(user.avatar_hash, "hashB");
}

#[test]
fn discord_user_global_name_decodes_escapes_and_survives_braces_in_values() {
    let mut data =
        br#"{"id":"123456789012345678","username":"brace-user","note":"evil } tail","global_name":"Pulse \"Dev\""}"#
            .to_vec();
    data.extend_from_slice(b" ".repeat(120).as_slice());

    let user = extract_discord_user(&data).expect("discord user");

    assert_eq!(user.global_name.as_deref(), Some("Pulse \"Dev\""));
    assert_eq!(user.username, "brace-user");
}

fn proofed_route(
    provider: &str,
    signal: UsageSignal,
    windows: Vec<QuotaWindow>,
) -> AccessRouteSnapshot {
    let mut source = subscription_source(provider, None);
    source.proof = AccessProof::QuotaResponse;
    access_route_from_usage(
        source,
        quota_usage(provider, signal, windows),
        chrono::Utc::now(),
        chrono::Duration::seconds(30),
        chrono::Utc::now(),
    )
}

#[test]
fn simultaneous_codex_and_claude_routes_are_retained() {
    let codex = proofed_route(
        "codex",
        UsageSignal::CodexSubscriptionUsage,
        vec![QuotaWindow {
            window_minutes: 7_777,
            used_percent: 12.0,
            remaining_percent: 88.0,
            resets_at: None,
        }],
    );
    let claude = proofed_route(
        "claude",
        UsageSignal::ClaudeSubscriptionUsage,
        vec![QuotaWindow {
            window_minutes: 2_345,
            used_percent: 34.0,
            remaining_percent: 66.0,
            resets_at: None,
        }],
    );
    let snapshot = merge_access_routes(&[], [codex, claude]);

    assert_eq!(snapshot.routes.len(), 2);
    assert!(
        snapshot
            .routes
            .iter()
            .any(|route| route.source.kind == AccessSourceKind::CodexSubscription)
    );
    assert!(
        snapshot
            .routes
            .iter()
            .any(|route| route.source.kind == AccessSourceKind::ClaudeSubscription)
    );
}

#[test]
fn active_provider_selection_does_not_depend_on_route_order() {
    let codex = proofed_route(
        "codex",
        UsageSignal::CodexSubscriptionUsage,
        vec![QuotaWindow {
            window_minutes: 17,
            used_percent: 1.0,
            remaining_percent: 99.0,
            resets_at: None,
        }],
    );
    let claude = proofed_route(
        "claude",
        UsageSignal::ClaudeSubscriptionUsage,
        vec![QuotaWindow {
            window_minutes: 4_321,
            used_percent: 2.0,
            remaining_percent: 98.0,
            resets_at: None,
        }],
    );
    let mut data = super::CachedData {
        active_provider: Provider::Claude,
        access: AccessSnapshot {
            routes: vec![codex, claude],
        },
        ..Default::default()
    };
    assert_eq!(active_access_route(&data).source.provider, "claude");
    data.active_provider = Provider::Codex;
    assert_eq!(active_access_route(&data).source.provider, "codex");
    assert_eq!(
        route_for_provider(&data, Provider::Codex)
            .unwrap()
            .source
            .kind,
        AccessSourceKind::CodexSubscription
    );
}

#[test]
fn failed_api_auth_is_unavailable_and_unproofed() {
    let mut cache = super::ApiProbeEntry::default();
    let route = api_probe_route("openai", None, &mut cache);
    assert_eq!(route.availability, AccessAvailability::Unavailable);
    assert_eq!(route.source.proof, AccessProof::None);
    assert!(route.windows.is_empty());
    assert_eq!(route.source.kind, AccessSourceKind::OpenAiApi);
    assert_eq!(route.source.auth_method, AuthMethod::ApiKey);
    assert_eq!(AccessSnapshot::new(vec![route]).routes.len(), 1);
}

#[test]
fn failed_refresh_replaces_old_windows_instead_of_leaking_stale_usage() {
    let old = proofed_route(
        "codex",
        UsageSignal::CodexSubscriptionUsage,
        vec![QuotaWindow {
            window_minutes: 300,
            used_percent: 75.0,
            remaining_percent: 25.0,
            resets_at: None,
        }],
    );
    let replacement = AccessRouteSnapshot::unavailable(
        subscription_source("codex", None),
        "account probe failed",
    );
    let snapshot = merge_access_routes(&[old], [replacement]);
    assert_eq!(snapshot.routes.len(), 1);
    assert!(snapshot.routes[0].windows.is_empty());
    assert_eq!(
        snapshot.routes[0].availability,
        AccessAvailability::Unavailable
    );
}

#[test]
fn provider_windows_keep_arbitrary_durations_without_synthetic_buckets() {
    let route = proofed_route(
        "claude",
        UsageSignal::ClaudeSubscriptionUsage,
        vec![
            QuotaWindow {
                window_minutes: 17,
                used_percent: 11.0,
                remaining_percent: 89.0,
                resets_at: None,
            },
            QuotaWindow {
                window_minutes: 4_321,
                used_percent: 22.0,
                remaining_percent: 78.0,
                resets_at: None,
            },
        ],
    );
    assert_eq!(
        route
            .windows
            .iter()
            .map(|window| window.quota.window_minutes)
            .collect::<Vec<_>>(),
        vec![17, 4_321]
    );
}

#[test]
fn snapshot_fingerprint_ignores_wall_clock_fields_but_not_telemetry() {
    let first = serde_json::json!({
        "sync_state": "syncing",
        "snapshot_captured_at": "2026-08-12T10:00:00Z",
        "health": {"uptime_seconds": 10, "status": "ok"},
        "discord_preview": {"duration_secs": 10, "state": "7d 96%"},
        "sessions": [{"duration_secs": 10, "tokens_per_sec": 4.5, "tokens": 100}]
    });
    let second = serde_json::json!({
        "sync_state": "live",
        "snapshot_captured_at": "2026-08-12T10:00:05Z",
        "health": {"uptime_seconds": 15, "status": "ok"},
        "discord_preview": {"duration_secs": 15, "state": "7d 96%"},
        "sessions": [{"duration_secs": 15, "tokens_per_sec": 3.0, "tokens": 100}]
    });
    let changed = serde_json::json!({
        "health": {"uptime_seconds": 15, "status": "ok"},
        "discord_preview": {"duration_secs": 15, "state": "7d 95%"},
        "sessions": [{"duration_secs": 15, "tokens_per_sec": 3.0, "tokens": 101}]
    });

    assert_eq!(
        semantic_snapshot_fingerprint(first),
        semantic_snapshot_fingerprint(second)
    );
    assert_ne!(
        semantic_snapshot_fingerprint(serde_json::json!({
            "health": {"uptime_seconds": 15, "status": "ok"},
            "discord_preview": {"duration_secs": 15, "state": "7d 96%"},
            "sessions": [{"duration_secs": 15, "tokens_per_sec": 3.0, "tokens": 100}]
        })),
        semantic_snapshot_fingerprint(changed)
    );
}

fn persisted_snapshot_fixture() -> serde_json::Value {
    serde_json::json!({
        "revision": 1,
        "sync_state": "live",
        "snapshot_captured_at": "2026-08-12T10:00:00Z",
        "health": {},
        "metrics": {},
        "sessions": [],
        "access": {},
        "rate_limits": null,
        "discord_preview": {},
        "discord_settings": {},
        "plan": {}
    })
}

#[test]
fn persisted_snapshot_is_provider_scoped_and_returns_as_syncing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("snapshot.json");
    super::persist_app_snapshot_value_at(&path, Provider::Claude, persisted_snapshot_fixture())
        .expect("persist snapshot");

    let loaded = super::load_persisted_app_snapshot_at(&path, Provider::Claude)
        .expect("load snapshot")
        .expect("matching provider snapshot");
    assert_eq!(loaded["sync_state"], "syncing");
    assert_eq!(loaded["snapshot_captured_at"], "2026-08-12T10:00:00Z");
    assert!(
        super::load_persisted_app_snapshot_at(&path, Provider::Codex)
            .expect("provider mismatch is not an I/O failure")
            .is_none()
    );
}

/// A snapshot from another build or from long ago would show values that
/// build computed, so startup waits for the first live poll instead.
#[test]
fn persisted_snapshot_from_another_build_or_too_old_is_not_shown() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("snapshot.json");
    super::persist_app_snapshot_value_at(&path, Provider::Claude, persisted_snapshot_fixture())
        .expect("persist snapshot");
    let rewrite = |edit: &dyn Fn(&mut serde_json::Value)| {
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        edit(&mut value);
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    };
    let loads = || {
        super::load_persisted_app_snapshot_at(&path, Provider::Claude)
            .expect("not an I/O failure")
            .is_some()
    };
    assert!(loads(), "same build, fresh");

    rewrite(&|value| value["app_version"] = serde_json::json!("0.0.0-older-build"));
    assert!(!loads(), "another build");

    rewrite(&|value| {
        value.as_object_mut().unwrap().remove("app_version");
    });
    assert!(!loads(), "unversioned snapshot");

    rewrite(&|value| {
        value["app_version"] = serde_json::json!(env!("CARGO_PKG_VERSION"));
        value["saved_at"] =
            serde_json::json!((chrono::Utc::now() - chrono::Duration::hours(25)).to_rfc3339());
    });
    assert!(!loads(), "older than the maximum age");

    rewrite(&|value| value["saved_at"] = serde_json::json!("not-a-date"));
    assert!(!loads(), "unparseable save time");
}

#[test]
fn corrupt_or_wrong_contract_persisted_snapshot_is_never_promoted() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("snapshot.json");
    std::fs::write(&path, b"not-json").expect("write corrupt cache");
    assert!(super::load_persisted_app_snapshot_at(&path, Provider::Claude).is_err());

    super::persist_app_snapshot_value_at(
        &path,
        Provider::Claude,
        serde_json::json!({"revision": 1}),
    )
    .expect("persist invalid contract");
    assert!(super::load_persisted_app_snapshot_at(&path, Provider::Claude).is_err());
}

#[test]
fn usage_snapshot_transport_uses_public_scope_labels() {
    let snapshot = codex_presence_core::UsageSnapshot {
        source: UsageSource::new("codex-session-jsonl", [UsageSignal::CodexSessionJsonl]),
        scopes: vec![codex_presence_core::QuotaScope {
            id: Some("codex".to_string()),
            name: None,
            kind: codex_presence_core::RateLimitScope::GlobalAccount,
            windows: Vec::new(),
        }],
        credits: None,
        observed_at: None,
        provenance_source: "fixture".to_string(),
    };
    let value = serde_json::to_value(super::UsageSnapshotTransport::from_snapshot(
        "codex", snapshot,
    ))
    .expect("usage snapshot JSON");
    assert_eq!(value["provider"], "codex");
    assert_eq!(value["source"], "fixture");
    assert_eq!(value["usage_source"]["stream_id"], "codex-session-jsonl");
    assert_eq!(value["scopes"][0]["kind"], "global_account");
}

#[test]
fn legacy_model_distribution_transport_remains_tuple_shaped() {
    let rows = vec![crate::db::ModelStat {
        model: "gpt-5.6-sol".to_string(),
        session_count: 2,
        priced_sessions: 1,
        cost_basis: crate::db::CostBasis::Partial,
        cost_sources: vec!["provider_billed".to_string()],
        total_cost: 4.25,
    }];

    assert_eq!(
        super::legacy_model_distribution(rows),
        vec![("gpt-5.6-sol".to_string(), 2, 4.25)]
    );
}

#[test]
fn budget_validation_rejects_invalid_values_before_persistence() {
    assert!(super::set_budget(-1.0, Some(80.0)).is_err());
    assert!(super::set_budget(f64::NAN, Some(80.0)).is_err());
    assert!(super::set_budget(10.0, Some(101.0)).is_err());
    assert!(super::set_budget(10.0, Some(f64::INFINITY)).is_err());
}

fn sample_claude_snapshot(model_id: &str) -> ClaudeSessionSnapshot {
    ClaudeSessionSnapshot {
        session_id: format!("{model_id}-session"),
        cwd: PathBuf::from("D:/X/Pulse"),
        project_name: "pulse".into(),
        git_branch: None,
        model: Some(model_id.to_string()),
        model_display: Some(cost::model_display_name(model_id)),
        session_total_tokens: Some(120_000),
        last_turn_tokens: Some(2_000),
        session_delta_tokens: None,
        input_tokens: 100_000,
        output_tokens: 20_000,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        max_turn_api_input: 100_000,
        current_context_tokens: 100_000,
        reasoning_effort: ReasoningEffort::High,
        reasoning_effort_explicit: true,
        has_thinking_blocks: false,
        speed: Speed::Standard,
        service_tier: None,
        total_cost: 2.0,
        input_cost: 1.0,
        output_cost: 1.0,
        cache_write_cost: 0.0,
        cache_read_cost: 0.0,
        total_api_duration_ms: 0,
        limits: cc_discord_presence::session::RateLimits::default(),
        activity: None,
        started_at: None,
        last_token_event_at: None,
        last_activity: SystemTime::now(),
        source: DataSource::Jsonl,
        source_file: PathBuf::from("session.jsonl"),
        background_work: cc_discord_presence::workflow_state::BackgroundWorkInfo::default(),
        subagents: Vec::new(),
        is_subagent: false,
        parent_session_id: None,
    }
}

/// `CLAUDE_HOME` / `CODEX_HOME` are process-global, so every test that
/// redirects them must hold this lock or they clobber each other under
/// libtest's default multi-threaded runner.
static HOME_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Pulse, Claude and Codex homes in one scratch folder, removed on drop; the
/// guard keeps other home-mutating tests out until the caller is done.
struct IsolatedHomes {
    _guard: std::sync::MutexGuard<'static, ()>,
    _root: tempfile::TempDir,
    codex: PathBuf,
}

fn isolated_homes() -> IsolatedHomes {
    let guard = HOME_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().expect("fixture root");
    let claude_home = root.path().join("claude-home");
    let codex_home = root.path().join("codex-home");
    std::fs::create_dir_all(&claude_home).expect("claude home");
    std::fs::create_dir_all(&codex_home).expect("codex home");
    unsafe {
        std::env::set_var("CLAUDE_HOME", &claude_home);
        std::env::set_var("CODEX_HOME", &codex_home);
        std::env::set_var("PULSE_HOME", root.path().join("pulse"));
    }
    IsolatedHomes {
        _guard: guard,
        _root: root,
        codex: codex_home,
    }
}

#[test]
fn claude_rich_presence_toggle_survives_a_restart() {
    let _homes = isolated_homes();
    super::set_active_provider("claude".to_string()).expect("save Claude provider");

    let settings = super::set_discord_enabled(false).expect("disable Claude presence");
    assert!(!settings.enabled, "the command must report the new state");

    // Restart equivalent: nothing but the on-disk config survives, and the
    // startup path is what seeds the in-memory cache.
    let claude = TestClaudePresenceConfig::load_or_init().expect("claude config");
    assert!(
        !claude.presence_enabled,
        "turning Rich Presence off must be written to the Claude config"
    );

    if let Ok(mut data) = super::shared().lock() {
        data.discord_enabled = true; // pretend a fresh process default
    }
    super::seed_discord_state_from_disk();

    // The poller decides whether to publish to Discord from this cached flag,
    // so seeding it wrong is what makes a paused presence come back to life.
    let cached_enabled = super::shared()
        .lock()
        .expect("shared state")
        .discord_enabled;
    assert!(
        !cached_enabled,
        "startup must seed the cached presence flag from disk, not force it on"
    );

    let after_restart = super::get_discord_settings().expect("settings after restart");
    assert!(
        !after_restart.enabled,
        "startup must honour the persisted Claude presence flag instead of forcing it on"
    );
}

#[test]
fn claude_display_prefs_persist_even_when_the_codex_mirror_write_fails() {
    let homes = isolated_homes();
    let codex_home = homes.codex.clone();
    super::set_active_provider("claude".to_string()).expect("save Claude provider");

    // Make the Codex home unusable: a regular file where a directory is
    // expected, so `CodexPresenceConfig::load_or_init` cannot succeed.
    std::fs::remove_dir_all(&codex_home).expect("clear codex home");
    std::fs::write(&codex_home, b"not a directory").expect("occupy codex home");

    let settings = super::set_discord_display_prefs(
        true, false, true, true, true, true, true, false, true, true,
    )
    .expect("a broken Codex mirror must not fail the active provider's save");

    assert!(
        !settings.display_prefs.show_branch,
        "the returned canonical payload must reflect the change the user made"
    );
    let claude = TestClaudePresenceConfig::load_or_init().expect("claude config");
    assert!(
        !claude.privacy.show_git_branch,
        "the Claude config must keep the change even though the mirror write failed"
    );
}

#[test]
fn saving_claude_toggles_preserves_the_codex_credits_preference() {
    let _homes = isolated_homes();

    // Codex owns Credits and has it on.
    super::set_active_provider("codex".to_string()).expect("save Codex provider");
    super::set_discord_display_prefs(true, true, true, true, true, true, true, true, true, true)
        .expect("save under codex");
    assert!(
        TestCodexPresenceConfig::load_or_init()
            .expect("codex config")
            .privacy
            .show_credits
    );

    // Claude has no Credits field, so its payload always carries `false`.
    // Saving an unrelated Claude toggle must not disable Codex Credits.
    super::set_active_provider("claude".to_string()).expect("save Claude provider");
    super::set_discord_display_prefs(true, false, true, true, true, true, true, false, true, true)
        .expect("save under claude");

    let codex = TestCodexPresenceConfig::load_or_init().expect("codex config");
    assert!(
        codex.privacy.show_credits,
        "a Claude-side save must not clobber a Codex-only preference"
    );
    assert!(
        !codex.privacy.show_git_branch,
        "shared fields still mirror across providers"
    );
}

#[test]
fn discord_controls_are_saved_for_claude_and_codex_together() {
    let _homes = isolated_homes();

    super::set_discord_display_prefs(
        true, false, true, true, true, true, false, true, false, true,
    )
    .expect("save display preferences");
    super::set_active_provider("codex".to_string()).expect("save Codex provider");
    let settings = super::set_discord_enabled(false).expect("disable Codex presence");

    let claude = TestClaudePresenceConfig::load_or_init().expect("claude config");
    let codex = TestCodexPresenceConfig::load_or_init().expect("codex config");

    assert!(!claude.privacy.show_git_branch);
    assert!(!codex.privacy.show_git_branch);
    assert!(claude.privacy.show_cost);
    assert!(codex.privacy.show_cost);
    assert!(!claude.privacy.show_limits);
    assert!(!codex.privacy.show_limits);
    assert!(codex.privacy.show_credits);
    assert!(!claude.privacy.show_context);
    assert!(!codex.privacy.show_context);
    assert!(claude.privacy.show_systems);
    assert!(codex.privacy.show_systems);
    assert!(!codex.presence_enabled);
    assert!(!settings.enabled);
}

#[test]
fn switching_provider_keeps_lane_diagnostics_without_selecting_old_provider() {
    let _homes = isolated_homes();
    if let Ok(mut data) = super::shared().lock() {
        data.access = super::AccessSnapshot {
            routes: vec![crate::access::AccessRouteSnapshot::unavailable(
                crate::access::subscription_source("codex", None),
                "fixture",
            )],
        };
    }

    super::seed_discord_state_for(Provider::Claude);

    let routes = super::shared()
        .lock()
        .expect("shared state")
        .access
        .routes
        .clone();
    assert_eq!(routes.len(), 1, "independent lane diagnostics stay cached");
    assert_eq!(routes[0].source.provider, "codex");
    let data = super::shared().lock().expect("shared state");
    assert_eq!(super::active_access_route(&data).source.provider, "claude");
}

#[test]
fn switching_to_claude_does_not_leak_a_stale_codex_plan_info() {
    let _homes = isolated_homes();
    let mut codex = TestCodexPresenceConfig::load_or_init().expect("codex config");
    codex.openai_plan.mode = OpenAiPlanMode::Manual;
    codex.openai_plan.tier = cc_discord_presence::codex::config::OpenAiPlanTier::Pro20x;
    codex.save().expect("codex plan override");

    super::set_active_provider("codex".to_string()).expect("save Codex provider");
    assert_eq!(super::get_plan_info().plan_key, "pro_20x");

    super::set_active_provider("claude".to_string()).expect("save Claude provider");
    let claude_plan = super::get_plan_info();
    assert_eq!(claude_plan.provider, "claude");
    assert_ne!(claude_plan.plan_key, "pro_20x");
    assert_eq!(claude_plan.plan_name, "Unknown");
    assert!(
        !claude_plan.detected,
        "no Claude plan proof must be Unknown"
    );
}

#[test]
fn invalid_claude_max_override_is_not_reported_as_a_plan() {
    let _homes = isolated_homes();
    super::set_active_provider("claude".to_string()).expect("save Claude provider");

    let mut config = TestClaudePresenceConfig::load_or_init().expect("Claude config");
    config.plan = Some("max".to_string());
    config.save().expect("save invalid plan fixture");

    let plan = super::get_plan_info();
    assert_eq!(plan.provider, "claude");
    assert_eq!(plan.plan_key, "");
    assert_eq!(plan.plan_name, "Unknown");
    assert!(!plan.detected);
}

#[test]
fn notification_commands_share_the_durable_store_lifecycle() {
    let _homes = isolated_homes();
    let connection = crate::notifications::open_default_database().expect("notification db");
    let record = NotificationStore::new(&connection)
        .observe(
            crate::notifications::NotificationSpec {
                kind: crate::notifications::NotificationKind::ProviderHealth,
                provider: "codex".to_string(),
                key: "account".to_string(),
                title: "Codex provider".to_string(),
                body: "Provider unavailable".to_string(),
                action: None,
            },
            chrono::Utc::now(),
        )
        .expect("record notification")
        .expect("new notification");

    assert_eq!(get_unread_notification_count(), 1);
    assert_eq!(
        get_notifications(Some(10))
            .expect("list notifications")
            .len(),
        1
    );
    assert!(mark_notification_read(record.id).expect("mark notification read"));
    assert_eq!(get_unread_notification_count(), 0);
    assert_eq!(
        mark_all_notifications_read().expect("mark all notifications read"),
        0
    );
    assert!(super::mark_notification_unread(record.id).unwrap());
    assert_eq!(get_unread_notification_count(), 1);
    assert_eq!(mark_all_notifications_read().unwrap(), 1);
    assert_eq!(super::mark_all_notifications_unread().unwrap(), 1);
    let cleared = super::dismiss_all_notifications().unwrap();
    assert_eq!(cleared.count, 1);
    assert!(get_notifications(Some(10)).unwrap().is_empty());
    assert_eq!(super::restore_notifications(cleared.undo_token).unwrap(), 1);
    assert!(dismiss_notification(record.id).expect("dismiss notification"));
    assert!(
        get_notifications(Some(10))
            .expect("list notifications after dismiss")
            .is_empty()
    );
}

#[test]
fn access_diagnostics_never_become_native_health_or_threshold_alerts() {
    let connection = rusqlite::Connection::open_in_memory().expect("notification sqlite");
    NotificationStore::initialize(&connection).expect("notification schema");
    let store = NotificationStore::new(&connection);

    let mut authenticated = subscription_source("claude", None);
    authenticated.proof = AccessProof::QuotaResponse;
    let healthy = AccessRouteSnapshot {
        source: authenticated,
        availability: AccessAvailability::Available,
        freshness: AccessFreshness::Fresh,
        provenance: AccessProvenance::ProviderApi,
        observed_at: None,
        fetched_at: None,
        expires_at: None,
        windows: Vec::new(),
        credits: None,
        rate_limit_reset_credits: None,
        individual_limits: Vec::new(),
        extra_usage: None,
        local_history: Default::default(),
        error: None,
        unavailable_reason: None,
        usage: None,
    };

    // A proofed healthy sample is still only a baseline.
    super::observe_access_notifications(None, Some(&store), Provider::Claude, &healthy);
    assert!(store.list_all(Some(20)).expect("baseline rows").is_empty());

    // A displayable cache with no provider proof (or an unavailable route)
    // must not emit a native quota or health alert at the poll cadence;
    // only genuine quota-reset edges do.
    let mut cached = healthy.clone();
    cached.source.proof = AccessProof::None;
    cached.provenance = AccessProvenance::MemoryCache;
    cached.windows = vec![AccessWindow {
        key: "weekly".to_string(),
        label: None,
        quota: QuotaWindow {
            window_minutes: 10_080,
            used_percent: 95.0,
            remaining_percent: 5.0,
            resets_at: None,
        },
    }];
    super::observe_access_notifications(None, Some(&store), Provider::Claude, &cached);
    assert!(store.list_all(Some(20)).expect("cached rows").is_empty());

    let unavailable = AccessRouteSnapshot::unavailable(
        subscription_source("claude", None),
        "no credentials — check .credentials.json",
    );
    super::observe_access_notifications(None, Some(&store), Provider::Claude, &unavailable);
    assert!(store.list_all(Some(20)).expect("edge rows").is_empty());
}

#[test]
fn codex_discord_settings_reflect_persisted_privacy_design_and_publisher() {
    let config = TestCodexPresenceConfig {
        presence_enabled: false,
        privacy: PrivacyConfig {
            show_git_branch: false,
            ..PrivacyConfig::default()
        },
        display: DisplayConfig {
            desktop_presence_design: DesktopPresenceDesign::ChatGptApp,
            ..DisplayConfig::default()
        },
        ..TestCodexPresenceConfig::default()
    };
    let cached = super::CachedData {
        active_provider: cc_discord_presence::provider::Provider::Codex,
        discord_status: "Controlled by external daemon".to_string(),
        discord_publisher: "external_daemon".to_string(),
        discord_enabled: true,
        ..Default::default()
    };

    let settings = super::build_discord_settings(
        cc_discord_presence::provider::Provider::Codex,
        &cached,
        None,
        Some(&config),
    );

    assert_eq!(settings.provider, "codex");
    assert!(!settings.enabled);
    assert_eq!(settings.publisher, "external_daemon");
    assert!(!settings.display_prefs.show_branch);
    assert_eq!(settings.desktop_design.as_deref(), Some("chatgpt_app"));
    assert!(settings.supports_desktop_design);
}

#[test]
fn claude_session_info_surfaces_ultracode_background_work_safely() {
    let mut snapshot = sample_claude_snapshot("claude-opus-4-8");
    snapshot.background_work = cc_discord_presence::workflow_state::BackgroundWorkInfo {
        workflow_active: true,
        active_agent_count: 2,
        latest_signal_at: Some(SystemTime::now()),
    };

    let infos = build_claude_session_infos(&[snapshot]);

    assert_eq!(infos[0].workflow_label.as_deref(), Some("ULTRACODE"));
    assert_eq!(infos[0].subagent_count, 2);
    assert!(infos[0].subagents.is_empty());
}

#[test]
fn claude_session_info_does_not_call_plain_thinking_a_workflow() {
    let mut snapshot = sample_claude_snapshot("claude-opus-4-8");
    snapshot.has_thinking_blocks = true;
    snapshot.background_work = cc_discord_presence::workflow_state::BackgroundWorkInfo::default();

    let infos = build_claude_session_infos(&[snapshot]);

    assert_eq!(infos[0].workflow_label, None);
}

#[test]
fn claude_session_cost_provenance_distinguishes_jsonl_estimates_from_statusline() {
    let jsonl = sample_claude_snapshot("claude-opus-4-8");
    let mut statusline = jsonl.clone();
    statusline.source = DataSource::Statusline;

    let estimated = build_claude_session_infos(&[jsonl]);
    let exact = build_claude_session_infos(&[statusline]);

    assert_eq!(estimated[0].cost_basis, "estimated");
    assert_eq!(exact[0].cost_basis, "exact");
}

#[test]
fn discord_preview_uses_the_same_claude_presence_lines_as_publish() {
    let mut snapshot = sample_claude_snapshot("claude-opus-4-8");
    snapshot.project_name = "PropertyAlpha-Agent".to_string();
    snapshot.git_branch = Some("feat/marketplace-addtochat-liveview-management".to_string());
    snapshot.background_work = cc_discord_presence::workflow_state::BackgroundWorkInfo {
        workflow_active: true,
        active_agent_count: 1,
        latest_signal_at: Some(SystemTime::now()),
    };

    let mut config = TestClaudePresenceConfig {
        plan: Some("max_20x".to_string()),
        ..Default::default()
    };
    super::apply_claude_display_prefs(
        &mut config,
        &super::DiscordDisplayPrefs {
            show_project: true,
            show_branch: false,
            show_model: true,
            show_activity: true,
            show_tokens: true,
            show_cost: true,
            show_limits: false,
            show_credits: false,
            show_context: false,
            show_systems: true,
        },
    );

    let preview = super::build_claude_discord_preview(&[snapshot], &config);

    assert!(preview.has_session);
    assert!(preview.details.contains("PropertyAlpha-Agent"));
    assert!(!preview.details.contains("feat/marketplace"));
    assert!(preview.state.contains("ULTRACODE"));
    assert!(preview.state.contains("1 agent"));
    assert!(!preview.state.contains("5h"));
    assert!(!preview.state.contains("7d"));
    assert!(!preview.state.contains("Ctx"));
}

#[test]
fn claude_preview_and_live_share_extra_usage_from_the_access_route() {
    let now = chrono::Utc::now();
    let snapshot = sample_claude_snapshot("claude-opus-4-8");
    let usage = UsageData {
        five_hour: UsageWindow {
            utilization: 20.0,
            resets_at: None,
        },
        seven_day: UsageWindow {
            utilization: 35.0,
            resets_at: None,
        },
        sonnet_free: None,
        limits: Vec::new(),
        extra_usage: Some(ExtraUsage {
            is_enabled: true,
            monthly_limit: Some(75.0),
            used_credits: Some(10.0),
            utilization: Some(13.333),
        }),
    };
    let route = claude_route_from_usage(
        subscription_source("claude", None),
        &usage,
        now,
        now,
        chrono::Duration::seconds(300),
        now,
        "OAuth usage API",
    );
    let mut config = TestClaudePresenceConfig::default();
    config.privacy.show_cost = true;
    config.privacy.show_limits = true;

    let preview = super::build_claude_discord_preview_with_access(
        std::slice::from_ref(&snapshot),
        &config,
        Some(&route),
        Some(&usage),
    );
    let limits = route_limits_for_claude_presence(Some(&route));
    let (_details, live_state, _tooltip) =
        super::claude_presence_lines(&snapshot, limits.as_ref(), Some(&usage), &config);

    assert_eq!(preview.state, live_state);
    assert!(preview.state.contains("5h 20% used"));
    assert!(preview.state.contains("7d 35% used"));
    assert!(preview.state.contains("Extra $10.00/$75.00"));
}

#[test]
fn claude_presence_candidate_keeps_background_work_when_token_event_is_stale() {
    let mut snapshot = sample_claude_snapshot("claude-opus-4-8");
    snapshot.last_token_event_at = Some(chrono::Utc::now() - chrono::Duration::minutes(20));
    snapshot.last_activity = SystemTime::now();
    snapshot.background_work = cc_discord_presence::workflow_state::BackgroundWorkInfo {
        workflow_active: true,
        active_agent_count: 1,
        latest_signal_at: Some(snapshot.last_activity),
    };

    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(600))
        .expect("cutoff");
    let cutoff_chrono = chrono::Utc::now() - chrono::Duration::seconds(600);

    assert!(super::is_claude_presence_candidate(
        &snapshot,
        cutoff,
        cutoff_chrono
    ));
}

#[test]
fn claude_session_info_flags_inflated_tokenizer_for_sonnet_5_and_opus_4_7_plus() {
    for model_id in ["claude-sonnet-5", "claude-opus-4-8"] {
        let snapshots = [sample_claude_snapshot(model_id)];
        let infos = build_claude_session_infos(&snapshots);
        assert!(infos[0].has_inflated_tokenizer, "{model_id}");
    }
}

#[test]
fn claude_session_info_does_not_flag_inflated_tokenizer_for_sonnet_4_6() {
    let snapshots = [sample_claude_snapshot("claude-sonnet-4-6")];
    let infos = build_claude_session_infos(&snapshots);

    assert!(!infos[0].has_inflated_tokenizer);
}

#[test]
fn codex_session_info_never_carries_an_inflated_tokenizer_flag() {
    let standard = build_codex_session_infos(
        &[sample_codex_snapshot()],
        &TestCodexPresenceConfig::default(),
        TestPresenceSurface::Cli,
    );

    assert!(!standard[0].has_inflated_tokenizer);
}

#[test]
fn codex_session_info_does_not_invent_subagent_count() {
    let infos = build_codex_session_infos(
        &[sample_codex_snapshot()],
        &TestCodexPresenceConfig::default(),
        TestPresenceSurface::Cli,
    );

    assert_eq!(infos[0].subagent_count, 0);
    assert!(infos[0].subagents.is_empty());
    assert_eq!(infos[0].workflow_label, None);
}

#[test]
fn session_info_context_used_tokens_reflects_current_fill_not_the_historical_peak() {
    let snapshot = ClaudeSessionSnapshot {
        max_turn_api_input: 999_486,
        current_context_tokens: 25_500,
        ..sample_claude_snapshot("claude-sonnet-5")
    };
    let infos = build_claude_session_infos(&[snapshot]);

    assert_eq!(
        infos[0].context_used_tokens, 25_500,
        "the live-session ctx-1m badge must show current fill, not the all-time peak"
    );
    assert_eq!(
        infos[0].context_window_tokens, 1_000_000,
        "the 1M-vs-200K window-size decision is unaffected -- it stays keyed off the \
             historical peak (max_turn_api_input), which correctly never decreases"
    );
}

#[test]
fn context_breakdown_used_tokens_reflects_current_fill_not_the_historical_peak() {
    let snapshot = ClaudeSessionSnapshot {
        max_turn_api_input: 999_486,
        current_context_tokens: 25_500,
        ..sample_claude_snapshot("claude-sonnet-5")
    };
    let breakdown = build_claude_context_breakdown(Some(&snapshot));

    assert_eq!(
        breakdown.used_tokens, 25_500,
        "the Context Window view must show current fill, not the session's all-time peak"
    );
    assert_eq!(breakdown.context_window, 1_000_000);
    assert_eq!(breakdown.system_prompt, 0);
    assert_eq!(breakdown.system_tools, 0);
    assert_eq!(breakdown.autocompact_buffer, 0);
    assert_eq!(breakdown.messages, breakdown.used_tokens);
    assert_eq!(breakdown.free_space, 1_000_000 - 25_500);
    assert!(
        breakdown.free_space > 0,
        "a session that genuinely emptied out after compaction must show real free space, \
             not 0 (the exact symptom Tony reported: a CRITICAL \"100% full\" recommendation \
             for a session that isn't actually full right now)"
    );

    let empty = super::empty_context_breakdown("Claude", 200_000);
    assert_eq!(empty.used_tokens, 0);
    assert_eq!(empty.autocompact_buffer, 0);
    assert_eq!(empty.free_space, 200_000);
}

#[test]
fn plan_key_from_override_accepts_display_labels_and_auto() {
    assert_eq!(plan_key_from_override("Max 20x ($200/mo)"), Some("max_20x"));
    assert_eq!(plan_key_from_override("Max 5x ($100/mo)"), Some("max_5x"));
    assert_eq!(plan_key_from_override("  Teams plan  "), Some("team"));
    assert_eq!(plan_key_from_override("enterprise"), Some("enterprise"));
    assert_eq!(plan_key_from_override("pro monthly"), Some("pro"));
    assert_eq!(plan_key_from_override("free"), Some("free"));
    assert_eq!(plan_key_from_override("Max"), None);
    assert_eq!(plan_key_from_override("auto"), None);
    assert_eq!(plan_key_from_override(""), None);
}

#[test]
fn codex_plan_key_maps_detected_tiers_to_frontend_contract() {
    assert_eq!(codex_plan_key_from_tier(DetectedPlanTier::Free), "free");
    assert_eq!(codex_plan_key_from_tier(DetectedPlanTier::Go), "go");
    assert_eq!(codex_plan_key_from_tier(DetectedPlanTier::Plus), "plus");
    assert_eq!(
        codex_plan_key_from_tier(DetectedPlanTier::Business),
        "business"
    );
    assert_eq!(
        codex_plan_key_from_tier(DetectedPlanTier::Enterprise),
        "enterprise"
    );
    assert_eq!(codex_plan_key_from_tier(DetectedPlanTier::Pro5x), "pro_5x");
    assert_eq!(
        codex_plan_key_from_tier(DetectedPlanTier::Pro20x),
        "pro_20x"
    );
    assert_eq!(codex_plan_key_from_tier(DetectedPlanTier::Unknown), "");
}

fn sample_codex_snapshot() -> CodexSessionSnapshot {
    CodexSessionSnapshot {
        session_id: "session".into(),
        cwd: PathBuf::from("D:/X/Web Development/MCP Servers/cc-discord-presence"),
        project_name: "pulse".into(),
        git_branch: None,
        model: Some("gpt-5.4".into()),
        originator: None,
        source: None,
        reasoning_effort: None,
        speed: SessionSpeed::default(),
        approval_policy: None,
        sandbox_policy: None,
        session_total_tokens: Some(54_764_083),
        last_turn_tokens: None,
        session_delta_tokens: None,
        input_tokens_total: 54_626_018,
        cached_input_tokens_total: 52_219_136,
        cache_write_tokens_total: None,
        output_tokens_total: 138_065,
        last_input_tokens: None,
        last_cached_input_tokens: None,
        last_output_tokens: None,
        total_cost_usd: 0.0,
        known_cost_usd: None,
        cost_breakdown: TokenCostBreakdown {
            input_cost_usd: 0.0,
            cache_write_cost_usd: 0.0,
            cached_input_cost_usd: 0.0,
            output_cost_usd: 0.0,
            cached_input_savings_usd: 0.0,
        },
        pricing_source: PricingSource::Fallback,
        pricing_status: PricingStatus::Unavailable,
        cost_attribution: CostAttribution::SingleModel,
        cost_breakdown_reconciled: false,
        context_window: None,
        limits: RateLimits::default(),
        rate_limit_envelopes: Vec::new(),
        activity: None,
        started_at: None,
        last_token_event_at: None,
        last_activity: SystemTime::UNIX_EPOCH,
        source_file: PathBuf::from("C:/Users/xt0n1/.codex/sessions/sample.jsonl"),
    }
}

#[test]
fn codex_total_input_tokens_uses_telemetry_total_without_double_counting_cache() {
    let snapshot = sample_codex_snapshot();
    assert_eq!(codex_total_input_tokens(&snapshot), 54_626_018);
}

#[test]
fn codex_session_info_uses_the_canonical_effective_context_for_gpt_5_4() {
    // The snapshot carries no observed context window, so resolution falls
    // back to the model catalogue. Pin it here: without an explicit window
    // the resolver also consults ~/.codex/models_cache.json, which makes
    // the expected value depend on whichever Codex build last wrote that
    // file on the developer's machine.
    let mut snapshot = sample_codex_snapshot();
    snapshot.context_window = Some(ContextWindowSnapshot {
        raw_window_tokens: 272_000,
        window_tokens: 258_400,
        effective_percent: Some(95),
        used_tokens: 0,
        remaining_tokens: 258_400,
        remaining_percent: 100.0,
        source: ContextWindowSource::Event,
        raw_source: ContextWindowSource::Event,
    });

    let infos = build_codex_session_infos(
        &[snapshot],
        &TestCodexPresenceConfig::default(),
        TestPresenceSurface::Cli,
    );

    assert_eq!(infos[0].context_window, "258.4K");
    assert_eq!(infos[0].context_window_tokens, 258_400);
}

#[test]
fn codex_discord_preview_idles_as_codex_app_when_desktop_is_running() {
    let config = TestCodexPresenceConfig::default();
    let preview = build_codex_discord_preview(&[], &config, true);

    assert_eq!(preview.app_name, "Codex App");
    assert_eq!(preview.details, "Codex App");
    assert_eq!(preview.state, "Idling...");
    assert!(!preview.has_session);
}

#[test]
fn codex_discord_preview_uses_selected_chatgpt_identity_and_hides_branch() {
    let mut snapshot = sample_codex_snapshot();
    snapshot.model = Some("gpt-5.6-sol".into());
    snapshot.reasoning_effort = Some(cc_discord_presence::codex::model::ReasoningEffort::Max);
    snapshot.git_branch = Some("main".into());
    snapshot.originator = Some("Codex Desktop".into());
    let mut config = TestCodexPresenceConfig::default();
    config.display.desktop_presence_design = DesktopPresenceDesign::ChatGptApp;
    config.privacy.show_git_branch = false;
    config.openai_plan.mode = OpenAiPlanMode::Manual;

    let preview = build_codex_discord_preview(&[snapshot], &config, true);

    assert_eq!(preview.app_name, "ChatGPT App");
    assert_eq!(preview.large_image_key, "codex-logo");
    assert_eq!(preview.large_text, "ChatGPT App");
    assert!(!preview.details.contains("main"));
    assert!(
        preview
            .state
            .starts_with("GPT-5.6 Sol · Max | Pro 20x ($200/month)")
    );
}

#[test]
fn discord_preview_and_live_share_access_route_for_quota_and_cost() {
    let now = chrono::Utc::now();
    let mut snapshot = sample_codex_snapshot();
    snapshot.known_cost_usd = Some(4.25);
    snapshot.pricing_status = PricingStatus::Exact;
    snapshot.cost_breakdown = TokenCostBreakdown {
        input_cost_usd: 1.25,
        cache_write_cost_usd: 0.0,
        cached_input_cost_usd: 1.0,
        output_cost_usd: 2.0,
        cached_input_savings_usd: 0.0,
    };
    snapshot.limits = RateLimits::new(vec![
        cc_discord_presence::codex::telemetry::limits::UsageWindow {
            used_percent: 99.0,
            remaining_percent: 1.0,
            window_minutes: 300,
            resets_at: None,
        },
        cc_discord_presence::codex::telemetry::limits::UsageWindow {
            used_percent: 88.0,
            remaining_percent: 12.0,
            window_minutes: 10_080,
            resets_at: None,
        },
    ]);
    let route = access_route_from_usage(
        subscription_source("codex", Some("Pro 20x".to_string())),
        UsageSnapshot {
            source: UsageSource::new(
                "codex-subscription:default",
                [UsageSignal::CodexSubscriptionUsage],
            ),
            scopes: vec![QuotaScope {
                id: Some("codex".to_string()),
                name: Some("Global account quota".to_string()),
                kind: RateLimitScope::GlobalAccount,
                windows: vec![
                    QuotaWindow {
                        window_minutes: 300,
                        used_percent: 17.0,
                        remaining_percent: 83.0,
                        resets_at: None,
                    },
                    QuotaWindow {
                        window_minutes: 10_080,
                        used_percent: 42.0,
                        remaining_percent: 58.0,
                        resets_at: None,
                    },
                ],
            }],
            credits: None,
            observed_at: Some(now),
            provenance_source: "app_server".to_string(),
        },
        now,
        chrono::Duration::seconds(30),
        now,
    );
    let mut config = TestCodexPresenceConfig::default();
    config.privacy.show_limits = true;
    config.privacy.show_cost = true;
    let preview =
        build_codex_discord_preview_with_access(&[snapshot.clone()], &config, false, Some(&route));
    let plan = PlanDetector::with_cache_path(tempfile::tempdir().unwrap().path().join("plan.json"))
        .resolve_from_sessions(&[snapshot.clone()], &config.openai_plan);
    let service_tier = resolve_service_tier();
    let live = active_presence_presentation(
        codex_session_surface(&snapshot, codex_fallback_surface(false)),
        &snapshot,
        route_limits_for_presence(Some(&route)).as_ref(),
        &plan,
        &service_tier,
        &config,
    );

    assert_eq!(preview.details, live.details);
    assert_eq!(preview.state, live.state);
    assert!(preview.state.contains("5h 83%"));
    assert!(preview.state.contains("7d 58%"));
    assert!(preview.state.contains("$4.25"));

    let mut stale = route.clone();
    stale.freshness = AccessFreshness::Stale;
    let stale_preview =
        build_codex_discord_preview_with_access(&[snapshot], &config, false, Some(&stale));
    assert!(!stale_preview.state.contains("5h"));
    assert!(!stale_preview.state.contains("7d"));
    assert!(stale_preview.state.contains("$4.25"));
}

#[test]
fn codex_weekly_and_model_weekly_do_not_duplicate_the_global_window() {
    let now = chrono::Utc::now();
    let route = access_route_from_usage(
        subscription_source("codex", None),
        UsageSnapshot {
            source: UsageSource::new(
                "codex-subscription:default",
                [UsageSignal::CodexSubscriptionUsage],
            ),
            scopes: vec![
                QuotaScope {
                    id: Some("codex".to_string()),
                    name: None,
                    kind: RateLimitScope::GlobalAccount,
                    windows: vec![QuotaWindow {
                        window_minutes: 10_080,
                        used_percent: 52.0,
                        remaining_percent: 48.0,
                        resets_at: Some(now + chrono::Duration::days(5)),
                    }],
                },
                QuotaScope {
                    id: Some("codex_bengalfox".to_string()),
                    name: Some("GPT-5.3-Codex-Spark".to_string()),
                    kind: RateLimitScope::ModelScoped,
                    windows: vec![QuotaWindow {
                        window_minutes: 10_080,
                        used_percent: 0.0,
                        remaining_percent: 100.0,
                        resets_at: Some(now + chrono::Duration::days(7)),
                    }],
                },
            ],
            credits: None,
            observed_at: Some(now),
            provenance_source: "app_server".to_string(),
        },
        now,
        chrono::Duration::seconds(30),
        now,
    );

    let (primary, secondary) = super::route_window_slots(&route);
    assert!(primary.is_none(), "weekly must not be reused as a 5h slot");
    assert_eq!(secondary.map(|window| window.key.as_str()), Some("weekly"));

    let legacy = super::rate_limit_info_from_access(&route);
    assert_eq!(legacy.five_hour_window_minutes, None);
    assert_eq!(legacy.seven_day_window_minutes, Some(10_080));
}

#[test]
fn codex_model_scoped_five_hour_is_not_promoted_to_global_discord() {
    let now = chrono::Utc::now();
    let route = access_route_from_usage(
        subscription_source("codex", None),
        UsageSnapshot {
            source: UsageSource::new(
                "codex-subscription:default",
                [UsageSignal::CodexSubscriptionUsage],
            ),
            scopes: vec![
                QuotaScope {
                    id: Some("codex".to_string()),
                    name: None,
                    kind: RateLimitScope::GlobalAccount,
                    windows: vec![QuotaWindow {
                        window_minutes: 10_080,
                        used_percent: 5.0,
                        remaining_percent: 95.0,
                        resets_at: Some(now + chrono::Duration::days(7)),
                    }],
                },
                QuotaScope {
                    id: Some("codex_bengalfox".to_string()),
                    name: Some("GPT-5.3-Codex-Spark".to_string()),
                    kind: RateLimitScope::ModelScoped,
                    windows: vec![
                        QuotaWindow {
                            window_minutes: 300,
                            used_percent: 0.0,
                            remaining_percent: 100.0,
                            resets_at: Some(now + chrono::Duration::hours(5)),
                        },
                        QuotaWindow {
                            window_minutes: 10_080,
                            used_percent: 0.0,
                            remaining_percent: 100.0,
                            resets_at: Some(now + chrono::Duration::days(7)),
                        },
                    ],
                },
            ],
            credits: None,
            observed_at: Some(now),
            provenance_source: "app_server".to_string(),
        },
        now,
        chrono::Duration::seconds(30),
        now,
    );

    let limits = route_limits_for_presence(Some(&route)).expect("fresh global quota");
    assert_eq!(
        limits.primary().map(|window| window.window_minutes),
        Some(10_080),
        "a model-scoped 5h window must not become the global Discord primary"
    );
    assert_eq!(
        limits.primary().map(|window| window.remaining_percent),
        Some(95.0)
    );
    assert!(limits.secondary().is_none());

    let mut config = TestCodexPresenceConfig::default();
    config.privacy.show_limits = true;
    let preview = build_codex_discord_preview_with_access(
        &[sample_codex_snapshot()],
        &config,
        false,
        Some(&route),
    );
    assert!(
        preview.state.contains("7d 95% remaining"),
        "{}",
        preview.state
    );
    assert!(!preview.state.contains("5h"), "{}", preview.state);
}

#[test]
fn context_recommendation_maps_each_tier_at_boundaries() {
    use super::{
        CONTEXT_COMPACT_NOW_PCT, CONTEXT_COMPACT_SOON_PCT, CONTEXT_WATCH_PCT,
        context_recommendation,
    };

    assert_eq!(CONTEXT_WATCH_PCT, 50.0);
    assert_eq!(CONTEXT_COMPACT_SOON_PCT, 80.0);
    assert_eq!(CONTEXT_COMPACT_NOW_PCT, 95.0);

    let healthy = context_recommendation(49.0);
    let watch = context_recommendation(50.0);
    let soon = context_recommendation(80.0);
    let now = context_recommendation(95.0);
    let full = context_recommendation(100.0);

    assert!(healthy.contains("healthy"));
    assert!(watch.contains("half"));
    assert!(soon.contains("compact soon"));
    assert!(now.contains("compact now"));
    assert!(full.contains("compact now"));

    assert_ne!(healthy, watch);
    assert_ne!(watch, soon);
    assert_ne!(soon, now);
}

#[test]
fn codex_session_info_does_not_multiply_canonical_fast_cost_twice() {
    use super::build_codex_session_infos;

    let mut snapshot = sample_codex_snapshot();
    snapshot.model = Some("gpt-5.5".into());
    snapshot.speed = SessionSpeed::explicit(SpeedMode::Fast, SpeedSource::ThreadSettings);
    snapshot.total_cost_usd = 4.0;
    snapshot.known_cost_usd = Some(4.0);
    snapshot.pricing_status = PricingStatus::Exact;
    snapshot.cost_breakdown = TokenCostBreakdown {
        input_cost_usd: 1.0,
        cache_write_cost_usd: 0.0,
        cached_input_cost_usd: 0.5,
        output_cost_usd: 2.5,
        cached_input_savings_usd: 4.5,
    };

    let info = build_codex_session_infos(
        &[snapshot],
        &TestCodexPresenceConfig::default(),
        TestPresenceSurface::Cli,
    );

    assert!((info[0].cost - 4.0).abs() < 0.0001);
    assert!((info[0].input_cost - 1.0).abs() < 0.0001);
    assert!((info[0].output_cost - 2.5).abs() < 0.0001);
    assert!((info[0].cache_read_cost - 0.5).abs() < 0.0001);
    assert!(info[0].cost_available);
    assert_eq!(info[0].cost_basis, "exact");
    assert!(info[0].fast);
}

#[test]
fn codex_session_info_preserves_partial_cost_from_shared_metrics() {
    let mut snapshot = sample_codex_snapshot();
    snapshot.known_cost_usd = Some(0.454);
    snapshot.total_cost_usd = 0.454;
    snapshot.pricing_status = PricingStatus::Partial;
    snapshot.cost_breakdown = TokenCostBreakdown {
        input_cost_usd: 0.2,
        output_cost_usd: 0.254,
        ..TokenCostBreakdown::default()
    };

    let info = build_codex_session_infos(
        &[snapshot],
        &TestCodexPresenceConfig::default(),
        TestPresenceSurface::Cli,
    );

    assert_eq!(info[0].cost, 0.454);
    assert_eq!(info[0].input_cost, 0.2);
    assert_eq!(info[0].output_cost, 0.254);
    assert!(info[0].cost_available);
    assert_eq!(info[0].cost_basis, "partial");
}

/// Builds a history row that landed `days_ago` days before today.
/// A SQL daily total placed `days_ago` calendar days before today.
fn cost_row(days_ago: i64, cost: f64, sessions: i64) -> crate::db::DailyCostRow {
    let date = chrono::Utc::now().date_naive() - chrono::Duration::days(days_ago);
    crate::db::DailyCostRow {
        date: date.format("%Y-%m-%d").to_string(),
        cost,
        sessions,
        priced_sessions: sessions,
        cost_basis: crate::db::CostBasis::Exact,
        cost_sources: vec!["fixture".to_string()],
    }
}

/// A stored session dated `days_ago` calendar days before today.
fn session_on_day(days_ago: i64, cost: f64) -> crate::db::HistoricalSession {
    let date = chrono::Utc::now().date_naive() - chrono::Duration::days(days_ago);
    crate::db::HistoricalSession {
        started_at: Some(format!("{}T12:00:00Z", date.format("%Y-%m-%d"))),
        total_cost: cost,
        cost_basis: crate::db::CostBasis::Exact,
        cost_source: "fixture".to_string(),
        known_cost: Some(cost),
        ..Default::default()
    }
}

#[test]
fn daily_cost_series_returns_one_point_per_day_oldest_first() {
    let series = daily_cost_series(&[], 7);
    assert_eq!(series.len(), 7);
    let dates: Vec<&str> = series.iter().map(|p| p.date.as_str()).collect();
    let mut sorted = dates.clone();
    sorted.sort_unstable();
    assert_eq!(dates, sorted, "series must run oldest to newest");
}

/// A day without sessions is a real "you spent nothing" observation, so it
/// must appear as a zero point rather than vanish from the timeline.
#[test]
fn daily_cost_series_zero_fills_days_without_sessions() {
    let series = daily_cost_series(&[cost_row(1, 12.5, 1)], 5);
    assert_eq!(series.len(), 5);
    let zero_days = series.iter().filter(|p| p.cost == 0.0).count();
    assert_eq!(zero_days, 4);
    let spend_day = series.iter().find(|p| p.cost > 0.0).expect("spend day");
    assert!((spend_day.cost - 12.5).abs() < 0.000_001);
    assert_eq!(spend_day.sessions, 1);
}

#[test]
fn daily_cost_series_carries_the_aggregated_day_total() {
    let series = daily_cost_series(&[cost_row(2, 8.0, 3)], 5);
    let day = series.iter().find(|p| p.sessions > 0).expect("busy day");
    assert!((day.cost - 8.0).abs() < 0.000_001);
    assert_eq!(day.sessions, 3);
    assert_eq!(day.priced_sessions, 3);
    assert_eq!(day.cost_basis, crate::db::CostBasis::Exact);
    assert_eq!(day.cost_sources, vec!["fixture".to_string()]);
}

#[test]
fn report_windows_preserve_cost_coverage_for_7_30_90_and_365_days() {
    for days in [7, 30, 90, 365] {
        let series = daily_cost_series(&[cost_row(0, 4.25, 2)], days);
        assert_eq!(series.len(), days as usize);
        let current = series.last().expect("current day");
        assert_eq!(current.cost, 4.25);
        assert_eq!(current.sessions, 2);
        assert_eq!(current.priced_sessions, 2);
        assert_eq!(current.cost_basis, crate::db::CostBasis::Exact);
    }
}

#[test]
fn cached_reports_bundle_reuses_fresh_bundle_for_same_key() {
    let _test_guard = REPORTS_BUNDLE_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *REPORTS_BUNDLE_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    let builds = Cell::new(0);
    let project = Some("cache-test-same-key-7717".to_string());
    let provider = "all-cache-test-7717".to_string();

    let first = cached_reports_bundle(7717, project.clone(), provider.clone(), || {
        builds.set(builds.get() + 1);
        build_reports_bundle_for_scope_from_roots(
            &provider,
            Some(7717),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    });
    let second = cached_reports_bundle(7717, project, provider, || {
        builds.set(builds.get() + 1);
        unreachable!("fresh matching cache entry should skip the builder")
    });

    assert_eq!(builds.get(), 1);
    assert_eq!(
        serde_json::to_value(first).expect("serialize first bundle"),
        serde_json::to_value(second).expect("serialize cached bundle")
    );
}

#[test]
fn cached_reports_bundle_rebuilds_for_different_key() {
    let _test_guard = REPORTS_BUNDLE_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *REPORTS_BUNDLE_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    let builds = Cell::new(0);

    for (days, project) in [
        (7717, Some("cache-test-key-a-7717".to_string())),
        (7718, Some("cache-test-key-b-7718".to_string())),
    ] {
        cached_reports_bundle(
            days,
            project,
            "all-cache-test-key-change".to_string(),
            || {
                builds.set(builds.get() + 1);
                build_reports_bundle_for_scope_from_roots(
                    "all-cache-test-key-change",
                    Some(i64::from(days)),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                )
            },
        );
    }

    assert_eq!(builds.get(), 2);
}

#[test]
fn all_provider_report_bundle_aggregates_without_cross_provider_guidance() {
    let mut codex = session_on_day(0, 2.0);
    codex.id = "codex:scope".to_string();
    codex.provider = "codex".to_string();
    codex.model = "GPT-5.6 Sol".to_string();
    let mut claude = session_on_day(0, 7.0);
    claude.id = "claude:scope".to_string();
    claude.provider = "claude".to_string();
    claude.model = "Claude Opus 5".to_string();

    let bundle = build_reports_bundle_for_scope_from_roots(
        "all",
        Some(7),
        vec![codex, claude],
        vec![cost_row(0, 9.0, 2)],
        Vec::new(),
        Vec::new(),
    );

    assert_eq!(bundle.provider, "all");
    assert_eq!(bundle.total_sessions, 2);
    assert_eq!(bundle.priced_sessions, 2);
    assert_eq!(bundle.cost_basis, crate::db::CostBasis::Exact);
    assert_eq!(bundle.daily_costs.last().expect("today").cost, 9.0);
    assert!(bundle.model_routing.is_none());
    assert!(bundle.recommendations.is_empty());
    assert_eq!(bundle.trace_overview.provider_display, "All providers");
    assert!(
        bundle
            .cache_health
            .diagnosis
            .contains("aggregated across All providers")
    );
}

#[test]
fn daily_cost_series_ignores_sessions_outside_the_window() {
    // 60 days ago is outside a 7-day window; the series stays flat.
    let series = daily_cost_series(&[cost_row(60, 40.0, 1)], 7);
    assert_eq!(series.len(), 7);
    assert!(series.iter().all(|p| p.cost == 0.0));
}

/// The first plotted date is the same day the SQL aggregate starts from,
/// so a session on the boundary day is charted instead of silently
/// disagreeing with the analyzers that counted it.
#[test]
fn daily_cost_series_plots_the_boundary_day_of_the_window() {
    let series = daily_cost_series(&[cost_row(6, 7.25, 2)], 7);
    let first = series.first().expect("first day");
    assert_eq!(first.date, cost_row(6, 0.0, 0).date);
    assert!((first.cost - 7.25).abs() < 0.000_001);
    assert_eq!(first.sessions, 2);
}

/// KPIs must cover every session in the window, not just the page the
/// table happens to display.
#[test]
fn cost_totals_sum_every_session_in_the_window() {
    let sessions: Vec<_> = (0..250).map(|i| session_on_day(i % 30, 2.0)).collect();
    let totals = aggregate_cost_totals(30, &sessions);
    assert_eq!(totals.sessions, 250);
    assert!((totals.total_cost - 500.0).abs() < 0.000_001);
}

/// The four cost categories must reconcile with the headline total, or the
/// "Cost by type" breakdown silently disagrees with "Total spent".
#[test]
fn cost_totals_categories_reconcile_with_the_headline() {
    let mut a = session_on_day(1, 0.0);
    a.input_cost = 1.5;
    a.output_cost = 2.0;
    a.cache_write_cost = 0.75;
    a.cache_read_cost = 0.25;
    a.total_cost = 4.5;
    a.known_cost = Some(4.5);

    let totals = aggregate_cost_totals(30, &[a]);
    let by_category =
        totals.input_cost + totals.output_cost + totals.cache_write_cost + totals.cache_read_cost;
    assert!((by_category - totals.total_cost).abs() < 0.000_001);
}

#[test]
fn cost_totals_are_zero_for_an_empty_window() {
    let totals = aggregate_cost_totals(7, &[]);
    assert_eq!(totals.sessions, 0);
    assert_eq!(totals.total_cost, 0.0);
    assert_eq!(totals.days, 7);
    assert_eq!(
        totals.cost_basis,
        crate::db::CostBasis::Unavailable,
        "an empty window is not an exact zero-cost observation"
    );
}

#[test]
fn cost_totals_keep_partial_coverage_and_ignore_unavailable_raw_costs() {
    let mut exact = session_on_day(1, 2.5);
    exact.cost_basis = crate::db::CostBasis::Exact;
    exact.cost_source = "session-calculated".to_string();
    exact.known_cost = Some(2.5);

    let mut unavailable = session_on_day(1, 99.0);
    unavailable.cost_basis = crate::db::CostBasis::Unavailable;
    unavailable.cost_source = "unknown".to_string();
    unavailable.known_cost = None;

    let totals = aggregate_cost_totals(30, &[exact, unavailable]);

    assert_eq!(totals.cost_basis, crate::db::CostBasis::Partial);
    assert_eq!(totals.sessions, 2);
    assert_eq!(totals.priced_sessions, 1);
    assert_eq!(totals.total_cost, 2.5);
    assert_eq!(totals.cost_sources, vec!["session-calculated".to_string()]);
    assert_eq!(totals.billed_spend_usd, None);
    assert_eq!(totals.api_equivalent_usd, Some(2.5));
    assert_eq!(totals.billed_sessions, 0);
    assert_eq!(totals.api_equivalent_sessions, 1);
}

#[test]
fn cost_totals_keep_billed_and_api_equivalent_values_in_separate_fields() {
    let mut billed = session_on_day(1, 10.0);
    billed.cost_basis = crate::db::CostBasis::Exact;
    billed.cost_source = "provider_billed".to_string();
    billed.known_cost = Some(10.0);

    let mut estimated = session_on_day(1, 90.0);
    estimated.cost_basis = crate::db::CostBasis::Estimated;
    estimated.cost_source = "api_equivalent".to_string();
    estimated.known_cost = Some(90.0);

    let totals = aggregate_cost_totals(30, &[billed, estimated]);

    assert_eq!(totals.total_cost, 100.0);
    assert_eq!(totals.billed_spend_usd, Some(10.0));
    assert_eq!(totals.api_equivalent_usd, Some(90.0));
    assert_eq!(totals.billed_sessions, 1);
    assert_eq!(totals.api_equivalent_sessions, 1);
}

#[test]
fn cost_totals_keep_usage_breakdown_when_cost_is_unavailable() {
    let mut unavailable = session_on_day(1, 99.0);
    unavailable.cost_basis = crate::db::CostBasis::Unavailable;
    unavailable.cost_source = "unknown".to_string();
    unavailable.known_cost = None;
    unavailable.input_tokens = 100_000;
    unavailable.output_tokens = 20_000;
    unavailable.cache_write_tokens = 5_000;
    unavailable.cache_read_tokens = 75_000;
    unavailable.total_tokens = 200_000;

    let totals = aggregate_cost_totals(30, &[unavailable]);

    assert_eq!(totals.cost_basis, crate::db::CostBasis::Unavailable);
    assert_eq!(totals.input_tokens, 100_000);
    assert_eq!(totals.output_tokens, 20_000);
    assert_eq!(totals.cache_write_tokens, 5_000);
    assert_eq!(totals.cache_read_tokens, 75_000);
    assert_eq!(totals.total_tokens, 200_000);
}

#[test]
fn metrics_cost_availability_distinguishes_empty_partial_and_exact_zero() {
    assert_eq!(
        cost_availability(&[]),
        (false, "unavailable".to_string()),
        "an empty session set has no cost observation"
    );

    let mut exact = build_claude_session_infos(&[sample_claude_snapshot("claude-opus-4-8")]);
    exact[0].cost = 0.0;
    exact[0].cost_basis = "exact".into();
    assert_eq!(cost_availability(&exact), (true, "exact".to_string()));

    let mut mixed = build_claude_session_infos(&[sample_claude_snapshot("claude-opus-4-8")]);
    mixed[0].cost_available = false;
    mixed[0].cost_basis = "unavailable".to_string();
    assert_eq!(
        cost_availability(&mixed),
        (false, "unavailable".to_string())
    );
    mixed[0].cost_available = true;
    mixed[0].cost_basis = "partial".into();
    assert_eq!(cost_availability(&mixed), (true, "partial".to_string()));
}

/// The cache-savings estimate multiplies a token count by a per-token rate.
/// Both must come from this aggregate, so `pure_input_tokens` has to be the
/// denominator that matches `input_cost` over the same sessions.
#[test]
fn cost_totals_expose_pure_input_tokens_for_the_input_rate() {
    let mut s = session_on_day(1, 0.0);
    s.input_tokens = 100_000;
    s.cache_write_tokens = 30_000;
    s.cache_read_tokens = 50_000;
    s.input_cost = 0.1;

    let totals = aggregate_cost_totals(30, &[s]);
    // 100k total input minus 30k written minus 50k read = 20k pure.
    assert_eq!(totals.pure_input_tokens, 20_000);
    assert_eq!(totals.cache_read_tokens, 50_000);

    let rate = totals.input_cost / totals.pure_input_tokens as f64;
    assert!((rate - 0.000_005).abs() < 1e-9, "rate was {rate}");
}

/// Cache-heavy sessions can report more cached than raw input tokens; the
/// pure-input count must clamp at zero rather than going negative and
/// flipping the derived rate.
#[test]
fn cost_totals_never_report_negative_pure_input() {
    let mut s = session_on_day(1, 0.0);
    s.input_tokens = 1_000;
    s.cache_write_tokens = 5_000;
    s.cache_read_tokens = 9_000;

    let totals = aggregate_cost_totals(30, &[s]);
    assert_eq!(totals.pure_input_tokens, 0);
}

/// Per-model and per-project breakdowns must reconcile with the headline.
/// Summing only the visible page made the bars claim $7.72 on a window
/// whose real spend was $7,371.
#[test]
fn cost_totals_breakdowns_reconcile_with_the_headline() {
    let mut a = session_on_day(1, 10.0);
    a.model = "Claude Opus 5".into();
    a.project = "pulse".into();
    let mut b = session_on_day(2, 4.0);
    b.model = "Claude Sonnet 5".into();
    b.project = "pulse".into();
    let mut c = session_on_day(3, 6.0);
    c.model = "Claude Opus 5".into();
    c.project = "other".into();

    let totals = aggregate_cost_totals(30, &[a, b, c]);

    let model_sum: f64 = totals.by_model.iter().map(|m| m.cost).sum();
    let project_sum: f64 = totals.by_project.iter().map(|p| p.cost).sum();
    assert!((model_sum - totals.total_cost).abs() < 0.000_001);
    assert!((project_sum - totals.total_cost).abs() < 0.000_001);

    // Highest spend first, with sessions grouped rather than listed.
    assert_eq!(totals.by_model[0].label, "Claude Opus 5");
    assert!((totals.by_model[0].cost - 16.0).abs() < 0.000_001);
    assert_eq!(totals.by_model[0].sessions, 2);
    assert_eq!(totals.by_project[0].label, "pulse");
    assert!((totals.by_project[0].cost - 14.0).abs() < 0.000_001);
}

/// Runs only as an isolated process because poisoning the global cache is
/// intentionally irreversible for that test binary.
#[test]
#[ignore = "poisons process-global shared state; run this test alone"]
fn poisoned_shared_state_fails_the_entire_snapshot_instead_of_fabricating_defaults() {
    let _homes = isolated_homes();
    let shared = super::shared();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = shared.lock().expect("lock shared state before poison");
        panic!("simulated poller panic while holding shared state");
    }));

    let Err(error) = super::build_app_snapshot("live") else {
        panic!("a poisoned core state must fail the whole snapshot");
    };
    assert!(error.contains("shared snapshot state is unavailable"));
}
#[test]
fn every_plan_override_survives_reload_and_matches_the_discord_payload() {
    let _homes = isolated_homes();
    super::set_active_provider("codex".into()).unwrap();
    let mut session = sample_codex_snapshot();
    session.model = Some("gpt-6-astra".into());
    let automatic = codex_presence_core::RateLimitEnvelope {
        plan_type: Some("prolite".into()),
        scope: RateLimitScope::GlobalAccount,
        observed_at: Some(chrono::Utc::now()),
        ..Default::default()
    };
    if let Ok(mut data) = super::shared().lock() {
        data.codex_plan_envelopes = vec![automatic.clone()];
    }
    for (key, label) in [
        ("free", "Free"),
        ("go", "Go"),
        ("plus", "Plus"),
        ("pro_5x", "Pro 5x"),
        ("pro_20x", "Pro 20x"),
        ("business", "Business"),
        ("enterprise", "Enterprise"),
        ("edu", "Edu"),
    ] {
        super::set_plan_override(key.into(), Some("codex".into())).unwrap();
        super::seed_discord_state_from_disk();
        let config = TestCodexPresenceConfig::load_or_init().unwrap();
        let plan = super::resolve_codex_plan(
            &[session.clone()],
            &config,
            std::slice::from_ref(&automatic),
        );
        let info = super::get_plan_info();
        assert_eq!(info.plan_key, key);
        assert!(!info.detected);
        assert!(info.plan_name.starts_with(label));
        let preview = super::build_codex_discord_preview_with_plan(
            &[session.clone()],
            &config,
            true,
            None,
            &plan,
        );
        assert!(preview.state.contains(label), "{key}: {}", preview.state);
        assert!(!preview.state.contains("Unknown"));
    }
    assert!(super::set_plan_override("invalid".into(), Some("codex".into())).is_err());
    assert_eq!(super::get_plan_info().plan_key, "edu");
    super::set_plan_override("auto".into(), Some("codex".into())).unwrap();
    let automatic_info = super::get_plan_info();
    assert!(automatic_info.detected);
    assert_eq!(automatic_info.plan_key, "pro_5x");
}
