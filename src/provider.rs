use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    Claude,
    Codex,
    OpenCode,
    #[serde(rename = "commandcode")]
    CommandCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub cache_health: bool,
    pub model_routing: bool,
    pub extra_usage: bool,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CommandCode => "commandcode",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::CommandCode => "Command Code",
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
        }
    }

    pub fn short_name(self) -> &'static str {
        match self {
            Self::CommandCode => "Command Code",
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
        }
    }

    pub fn instruction_file_name(self) -> &'static str {
        match self {
            Self::CommandCode => "AGENTS.md",
            Self::Claude => "CLAUDE.md",
            Self::Codex => "AGENTS.md",
            Self::OpenCode => "AGENTS.md",
        }
    }

    pub fn home_dir_name(self) -> &'static str {
        match self {
            Self::CommandCode => ".commandcode",
            Self::Claude => ".claude",
            Self::Codex => ".codex",
            Self::OpenCode => "opencode",
        }
    }

    pub fn sessions_glob_label(self) -> &'static str {
        match self {
            Self::CommandCode => "~/.commandcode/projects/**/*.jsonl",
            Self::Claude => "~/.claude/projects/**/*.jsonl",
            Self::Codex => "~/.codex/sessions/**/*.jsonl",
            Self::OpenCode => "~/.local/share/opencode/opencode*.db",
        }
    }

    pub fn global_state_label(self) -> &'static str {
        match self {
            Self::CommandCode => "Command Code local transcripts and account API",
            Self::Claude => "~/.claude/discord-presence-data.json + usage API",
            Self::Codex => "~/.codex/.codex-global-state.json + session telemetry",
            Self::OpenCode => "OpenCode local SQLite metadata",
        }
    }

    pub fn fix_action_label(self) -> &'static str {
        match self {
            Self::CommandCode => "Fix with Command Code",
            Self::Claude => "Fix with Claude Code",
            Self::Codex => "Fix with Codex",
            Self::OpenCode => "Fix with OpenCode",
        }
    }

    pub fn fix_prompt_target(self) -> &'static str {
        self.display_name()
    }

    pub const fn capabilities(self) -> ProviderCapabilities {
        match self {
            Self::Claude => ProviderCapabilities {
                cache_health: true,
                model_routing: true,
                extra_usage: true,
            },
            Self::OpenCode | Self::CommandCode => ProviderCapabilities {
                cache_health: false,
                model_routing: false,
                extra_usage: false,
            },
            Self::Codex => ProviderCapabilities {
                cache_health: true,
                model_routing: false,
                extra_usage: false,
            },
        }
    }

    pub fn home_path(self) -> PathBuf {
        match self {
            Self::CommandCode => crate::commandcode::home(),
            Self::Claude => crate::config::claude_home(),
            Self::Codex => crate::codex::config::codex_home(),
            Self::OpenCode => crate::opencode::data_dir(),
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude_code" | "claude-code" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "opencode" => Some(Self::OpenCode),
            "commandcode" | "command_code" | "command-code" => Some(Self::CommandCode),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderState {
    active_provider: Provider,
}

impl Default for ProviderState {
    fn default() -> Self {
        Self {
            active_provider: Provider::Claude,
        }
    }
}

fn state_path() -> PathBuf {
    crate::storage::home().join("pulse-provider.json")
}

pub fn load_active_provider() -> Provider {
    let path = state_path();
    let Ok(raw) = fs::read_to_string(path) else {
        return Provider::Claude;
    };
    serde_json::from_str::<ProviderState>(&raw)
        .map(|state| state.active_provider)
        .unwrap_or(Provider::Claude)
}

pub fn save_active_provider(provider: Provider) -> Result<()> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create provider config dir {}", parent.display())
        })?;
    }
    let payload = serde_json::to_string_pretty(&ProviderState {
        active_provider: provider,
    })?;
    fs::write(&path, payload)
        .with_context(|| format!("failed to write provider state {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod capability_tests {
    use super::Provider;

    #[test]
    fn provider_capabilities_match_observed_analytics_contracts() {
        let claude = Provider::Claude.capabilities();
        assert!(claude.cache_health);
        assert!(claude.model_routing);
        assert!(claude.extra_usage);

        let codex = Provider::Codex.capabilities();
        assert!(codex.cache_health);
        assert!(!codex.model_routing);
        assert!(!codex.extra_usage);
    }
}
