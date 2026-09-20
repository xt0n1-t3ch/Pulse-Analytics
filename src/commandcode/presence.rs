use super::{ASSET_KEY, Config, NAME, Session};
use codex_presence_core::{PresenceFieldId, PresenceLines, PresenceValues, compose_presence};
use discord_rich_presence::{
    DiscordIpc, DiscordIpcClient,
    activity::{Activity, Assets, Timestamps},
};
use std::time::{Duration, Instant};

pub fn lines(
    session: Option<&Session>,
    config: &Config,
    quotas: Option<&str>,
    credits: Option<&str>,
) -> PresenceLines {
    let Some(session) = session else {
        return PresenceLines {
            details: "No active session".into(),
            state: "Waiting for Command Code".into(),
        };
    };
    let mut values = PresenceValues::default();
    values.insert(PresenceFieldId::Model, session.model_label());
    values.insert(PresenceFieldId::Activity, &session.activity);
    if !config.privacy_enabled {
        values.insert(PresenceFieldId::Project, &session.project);
        if let Some(branch) = &session.branch {
            values.insert(PresenceFieldId::Branch, branch);
        }
    }
    if let Some(cost) = session.known_cost {
        values.insert(PresenceFieldId::Cost, format!("{}{cost:.2}", '$'));
    }
    if session.usage_messages > 0 {
        values.insert(
            PresenceFieldId::Tokens,
            format!("{} tok", crate::util::format_tokens(session.usage.total())),
        );
    }
    if let Some(quotas) = quotas.filter(|_| !session.byok) {
        values.insert(PresenceFieldId::Quotas, quotas);
    }
    if let Some(credits) = credits.filter(|_| !session.byok) {
        values.insert(PresenceFieldId::Credits, credits);
    }
    if let Some((used, window)) = session
        .context_used
        .zip(session.context_window)
        .filter(|(_, window)| *window > 0)
    {
        values.insert(
            PresenceFieldId::Context,
            format!(
                "Ctx {:.0}% used",
                (used as f64 / window as f64 * 100.0).min(100.0)
            ),
        );
    }
    compose_presence(&config.layout, &values, NAME, "Command Code session")
}

#[derive(Default)]
pub struct Publisher {
    client: Option<DiscordIpcClient>,
    client_id: String,
    sent: Option<(String, String, i64)>,
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
        self.sent = None;
        self.status = "Disconnected".into();
    }
    pub fn update(
        &mut self,
        session: Option<&Session>,
        config: &Config,
        quotas: Option<&str>,
        credits: Option<&str>,
    ) {
        if !config.enabled {
            self.shutdown();
            self.status = "Disabled".into();
            return;
        }
        let Some(session) =
            session.filter(|session| session.is_active(chrono::Utc::now().timestamp_millis()))
        else {
            self.shutdown();
            self.status = "Waiting for Command Code session".into();
            return;
        };
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
        let lines = lines(Some(session), config, quotas, credits);
        let payload = (lines.details, lines.state, session.created / 1000);
        if self.sent.as_ref() == Some(&payload)
            && self
                .last_sent
                .is_some_and(|last| last.elapsed() < Duration::from_secs(30))
        {
            return;
        }
        let activity = Activity::new()
            .details(&payload.0)
            .state(&payload.1)
            .assets(Assets::new().large_image(ASSET_KEY).large_text(NAME))
            .timestamps(Timestamps::new().start(payload.2));
        if self
            .client
            .as_mut()
            .is_some_and(|client| client.set_activity(activity).is_ok())
        {
            self.sent = Some(payload);
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
    fn byok_sessions_do_not_inherit_command_subscription_allowances() {
        let config = Config::default();
        let gateway = Session {
            model: "example/model".into(),
            ..Default::default()
        };
        let byok = Session {
            byok: true,
            ..gateway.clone()
        };
        assert!(
            lines(
                Some(&gateway),
                &config,
                Some("5h 12% used"),
                Some("Credits 20")
            )
            .state
            .contains("5h 12%")
        );
        let result = lines(
            Some(&byok),
            &config,
            Some("5h 12% used"),
            Some("Credits 20"),
        );
        assert!(!result.state.contains("5h 12%"));
        assert!(!result.state.contains("Credits 20"));
    }
    #[test]
    fn cli_and_desktop_share_identity_art_and_composition() {
        let cli = Session {
            model: "provider/model".into(),
            activity: "Thinking".into(),
            known_cost: Some(1.23),
            ..Default::default()
        };
        let desktop = Session {
            desktop: true,
            ..cli.clone()
        };
        let config = Config::default();
        assert_eq!(config.client_id, "1551026507806281829");
        assert_eq!(NAME, "Command Code");
        assert_eq!(ASSET_KEY, "commandcode");
        assert_eq!(
            lines(Some(&cli), &config, None, None),
            lines(Some(&desktop), &config, None, None)
        );
        let public = lines(Some(&cli), &config, None, None);
        assert!(public.state.contains("$1.23"));
        assert!(!public.state.contains("partial"));
        assert!(!public.state.contains('~'));
    }
    #[test]
    fn private_cost_and_project_are_not_published() {
        let session = Session {
            project: "private-project".into(),
            known_cost: Some(2.25),
            ..Default::default()
        };
        let mut config = Config {
            privacy_enabled: true,
            ..Default::default()
        };
        for field in &mut config.layout.fields {
            if field.field == PresenceFieldId::Cost {
                field.enabled = false;
            }
        }
        let value = lines(Some(&session), &config, None, None);
        assert!(!value.state.contains("$2.25"));
        assert!(!value.details.contains("private-project"));
    }
}
