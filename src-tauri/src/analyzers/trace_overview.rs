//! Trace overview analyzer: summarizes tool calls and their shares for one provider's sessions.

use std::collections::HashMap;

use serde::Serialize;

use crate::db::HistoricalSession;

use super::session_trace::SessionTrace;

#[derive(Debug, Clone, Serialize)]
pub struct TraceToolUsage {
    pub name: String,
    pub calls: usize,
    pub share_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceOverview {
    pub provider: String,
    pub provider_display: String,
    pub instruction_file: String,
    pub fix_button_label: String,
    pub session_store: String,
    pub global_state_source: String,
    pub traced_sessions: usize,
    pub total_sessions: usize,
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub total_tool_calls: usize,
    pub total_compactions: usize,
    pub mcp_tool_calls: usize,
    pub cache_hit_ratio: f64,
    pub top_tools: Vec<TraceToolUsage>,
    pub telemetry_mermaid: String,
    pub cache_mermaid: String,
}

pub fn build(
    provider: cc_discord_presence::provider::Provider,
    sessions: &[HistoricalSession],
    traces: &HashMap<String, SessionTrace>,
    cache_hit_ratio: f64,
) -> TraceOverview {
    let traced_sessions = traces.len();
    let total_sessions = sessions.len();
    let user_messages: usize = traces.values().map(|trace| trace.user_messages).sum();
    let assistant_messages: usize = traces.values().map(|trace| trace.assistant_messages).sum();
    let total_tool_calls: usize = traces.values().map(|trace| trace.total_tools).sum();
    let total_compactions: usize = traces.values().map(|trace| trace.compact_commands).sum();
    let mcp_tool_calls: usize = traces.values().map(|trace| trace.mcp_tool_calls).sum();

    let mut tool_map: HashMap<String, usize> = HashMap::new();
    for trace in traces.values() {
        for (name, count) in &trace.tool_counts {
            *tool_map.entry(name.clone()).or_insert(0) += count;
        }
    }
    let mut top_tools: Vec<TraceToolUsage> = tool_map
        .into_iter()
        .map(|(name, calls)| TraceToolUsage {
            share_pct: if total_tool_calls == 0 {
                0.0
            } else {
                calls as f64 / total_tool_calls as f64 * 100.0
            },
            name,
            calls,
        })
        .collect();
    top_tools.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.name.cmp(&b.name)));
    top_tools.truncate(8);

    let provider_display = provider.display_name().to_string();
    let instruction_file = provider.instruction_file_name().to_string();
    let fix_button_label = provider.fix_action_label().to_string();
    let session_store = provider.sessions_glob_label().to_string();
    let global_state_source = provider.global_state_label().to_string();

    let top_tool_lines = if top_tools.is_empty() {
        "no traced tool mix yet".to_string()
    } else {
        top_tools
            .iter()
            .take(4)
            .map(|tool| format!("{} ({:.0}%)", tool.name, tool.share_pct))
            .collect::<Vec<_>>()
            .join(" · ")
    };

    let telemetry_mermaid = format!(
        "flowchart LR\n  Source[\"Session store\\n{session_store}\"] --> Parser[\"Pulse parser\\n{traced_sessions}/{total_sessions} traced sessions\"]\n  Limits[\"Plan + limits\\n{global_state_source}\"] --> Parser\n  Rules[\"Instructions\\n{instruction_file}\"] --> Parser\n  Parser --> Db[\"pulse-analytics.db\\n{total_sessions} sessions\"]\n  Parser --> Discord[\"Discord Rich Presence\"]\n  Db --> Reports[\"Reports & Insights\"]\n  Db --> Budget[\"Forecasts + budgets\"]\n  Reports --> Fix[\"{fix_button_label}\"]",
        session_store = escape_mermaid(&session_store),
        traced_sessions = traced_sessions,
        total_sessions = total_sessions,
        global_state_source = escape_mermaid(&global_state_source),
        instruction_file = escape_mermaid(&instruction_file),
        fix_button_label = escape_mermaid(&fix_button_label),
    );

    let cache_mermaid = format!(
        "flowchart LR\n  Prefix[\"Stable prefix\\n{instruction_file}\"] --> Input[\"Pure input\\n{user_messages} user msgs\"]\n  Input --> Cache[\"Cache reuse\\n{cache_hit_ratio:.1}% hit ratio\"]\n  Input --> Tools[\"Tool traffic\\n{total_tool_calls} calls\"]\n  Tools --> Output[\"Assistant output\\n{assistant_messages} msgs\"]\n  Cache --> Cost[\"Cost + analytics\"]\n  Output --> Cost\n  Tools --> Mcp[\"MCP share\\n{mcp_tool_calls} MCP calls\"]\n  Cost --> Summary[\"Top tools\\n{top_tool_lines}\"]",
        instruction_file = escape_mermaid(&instruction_file),
        user_messages = user_messages,
        cache_hit_ratio = cache_hit_ratio,
        total_tool_calls = total_tool_calls,
        assistant_messages = assistant_messages,
        mcp_tool_calls = mcp_tool_calls,
        top_tool_lines = escape_mermaid(&top_tool_lines),
    );

    TraceOverview {
        provider: provider.as_str().to_string(),
        provider_display,
        instruction_file,
        fix_button_label,
        session_store,
        global_state_source,
        traced_sessions,
        total_sessions,
        user_messages,
        assistant_messages,
        total_tool_calls,
        total_compactions,
        mcp_tool_calls,
        cache_hit_ratio,
        top_tools,
        telemetry_mermaid,
        cache_mermaid,
    }
}

fn escape_mermaid(value: &str) -> String {
    value
        .replace('&', "and")
        .replace('"', "'")
        .replace('<', "(")
        .replace('>', ")")
}
