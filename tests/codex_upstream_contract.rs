//! Checks that the pinned Codex core still exposes the modules and boundaries Pulse relies on.

use std::time::{Duration, SystemTime};

use cc_discord_presence::codex::config::PresenceConfig;
use cc_discord_presence::codex::cost::{
    self, CostAttribution, PricingSource, PricingStatus, TokenCostBreakdown,
};
use cc_discord_presence::codex::model::SessionSpeed;
use cc_discord_presence::codex::session::{
    CodexSessionSnapshot, SessionActivityKind, SessionActivitySnapshot, preferred_active_session,
};
use cc_discord_presence::codex::telemetry::limits::RateLimits;
use cc_discord_presence::codex::telemetry::service_tier::{ResolvedServiceTier, ServiceTier};

#[test]
fn codex_upstream_contract_exposes_pulse_facing_modules() {
    let config = PresenceConfig::default();
    let model = "gpt-5.5";
    let fast_tier = ResolvedServiceTier {
        tier: ServiceTier::Fast,
        raw_tier: Some("fast".to_string()),
        observed_at: None,
        source_path: None,
    };

    let display = cc_discord_presence::codex::util::format_model_display(model, None, true);

    assert!(config.effective_client_id().is_some());
    assert_eq!(cost::default_model_context_window(model), Some(258_400));
    assert!(cost::speed_multiplier(model, fast_tier.is_fast()) > 1.0);
    assert!(display.contains("GPT-5.5"), "display: {display}");
    assert!(
        cc_discord_presence::codex::config::codex_home()
            .to_string_lossy()
            .contains(".codex")
    );
}

#[test]
fn codex_upstream_contract_preserves_session_selection_boundary() {
    let now = SystemTime::now();
    let older = snapshot(
        "older",
        now - Duration::from_secs(120),
        SessionActivityKind::WaitingInput,
    );
    let newer = snapshot("newer", now, SessionActivityKind::Thinking);

    let sessions = [older, newer];
    let selected = preferred_active_session(&sessions).expect("selected active session");

    assert_eq!(selected.session_id, "newer");
}

#[test]
fn codex_process_probe_remains_pulse_compatibility_glue() {
    let _ = cc_discord_presence::codex::process::is_opencode_running();
    let _ = cc_discord_presence::codex::process::is_codex_app_running();
    let _ = cc_discord_presence::codex::process::is_desktop_surface_running();

    let source = include_str!("../src/codex/process.rs");
    assert!(source.contains("Codex.exe"));
}

#[test]
fn codex_wsl_probe_commands_use_hidden_windows_launcher() {
    let source = include_str!("../src/codex/config.rs");
    let direct_spawn = ["Command::new(", "\"wsl.exe\"", ")"].concat();
    let hidden_spawn = ["crate::codex::util::silent_command(", "\"wsl.exe\"", ")"].concat();

    assert!(
        !source.contains(&direct_spawn),
        "Pulse must not reintroduce visible WSL subprocess launches from the mirrored Codex runtime"
    );
    assert_eq!(
        source.matches(&hidden_spawn).count(),
        2,
        "the mirrored Codex WSL probes must use Pulse's hidden command helper"
    );
    assert!(
        source.contains("if include_wsl_session_roots()"),
        "Pulse must not invoke mirrored Codex WSL discovery unless WSL scanning is explicitly enabled"
    );
}

#[test]
fn the_polled_branch_lookup_launches_no_git_process() {
    let parser = include_str!("../src/codex/session/parser.rs");

    assert!(
        !parser.contains(r#"Command::new("git")"#) && !parser.contains(r#"silent_command("git")"#),
        "the five-second branch lookup must not start a git process"
    );
    assert!(parser.contains("crate::git_head::branch(project_path)"));
}

/// The core pin is an immutable Git revision that the manifest and both Cargo manifests
/// must repeat; the release contract (`cargo xtask release-contract`) checks the same
/// facts against the release tag.
#[test]
fn codex_core_pin_agrees_across_the_manifest_and_both_cargo_files() {
    fn core_dependency(cargo: &str) -> &str {
        cargo
            .lines()
            .find(|line| line.starts_with("codex-presence-core"))
            .expect("Cargo manifest declares codex-presence-core")
    }
    fn quoted<'a>(line: &'a str, key: &str) -> &'a str {
        let marker = format!("{key} = \"");
        let tail = line.split(&marker).nth(1).expect("key present");
        tail.split('"').next().expect("closing quote")
    }

    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../src/codex/UPSTREAM.json")).expect("UPSTREAM.json");
    // The workspace declares the pin once; both crates inherit it.
    let root_manifest = include_str!("../Cargo.toml");
    let workspace = core_dependency(root_manifest);
    for crate_manifest in [root_manifest, include_str!("../src-tauri/Cargo.toml")] {
        assert!(
            crate_manifest
                .lines()
                .any(|line| line.trim() == "codex-presence-core.workspace = true"),
            "every crate inherits the workspace pin"
        );
    }
    let rev = manifest["integration"]["rev"]
        .as_str()
        .expect("integration rev");

    assert_eq!(manifest["schema_version"], 3);
    assert_eq!(manifest["integration"]["mode"], "git-rev");
    assert_eq!(manifest["integration"]["package"], "codex-presence-core");
    assert_eq!(manifest["canonical_commit"].as_str(), Some(rev));
    assert_eq!(rev.len(), 40, "the pin must be a full commit id");
    assert!(workspace.contains("git = "), "{workspace}");
    assert!(!workspace.contains("path = "), "{workspace}");
    assert_eq!(quoted(workspace, "rev"), rev);
    assert_eq!(
        Some(quoted(workspace, "version")),
        manifest["integration"]["version"].as_str()
    );
}

#[cfg(windows)]
#[test]
fn codex_session_paths_do_not_include_wsl_roots_without_opt_in() {
    let paths = cc_discord_presence::codex::config::sessions_paths();

    assert!(
        paths.iter().all(|path| {
            let normalized = path
                .to_string_lossy()
                .replace('\\', "/")
                .to_ascii_lowercase();
            !normalized.contains("wsl.localhost") && !normalized.contains("//wsl$")
        }),
        "Codex WSL roots must stay absent unless CODEX_PRESENCE_INCLUDE_WSL or CC_PRESENCE_INCLUDE_WSL is enabled: {paths:?}"
    );
}

fn snapshot(
    session_id: &str,
    last_activity: SystemTime,
    activity_kind: SessionActivityKind,
) -> CodexSessionSnapshot {
    CodexSessionSnapshot {
        session_id: session_id.to_string(),
        cwd: std::env::temp_dir(),
        project_name: "pulse".to_string(),
        git_branch: None,
        originator: None,
        source: None,
        model: Some("gpt-5.5".to_string()),
        reasoning_effort: None,
        speed: SessionSpeed::default(),
        approval_policy: None,
        sandbox_policy: None,
        session_total_tokens: None,
        last_turn_tokens: None,
        session_delta_tokens: None,
        input_tokens_total: 0,
        cached_input_tokens_total: 0,
        cache_write_tokens_total: None,
        output_tokens_total: 0,
        last_input_tokens: None,
        last_cached_input_tokens: None,
        last_output_tokens: None,
        total_cost_usd: 0.0,
        known_cost_usd: None,
        cost_breakdown: TokenCostBreakdown::default(),
        pricing_source: PricingSource::Fallback,
        pricing_status: PricingStatus::Unavailable,
        cost_attribution: CostAttribution::SingleModel,
        cost_breakdown_reconciled: false,
        context_window: None,
        limits: RateLimits::default(),
        rate_limit_envelopes: Vec::new(),
        activity: Some(SessionActivitySnapshot {
            kind: activity_kind,
            ..SessionActivitySnapshot::default()
        }),
        started_at: None,
        last_token_event_at: None,
        last_activity,
        source_file: std::env::temp_dir().join(format!("{session_id}.jsonl")),
    }
}
