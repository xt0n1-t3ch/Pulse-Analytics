use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use cc_discord_presence::provider::Provider;
use chrono::{DateTime, Timelike, Utc};
use serde_json::Value;

use crate::db::HistoricalSession;

pub const MAX_JSONL_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_JSONL_FILES: usize = 4000;
const MAX_SCAN_DEPTH: usize = 16;
const MAX_SCAN_DIRS: usize = 20_000;

use std::sync::atomic::{AtomicUsize, Ordering};

static SCAN_PASSES: AtomicUsize = AtomicUsize::new(0);

pub fn scan_passes() -> usize {
    SCAN_PASSES.load(Ordering::SeqCst)
}

pub fn reset_scan_passes() {
    SCAN_PASSES.store(0, Ordering::SeqCst);
}

#[derive(Debug, Clone, Default)]
pub struct SessionTrace {
    pub session_id: String,
    pub first_prompt: Option<String>,
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub total_tools: usize,
    pub tool_counts: HashMap<String, usize>,
    pub compact_commands: usize,
    pub mcp_tool_calls: usize,
    pub hour_distribution: [u32; 24],
}

impl SessionTrace {
    pub fn peak_overlap_pct(&self) -> u32 {
        let total: u32 = self.hour_distribution.iter().sum();
        if total == 0 {
            return 0;
        }
        let overlap: u32 = self.hour_distribution[12..=18].iter().sum();
        ((overlap as f64 / total as f64) * 100.0).round() as u32
    }
}

pub fn load_session_traces(sessions: &[HistoricalSession]) -> HashMap<String, SessionTrace> {
    load_session_traces_from_roots(
        sessions,
        cc_discord_presence::config::projects_paths(),
        cc_discord_presence::codex::config::sessions_paths(),
    )
}

pub fn load_session_traces_from_roots(
    sessions: &[HistoricalSession],
    claude_roots: Vec<PathBuf>,
    codex_roots: Vec<PathBuf>,
) -> HashMap<String, SessionTrace> {
    if sessions.is_empty() {
        return HashMap::new();
    }

    let index = build_jsonl_index(sessions, claude_roots, codex_roots);
    sessions
        .iter()
        .filter_map(|session| {
            let raw_id = raw_session_id(session);
            let path = index.get(&(session.provider.clone(), raw_id.clone()))?;
            let provider = Provider::parse(&session.provider)?;
            Some((
                session.id.clone(),
                parse_session_trace(provider, &raw_id, path),
            ))
        })
        .collect()
}

fn build_jsonl_index(
    sessions: &[HistoricalSession],
    claude_roots: Vec<PathBuf>,
    codex_roots: Vec<PathBuf>,
) -> HashMap<(String, String), PathBuf> {
    let mut wanted_by_provider: HashMap<Provider, HashSet<String>> = HashMap::new();
    for session in sessions {
        if let Some(provider) = Provider::parse(&session.provider) {
            wanted_by_provider
                .entry(provider)
                .or_default()
                .insert(raw_session_id(session));
        }
    }

    let mut found = HashMap::new();
    let mut budget = MAX_JSONL_FILES;
    if let Some(wanted) = wanted_by_provider.get(&Provider::Claude).cloned() {
        scan_provider_roots(
            Provider::Claude,
            claude_roots,
            wanted,
            &mut found,
            &mut budget,
        );
    }
    if let Some(wanted) = wanted_by_provider.get(&Provider::Codex).cloned() {
        scan_provider_roots(
            Provider::Codex,
            codex_roots,
            wanted,
            &mut found,
            &mut budget,
        );
    }

    found
}

fn scan_provider_roots(
    provider: Provider,
    roots: Vec<PathBuf>,
    mut remaining: HashSet<String>,
    found: &mut HashMap<(String, String), PathBuf>,
    budget: &mut usize,
) {
    SCAN_PASSES.fetch_add(1, Ordering::SeqCst);

    for root in roots {
        if remaining.is_empty() || !root.exists() {
            continue;
        }
        let mut dirs_scanned = 0usize;
        let mut stack = vec![(root, 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if remaining.is_empty() {
                break;
            }
            if dirs_scanned >= MAX_SCAN_DIRS {
                tracing::warn!(
                    provider = provider.as_str(),
                    cap = MAX_SCAN_DIRS,
                    "session-trace scan hit MAX_SCAN_DIRS cap; remaining directories skipped"
                );
                return;
            }
            dirs_scanned += 1;
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    if depth < MAX_SCAN_DEPTH {
                        stack.push((path, depth + 1));
                    }
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                    continue;
                }
                if *budget == 0 {
                    tracing::warn!(
                        provider = provider.as_str(),
                        cap = MAX_JSONL_FILES,
                        "session-trace scan hit MAX_JSONL_FILES cap; remaining files skipped"
                    );
                    return;
                }
                *budget -= 1;
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let Some(matched_id) = (match provider {
                    Provider::OpenCode | Provider::CommandCode => None,
                    Provider::Claude => remaining.contains(stem).then(|| stem.to_string()),
                    Provider::Codex => remaining
                        .iter()
                        .find(|session_id| {
                            stem == session_id.as_str() || stem.ends_with(session_id.as_str())
                        })
                        .cloned(),
                }) else {
                    continue;
                };
                if file_exceeds_size_cap(&path) {
                    tracing::warn!(
                        provider = provider.as_str(),
                        path = %path.display(),
                        cap_bytes = MAX_JSONL_BYTES,
                        "session-trace JSONL exceeds MAX_JSONL_BYTES; file skipped"
                    );
                    remaining.remove(&matched_id);
                    continue;
                }
                remaining.remove(&matched_id);
                found.insert((provider.as_str().to_string(), matched_id), path);
            }
        }
    }
}

fn file_exceeds_size_cap(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.len() > MAX_JSONL_BYTES)
        .unwrap_or(false)
}

fn raw_session_id(session: &HistoricalSession) -> String {
    session
        .id
        .split_once(':')
        .and_then(|(prefix, value)| Provider::parse(prefix).map(|_| value.to_string()))
        .unwrap_or_else(|| session.id.clone())
}

fn parse_session_trace(provider: Provider, session_id: &str, path: &Path) -> SessionTrace {
    let mut trace = SessionTrace {
        session_id: session_id.to_string(),
        ..Default::default()
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return trace;
    };

    for line in raw.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match provider {
            Provider::OpenCode | Provider::CommandCode => {}
            Provider::Claude => parse_claude_trace_line(&value, &mut trace),
            Provider::Codex => parse_codex_trace_line(&value, &mut trace),
        }
    }

    trace
}

fn parse_claude_trace_line(value: &Value, trace: &mut SessionTrace) {
    let msg_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(parse_timestamp_hour);
    if matches!(msg_type, "user" | "assistant")
        && let Some(hour) = timestamp
    {
        trace.hour_distribution[hour as usize] += 1;
    }

    let content = value.get("message").and_then(|m| m.get("content"));
    if msg_type == "user" {
        trace.user_messages += 1;
        let prompt_text = content.and_then(extract_text);
        if trace.first_prompt.is_none()
            && let Some(prompt) = prompt_text.clone()
        {
            trace.first_prompt = Some(truncate_prompt(&prompt));
        }
        if let Some(prompt) = prompt_text
            && prompt.contains("/compact")
        {
            trace.compact_commands += 1;
        }
    } else if msg_type == "assistant" {
        trace.assistant_messages += 1;
    }

    if let Some(content) = content {
        scan_claude_tools(content, trace);
    }
}

fn parse_codex_trace_line(value: &Value, trace: &mut SessionTrace) {
    let outer_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(parse_timestamp_hour);
    let payload = value.get("payload").and_then(|v| v.as_object());
    let payload_type = payload
        .and_then(|map| map.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if matches!(outer_type, "compacted") || matches!(payload_type, "context_compacted") {
        trace.compact_commands += 1;
    }

    if outer_type == "response_item" && payload_type == "message" {
        let role = payload
            .and_then(|map| map.get("role"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let content = payload.and_then(|map| map.get("content"));
        if matches!(role, "user" | "assistant")
            && let Some(hour) = timestamp
        {
            trace.hour_distribution[hour as usize] += 1;
        }
        if role == "user" {
            trace.user_messages += 1;
            let prompt_text = content.and_then(extract_codex_text);
            if trace.first_prompt.is_none()
                && let Some(prompt) = prompt_text.clone()
            {
                trace.first_prompt = Some(truncate_prompt(&prompt));
            }
            if let Some(prompt) = prompt_text
                && prompt.contains("/compact")
            {
                trace.compact_commands += 1;
            }
        } else if role == "assistant" {
            trace.assistant_messages += 1;
        }
    }

    if let Some(payload) = payload {
        scan_codex_tools(payload, trace);
    }
}

fn parse_timestamp_hour(input: &str) -> Option<u32> {
    DateTime::parse_from_rfc3339(input)
        .ok()
        .map(|dt| dt.with_timezone(&Utc).hour())
}

fn extract_text(content: &Value) -> Option<String> {
    if let Some(s) = content.as_str() {
        return Some(s.trim().to_string()).filter(|s| !s.is_empty());
    }
    let arr = content.as_array()?;
    let mut parts = Vec::new();
    for item in arr {
        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
        }
        if let Some(text) = item.get("content").and_then(|v| v.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn truncate_prompt(prompt: &str) -> String {
    const MAX: usize = 220;
    if prompt.len() <= MAX {
        return prompt.to_string();
    }
    let mut end = MAX;
    while end > 0 && !prompt.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &prompt[..end])
}

fn scan_claude_tools(content: &Value, trace: &mut SessionTrace) {
    let items: Vec<&Value> = if let Some(arr) = content.as_array() {
        arr.iter().collect()
    } else if content.is_object() {
        vec![content]
    } else {
        Vec::new()
    };

    for item in items {
        if item.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
            continue;
        }
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .map(normalize_tool_name)
            .unwrap_or_else(|| "unknown".to_string());
        *trace.tool_counts.entry(name.clone()).or_insert(0) += 1;
        trace.total_tools += 1;
        if name.starts_with("mcp__") {
            trace.mcp_tool_calls += 1;
        }
    }
}

fn scan_codex_tools(payload: &serde_json::Map<String, Value>, trace: &mut SessionTrace) {
    let payload_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let tool_name = match payload_type {
        "function_call" | "custom_tool_call" => payload
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        "view_image_tool_call" => Some("view_image".to_string()),
        "web_search_call" => Some("web_search".to_string()),
        "patch_apply_end" => Some("apply_patch".to_string()),
        "mcp_tool_call_end" => {
            payload
                .get("invocation")
                .and_then(|v| v.as_object())
                .map(|invocation| {
                    let server = invocation
                        .get("server")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let tool = invocation
                        .get("tool")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    format!("mcp__{server}__{tool}")
                })
        }
        _ => None,
    };
    let Some(name) = tool_name else {
        return;
    };
    let normalized = normalize_tool_name(&name);
    *trace.tool_counts.entry(normalized.clone()).or_insert(0) += 1;
    trace.total_tools += 1;
    if normalized.starts_with("mcp__") {
        trace.mcp_tool_calls += 1;
    }
}

fn normalize_tool_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

fn extract_codex_text(content: &Value) -> Option<String> {
    let arr = content.as_array()?;
    let mut parts = Vec::new();
    for item in arr {
        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
        }
        if let Some(text) = item.get("input").and_then(|v| v.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
        }
        if let Some(text) = item.get("content").and_then(|v| v.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn historical_session(provider: &str, id: &str) -> HistoricalSession {
        HistoricalSession {
            opencode: None,
            id: id.into(),
            provider: provider.into(),
            session_name: None,
            project: "pulse".into(),
            model: "model".into(),
            model_id: "model".into(),
            context_window: "200K".into(),
            branch: None,
            effort: "Medium".into(),
            speed: "standard".into(),
            started_at: None,
            ended_at: None,
            duration_secs: 0,
            total_cost: 0.0,
            cost_basis: Default::default(),
            cost_source: String::new(),
            known_cost: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: 0,
            input_cost: 0.0,
            output_cost: 0.0,
            cache_write_cost: 0.0,
            cache_read_cost: 0.0,
            has_thinking: false,
            subagent_count: 0,
            is_active: false,
            used_tokens: 0,
            window_tokens: 0,
        }
    }

    fn temp_trace_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::current_dir().unwrap().join(format!(
            "pulse-session-trace-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn extract_text_joins_text_items() {
        let value = serde_json::json!([
            { "type": "text", "text": "Fix auth bug" },
            { "type": "text", "content": "line 42 in src/auth.rs" }
        ]);
        let text = extract_text(&value).unwrap();
        assert!(text.contains("Fix auth bug"));
        assert!(text.contains("line 42"));
    }

    #[test]
    fn peak_overlap_pct_counts_utc_window() {
        let mut trace = SessionTrace::default();
        trace.hour_distribution[12] = 3;
        trace.hour_distribution[8] = 3;
        assert_eq!(trace.peak_overlap_pct(), 50);
    }

    #[test]
    fn raw_session_id_strips_provider_prefix() {
        let session = HistoricalSession {
            opencode: None,
            id: "codex:019daa02-33ad-7c40-8ea4-6d003a58e803".into(),
            provider: "codex".into(),
            session_name: None,
            project: "pulse".into(),
            model: "gpt-5.4".into(),
            model_id: "gpt-5.4".into(),
            context_window: "1.0M".into(),
            branch: None,
            effort: "High".into(),
            speed: "standard".into(),
            started_at: None,
            ended_at: None,
            duration_secs: 0,
            total_cost: 0.0,
            cost_basis: Default::default(),
            cost_source: String::new(),
            known_cost: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: 0,
            input_cost: 0.0,
            output_cost: 0.0,
            cache_write_cost: 0.0,
            cache_read_cost: 0.0,
            has_thinking: false,
            subagent_count: 0,
            is_active: false,
            used_tokens: 0,
            window_tokens: 0,
        };
        assert_eq!(
            raw_session_id(&session),
            "019daa02-33ad-7c40-8ea4-6d003a58e803"
        );
    }

    #[test]
    fn trace_scan_ignores_files_beyond_depth_cap() {
        let root = temp_trace_dir("depth");
        let mut nested = root.clone();
        for idx in 0..=MAX_SCAN_DEPTH {
            nested = nested.join(format!("d{idx}"));
        }
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("wanted.jsonl"), b"{}\n").unwrap();

        let sessions = vec![historical_session("claude", "wanted")];
        let traces = load_session_traces_from_roots(&sessions, vec![root.clone()], Vec::new());

        assert!(!traces.contains_key("wanted"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parse_codex_counts_function_and_mcp_tools() {
        let line = serde_json::json!({
            "timestamp": "2026-04-20T08:33:16.695Z",
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "shell_command"
            }
        });
        let mcp = serde_json::json!({
            "timestamp": "2026-04-20T08:33:17.695Z",
            "type": "event_msg",
            "payload": {
                "type": "mcp_tool_call_end",
                "invocation": {
                    "server": "context-mode",
                    "tool": "ctx_batch_execute"
                }
            }
        });
        let mut trace = SessionTrace::default();
        parse_codex_trace_line(&line, &mut trace);
        parse_codex_trace_line(&mcp, &mut trace);
        assert_eq!(trace.total_tools, 2);
        assert_eq!(trace.mcp_tool_calls, 1);
        assert_eq!(
            trace
                .tool_counts
                .get("mcp__context-mode__ctx_batch_execute"),
            Some(&1)
        );
    }
}
