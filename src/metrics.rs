//! Session metrics: token totals, cost breakdowns and per-model figures from the latest session
//! snapshots.

use std::collections::HashMap;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cost;
use crate::session::ClaudeSessionSnapshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub daemon_started_at: DateTime<Utc>,
    pub snapshot_at: DateTime<Utc>,
    pub uptime_seconds: u64,
    pub totals: TokenTotals,
    pub cost_breakdown: CostBreakdown,
    pub by_model: Vec<ModelMetrics>,
    pub active_sessions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenTotals {
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    /// Pure input tokens (input minus cache write and cache read).
    #[serde(default)]
    pub pure_input_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CostBreakdown {
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_write_cost: f64,
    pub cache_read_cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetrics {
    pub model_id: String,
    pub display_name: String,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    pub session_count: u32,
}

/// Internal per-session record (latest values, NOT cumulative deltas).
#[derive(Debug, Clone, Default)]
struct SessionRecord {
    model_id: String,
    display_name: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    total_cost: f64,
    input_cost: f64,
    output_cost: f64,
    cache_write_cost: f64,
    cache_read_cost: f64,
}

pub struct MetricsTracker {
    daemon_started_at: DateTime<Utc>,
    started_instant: Instant,
    /// Latest snapshot per session_id: replace-on-update, not additive.
    sessions: HashMap<String, SessionRecord>,
    /// Cached snapshot for TUI rendering (avoids recomputing every frame).
    cached_snapshot: Option<MetricsSnapshot>,
}

impl MetricsTracker {
    pub fn new() -> Self {
        Self {
            daemon_started_at: Utc::now(),
            started_instant: Instant::now(),
            sessions: HashMap::new(),
            cached_snapshot: None,
        }
    }

    /// Update internal records from the latest session snapshots.
    pub fn update(&mut self, sessions: &[ClaudeSessionSnapshot]) {
        for session in sessions {
            let model_id = session.model.clone().unwrap_or_default();
            let display_name = session
                .model_display
                .clone()
                .unwrap_or_else(|| cost::model_display_name(&model_id));
            let record = SessionRecord {
                model_id,
                display_name,
                input_tokens: session.input_tokens,
                output_tokens: session.output_tokens,
                cache_creation_tokens: session.cache_creation_tokens,
                cache_read_tokens: session.cache_read_tokens,
                total_cost: session.total_cost,
                input_cost: session.input_cost,
                output_cost: session.output_cost,
                cache_write_cost: session.cache_write_cost,
                cache_read_cost: session.cache_read_cost,
            };
            self.sessions.insert(session.session_id.clone(), record);
        }
        self.cached_snapshot = Some(self.compute_snapshot(sessions.len()));
    }

    /// Get the latest computed snapshot for TUI rendering.
    pub fn snapshot(&self) -> Option<&MetricsSnapshot> {
        self.cached_snapshot.as_ref()
    }

    fn compute_snapshot(&self, active_count: usize) -> MetricsSnapshot {
        let mut totals = TokenTotals::default();
        let mut cost_bd = CostBreakdown::default();
        let mut by_model_map: HashMap<String, ModelMetrics> = HashMap::new();

        for record in self.sessions.values() {
            totals.input_tokens += record.input_tokens;
            totals.output_tokens += record.output_tokens;
            totals.cache_write_tokens += record.cache_creation_tokens;
            totals.cache_read_tokens += record.cache_read_tokens;
            totals.cost_usd += record.total_cost;

            cost_bd.input_cost += record.input_cost;
            cost_bd.output_cost += record.output_cost;
            cost_bd.cache_write_cost += record.cache_write_cost;
            cost_bd.cache_read_cost += record.cache_read_cost;

            let key = record.display_name.clone();
            let entry = by_model_map.entry(key).or_insert_with(|| ModelMetrics {
                model_id: record.model_id.clone(),
                display_name: record.display_name.clone(),
                cost_usd: 0.0,
                input_tokens: 0,
                output_tokens: 0,
                cache_write_tokens: 0,
                cache_read_tokens: 0,
                session_count: 0,
            });
            entry.cost_usd += record.total_cost;
            entry.input_tokens += record.input_tokens;
            entry.output_tokens += record.output_tokens;
            entry.cache_write_tokens += record.cache_creation_tokens;
            entry.cache_read_tokens += record.cache_read_tokens;
            entry.session_count += 1;
        }

        totals.total_tokens = totals.input_tokens + totals.output_tokens;
        totals.pure_input_tokens = totals
            .input_tokens
            .saturating_sub(totals.cache_write_tokens)
            .saturating_sub(totals.cache_read_tokens);

        let mut by_model: Vec<ModelMetrics> = by_model_map.into_values().collect();
        by_model.sort_by(|a, b| {
            b.cost_usd
                .partial_cmp(&a.cost_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        MetricsSnapshot {
            daemon_started_at: self.daemon_started_at,
            snapshot_at: Utc::now(),
            uptime_seconds: self.started_instant.elapsed().as_secs(),
            totals,
            cost_breakdown: cost_bd,
            by_model,
            active_sessions: active_count,
        }
    }
}

impl Default for MetricsTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{DataSource, RateLimits, ReasoningEffort, Speed};
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn make_session(
        id: &str,
        model: &str,
        input: u64,
        output: u64,
        cache_write: u64,
        cache_read: u64,
        cost: f64,
    ) -> ClaudeSessionSnapshot {
        ClaudeSessionSnapshot {
            session_id: id.to_string(),
            cwd: PathBuf::from("/test"),
            project_name: "test".to_string(),
            git_branch: None,
            model: Some(model.to_string()),
            model_display: Some(cost::model_display_name(model)),
            session_total_tokens: Some(input + output),
            last_turn_tokens: None,
            session_delta_tokens: None,
            input_tokens: input,
            output_tokens: output,
            cache_creation_tokens: cache_write,
            cache_read_tokens: cache_read,
            max_turn_api_input: 0,
            current_context_tokens: 0,
            total_api_duration_ms: 0,
            total_cost: cost,
            input_cost: 0.0,
            output_cost: 0.0,
            cache_write_cost: 0.0,
            cache_read_cost: 0.0,
            limits: RateLimits::default(),
            activity: None,
            started_at: None,
            last_token_event_at: None,
            last_activity: SystemTime::now(),
            reasoning_effort: ReasoningEffort::default(),
            reasoning_effort_explicit: false,
            has_thinking_blocks: false,
            speed: Speed::default(),
            service_tier: None,
            source: DataSource::Jsonl,
            source_file: PathBuf::from("/test.jsonl"),
            background_work: crate::workflow_state::BackgroundWorkInfo::default(),
            subagents: Vec::new(),
            is_subagent: false,
            parent_session_id: None,
        }
    }

    fn make_fast_session_with_breakdown(
        id: &str,
        model: &str,
        input: u64,
        output: u64,
        cache_write: u64,
        cache_read: u64,
    ) -> ClaudeSessionSnapshot {
        let bd =
            cost::calculate_category_costs(model, input, output, cache_write, cache_read, true);
        let mut s = make_session(
            id,
            model,
            input + cache_write + cache_read,
            output,
            cache_write,
            cache_read,
            bd.total(),
        );
        s.speed = Speed::Fast;
        s.input_cost = bd.input_cost;
        s.output_cost = bd.output_cost;
        s.cache_write_cost = bd.cache_write_cost;
        s.cache_read_cost = bd.cache_read_cost;
        s
    }

    #[test]
    fn fast_opus_48_snapshot_breakdown_reconciles_with_total() {
        let mut tracker = MetricsTracker::new();
        let sessions = vec![make_fast_session_with_breakdown(
            "s1",
            "claude-opus-4-8",
            100_000,
            20_000,
            8_000,
            60_000,
        )];
        tracker.update(&sessions);
        let snap = tracker.snapshot().unwrap();

        let bd = &snap.cost_breakdown;
        let breakdown_sum =
            bd.input_cost + bd.output_cost + bd.cache_write_cost + bd.cache_read_cost;
        assert!((breakdown_sum - snap.totals.cost_usd).abs() < 1e-7);
        assert!(snap.totals.cost_usd > 0.0);
    }

    /// Opus 5 is fast-capable, so a fast turn must still reconcile the four
    /// billable categories against the headline total.
    #[test]
    fn fast_opus_5_snapshot_breakdown_reconciles_with_total() {
        let mut tracker = MetricsTracker::new();
        let sessions = vec![make_fast_session_with_breakdown(
            "s1",
            "claude-opus-5",
            100_000,
            20_000,
            8_000,
            60_000,
        )];
        tracker.update(&sessions);
        let snap = tracker.snapshot().unwrap();

        let bd = &snap.cost_breakdown;
        let breakdown_sum =
            bd.input_cost + bd.output_cost + bd.cache_write_cost + bd.cache_read_cost;
        assert!((breakdown_sum - snap.totals.cost_usd).abs() < 1e-7);
        assert!(snap.totals.cost_usd > 0.0);
        // The tracker falls back to the plain display name; the "(1M)" context
        // suffix is added by callers that pass a per-turn input figure.
        assert_eq!(snap.by_model[0].display_name, "Claude Opus 5");
    }

    #[test]
    fn accumulates_sessions_without_double_counting() {
        let mut tracker = MetricsTracker::new();

        let sessions = vec![make_session(
            "s1",
            "claude-opus-4-6",
            1000,
            500,
            200,
            300,
            0.05,
        )];
        tracker.update(&sessions);
        let snap = tracker.snapshot().unwrap();
        assert_eq!(snap.totals.input_tokens, 1000);
        assert_eq!(snap.totals.output_tokens, 500);
        assert!((snap.totals.cost_usd - 0.05).abs() < 0.001);

        let sessions = vec![make_session(
            "s1",
            "claude-opus-4-6",
            2000,
            1000,
            400,
            600,
            0.10,
        )];
        tracker.update(&sessions);
        let snap = tracker.snapshot().unwrap();
        assert_eq!(snap.totals.input_tokens, 2000);
        assert_eq!(snap.totals.output_tokens, 1000);
        assert!((snap.totals.cost_usd - 0.10).abs() < 0.001);
    }

    #[test]
    fn aggregates_multiple_sessions() {
        let mut tracker = MetricsTracker::new();
        let sessions = vec![
            make_session("s1", "claude-opus-4-6", 1000, 500, 100, 200, 0.05),
            make_session("s2", "claude-sonnet-4-5", 2000, 800, 0, 0, 0.03),
        ];
        tracker.update(&sessions);
        let snap = tracker.snapshot().unwrap();
        assert_eq!(snap.totals.input_tokens, 3000);
        assert_eq!(snap.totals.output_tokens, 1300);
        assert!((snap.totals.cost_usd - 0.08).abs() < 0.001);
        assert_eq!(snap.by_model.len(), 2);
        assert_eq!(snap.active_sessions, 2);
    }

    #[test]
    fn per_model_breakdown() {
        let mut tracker = MetricsTracker::new();
        let sessions = vec![
            make_session("s1", "claude-opus-4-6", 1000, 500, 0, 0, 0.05),
            make_session("s2", "claude-opus-4-6", 2000, 800, 0, 0, 0.08),
            make_session("s3", "claude-sonnet-4-5", 500, 200, 0, 0, 0.01),
        ];
        tracker.update(&sessions);
        let snap = tracker.snapshot().unwrap();

        assert_eq!(snap.by_model[0].display_name, "Claude Opus 4.6");
        assert_eq!(snap.by_model[0].session_count, 2);
        assert!((snap.by_model[0].cost_usd - 0.13).abs() < 0.001);

        assert_eq!(snap.by_model[1].display_name, "Claude Sonnet 4.5");
        assert_eq!(snap.by_model[1].session_count, 1);
    }

    #[test]
    fn json_roundtrip() {
        let snap = MetricsSnapshot {
            daemon_started_at: Utc::now(),
            snapshot_at: Utc::now(),
            uptime_seconds: 60,
            totals: TokenTotals {
                cost_usd: 0.05,
                input_tokens: 1000,
                output_tokens: 500,
                cache_write_tokens: 0,
                cache_read_tokens: 0,
                total_tokens: 1500,
                pure_input_tokens: 1000,
            },
            cost_breakdown: CostBreakdown::default(),
            by_model: vec![],
            active_sessions: 1,
        };
        let json = serde_json::to_string_pretty(&snap).unwrap();
        let parsed: MetricsSnapshot = serde_json::from_str(&json).unwrap();
        assert!((parsed.totals.cost_usd - 0.05).abs() < 0.001);
        assert_eq!(parsed.totals.input_tokens, 1000);
    }
}
