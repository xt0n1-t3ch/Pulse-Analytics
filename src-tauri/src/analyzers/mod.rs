//! Analyzers: ported natively from the cchubber Node.js CLI
//! (https://github.com/azkhh/cchubber, MIT licensed).
//!
//! Each submodule operates on the `HistoricalSession` slice we already store
//! in the Pulse SQLite DB, so analysis needs no external processes or fresh
//! JSONL parses: everything is a pure function over durable session rows.

pub mod cache_health;
pub mod inflection;
pub mod model_routing;
pub mod prompt_complexity;
pub mod recommendations;
pub mod session_health;
pub mod session_trace;
pub mod tool_frequency;
pub mod trace_overview;

use serde::Serialize;

/// Severity bucket used by both the Recommendations engine and the Dashboard
/// action cards. Ordered from most to least urgent for UI sort.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    Warning,
    Info,
    Positive,
}

impl Severity {
    pub fn color_hint(&self) -> &'static str {
        match self {
            Self::Critical => "#e5484d",
            Self::Warning => "#f5a524",
            Self::Info => "#d97757",
            Self::Positive => "#62b462",
        }
    }
}
