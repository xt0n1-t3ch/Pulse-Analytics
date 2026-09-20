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
