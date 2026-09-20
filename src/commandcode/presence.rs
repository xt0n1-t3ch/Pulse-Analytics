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
            details: "Idle".into(),
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct Payload {
    details: String,
    state: String,
    started_at: Option<i64>,
}

fn desired_payload(
    session: Option<&Session>,
    config: &Config,
    quotas: Option<&str>,
    credits: Option<&str>,
    now: i64,
) -> Option<Payload> {
    if !config.enabled {
        return None;
    }
    let session = session.filter(|session| session.is_active(now));
    let lines = lines(session, config, quotas, credits);
    Some(Payload {
        details: lines.details,
        state: lines.state,
        started_at: session
            .and_then(|session| (session.created > 0).then_some(session.created / 1000)),
    })
}

#[derive(Default)]
pub struct Publisher {
    client: Option<DiscordIpcClient>,
    client_id: String,
    sent: Option<Payload>,
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
        let Some(payload) = desired_payload(
            session,
            config,
            quotas,
            credits,
            chrono::Utc::now().timestamp_millis(),
        ) else {
            self.shutdown();
            self.status = "Disabled".into();
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
        if self.sent.as_ref() == Some(&payload)
            && self
                .last_sent
                .is_some_and(|last| last.elapsed() < Duration::from_secs(30))
        {
            return;
        }
        let mut activity = Activity::new()
            .details(&payload.details)
            .state(&payload.state)
            .assets(Assets::new().large_image(ASSET_KEY).large_text(NAME));
        if let Some(started_at) = payload.started_at {
            activity = activity.timestamps(Timestamps::new().start(started_at));
        }
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
    #[ignore = "Publishes real idle presence to local Discord; stop other Pulse publishers first"]
    fn local_discord_acknowledges_idle_from_the_production_publisher() {
        let output =
            std::env::var("PULSE_COMMANDCODE_IDLE_PROOF").expect("Explicit proof path required");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<serde_json::Value> {
                let mut publisher = Publisher::default();
                let mut config = Config::default();
                publisher.update(None, &config, None, None);
                anyhow::ensure!(publisher.status() == "Connected", "Idle did not connect");
                let (_, accepted) = publisher.client.as_mut().unwrap().recv()?;
                config.enabled = false;
                publisher.update(None, &config, None, None);
                anyhow::ensure!(
                    publisher.client.is_none() && publisher.status() == "Disabled",
                    "Disabled publisher remained connected"
                );
                anyhow::ensure!(
                    accepted["cmd"] == "SET_ACTIVITY" && accepted["evt"] != "ERROR",
                    "Discord rejected idle presence"
                );
                anyhow::ensure!(
                    accepted["data"]["name"] == NAME,
                    "Incorrect application name"
                );
                anyhow::ensure!(
                    accepted["data"]["application_id"] == super::super::CLIENT_ID,
                    "Incorrect application identity"
                );
                anyhow::ensure!(accepted["data"]["details"] == "Idle", "Missing idle state");
                anyhow::ensure!(
                    accepted["data"]["timestamps"].is_null(),
                    "Idle retained an elapsed session timer"
                );
                Ok(
                    serde_json::json!({"application_id": accepted["data"]["application_id"], "name": accepted["data"]["name"], "details": accepted["data"]["details"], "state": accepted["data"]["state"], "assets": accepted["data"]["assets"], "idle_acknowledged":true,"disabled_disconnected":true,"timer_absent":true}),
                )
            })();
            let _ = tx.send(result);
        });
        let result = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("Discord IPC timed out")
            .expect("Idle publisher proof failed");
        std::fs::write(output, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
        println!("{result}");
    }
    #[test]
    fn enabled_idle_presence_has_no_session_values_or_elapsed_timer() {
        let config = Config::default();
        let now = 1_789_920_000_000;
        let idle =
            desired_payload(None, &config, Some("5h 12% used"), Some("Credits 20"), now).unwrap();
        assert_eq!(idle.details, "Idle");
        assert_eq!(idle.state, "Waiting for Command Code");
        assert_eq!(idle.started_at, None);
        let completed = Session {
            project: "old-project".into(),
            model: "old-model".into(),
            created: now - 60_000,
            updated: now,
            activity: "Waiting for input".into(),
            known_cost: Some(25.0),
            ..Default::default()
        };
        assert_eq!(
            desired_payload(Some(&completed), &config, None, None, now),
            Some(idle.clone())
        );
        let stale = Session {
            updated: now - 180_000,
            activity: "Thinking".into(),
            ..completed.clone()
        };
        assert_eq!(
            desired_payload(Some(&stale), &config, None, None, now),
            Some(idle)
        );
        let active = Session {
            activity: "Thinking".into(),
            ..completed
        };
        assert_eq!(
            desired_payload(Some(&active), &config, None, None, now)
                .unwrap()
                .started_at,
            Some(active.created / 1000)
        );
    }

    #[test]
    fn disabling_presence_clears_both_active_and_idle_publication() {
        let config = Config {
            enabled: false,
            ..Default::default()
        };
        let now = 1_789_920_000_000;
        let active = Session {
            created: now,
            updated: now,
            activity: "Thinking".into(),
            ..Default::default()
        };
        assert_eq!(desired_payload(None, &config, None, None, now), None);
        assert_eq!(
            desired_payload(Some(&active), &config, None, None, now),
            None
        );
    }
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
