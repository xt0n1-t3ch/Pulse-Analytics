//! End-to-end tests of the daemon's Claude session collection over fixture transcripts.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cc_discord_presence::codex::config::PricingConfig;
use cc_discord_presence::codex::session::{
    self as codex_session, GitBranchCache as CodexGitBranchCache, ReasoningEffort as CodexEffort,
    SessionParseCache as CodexSessionParseCache,
};
use cc_discord_presence::codex::telemetry::plan::{
    DetectedPlanSource, DetectedPlanTier, ResolvedPlan,
};
use cc_discord_presence::codex::telemetry::service_tier::{ResolvedServiceTier, ServiceTier};
use cc_discord_presence::config::PresenceConfig;
use cc_discord_presence::cost::{
    self, calculate_category_costs, calculate_cost_with_context_and_speed,
};
use cc_discord_presence::discord::presence_lines;
use cc_discord_presence::session::{
    GitBranchCache, ReasoningEffort, SessionParseCache, Speed, collect_active_sessions_multi,
};

const OPUS_48: &str = "claude-opus-4-8";
const FAST_INPUT: u64 = 40_000;
const FAST_OUTPUT: u64 = 6_000;
const FAST_CACHE_WRITE: u64 = 12_000;
const FAST_CACHE_READ: u64 = 80_000;
const STANDARD_INPUT: u64 = 25_000;
const STANDARD_OUTPUT: u64 = 4_000;
const STANDARD_CACHE_WRITE: u64 = 8_000;
const STANDARD_CACHE_READ: u64 = 50_000;
const CLAUDE_SESSION_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const CODEX_SESSION_ID: &str = "codex-daemon-e2e";

struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pulse-daemon-e2e-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&path).expect("create temp root");
        Self { path }
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).ok();
    }
}

fn assistant_turn(
    model: &str,
    fast: bool,
    input: u64,
    output: u64,
    cache_write: u64,
    cache_read: u64,
) -> String {
    serde_json::json!({
        "type": "assistant",
        "timestamp": "2026-05-28T12:00:00.000Z",
        "sessionId": CLAUDE_SESSION_ID,
        "cwd": "/home/tony/pulse",
        "message": {
            "model": model,
            "usage": {
                "input_tokens": input,
                "output_tokens": output,
                "cache_creation_input_tokens": cache_write,
                "cache_read_input_tokens": cache_read,
                "speed": if fast { "fast" } else { "standard" },
                "service_tier": "priority"
            },
            "content": [{ "type": "text", "text": "working" }]
        }
    })
    .to_string()
}

fn effort_user_line() -> String {
    serde_json::json!({
        "type": "user",
        "timestamp": "2026-05-28T11:59:59.000Z",
        "sessionId": CLAUDE_SESSION_ID,
        "cwd": "/home/tony/pulse",
        "message": {
            "content": [{
                "type": "text",
                "text": "<system-reminder>reasoning effort level: high</system-reminder>"
            }]
        }
    })
    .to_string()
}

fn write_jsonl(dir: &Path, session_id: &str, lines: &[String]) {
    fs::create_dir_all(dir).expect("create session dir");
    fs::write(dir.join(format!("{session_id}.jsonl")), lines.join("\n")).expect("write jsonl");
}

fn collect_single_claude(root: &Path) -> cc_discord_presence::session::ClaudeSessionSnapshot {
    let mut git_cache = GitBranchCache::new(Duration::from_secs(30));
    let mut parse_cache = SessionParseCache::default();
    let sessions = collect_active_sessions_multi(
        &[root.to_path_buf()],
        Duration::from_secs(3600),
        Duration::from_secs(7200),
        &mut git_cache,
        &mut parse_cache,
        &[],
    )
    .expect("collect claude sessions");
    assert_eq!(sessions.len(), 1, "exactly one fixture session expected");
    sessions.into_iter().next().unwrap()
}

#[test]
fn claude_daemon_pipeline_accumulates_speed_aware_cost_and_reconciles_categories() {
    let temp = TempRoot::new("claude");
    let projects = temp.path.join("projects");
    write_jsonl(
        &projects.join("-home-tony-pulse"),
        CLAUDE_SESSION_ID,
        &[
            effort_user_line(),
            assistant_turn(
                OPUS_48,
                true,
                FAST_INPUT,
                FAST_OUTPUT,
                FAST_CACHE_WRITE,
                FAST_CACHE_READ,
            ),
            assistant_turn(
                OPUS_48,
                false,
                STANDARD_INPUT,
                STANDARD_OUTPUT,
                STANDARD_CACHE_WRITE,
                STANDARD_CACHE_READ,
            ),
        ],
    );

    let snapshot = collect_single_claude(&projects);

    assert_eq!(snapshot.session_id, CLAUDE_SESSION_ID);
    assert_eq!(snapshot.model.as_deref(), Some(OPUS_48));
    assert_eq!(
        snapshot.input_tokens,
        FAST_INPUT
            + FAST_CACHE_WRITE
            + FAST_CACHE_READ
            + STANDARD_INPUT
            + STANDARD_CACHE_WRITE
            + STANDARD_CACHE_READ
    );
    assert_eq!(snapshot.output_tokens, FAST_OUTPUT + STANDARD_OUTPUT);
    assert_eq!(
        snapshot.cache_creation_tokens,
        FAST_CACHE_WRITE + STANDARD_CACHE_WRITE
    );
    assert_eq!(
        snapshot.cache_read_tokens,
        FAST_CACHE_READ + STANDARD_CACHE_READ
    );

    let fast_turn = calculate_category_costs(
        OPUS_48,
        FAST_INPUT,
        FAST_OUTPUT,
        FAST_CACHE_WRITE,
        FAST_CACHE_READ,
        true,
    );
    let standard_turn = calculate_category_costs(
        OPUS_48,
        STANDARD_INPUT,
        STANDARD_OUTPUT,
        STANDARD_CACHE_WRITE,
        STANDARD_CACHE_READ,
        false,
    );

    let expected_input = fast_turn.input_cost + standard_turn.input_cost;
    let expected_output = fast_turn.output_cost + standard_turn.output_cost;
    let expected_cache_write = fast_turn.cache_write_cost + standard_turn.cache_write_cost;
    let expected_cache_read = fast_turn.cache_read_cost + standard_turn.cache_read_cost;
    let expected_total =
        expected_input + expected_output + expected_cache_write + expected_cache_read;

    assert!(
        (snapshot.input_cost - expected_input).abs() < 1e-9,
        "input cost"
    );
    assert!(
        (snapshot.output_cost - expected_output).abs() < 1e-9,
        "output cost"
    );
    assert!(
        (snapshot.cache_write_cost - expected_cache_write).abs() < 1e-9,
        "cache write cost"
    );
    assert!(
        (snapshot.cache_read_cost - expected_cache_read).abs() < 1e-9,
        "cache read cost"
    );
    assert!(
        (snapshot.total_cost - expected_total).abs() < 1e-9,
        "headline total"
    );

    let category_sum = snapshot.input_cost
        + snapshot.output_cost
        + snapshot.cache_write_cost
        + snapshot.cache_read_cost;
    assert!(
        (snapshot.total_cost - category_sum).abs() < 1e-9,
        "categories must reconcile with headline total"
    );

    let standalone_fast = calculate_cost_with_context_and_speed(
        OPUS_48,
        FAST_INPUT,
        FAST_OUTPUT,
        FAST_CACHE_WRITE,
        FAST_CACHE_READ,
        true,
    );
    let standalone_standard = calculate_cost_with_context_and_speed(
        OPUS_48,
        STANDARD_INPUT,
        STANDARD_OUTPUT,
        STANDARD_CACHE_WRITE,
        STANDARD_CACHE_READ,
        false,
    );
    assert!(
        (snapshot.total_cost - (standalone_fast + standalone_standard)).abs() < 1e-9,
        "accumulated total must match per-turn speed-aware totals"
    );
}

#[test]
fn claude_daemon_pipeline_tracks_last_turn_speed_effort_and_presence_markers() {
    let temp = TempRoot::new("claude-presence");
    let projects = temp.path.join("projects");
    write_jsonl(
        &projects.join("-home-tony-pulse"),
        CLAUDE_SESSION_ID,
        &[
            effort_user_line(),
            assistant_turn(
                OPUS_48,
                false,
                STANDARD_INPUT,
                STANDARD_OUTPUT,
                STANDARD_CACHE_WRITE,
                STANDARD_CACHE_READ,
            ),
            assistant_turn(
                OPUS_48,
                true,
                FAST_INPUT,
                FAST_OUTPUT,
                FAST_CACHE_WRITE,
                FAST_CACHE_READ,
            ),
        ],
    );

    let snapshot = collect_single_claude(&projects);

    assert_eq!(snapshot.speed, Speed::Fast, "last turn was fast");
    assert!(snapshot.speed.is_fast());
    assert_eq!(snapshot.reasoning_effort, ReasoningEffort::High);
    assert!(snapshot.reasoning_effort_explicit);
    assert_eq!(snapshot.service_tier.as_deref(), Some("priority"));

    let config = PresenceConfig::default();
    let (details, state, tooltip) = presence_lines(&snapshot, None, None, &config);

    assert!(details.contains("pulse"), "details: {details}");
    assert!(
        state.contains("Opus 4.8"),
        "state must carry model: {state}"
    );
    assert!(state.contains("(1M)"), "Opus 4.8 is GA 1M: {state}");
    assert!(state.contains('\u{26a1}'), "fast marker expected: {state}");
    assert!(
        state.contains(ReasoningEffort::High.label()),
        "effort label: {state}"
    );
    assert!(!tooltip.is_empty(), "tooltip carries token/cost breakdown");
    assert!(cost::is_fast_capable(OPUS_48));
}

fn codex_meta_line() -> String {
    serde_json::json!({
        "type": "session_meta",
        "payload": { "id": CODEX_SESSION_ID, "cwd": "/repo/pulse" }
    })
    .to_string()
}

fn codex_turn_context_line(timestamp: &str) -> String {
    serde_json::json!({
        "timestamp": timestamp,
        "type": "turn_context",
        "payload": { "cwd": "/repo/pulse", "model": "gpt-5.3-codex", "effort": "xhigh" }
    })
    .to_string()
}

fn codex_token_line(timestamp: &str) -> String {
    serde_json::json!({
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "total_token_usage": {
                    "input_tokens": 24_000,
                    "cached_input_tokens": 15_000,
                    "output_tokens": 6_000,
                    "total_tokens": 30_000
                },
                "last_token_usage": {
                    "input_tokens": 1_500,
                    "cached_input_tokens": 900,
                    "output_tokens": 200,
                    "total_tokens": 1_700
                },
                "model_context_window": 258_400
            }
        }
    })
    .to_string()
}

fn resolved_pro_plan() -> ResolvedPlan {
    ResolvedPlan {
        tier: DetectedPlanTier::Pro20x,
        source: DetectedPlanSource::Telemetry,
        observed_at: None,
        raw_plan_type: Some("pro_20x".to_string()),
    }
}

fn resolved_fast_tier() -> ResolvedServiceTier {
    ResolvedServiceTier {
        tier: ServiceTier::Fast,
        raw_tier: Some("fast".to_string()),
        observed_at: None,
        source_path: None,
    }
}

#[test]
fn codex_daemon_pipeline_parses_fixture_and_builds_presence_state() {
    let temp = TempRoot::new("codex");
    let sessions_root = temp.path.join("sessions");
    let turn_ts = (chrono::Utc::now() - chrono::Duration::seconds(20)).to_rfc3339();
    let token_ts = (chrono::Utc::now() - chrono::Duration::seconds(15)).to_rfc3339();
    write_jsonl(
        &sessions_root,
        CODEX_SESSION_ID,
        &[
            codex_meta_line(),
            codex_turn_context_line(&turn_ts),
            codex_token_line(&token_ts),
        ],
    );

    let mut git_cache = CodexGitBranchCache::new(Duration::from_secs(30));
    let mut parse_cache = CodexSessionParseCache::default();
    let pricing = PricingConfig::default();
    let sessions = codex_session::collect_active_sessions_multi(
        std::slice::from_ref(&sessions_root),
        Duration::from_secs(3600),
        Duration::from_secs(7200),
        &mut git_cache,
        &mut parse_cache,
        &pricing,
    )
    .expect("collect codex sessions");

    assert_eq!(
        sessions.len(),
        1,
        "exactly one codex fixture session expected"
    );
    let snapshot = &sessions[0];

    assert_eq!(snapshot.session_id, CODEX_SESSION_ID);
    assert_eq!(snapshot.model.as_deref(), Some("gpt-5.3-codex"));
    assert_eq!(snapshot.reasoning_effort, Some(CodexEffort::XHigh));
    assert_eq!(snapshot.session_total_tokens, Some(30_000));
    assert_eq!(snapshot.last_turn_tokens, Some(1_700));
    let context = snapshot.context_window.as_ref().expect("context window");
    assert_eq!(context.window_tokens, 258_400);

    let plan = resolved_pro_plan();
    let service_tier = resolved_fast_tier();
    let model_display = cc_discord_presence::codex::util::format_model_display(
        snapshot.model.as_deref().unwrap_or_default(),
        snapshot.reasoning_effort,
        service_tier.is_fast(),
    );

    assert_eq!(snapshot.project_name, "pulse");
    assert_eq!(model_display, "GPT-5.3 Codex · Extra High");
    assert!(
        !model_display.contains('\u{26a1}'),
        "unsupported Fast state must not be invented: {model_display}"
    );
    assert_eq!(plan.label(true), "Pro 20x ($200/month)");
}
