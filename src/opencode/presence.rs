//! OpenCode Discord presence lines and publisher.

use std::time::{Duration, Instant};

use codex_presence_core::{PresenceFieldId, PresenceLines, PresenceValues, compose_presence};
use discord_rich_presence::{
    DiscordIpc, DiscordIpcClient,
    activity::{Activity, Assets, Timestamps},
};

use super::{Config, Session};

pub const ASSET_KEY: &str = "opencode-v2";

pub fn lines(session: Option<&Session>, config: &Config, go_quotas: Option<&str>) -> PresenceLines {
    if session.is_none() {
        return PresenceLines {
            details: "No active session".into(),
            state: "Waiting for OpenCode".into(),
        };
    }
    let mut values = PresenceValues::default();
    if let Some(quotas) = go_quotas {
        values.insert(PresenceFieldId::Quotas, quotas);
    }
    if let Some(session) = session {
        values.insert(PresenceFieldId::Model, session.model_label());
        values.insert(
            PresenceFieldId::Activity,
            if session.is_idle(chrono::Utc::now().timestamp_millis()) {
                "Waiting for input"
            } else {
                &session.activity
            },
        );
        if !config.privacy_enabled {
            values.insert(PresenceFieldId::Project, &session.project);
            if let Some(branch) = &session.branch {
                values.insert(PresenceFieldId::Branch, branch);
            }
        }
        if let Some(cost) = session.cost {
            values.insert(PresenceFieldId::Cost, format!("${cost:.2}"));
        }
        values.insert(
            PresenceFieldId::Tokens,
            crate::util::format_tokens(session.usage.total_tokens()),
        );
        if let (Some(used), Some(window)) = (
            session.context_used,
            session.context_window.filter(|window| *window > 0),
        ) {
            values.insert(
                PresenceFieldId::Context,
                format!(
                    "{} / {} ctx",
                    crate::util::format_tokens(used),
                    crate::util::format_tokens(window)
                ),
            );
        }
    }
    compose_presence(&config.layout, &values, "OpenCode", "OpenCode session")
}

#[derive(Default)]
pub struct Publisher {
    client: Option<DiscordIpcClient>,
    client_id: String,
    last_payload: Option<(String, String, Option<i64>)>,
    last_sent: Option<Instant>,
    last_attempt: Option<Instant>,
    status: String,
}

impl Publisher {
    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn shutdown(&mut self) {
        if let Some(mut client) = self.client.take() {
            let _ = client.clear_activity();
            let _ = client.close();
        }
        self.last_payload = None;
        self.last_sent = None;
        self.status = "Disconnected".into();
    }

    pub fn update(&mut self, session: Option<&Session>, config: &Config, go_quotas: Option<&str>) {
        if !config.enabled || config.client_id.is_empty() {
            self.shutdown();
            self.status = if config.enabled {
                "OpenCode application ID not configured"
            } else {
                "Disabled"
            }
            .into();
            return;
        }
        if session.is_none() {
            self.shutdown();
            self.status = "Waiting for OpenCode session".into();
            return;
        }
        if self.client_id != config.client_id {
            self.shutdown();
            self.client_id = config.client_id.clone();
            self.last_attempt = None;
        }
        if self.client.is_none() {
            if self
                .last_attempt
                .is_some_and(|last| last.elapsed() < Duration::from_secs(5))
            {
                return;
            }
            self.last_attempt = Some(Instant::now());
            let mut client = DiscordIpcClient::new(&config.client_id);
            if client.connect().is_err() {
                self.status = "Discord unavailable".into();
                return;
            }
            self.client = Some(client);
        }
        let presentation = lines(session, config, go_quotas);
        let start = session.map(|session| session.created / 1000);
        let payload = (presentation.details, presentation.state, start);
        if self.last_payload.as_ref() == Some(&payload)
            && self
                .last_sent
                .is_some_and(|last| last.elapsed() < Duration::from_secs(30))
        {
            return;
        }
        let mut activity = Activity::new()
            .details(&payload.0)
            .state(&payload.1)
            .assets(Assets::new().large_image(ASSET_KEY).large_text("OpenCode"));
        if let Some(start) = start {
            activity = activity.timestamps(Timestamps::new().start(start));
        }
        if self
            .client
            .as_mut()
            .is_some_and(|client| client.set_activity(activity).is_ok())
        {
            self.last_payload = Some(payload);
            self.last_sent = Some(Instant::now());
            self.status = "Connected".into();
        } else {
            self.shutdown();
            self.status = "Discord unavailable".into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monetary_presence_uses_two_decimal_currency_precision() {
        let config = Config::default();
        for (cost, expected) in [(0.0, "$0.00"), (0.004, "$0.00"), (1.237, "$1.24")] {
            let session = Session {
                cost: Some(cost),
                ..Default::default()
            };
            let rendered = lines(Some(&session), &config, None);
            assert!(rendered.state.starts_with(expected));
            assert!(!rendered.state.contains("$0.0000"));
        }
    }

    #[test]
    fn default_order_starts_with_model_and_each_toggle_owns_its_value() {
        let mut session = Session {
            project: "test-project".into(),
            branch: Some("test-branch".into()),
            cost: Some(0.25),
            context_used: Some(100),
            context_window: Some(1000),
            updated: chrono::Utc::now().timestamp_millis(),
            activity: "Writing".into(),
            ..Default::default()
        };
        session.metadata.model_name = "Test Model".into();
        session.usage.output = 123;
        let config = Config::default();
        let rendered = lines(Some(&session), &config, Some("Go 5h 1%"));
        assert!(
            rendered
                .details
                .starts_with("Test Model · Writing · test-project · test-branch")
        );
        for (field, text) in [
            (PresenceFieldId::Model, "Test Model"),
            (PresenceFieldId::Activity, "Writing"),
            (PresenceFieldId::Project, "test-project"),
            (PresenceFieldId::Branch, "test-branch"),
            (PresenceFieldId::Cost, "$0.25"),
            (PresenceFieldId::Tokens, "123"),
            (PresenceFieldId::Context, "ctx"),
            (PresenceFieldId::Quotas, "Go 5h 1%"),
        ] {
            let mut config = config.clone();
            for item in &mut config.layout.fields {
                item.enabled = item.field == field;
            }
            let enabled = lines(Some(&session), &config, Some("Go 5h 1%"));
            assert!(format!("{} {}", enabled.details, enabled.state).contains(text));
            for item in &mut config.layout.fields {
                item.enabled = false;
            }
            let disabled = lines(Some(&session), &config, Some("Go 5h 1%"));
            assert!(!format!("{} {}", disabled.details, disabled.state).contains(text));
        }
        let idle = lines(None, &config, Some("Go 5h 1%"));
        assert_eq!(idle.details, "No active session");
        assert!(!idle.state.contains("Go"));
    }

    #[test]
    fn go_quotas_respect_presets_toggle_and_order() {
        use codex_presence_core::{PresenceLayoutConfig, PresencePreset, PresenceZone};
        let quotas = "Go 5h 0% · 7d 73% · month 79% used";
        for preset in [
            PresencePreset::Minimal,
            PresencePreset::Standard,
            PresencePreset::Full,
        ] {
            let mut config = Config {
                layout: PresenceLayoutConfig::for_preset(preset),
                ..Default::default()
            };
            let enabled = config
                .layout
                .fields
                .iter()
                .any(|item| item.field == PresenceFieldId::Quotas && item.enabled);
            let rendered = lines(Some(&Session::default()), &config, Some(quotas));
            assert_eq!(
                format!("{} {}", rendered.details, rendered.state).contains(quotas),
                enabled
            );
            config
                .layout
                .fields
                .sort_by_key(|item| item.field != PresenceFieldId::Quotas);
            for item in &mut config.layout.fields {
                item.zone = PresenceZone::State;
                if item.field == PresenceFieldId::Quotas {
                    item.enabled = true;
                }
            }
            assert!(
                lines(Some(&Session::default()), &config, Some(quotas))
                    .state
                    .trim_start_matches("Quotas: ")
                    .starts_with(quotas)
            );
            assert!(!lines(None, &config, None).state.contains("Go"));
        }
    }

    #[test]
    fn privacy_and_unknown_cost_never_leak_into_presence() {
        let mut session = Session {
            project: "secret-project".into(),
            branch: Some("private-branch".into()),
            ..Default::default()
        };
        session.metadata.model_name = "Custom model".into();
        let config = Config {
            privacy_enabled: true,
            ..Default::default()
        };
        let result = lines(Some(&session), &config, None);
        let text = format!("{} {}", result.details, result.state);
        assert!(!text.contains("secret-project"));
        assert!(!text.contains("private-branch"));
        assert!(!text.contains('$'));
        assert!(text.contains("Custom model"));
        assert_eq!(ASSET_KEY, "opencode-v2");
    }
}
