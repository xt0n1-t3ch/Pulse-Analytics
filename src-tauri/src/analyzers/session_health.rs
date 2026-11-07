//! Session health analyzer: grades session durations and the share of long sessions into one
//! health score.

use std::collections::HashMap;

use serde::Serialize;

use crate::db::HistoricalSession;

use super::prompt_complexity::PromptComplexityReport;
use super::session_trace::SessionTrace;
use super::tool_frequency::ToolFrequencyReport;

#[derive(Debug, Clone, Serialize)]
pub struct SessionHealthReport {
    pub available: bool,
    pub sessions_analyzed: usize,
    pub health_score: u8,
    pub grade: &'static str,
    pub avg_duration_minutes: f64,
    pub p90_duration_minutes: i64,
    pub long_session_pct: f64,
    pub avg_messages_per_session: f64,
    pub peak_overlap_pct: u32,
    pub compact_gap_pct: f64,
    pub diagnosis: String,
}

pub fn analyze(
    sessions: &[HistoricalSession],
    traces: &HashMap<String, SessionTrace>,
    tool_report: &ToolFrequencyReport,
    prompt_report: &PromptComplexityReport,
) -> SessionHealthReport {
    if sessions.is_empty() {
        return SessionHealthReport {
            available: false,
            sessions_analyzed: 0,
            health_score: 0,
            grade: "N/A",
            avg_duration_minutes: 0.0,
            p90_duration_minutes: 0,
            long_session_pct: 0.0,
            avg_messages_per_session: 0.0,
            peak_overlap_pct: 0,
            compact_gap_pct: 0.0,
            diagnosis: "No sessions analyzed yet.".to_string(),
        };
    }

    let mut durations: Vec<i64> = sessions
        .iter()
        .map(|s| (s.duration_secs.max(0) / 60).max(1))
        .collect();
    durations.sort_unstable();

    let avg_duration_minutes = durations.iter().sum::<i64>() as f64 / durations.len().max(1) as f64;
    let p90_duration_minutes = durations[((durations.len() - 1) as f64 * 0.9).round() as usize];
    let long_session_pct = if durations.is_empty() {
        0.0
    } else {
        (durations.iter().filter(|&&d| d > 120).count() as f64 / durations.len() as f64) * 100.0
    };

    let mut total_messages = 0usize;
    let mut peak_overlap_sum = 0u32;
    for trace in traces.values() {
        total_messages += trace.user_messages + trace.assistant_messages;
        peak_overlap_sum += trace.peak_overlap_pct();
    }
    let avg_messages_per_session = if traces.is_empty() {
        0.0
    } else {
        total_messages as f64 / traces.len() as f64
    };
    let peak_overlap_pct = if traces.is_empty() {
        0
    } else {
        (peak_overlap_sum as f64 / traces.len() as f64).round() as u32
    };
    let compact_gap_pct = if tool_report.traced_sessions > 0 {
        (tool_report.compact_gap_sessions as f64 / tool_report.traced_sessions as f64) * 100.0
    } else {
        0.0
    };

    let mut health_score = 100i32;
    if avg_duration_minutes > 60.0 {
        health_score -= ((avg_duration_minutes - 60.0) / 4.0).round() as i32;
    }
    if p90_duration_minutes > 150 {
        health_score -= 10;
    }
    if long_session_pct > 25.0 {
        health_score -= 12;
    }
    if peak_overlap_pct > 40 {
        health_score -= 10;
    }
    if compact_gap_pct > 30.0 {
        health_score -= 10;
    }
    if tool_report.avg_tools_per_session > 45.0 {
        health_score -= 10;
    }
    if prompt_report.available && prompt_report.avg_specificity_score < 45.0 {
        health_score -= 8;
    }

    let health_score = health_score.clamp(0, 100) as u8;
    let grade = match health_score {
        85..=100 => "Great",
        70..=84 => "Good",
        50..=69 => "Fair",
        30..=49 => "Risky",
        _ => "Critical",
    };

    let diagnosis = if health_score < 50 {
        "Session shape is expensive: long runs, weak prompt specificity, or high tool churn are compounding usage."
            .to_string()
    } else if peak_overlap_pct > 40 {
        "Session shape is okay, but a lot of work lands in throttled hours. Moving heavy sessions off-peak should help."
            .to_string()
    } else {
        "Session health looks stable. Nothing beyond routine compacting stands out.".to_string()
    };

    SessionHealthReport {
        available: true,
        sessions_analyzed: sessions.len(),
        health_score,
        grade,
        avg_duration_minutes,
        p90_duration_minutes,
        long_session_pct,
        avg_messages_per_session,
        peak_overlap_pct,
        compact_gap_pct,
        diagnosis,
    }
}
