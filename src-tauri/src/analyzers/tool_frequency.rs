//! Tool frequency analyzer: counts calls per tool and reports each tool's share.

use std::collections::HashMap;

use serde::Serialize;

use crate::db::HistoricalSession;

use super::session_trace::SessionTrace;

#[derive(Debug, Clone, Serialize)]
pub struct ToolUsageEntry {
    pub name: String,
    pub count: usize,
    pub share_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolFrequencyReport {
    pub available: bool,
    pub sessions_analyzed: usize,
    pub traced_sessions: usize,
    pub total_tool_calls: usize,
    pub avg_tools_per_session: f64,
    pub avg_tool_calls_per_hour: f64,
    pub mcp_tool_calls: usize,
    pub mcp_share_pct: f64,
    pub compact_gap_sessions: usize,
    pub diagnosis: String,
    pub top_tools: Vec<ToolUsageEntry>,
}

pub fn analyze(
    sessions: &[HistoricalSession],
    traces: &HashMap<String, SessionTrace>,
) -> ToolFrequencyReport {
    if traces.is_empty() {
        return ToolFrequencyReport {
            available: false,
            sessions_analyzed: sessions.len(),
            traced_sessions: 0,
            total_tool_calls: 0,
            avg_tools_per_session: 0.0,
            avg_tool_calls_per_hour: 0.0,
            mcp_tool_calls: 0,
            mcp_share_pct: 0.0,
            compact_gap_sessions: 0,
            diagnosis: "No JSONL tool traces available yet.".to_string(),
            top_tools: Vec::new(),
        };
    }

    let mut totals: HashMap<String, usize> = HashMap::new();
    let mut total_tool_calls = 0usize;
    let mut total_duration_hours = 0.0f64;
    let mut mcp_tool_calls = 0usize;
    let mut compact_gap_sessions = 0usize;

    for session in sessions {
        let Some(trace) = traces.get(&session.id) else {
            continue;
        };
        total_tool_calls += trace.total_tools;
        mcp_tool_calls += trace.mcp_tool_calls;
        total_duration_hours += (session.duration_secs.max(1) as f64) / 3600.0;
        if trace.total_tools >= 30 && trace.compact_commands == 0 {
            compact_gap_sessions += 1;
        }
        for (name, count) in &trace.tool_counts {
            *totals.entry(name.clone()).or_insert(0) += *count;
        }
    }

    let traced_sessions = traces.len();
    let avg_tools_per_session = if traced_sessions > 0 {
        total_tool_calls as f64 / traced_sessions as f64
    } else {
        0.0
    };
    let avg_tool_calls_per_hour = if total_duration_hours > 0.0 {
        total_tool_calls as f64 / total_duration_hours
    } else {
        0.0
    };
    let mcp_share_pct = if total_tool_calls > 0 {
        (mcp_tool_calls as f64 / total_tool_calls as f64) * 100.0
    } else {
        0.0
    };

    let mut top_tools: Vec<ToolUsageEntry> = totals
        .into_iter()
        .map(|(name, count)| ToolUsageEntry {
            name,
            count,
            share_pct: if total_tool_calls > 0 {
                (count as f64 / total_tool_calls as f64) * 100.0
            } else {
                0.0
            },
        })
        .collect();
    top_tools.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    top_tools.truncate(10);

    let diagnosis = if avg_tools_per_session >= 40.0 {
        "Sessions are tool-heavy. `/compact` cadence and narrower asks should cut churn."
            .to_string()
    } else if mcp_share_pct >= 20.0 {
        "MCP usage is a large slice of tool traffic. Disconnect idle servers between tasks to preserve cache stability."
            .to_string()
    } else {
        "Tool mix looks normal. Reads/searches dominate, not runaway loops.".to_string()
    };

    ToolFrequencyReport {
        available: true,
        sessions_analyzed: sessions.len(),
        traced_sessions,
        total_tool_calls,
        avg_tools_per_session,
        avg_tool_calls_per_hour,
        mcp_tool_calls,
        mcp_share_pct,
        compact_gap_sessions,
        diagnosis,
        top_tools,
    }
}
