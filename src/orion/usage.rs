//! Per-request telemetry from `telemetry/usage.sqlite`, opened read-only.
//!
//! Orion keeps the table for 30 days (`orion-agent-history` usage writer), so
//! it enriches a session and never replaces the transcript, which stays the
//! source of token totals and history. Rows that have expired leave the
//! enrichment unavailable (`None`), never zero. Orion records `input_tokens`
//! with both cache categories included, the same convention as a transcript
//! message, so one `Usage` and one pricing path serve both.

use super::{AgentUsage, HelperRequests, Usage, estimate_cost, layout, output_speed};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// How long Orion keeps `model_usage` rows.
const RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const MAIN_TURN: &str = "main_turn";
const SUBAGENT: &str = "subagent";

/// What the telemetry adds to one root session and its subagents.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Telemetry {
    pub ttft_ms_median: Option<f64>,
    pub output_tokens_per_sec: Option<f64>,
    pub helper_requests: Option<HelperRequests>,
    pub subagents_by_agent: Option<Vec<AgentUsage>>,
}

/// Telemetry database of an Orion root, or `None` when it is absent or cannot
/// be opened. Never created and never written.
pub(super) fn open(root: &Path) -> Option<Connection> {
    let path = layout::usage_path(root);
    if !path.is_file() {
        return None;
    }
    let connection = crate::foreign_sqlite::open_read_only(&path).ok()?;
    connection.busy_timeout(Duration::from_millis(200)).ok()?;
    Some(connection)
}

/// Whether the 30-day retention still covers a session created at `created`.
/// Sums over an older session would miss its expired requests, so they are
/// left unavailable instead of reported low.
pub(super) fn covers(created: i64, now: i64) -> bool {
    created >= now.saturating_sub(RETENTION_MS)
}

#[derive(Default)]
struct Bucket {
    requests: u64,
    input: u64,
    output: u64,
    cost: f64,
    priced: bool,
}

impl Bucket {
    fn add(&mut self, model: &str, usage: &Usage) {
        self.requests += 1;
        self.input = self.input.saturating_add(usage.input);
        self.output = self.output.saturating_add(usage.output);
        if usage.total() > 0
            && let Some(estimate) = estimate_cost(model, usage)
        {
            self.cost += estimate.parts.total();
            self.priced = true;
        }
    }

    fn cost(&self) -> Option<f64> {
        self.priced.then_some(self.cost)
    }
}

/// Reads the rows of `session_ids` (a root and its subagents). `None` when no
/// row exists for them. `covered` says whether the retention window reaches the
/// start of the session; only then are the additive helper and per-agent sums
/// reported.
pub(super) fn read(
    connection: &Connection,
    session_ids: &[String],
    covered: bool,
) -> rusqlite::Result<Option<Telemetry>> {
    let ids = serde_json::to_string(session_ids).unwrap_or_else(|_| "[]".into());
    let mut statement = connection.prepare_cached(
        "SELECT query_source, agent, model_id, status, input_tokens, output_tokens, \
                reasoning_tokens, cache_creation_input_tokens, cache_read_input_tokens, \
                time_to_first_token_ms, first_token_at, completed_at \
         FROM model_usage WHERE session_id IN (SELECT value FROM json_each(?1))",
    )?;
    let mut rows = statement.query([ids])?;
    let mut any = false;
    let mut ttfts: Vec<i64> = Vec::new();
    let (mut streamed_tokens, mut streamed_ms) = (0u64, 0u64);
    let mut helper = Bucket::default();
    let mut agents: HashMap<String, Bucket> = HashMap::new();
    while let Some(row) = rows.next()? {
        any = true;
        let source: String = row.get(0)?;
        let status: String = row.get(3)?;
        if status == "running" {
            continue;
        }
        let number = |index: usize| -> rusqlite::Result<u64> {
            Ok(row.get::<_, Option<i64>>(index)?.unwrap_or(0).max(0) as u64)
        };
        let usage = Usage {
            input: number(4)?,
            output: number(5)?,
            reasoning: number(6)?,
            cache_write: number(7)?,
            cache_read: number(8)?,
        };
        let model: String = row.get(2)?;
        if source != MAIN_TURN && source != SUBAGENT {
            helper.add(&model, &usage);
            continue;
        }
        if source == SUBAGENT {
            let agent = row
                .get::<_, Option<String>>(1)?
                .filter(|agent| !agent.trim().is_empty())
                .unwrap_or_else(|| "unknown".into());
            agents.entry(agent).or_default().add(&model, &usage);
        }
        if status != "completed" {
            continue;
        }
        if let Some(ttft) = row.get::<_, Option<i64>>(9)?.filter(|ttft| *ttft >= 0) {
            ttfts.push(ttft);
        }
        let first = row.get::<_, Option<i64>>(10)?;
        let done = row.get::<_, Option<i64>>(11)?;
        if let (Some(first), Some(done)) = (first, done)
            && usage.output > 0
            && done > first
        {
            streamed_tokens += usage.output;
            streamed_ms += (done - first) as u64;
        }
    }
    if !any {
        return Ok(None);
    }
    let mut by_agent: Vec<AgentUsage> = agents
        .into_iter()
        .map(|(agent, bucket)| AgentUsage {
            cost_usd: bucket.cost(),
            requests: bucket.requests,
            input_tokens: bucket.input,
            output_tokens: bucket.output,
            agent,
        })
        .collect();
    by_agent.sort_by(|a, b| {
        let cost = |usage: &AgentUsage| usage.cost_usd.unwrap_or(-1.0);
        cost(b)
            .total_cmp(&cost(a))
            .then(b.requests.cmp(&a.requests))
            .then_with(|| a.agent.cmp(&b.agent))
    });
    Ok(Some(Telemetry {
        ttft_ms_median: median(&mut ttfts),
        output_tokens_per_sec: output_speed(streamed_tokens, streamed_ms),
        helper_requests: covered.then(|| HelperRequests {
            count: helper.requests,
            input_tokens: helper.input,
            output_tokens: helper.output,
            cost_usd: helper.cost(),
        }),
        subagents_by_agent: covered.then_some(by_agent),
    }))
}

fn median(values: &mut [i64]) -> Option<f64> {
    values.sort_unstable();
    let middle = values.len() / 2;
    match values.len() {
        0 => None,
        len if len % 2 == 1 => Some(values[middle] as f64),
        _ => Some((values[middle - 1] + values[middle]) as f64 / 2.0),
    }
}
