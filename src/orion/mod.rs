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
    pub updated: i64,
    pub usage: Usage,
    pub known_cost: Option<f64>,
    pub cost_complete: bool,
    pub context_used: Option<u64>,
    pub context_window: Option<u64>,
    pub activity: String,
    pub activity_target: Option<String>,
    pub subagent_count: usize,
    pub messages: u64,
}

impl Session {
    pub fn is_recent(&self, now: i64) -> bool {
        self.updated > 0
            && self.updated <= now + 60_000
            && now.saturating_sub(self.updated) <= 600_000
    }

    pub fn is_active(&self, now: i64) -> bool {
        self.is_recent(now)
            && now.saturating_sub(self.updated) <= 120_000
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

pub fn preferred_session(sessions: &[Session]) -> Option<&Session> {
    let now = chrono::Utc::now().timestamp_millis();
    sessions
        .iter()
        .filter(|session| session.is_active(now))
        .max_by_key(|session| session.updated)
}

/// API-equivalent estimate for models with a known public rate. Orion stores
/// `cost: 0` on every message, which is not a price, so missing rates stay
/// unavailable instead of zero.
pub fn estimate_cost(model: &str, usage: &Usage) -> Option<(f64, bool)> {
    let id = model.rsplit('/').next().unwrap_or(model);
    let lower = id.to_ascii_lowercase();
    if lower.starts_with("claude-") || lower.contains("opus") || lower.contains("sonnet") {
        let cost = crate::cost::calculate_cost(
            id,
            usage.fresh_input(),
            usage.output,
            usage.cache_write,
            usage.cache_read,
        );
        return cost.is_finite().then_some((cost, true));
    }
    let computation = crate::codex::cost::compute_cost(
        id,
        crate::codex::cost::TokenUsage {
            input_tokens: usage.input,
            cached_input_tokens: usage.cache_read,
            cache_write_tokens: Some(usage.cache_write),
            output_tokens: usage.output,
        },
        crate::codex::model::SessionSpeed::explicit(
            crate::codex::model::SpeedMode::Standard,
            crate::codex::model::SpeedSource::LegacyDefault,
        ),
        &Default::default(),
    );
    let total = computation.known_total_cost_usd?;
    Some((
        total,
        computation.status == crate::codex::cost::PricingStatus::Exact,
    ))
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

    #[test]
    fn claude_models_use_claude_rates_and_unknown_models_stay_unavailable() {
        let usage = Usage {
            input: 1_000_000,
            output: 1_000_000,
            ..Default::default()
        };
        let (cost, complete) = estimate_cost("anthropic/claude-opus-5-5", &usage).unwrap();
        assert!(complete);
        assert!((cost - 24.0).abs() < 1e-9);
        assert_eq!(estimate_cost("opencode-go/unknown-model-x", &usage), None);
        assert_eq!(
            display_model_name("anthropic/claude-opus-5-5"),
            "Claude Opus 5.5"
        );
    }
}
