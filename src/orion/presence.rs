use super::{ASSET_KEY, Config, NAME, Session};
use codex_presence_core::{PresenceFieldId, PresenceLines, PresenceValues, compose_presence};
use discord_rich_presence::{
    DiscordIpc, DiscordIpcClient,
    activity::{Activity, Assets, Timestamps},
};
use std::time::{Duration, Instant};

pub fn lines(session: Option<&Session>, config: &Config) -> PresenceLines {
    let Some(session) = session else {
        return PresenceLines {
            details: "Idle".into(),
            state: "Waiting for Orion App".into(),
        };
    };
    let mut values = PresenceValues::default();
    values.insert(PresenceFieldId::Model, session.model_label());
    let activity = match (&session.activity_target, config.privacy_enabled) {
        (Some(target), false) => format!("{} {target}", session.activity),
        _ => session.activity.clone(),
    };
    values.insert(PresenceFieldId::Activity, activity);
    if !config.privacy_enabled {
        values.insert(PresenceFieldId::Project, &session.project);
        if let Some(branch) = &session.branch {
            values.insert(PresenceFieldId::Branch, branch);
        }
    }
    if let Some(cost) = session.known_cost {
        values.insert(PresenceFieldId::Cost, format!("${cost:.2}"));
    }
    if session.usage.total() > 0 {
        values.insert(
            PresenceFieldId::Tokens,
            format!("{} tok", crate::util::format_tokens(session.usage.total())),
        );
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
    if session.subagent_count > 0 {
        values.insert(
            PresenceFieldId::Systems,
            format!(
                "{} subagent{}",
                session.subagent_count,
                if session.subagent_count == 1 { "" } else { "s" }
            ),
        );
    }
    compose_presence(&config.layout, &values, NAME, "Orion App session")
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Payload {
    details: String,
    state: String,
    started_at: Option<i64>,
}

fn desired_payload(session: Option<&Session>, config: &Config, now: i64) -> Option<Payload> {
    if !config.enabled {
        return None;
    }
    let session = session.filter(|session| session.is_active(now));
    let lines = lines(session, config);
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

    pub fn update(&mut self, session: Option<&Session>, config: &Config) {
        let Some(payload) = desired_payload(session, config, chrono::Utc::now().timestamp_millis())
        else {
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

    fn active(now: i64) -> Session {
        Session {
            project: "pulse".into(),
            model_name: "Claude Opus 5.5".into(),
            effort: Some("Medium".into()),
            activity: "Editing files".into(),
            activity_target: Some("main.rs".into()),
            created: now - 60_000,
            updated: now,
            known_cost: Some(1.237),
            context_used: Some(250_000),
            context_window: Some(1_000_000),
            subagent_count: 2,
            usage: super::super::Usage {
                input: 1_000,
                output: 500,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    #[ignore = "Publishes a real Orion App presence to local Discord; stop other Pulse publishers first"]
    fn local_discord_acknowledges_orion_presence() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<serde_json::Value> {
                let now = chrono::Utc::now().timestamp_millis();
                let mut publisher = Publisher::default();
                publisher.update(Some(&active(now)), &Config::default());
                anyhow::ensure!(
                    publisher.status() == "Connected",
                    "Presence did not connect"
                );
                let (_, accepted) = publisher.client.as_mut().unwrap().recv()?;
                anyhow::ensure!(
                    accepted["cmd"] == "SET_ACTIVITY" && accepted["evt"] != "ERROR",
                    "Discord rejected Orion presence: {accepted}"
                );
                let data = &accepted["data"];
                anyhow::ensure!(data["name"] == NAME, "Incorrect application name");
                anyhow::ensure!(
                    data["application_id"] == super::super::CLIENT_ID,
                    "Incorrect application identity"
                );
                if std::env::var_os("PULSE_ORION_KEEP_PRESENCE").is_some() {
                    std::thread::sleep(Duration::from_secs(20));
                }
                publisher.shutdown();
                Ok(serde_json::json!({
                    "application_id": data["application_id"],
                    "name": data["name"],
                    "details": data["details"],
                    "state": data["state"],
                    "assets": data["assets"],
                }))
            })();
            let _ = tx.send(result);
        });
        let result = rx
            .recv_timeout(Duration::from_secs(40))
            .expect("Discord IPC timed out")
            .expect("Orion publisher proof failed");
        println!("{result}");
    }

    #[test]
    fn active_session_publishes_orion_fields_and_timer() {
        let now = 1_790_500_000_000;
        let config = Config::default();
        let payload = desired_payload(Some(&active(now)), &config, now).unwrap();
        let text = format!("{} {}", payload.details, payload.state);
        for expected in [
            "Claude Opus 5.5 · Medium",
            "Editing files main.rs",
            "pulse",
            "$1.24",
            "Ctx 25% used",
        ] {
            assert!(text.contains(expected), "{expected} missing from {text}");
        }
        assert_eq!(payload.started_at, Some((now - 60_000) / 1000));
        assert_eq!(ASSET_KEY, "orion");
        assert_eq!(NAME, "Orion App");
        assert_eq!(config.client_id, "1553775289274994708");
    }

    #[test]
    fn idle_and_completed_sessions_publish_idle_without_timer() {
        let now = 1_790_500_000_000;
        let config = Config::default();
        let idle = desired_payload(None, &config, now).unwrap();
        assert_eq!(idle.details, "Idle");
        assert_eq!(idle.state, "Waiting for Orion App");
        assert_eq!(idle.started_at, None);
        let done = Session {
            activity: "Waiting for input".into(),
            ..active(now)
        };
        assert_eq!(
            desired_payload(Some(&done), &config, now),
            Some(idle.clone())
        );
        let stale = Session {
            updated: now - 180_000,
            ..active(now)
        };
        assert_eq!(desired_payload(Some(&stale), &config, now), Some(idle));
    }

    #[test]
    fn privacy_hides_project_branch_and_file_target() {
        let now = 1_790_500_000_000;
        let config = Config {
            privacy_enabled: true,
            ..Default::default()
        };
        let session = Session {
            branch: Some("secret-branch".into()),
            ..active(now)
        };
        let result = lines(Some(&session), &config);
        let text = format!("{} {}", result.details, result.state);
        assert!(!text.contains("pulse"));
        assert!(!text.contains("secret-branch"));
        assert!(!text.contains("main.rs"));
    }

    #[test]
    fn disabled_presence_publishes_nothing() {
        let now = 1_790_500_000_000;
        let config = Config {
            enabled: false,
            ..Default::default()
        };
        assert_eq!(desired_payload(Some(&active(now)), &config, now), None);
        assert_eq!(desired_payload(None, &config, now), None);
    }
}
