//! OpenCode integration: configuration, data directory and session model types.

pub mod presence;
pub mod process;
mod store;

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub use store::{Collector, ImportBatch, discover_databases};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    pub client_id: String,
    pub database_paths: Vec<PathBuf>,
    pub privacy_enabled: bool,
    pub layout: codex_presence_core::PresenceLayoutConfig,
}

impl Default for Config {
    fn default() -> Self {
        let mut layout = codex_presence_core::PresenceLayoutConfig::default();
        layout
            .fields
            .sort_by_key(|item| item.field != codex_presence_core::PresenceFieldId::Model);
        for item in &mut layout.fields {
            if item.field == codex_presence_core::PresenceFieldId::Model {
                item.zone = codex_presence_core::PresenceZone::Details;
            }
        }
        Self {
            enabled: true,
            client_id: "1545590419763761303".into(),
            database_paths: Vec::new(),
            privacy_enabled: false,
            layout,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        crate::storage::config_file("opencode.json")
    }

    pub fn load() -> Result<Self> {
        match std::fs::read(Self::path()) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&self) -> Result<()> {
        if !self.client_id.is_empty() && !(17..=22).contains(&self.client_id.len())
            || !self.client_id.bytes().all(|byte| byte.is_ascii_digit())
        {
            anyhow::bail!("OpenCode application ID must be a Discord numeric application ID");
        }
        Ok(crate::codex::util::write_json_pretty_atomic(
            &Self::path(),
            self,
        )?)
    }
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"))
        .join("opencode")
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct ModelUsage {
    pub provider_id: String,
    pub model_id: String,
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: Option<f64>,
}

impl ModelUsage {
    pub fn total_tokens(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.reasoning)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Metadata {
    pub model_provider: String,
    pub model_id: String,
    pub model_name: String,
    pub variant: Option<String>,
    pub surface: String,
    pub models: Vec<ModelUsage>,
    pub context_source: Option<String>,
    pub source_database: PathBuf,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Session {
    pub id: String,
    pub directory: PathBuf,
    pub title: String,
    pub project: String,
    pub parent_id: Option<String>,
    pub created: i64,
    pub updated: i64,
    pub archived: bool,
    pub metadata: Metadata,
    pub cost: Option<f64>,
    pub usage: ModelUsage,
    pub context_used: Option<u64>,
    pub context_window: Option<u64>,
    pub activity: String,
    pub activity_target: Option<String>,
    pub branch: Option<String>,
}

impl Session {
    pub fn is_recent(&self, now: i64) -> bool {
        !self.archived
            && now.saturating_sub(self.updated) <= 600_000
            && self.updated <= now + 60_000
    }

    pub fn is_idle(&self, now: i64) -> bool {
        !self.is_recent(now)
            || now.saturating_sub(self.updated) > 300_000
            || matches!(self.activity.as_str(), "Idle" | "Waiting for input")
    }

    pub fn model_label(&self) -> String {
        let name = if self.metadata.model_name.is_empty() {
            "Unknown model"
        } else {
            &self.metadata.model_name
        };
        let mut label = name.to_string();
        if let Some(variant) = self
            .metadata
            .variant
            .as_deref()
            .filter(|value| *value != "default")
        {
            label.push_str(" · ");
            label.push_str(variant);
        }
        label
    }
}

pub fn preferred_session(sessions: &[Session]) -> Option<&Session> {
    let now = chrono::Utc::now().timestamp_millis();
    sessions
        .iter()
        .filter(|session| session.parent_id.is_none() && !session.is_idle(now))
        .max_by_key(|session| (!session.is_idle(now), session.updated))
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    #[test]
    fn completed_sessions_leave_presence_immediately_but_remain_history() {
        let now = chrono::Utc::now().timestamp_millis();
        let mut session = Session {
            updated: now,
            activity: "Thinking".into(),
            ..Default::default()
        };
        assert!(preferred_session(std::slice::from_ref(&session)).is_some());
        for activity in ["Waiting for input", "Idle"] {
            session.activity = activity.into();
            assert!(session.is_recent(now));
            assert!(preferred_session(std::slice::from_ref(&session)).is_none());
        }
    }
}
