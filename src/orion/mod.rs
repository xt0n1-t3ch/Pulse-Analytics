pub mod presence;
mod store;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
pub use store::{Batch, Collector};

pub const NAME: &str = "Orion App";
pub const CLIENT_ID: &str = "1553775289274994708";
pub const ASSET_KEY: &str = "orion";

/// Orion data root: `PULSE_ORION_HOME`, then `~/.orion`, then the legacy
/// `~/.zcode` link that Orion keeps after its data-root migration.
pub fn home() -> PathBuf {
    if let Some(path) = std::env::var_os("PULSE_ORION_HOME") {
        return PathBuf::from(path);
    }
    let home = dirs::home_dir().unwrap_or_default();
    let orion = home.join(".orion");
    if orion.join("cli/db/db.sqlite").is_file() {
        orion
    } else if home.join(".zcode/cli/db/db.sqlite").is_file() {
        home.join(".zcode")
    } else {
        orion
    }
}

pub fn database_path(root: &std::path::Path) -> PathBuf {
    root.join("cli").join("db").join("db.sqlite")
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    pub client_id: String,
    pub data_roots: Vec<PathBuf>,
    pub privacy_enabled: bool,
    pub layout: codex_presence_core::PresenceLayoutConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            client_id: CLIENT_ID.into(),
            data_roots: Vec::new(),
            privacy_enabled: false,
            layout: Default::default(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        crate::storage::home().join("pulse-orion.json")
    }

    pub fn load() -> Result<Self> {
        match std::fs::read(Self::path()) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&self) -> Result<()> {
        if !(17..=22).contains(&self.client_id.len())
            || !self.client_id.bytes().all(|byte| byte.is_ascii_digit())
        {
            anyhow::bail!("Orion App requires a numeric Discord application ID");
        }
        for path in &self.data_roots {
            if !path.is_absolute() {
                anyhow::bail!("Orion App data roots must use absolute paths");
            }
        }
        Ok(crate::codex::util::write_json_pretty_atomic(
            &Self::path(),
            self,
        )?)
    }

    pub fn roots(&self) -> Vec<PathBuf> {
        if self.data_roots.is_empty() {
            vec![home()]
        } else {
            self.data_roots.clone()
        }
    }
}

/// Token categories as Orion records them. `input` already includes cache
/// read and cache write; `fresh_input` is the uncached remainder.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    pub fn fresh_input(&self) -> u64 {
        self.input
            .saturating_sub(self.cache_read)
            .saturating_sub(self.cache_write)
    }
    pub fn total(&self) -> u64 {
        self.input.saturating_add(self.output)
    }
    pub fn add(&mut self, other: &Usage) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.reasoning = self.reasoning.saturating_add(other.reasoning);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
    }
}

#[derive(Clone, Debug, Default)]
pub struct Session {
    pub id: String,
    pub root: PathBuf,
    pub directory: PathBuf,
    pub title: Option<String>,
    pub project: String,
    pub branch: Option<String>,
    pub provider_id: String,
    pub model: String,
    pub model_name: String,
    pub effort: Option<String>,
    pub created: i64,
    /// When the root session row last changed; the history cursor uses it.
    pub updated: i64,
    /// Latest change in the root or any subagent, which is when the work
    /// last moved.
    pub tree_updated: i64,
    pub usage: Usage,
    /// API-equivalent session cost. `None` means no model in the session has a
    /// known rate; a session with no usage is a true zero.
    pub known_cost: Option<f64>,
    /// False when at least one model is unpriced or only partly priced, so
    /// `known_cost` is a lower bound.
    pub cost_complete: bool,
    /// The four billable categories that add up to `known_cost`.
    pub cost_parts: CostParts,
    /// Output tokens per second while the model was generating, from Orion's
    /// per-request timings. `None` when Orion recorded none.
    pub output_tokens_per_sec: Option<f64>,
    pub context_used: Option<u64>,
    pub context_window: Option<u64>,
    pub activity: String,
    pub activity_target: Option<String>,
    /// Every subagent the session ever started; history keeps this total.
    pub subagent_count: usize,
    /// Subagents still working right now. Finished subagents never count.
    pub running_subagents: usize,
    pub messages: u64,
}

impl Session {
    /// Last time the root or any of its subagents changed.
    pub fn last_change(&self) -> i64 {
        self.updated.max(self.tree_updated)
    }

    pub fn is_recent(&self, now: i64) -> bool {
        let last = self.last_change();
        last > 0 && last <= now + 60_000 && now.saturating_sub(last) <= 600_000
    }

    pub fn is_active(&self, now: i64) -> bool {
        self.is_recent(now)
            && now.saturating_sub(self.last_change()) <= 120_000
            && !matches!(self.activity.as_str(), "Waiting for input" | "Idle")
    }

    pub fn model_label(&self) -> String {
        let model = if !self.model_name.is_empty() {
            self.model_name.as_str()
        } else if !self.model.is_empty() {
            self.model.as_str()
        } else {
            "Unknown model"
        };
        self.effort
            .as_ref()
            .map(|effort| format!("{model} · {effort}"))
            .unwrap_or_else(|| model.to_owned())
    }
}

/// When the most recent Orion session last changed, for the idle timer.
pub fn last_activity(sessions: &[Session]) -> Option<i64> {
    sessions
        .iter()
        .map(Session::last_change)
        .filter(|updated| *updated > 0)
        .max()
}

pub fn preferred_session(sessions: &[Session]) -> Option<&Session> {
    let now = chrono::Utc::now().timestamp_millis();
    sessions
        .iter()
        .filter(|session| session.is_active(now))
        .max_by_key(|session| session.last_change())
}

/// Cost of one usage slice split into the four billable token categories, in
/// USD. The same split Claude sessions report.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CostParts {
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_read: f64,
}

impl CostParts {
    pub fn total(&self) -> f64 {
        self.input + self.output + self.cache_write + self.cache_read
    }

    pub fn add(&mut self, other: &CostParts) {
        self.input += other.input;
        self.output += other.output;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
    }

    fn is_finite(&self) -> bool {
        [self.input, self.output, self.cache_write, self.cache_read]
            .iter()
            .all(|part| part.is_finite() && *part >= 0.0)
    }
}

/// One priced request. `complete` is false when the rate card leaves part of
/// the request unpriced (for example an unpublished fast-mode multiplier).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    pub parts: CostParts,
    pub complete: bool,
}

/// Claude ids Pulse can price. An unrecognised `claude-*` family is left
/// unpriced instead of inheriting the default tier of `cost::model_pricing`.
fn is_priced_claude_family(lower: &str) -> bool {
    lower.starts_with("claude-")
        && ["opus", "sonnet", "haiku", "fable", "mythos"]
            .iter()
            .any(|family| lower.contains(family))
}

/// Splits Orion's `-fast` / `--fast` suffix off a model id.
fn split_fast_variant(lower: &str) -> (&str, bool) {
    match lower.strip_suffix("-fast") {
        Some(base) if !base.trim_end_matches('-').is_empty() => (base.trim_end_matches('-'), true),
        _ => (lower, false),
    }
}

/// Catalog key for an Orion model id. Orion also lists Codex-routed models by
/// display name (`5.6-Sol`), so a bare id resolves as `gpt-<id>` only when the
/// catalog display name matches it exactly. An ambiguous id such as `5.6-Cyber`,
/// whose catalog entries are Cyber-Red and Cyber-Blue, stays unresolved and
/// unpriced.
fn catalog_key(key: &str) -> String {
    if crate::codex::model::resolve_model(key).is_some() {
        return key.to_string();
    }
    let by_name = format!("gpt-{key}");
    match crate::codex::model::resolve_model(&by_name) {
        Some(model) if model.display_name().to_ascii_lowercase().replace(' ', "-") == key => {
            by_name
        }
        _ => key.to_string(),
    }
}

/// API-equivalent price of one request: exact token counts times the published
/// per-model rates. Claude models use `src/cost.rs`; models in the Codex
/// catalog use `src/codex/cost.rs`. Orion stores `cost: 0` on every message,
/// which is not a price, so a model without a known rate returns `None`
/// instead of zero.
pub fn estimate_cost(model: &str, usage: &Usage) -> Option<Estimate> {
    let id = model.rsplit('/').next().unwrap_or(model).trim();
    if id.is_empty() {
        return None;
    }
    let lower = id.to_ascii_lowercase();
    if is_priced_claude_family(&lower) {
        let breakdown = crate::cost::calculate_category_costs(
            id,
            usage.fresh_input(),
            usage.output,
            usage.cache_write,
            usage.cache_read,
            false,
        );
        let parts = CostParts {
            input: breakdown.input_cost,
            output: breakdown.output_cost,
            cache_write: breakdown.cache_write_cost,
            cache_read: breakdown.cache_read_cost,
        };
        return parts.is_finite().then_some(Estimate {
            parts,
            complete: true,
        });
    }
    let (base, fast) = split_fast_variant(&lower);
    let key = catalog_key(base);
    let speed = if fast {
        crate::codex::model::SessionSpeed::explicit(
            crate::codex::model::SpeedMode::Fast,
            crate::codex::model::SpeedSource::ModelSuffix,
        )
    } else {
        crate::codex::model::SessionSpeed::explicit(
            crate::codex::model::SpeedMode::Standard,
            crate::codex::model::SpeedSource::LegacyDefault,
        )
    };
    // One assistant message is one model request, so its input tokens are the
    // prompt size that decides any long-context rate.
    let computation = crate::codex::cost::compute_cost_for_request(
        &key,
        crate::codex::cost::TokenUsage {
            input_tokens: usage.input,
            cached_input_tokens: usage.cache_read,
            cache_write_tokens: Some(usage.cache_write),
            output_tokens: usage.output,
        },
        speed,
        &Default::default(),
        Some(usage.input),
    );
    computation.known_total_cost_usd?;
    let parts = CostParts {
        input: computation.breakdown.input_cost_usd,
        output: computation.breakdown.output_cost_usd,
        cache_write: computation.breakdown.cache_write_cost_usd,
        cache_read: computation.breakdown.cached_input_cost_usd,
    };
    parts.is_finite().then_some(Estimate {
        parts,
        complete: computation.status == crate::codex::cost::PricingStatus::Exact,
    })
}

/// Session cost after every request has been priced.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SessionCost {
    pub total: Option<f64>,
    pub parts: CostParts,
    pub complete: bool,
}

/// Accumulates priced requests into a session cost, per category.
#[derive(Clone, Debug, Default)]
pub struct CostLedger {
    parts: CostParts,
    observed: bool,
    priced: bool,
    unpriced: bool,
    inexact: bool,
}

impl CostLedger {
    pub fn add(&mut self, model: &str, usage: &Usage) {
        if usage.total() == 0 {
            return;
        }
        self.observed = true;
        match estimate_cost(model, usage) {
            Some(estimate) => {
                self.priced = true;
                self.inexact |= !estimate.complete;
                self.parts.add(&estimate.parts);
            }
            None => self.unpriced = true,
        }
    }

    /// No usage is a true zero. Usage where no model has a rate is unavailable.
    /// Usage where only some models have a rate is a lower bound.
    pub fn finish(&self) -> SessionCost {
        if !self.observed {
            return SessionCost {
                total: Some(0.0),
                parts: CostParts::default(),
                complete: true,
            };
        }
        if !self.priced {
            return SessionCost::default();
        }
        let total = self.parts.total();
        if !total.is_finite() {
            return SessionCost::default();
        }
        SessionCost {
            total: Some(total),
            parts: self.parts,
            complete: !self.unpriced && !self.inexact,
        }
    }
}

impl Session {
    pub fn apply_cost(&mut self, cost: SessionCost) {
        self.known_cost = cost.total;
        self.cost_complete = cost.total.is_some() && cost.complete;
        self.cost_parts = cost.parts;
    }
}

/// Output tokens per second while generating. `None` without both a token
/// count and a positive generation time.
pub fn output_speed(tokens: u64, millis: u64) -> Option<f64> {
    (tokens > 0 && millis > 0).then(|| tokens as f64 / (millis as f64 / 1000.0))
}

/// Project name from an Orion session directory. Orion records the path of the
/// machine that wrote it, so a Windows path must still split on `\` when Pulse
/// reads it on Linux or macOS.
pub fn project_name(directory: &str) -> String {
    let trimmed = directory.trim().trim_end_matches(['/', '\\']);
    let name = trimmed.rsplit(['/', '\\']).next().unwrap_or_default();
    let drive_only = name.len() == 2 && name.ends_with(':');
    if name.is_empty() || drive_only {
        NAME.to_string()
    } else {
        name.to_string()
    }
}

/// Display label for Orion's `reasoningLevel`: `xhigh` -> "Extra High".
/// `default` means Orion applied the model default, which is not a level.
pub fn effort_label(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("default") {
        return None;
    }
    if let Some(effort) = crate::session::ReasoningEffort::from_api(value) {
        return Some(effort.label().to_string());
    }
    let mut chars = value.chars();
    chars.next().map(|first| {
        first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect()
    })
}

pub fn display_model_name(model: &str) -> String {
    let id = model.rsplit('/').next().unwrap_or(model);
    if id.to_ascii_lowercase().starts_with("claude-") {
        return crate::cost::model_display_name(id);
    }
    crate::codex::model::resolve_model(id)
        .map(|model| model.display_name().to_string())
        .unwrap_or_else(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_input_excludes_cache_categories_once() {
        let usage = Usage {
            input: 68_601,
            output: 270,
            cache_read: 68_349,
            cache_write: 250,
            ..Default::default()
        };
        assert_eq!(usage.fresh_input(), 2);
        assert_eq!(usage.total(), 68_871);
    }

    #[test]
    fn effort_labels_use_title_case_and_hide_the_model_default() {
        for (raw, label) in [
            ("low", "Low"),
            ("medium", "Medium"),
            ("high", "High"),
            ("xhigh", "Extra High"),
            ("max", "Max"),
            ("ultra", "Ultra"),
            ("minimal", "Minimal"),
        ] {
            assert_eq!(effort_label(raw).as_deref(), Some(label), "{raw}");
        }
        assert_eq!(effort_label("default"), None);
        assert_eq!(effort_label(""), None);
    }

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    /// Orion's `input` already contains both cache categories.
    fn one_million_each() -> Usage {
        Usage {
            input: 3_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 1_000_000,
            ..Default::default()
        }
    }

    #[test]
    fn claude_models_price_each_category_at_the_published_rate() {
        // Claude Opus 5.5: $4 input, $20 output, $5 cache write, $0.20 cache read.
        let estimate = estimate_cost("anthropic/claude-opus-5-5", &one_million_each()).unwrap();
        assert!(estimate.complete);
        close(estimate.parts.input, 4.0);
        close(estimate.parts.output, 20.0);
        close(estimate.parts.cache_write, 5.0);
        close(estimate.parts.cache_read, 0.20);
        close(estimate.parts.total(), 29.20);
        assert_eq!(
            display_model_name("anthropic/claude-opus-5-5"),
            "Claude Opus 5.5"
        );
    }

    #[test]
    fn unknown_models_and_unknown_claude_families_stay_unpriced() {
        let usage = one_million_each();
        assert_eq!(estimate_cost("opencode-go/unknown-model-x", &usage), None);
        assert_eq!(
            estimate_cost("command-code/xiaomi/mimo-v2.6-pro", &usage),
            None
        );
        // Not a family Pulse has a rate card for: must not fall to a default tier.
        assert_eq!(estimate_cost("anthropic/claude-zzz-9", &usage), None);
        assert_eq!(estimate_cost("", &usage), None);
    }

    #[test]
    fn codex_catalog_models_split_categories_and_flag_unpublished_fast_rates() {
        let usage = one_million_each();
        // GPT-5.6 Sol: $5 input, $6.25 cache write, $0.50 cache read, $30 output.
        let sol = estimate_cost("gpt-5.6-sol", &usage).unwrap();
        assert!(sol.complete);
        close(sol.parts.input, 5.0);
        close(sol.parts.output, 30.0);
        close(sol.parts.cache_write, 6.25);
        close(sol.parts.cache_read, 0.50);
        // Orion lists Codex-routed models by display name; an exact display
        // match resolves, an ambiguous one does not.
        let named = estimate_cost("5.6-Sol", &usage).unwrap();
        close(named.parts.total(), sol.parts.total());
        assert_eq!(estimate_cost("5.6-Cyber", &usage), None);
        // Sol has a fast mode but no published multiplier: priced, not complete.
        let fast = estimate_cost("5.6-Sol-Fast", &usage).unwrap();
        assert!(!fast.complete);
        close(fast.parts.total(), sol.parts.total());
        // Astra publishes a 2x fast multiplier.
        let astra = estimate_cost("gpt-6-astra", &usage).unwrap();
        let astra_fast = estimate_cost("gpt-6-astra--fast", &usage).unwrap();
        assert!(astra_fast.complete);
        close(astra_fast.parts.total(), astra.parts.total() * 2.0);
    }

    #[test]
    fn ledger_sums_categories_across_models_and_keeps_partial_honest() {
        let mut ledger = CostLedger::default();
        ledger.add("anthropic/claude-opus-5-5", &one_million_each());
        ledger.add("anthropic/claude-opus-5-5", &one_million_each());
        let priced = ledger.finish();
        assert!(priced.complete);
        close(priced.total.unwrap(), 58.40);
        close(priced.parts.input, 8.0);
        close(priced.parts.output, 40.0);
        close(priced.parts.cache_write, 10.0);
        close(priced.parts.cache_read, 0.40);
        close(priced.parts.total(), priced.total.unwrap());

        // One unpriced model: the known part survives as a lower bound.
        ledger.add("opencode-go/unknown-model-x", &one_million_each());
        let partial = ledger.finish();
        assert!(!partial.complete);
        close(partial.total.unwrap(), 58.40);
    }

    #[test]
    fn ledger_reports_unavailable_without_a_priced_model_and_zero_without_usage() {
        let mut only_unknown = CostLedger::default();
        only_unknown.add("opencode-go/unknown-model-x", &one_million_each());
        let unavailable = only_unknown.finish();
        assert_eq!(unavailable.total, None);
        assert!(!unavailable.complete);
        assert_eq!(unavailable.parts, CostParts::default());

        // A session that never called a model is a true zero, not unavailable.
        let mut empty = CostLedger::default();
        empty.add("anthropic/claude-opus-5-5", &Usage::default());
        let zero = empty.finish();
        assert_eq!(zero.total, Some(0.0));
        assert!(zero.complete);
    }

    #[test]
    fn apply_cost_copies_the_ledger_into_the_session() {
        let mut ledger = CostLedger::default();
        ledger.add("anthropic/claude-opus-5-5", &one_million_each());
        let mut session = Session::default();
        session.apply_cost(ledger.finish());
        close(session.known_cost.unwrap(), 29.20);
        assert!(session.cost_complete);
        close(session.cost_parts.cache_write, 5.0);

        session.apply_cost(CostLedger::default().finish());
        assert_eq!(session.known_cost, Some(0.0));
        let mut only_unknown = CostLedger::default();
        only_unknown.add("opencode-go/unknown-model-x", &one_million_each());
        session.apply_cost(only_unknown.finish());
        assert_eq!(session.known_cost, None);
        assert!(!session.cost_complete);
        assert_eq!(session.cost_parts, CostParts::default());
    }

    #[test]
    fn output_speed_is_generated_tokens_over_generation_time() {
        close(output_speed(1_500, 10_000).unwrap(), 150.0);
        assert_eq!(output_speed(0, 10_000), None);
        assert_eq!(output_speed(1_500, 0), None);
    }

    #[test]
    fn project_name_splits_windows_and_posix_paths_on_every_os() {
        for (directory, expected) in [
            ("D:\\work\\pulse", "pulse"),
            ("D:\\work\\pulse\\", "pulse"),
            ("C:\\Users\\dev\\my project", "my project"),
            ("/home/dev/pulse", "pulse"),
            ("/home/dev/pulse/", "pulse"),
            ("\\\\server\\share\\repo", "repo"),
            ("pulse", "pulse"),
            ("D:\\", NAME),
            ("D:", NAME),
            ("/", NAME),
            ("", NAME),
        ] {
            assert_eq!(project_name(directory), expected, "{directory:?}");
        }
    }
}
