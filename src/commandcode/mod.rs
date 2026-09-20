pub mod presence;
mod store;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
pub use store::{Batch, Collector};

pub const NAME: &str = "Command Code";
pub const CLIENT_ID: &str = "1551026507806281829";
pub const ASSET_KEY: &str = "commandcode";

pub fn monthly_allocation(plan: &str) -> Option<f64> {
    match plan {
        "individual-go" => Some(10.0),
        "individual-goat" => Some(70.0),
        "individual-pro" => Some(30.0),
        "individual-pro-v1" => Some(80.0),
        "individual-max" => Some(150.0),
        "individual-ultra" => Some(300.0),
        _ => None,
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("PULSE_COMMANDCODE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".commandcode"))
}
pub fn desktop_home() -> PathBuf {
    dirs::config_dir().unwrap_or_default().join("Command Code")
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    pub client_id: String,
    pub profile_paths: Vec<PathBuf>,
    pub privacy_enabled: bool,
    pub layout: codex_presence_core::PresenceLayoutConfig,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            client_id: CLIENT_ID.into(),
            profile_paths: vec![],
            privacy_enabled: false,
            layout: Default::default(),
        }
    }
}
impl Config {
    pub fn path() -> PathBuf {
        crate::storage::home().join("pulse-commandcode.json")
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
            anyhow::bail!("Command Code requires a numeric Discord application ID");
        }
        for path in &self.profile_paths {
            if !path.is_absolute() {
                anyhow::bail!("Command Code profiles must use absolute paths");
            }
        }
        Ok(crate::codex::util::write_json_pretty_atomic(
            &Self::path(),
            self,
        )?)
    }
    pub fn roots(&self) -> Vec<PathBuf> {
        if self.profile_paths.is_empty() {
            vec![home()]
        } else {
            self.profile_paths.clone()
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}
impl Usage {
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }
}
#[derive(Clone, Debug, Default)]
pub struct Session {
    pub id: String,
    pub profile: PathBuf,
    pub directory: PathBuf,
    pub title: Option<String>,
    pub project: String,
    pub branch: Option<String>,
    pub model: String,
    pub effort: Option<String>,
    pub created: i64,
    pub updated: i64,
    pub usage: Usage,
    pub known_cost: Option<f64>,
    pub cost_complete: bool,
    pub context_used: Option<u64>,
    pub context_window: Option<u64>,
    pub activity: String,
    pub desktop: bool,
    pub byok: bool,
    pub usage_messages: u64,
    pub priced_messages: u64,
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
        let model = if self.model.is_empty() {
            "Unknown model"
        } else {
            &self.model
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
