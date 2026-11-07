//! End-to-end tests for report generation over fixture Claude transcripts.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use cc_discord_presence::provider::Provider;
use pulse::analyzers::session_trace::{self, MAX_JSONL_BYTES, load_session_traces_from_roots};
use pulse::commands::build_reports_bundle_from_roots;
use pulse::db::{CostBasis, HistoricalSession};

static SCAN_COUNTER_GUARD: Mutex<()> = Mutex::new(());

fn lock_scan_counter() -> MutexGuard<'static, ()> {
    SCAN_COUNTER_GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct Fixture {
    root: PathBuf,
    sessions: Vec<HistoricalSession>,
}

fn claude_session(id: &str, project: &str, model: &str) -> HistoricalSession {
    HistoricalSession {
        opencode: None,
        id: id.to_string(),
        provider: Provider::Claude.as_str().to_string(),
        session_name: None,
        project: project.to_string(),
        model: model.to_string(),
        model_id: model.to_string(),
        context_window: "200K".to_string(),
        branch: Some("main".to_string()),
        effort: "High".to_string(),
        speed: "standard".to_string(),
        started_at: Some("2026-05-20T14:00:00+00:00".to_string()),
        ended_at: Some("2026-05-20T14:30:00+00:00".to_string()),
        duration_secs: 1_800,
        total_cost: 1.25,
        cost_basis: CostBasis::Exact,
        cost_source: "fixture".to_string(),
        known_cost: Some(1.25),
        input_tokens: 120_000,
        output_tokens: 8_000,
        cache_write_tokens: 40_000,
        cache_read_tokens: 70_000,
        total_tokens: 128_000,
        input_cost: 0.4,
        output_cost: 0.3,
        cache_write_cost: 0.3,
        cache_read_cost: 0.25,
        has_thinking: true,
        subagent_count: 0,
        is_active: false,
        used_tokens: 120_000,
        window_tokens: 200_000,
    }
}

fn user_line(text: &str) -> String {
    serde_json::json!({
        "type": "user",
        "timestamp": "2026-05-20T14:01:00.000Z",
        "message": { "content": [{ "type": "text", "text": text }] }
    })
    .to_string()
}

fn assistant_tool_line(tool: &str) -> String {
    serde_json::json!({
        "type": "assistant",
        "timestamp": "2026-05-20T14:02:00.000Z",
        "message": { "content": [{ "type": "tool_use", "name": tool, "input": {} }] }
    })
    .to_string()
}

fn write_jsonl(dir: &Path, session_id: &str, lines: &[String]) {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("{session_id}.jsonl"));
    fs::write(path, lines.join("\n")).unwrap();
}

fn build_fixture() -> Fixture {
    let root = std::env::temp_dir().join(format!(
        "pulse-reports-e2e-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let projects = root.join("projects");

    let alpha_dir = projects.join("-home-tony-alpha");
    write_jsonl(
        &alpha_dir,
        "11111111-1111-4111-8111-111111111111",
        &[
            user_line("Refactor the auth module and add tests"),
            assistant_tool_line("Read"),
            assistant_tool_line("Edit"),
            assistant_tool_line("mcp__context-mode__ctx_search"),
        ],
    );

    let beta_dir = projects.join("-home-tony-beta");
    write_jsonl(
        &beta_dir,
        "22222222-2222-4222-8222-222222222222",
        &[
            user_line("Investigate the hanging report screen"),
            user_line("/compact"),
            assistant_tool_line("Bash"),
            assistant_tool_line("Read"),
        ],
    );

    let sessions = vec![
        claude_session(
            "11111111-1111-4111-8111-111111111111",
            "alpha",
            "claude-opus-4-8",
        ),
        claude_session(
            "22222222-2222-4222-8222-222222222222",
            "beta",
            "claude-sonnet-4-5",
        ),
    ];

    Fixture { root, sessions }
}

fn claude_roots(fixture: &Fixture) -> Vec<PathBuf> {
    vec![fixture.root.join("projects")]
}

#[test]
fn bundle_aggregates_fixture_traces() {
    let _guard = lock_scan_counter();
    let fixture = build_fixture();
    session_trace::reset_scan_passes();

    let bundle = build_reports_bundle_from_roots(
        Provider::Claude,
        Some(30),
        fixture.sessions.clone(),
        Vec::new(),
        claude_roots(&fixture),
        Vec::new(),
    );

    assert_eq!(bundle.provider, Provider::Claude.as_str());
    assert_eq!(bundle.days, 30);
    assert_eq!(bundle.total_sessions, 2);
    assert_eq!(bundle.trace_overview.total_sessions, 2);
    assert_eq!(bundle.trace_overview.traced_sessions, 2);
    assert_eq!(bundle.trace_overview.user_messages, 3);
    assert_eq!(bundle.trace_overview.assistant_messages, 5);
    assert_eq!(bundle.trace_overview.total_tool_calls, 5);
    assert_eq!(bundle.trace_overview.mcp_tool_calls, 1);
    assert_eq!(bundle.trace_overview.total_compactions, 1);

    assert!(bundle.tool_frequency.available);
    assert_eq!(bundle.tool_frequency.traced_sessions, 2);
    assert_eq!(bundle.tool_frequency.total_tool_calls, 5);
    assert_eq!(bundle.tool_frequency.mcp_tool_calls, 1);

    assert_eq!(bundle.cache_health.sessions_analyzed, 2);
    assert!(bundle.cache_health.total_cache_read > 0);

    fs::remove_dir_all(&fixture.root).ok();
}

#[test]
fn bundle_scans_jsonl_tree_exactly_once() {
    let _guard = lock_scan_counter();
    let fixture = build_fixture();
    session_trace::reset_scan_passes();

    let _ = build_reports_bundle_from_roots(
        Provider::Claude,
        Some(30),
        fixture.sessions.clone(),
        Vec::new(),
        claude_roots(&fixture),
        Vec::new(),
    );

    assert_eq!(
        session_trace::scan_passes(),
        1,
        "expected a single Claude scan pass per bundle (regression guard against double/8x scan)"
    );

    fs::remove_dir_all(&fixture.root).ok();
}

#[test]
fn codex_bundle_exposes_cache_health_without_claude_model_routing() {
    let _guard = lock_scan_counter();
    let fixture = build_fixture();
    let mut sessions = fixture.sessions.clone();
    for session in &mut sessions {
        session.provider = Provider::Codex.as_str().to_string();
        session.model = "GPT-5.6 Sol".to_string();
        session.model_id = "gpt-5.6-sol".to_string();
    }

    let bundle = build_reports_bundle_from_roots(
        Provider::Codex,
        Some(30),
        sessions,
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );

    assert!(bundle.capabilities.cache_health);
    assert!(!bundle.capabilities.model_routing);
    assert!(bundle.cache_health.sessions_analyzed > 0);
    assert!(bundle.model_routing.is_none());

    fs::remove_dir_all(&fixture.root).ok();
}

#[test]
fn oversized_jsonl_is_skipped() {
    let _guard = lock_scan_counter();
    let fixture = build_fixture();
    let projects = fixture.root.join("projects");
    let huge_id = "33333333-3333-4333-8333-333333333333";
    let huge_dir = projects.join("-home-tony-huge");
    fs::create_dir_all(&huge_dir).unwrap();
    let huge_path = huge_dir.join(format!("{huge_id}.jsonl"));
    let line = format!("{}\n", user_line("oversized payload line"));
    let repeats = (MAX_JSONL_BYTES as usize / line.len()) + 16;
    fs::write(&huge_path, line.repeat(repeats)).unwrap();
    assert!(fs::metadata(&huge_path).unwrap().len() > MAX_JSONL_BYTES);

    let mut sessions = fixture.sessions.clone();
    sessions.push(claude_session(huge_id, "huge", "claude-opus-4-8"));

    session_trace::reset_scan_passes();
    let traces = load_session_traces_from_roots(&sessions, claude_roots(&fixture), Vec::new());

    assert!(
        traces.contains_key("11111111-1111-4111-8111-111111111111"),
        "small files must still be traced"
    );
    assert!(
        !traces.contains_key(huge_id),
        "a JSONL file over MAX_JSONL_BYTES must be skipped"
    );

    fs::remove_dir_all(&fixture.root).ok();
}
