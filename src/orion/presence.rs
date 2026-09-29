use super::{ASSET_KEY, Config, NAME, Session};
use codex_presence_core::{PresenceFieldId, PresenceLines, PresenceValues, compose_presence};
use discord_rich_presence::{
    DiscordIpc, DiscordIpcClient,
    activity::{Activity, Assets, Timestamps},
};
use std::time::{Duration, Instant};

pub const IDLE_STATE: &str = "Idling...";

/// "Idling for 12m" style label for the Pulse preview; Discord shows the same
/// span as its own running timer.
pub fn idle_label(idle_since: Option<i64>, now: i64) -> String {
    let Some(since) = idle_since.filter(|since| *since > 0 && *since <= now) else {
        return IDLE_STATE.into();
    };
    let minutes = (now - since) / 60_000;
    let span = match minutes {
        0 => return IDLE_STATE.into(),
        1..=59 => format!("{minutes}m"),
        _ if minutes < 24 * 60 => format!("{}h {}m", minutes / 60, minutes % 60),
        _ => format!("{}d {}h", minutes / (24 * 60), minutes / 60 % 24),
    };
    format!("Idling for {span}")
}

pub fn lines(session: Option<&Session>, config: &Config) -> PresenceLines {
    let Some(session) = session else {
        return PresenceLines {
            details: NAME.into(),
            state: IDLE_STATE.into(),
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
    // Same rule as Claude presence: the best known amount as currency only,
    // partial subtotals included, and nothing until there is spend. Coverage
    // stays in the Pulse UI, not in the Discord text.
    if let Some(cost) = session.known_cost.filter(|cost| *cost > 0.0) {
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
    if session.running_subagents > 0 {
        values.insert(
            PresenceFieldId::Systems,
            format!(
                "{} subagent{}",
                session.running_subagents,
                if session.running_subagents == 1 {
                    ""
                } else {
                    "s"
                }
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

/// Active sessions time from their start. Idle times from the last Orion
/// activity, so Discord's timer reads as "idling for" that long.
fn desired_payload(
    session: Option<&Session>,
    idle_since: Option<i64>,
    config: &Config,
    now: i64,
) -> Option<Payload> {
    if !config.enabled {
        return None;
    }
    let session = session.filter(|session| session.is_active(now));
    let lines = lines(session, config);
    let started_at = match session {
        Some(session) => (session.created > 0).then_some(session.created / 1000),
        None => idle_since
            .filter(|since| *since > 0 && *since <= now)
            .map(|since| since / 1000),
    };
    Some(Payload {
        details: lines.details,
        state: lines.state,
        started_at,
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

    pub fn update(&mut self, session: Option<&Session>, idle_since: Option<i64>, config: &Config) {
        let now = chrono::Utc::now().timestamp_millis();
        let Some(payload) = desired_payload(session, idle_since, config, now) else {
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
            cost_complete: true,
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
                publisher.update(Some(&active(now)), None, &Config::default());
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
        let payload = desired_payload(Some(&active(now)), None, &config, now).unwrap();
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

    fn cost_text(session: &Session) -> String {
        let result = lines(Some(session), &Config::default());
        format!("{} {}", result.details, result.state)
    }

    #[test]
    fn cost_field_is_currency_only_and_hidden_at_zero_or_unavailable() {
        let now = 1_790_500_000_000;
        // A partial subtotal shows as plain currency, like every provider.
        let partial = Session {
            cost_complete: false,
            ..active(now)
        };
        let text = cost_text(&partial);
        assert!(text.contains("$1.24"), "{text}");
        assert!(!text.contains("$1.24+"), "{text}");
        for known_cost in [Some(0.0), None] {
            let text = cost_text(&Session {
                known_cost,
                ..active(now)
            });
            assert!(!text.contains('$'), "{text}");
        }
    }

    #[test]
    fn idle_and_completed_sessions_publish_idling_with_an_idle_timer() {
        let now = 1_790_500_000_000;
        let config = Config::default();
        let without_history = desired_payload(None, None, &config, now).unwrap();
        assert_eq!(without_history.details, "Orion App");
        assert_eq!(without_history.state, "Idling...");
        assert_eq!(without_history.started_at, None);

        let since = now - 754_000;
        let idle = desired_payload(None, Some(since), &config, now).unwrap();
        assert_eq!(idle.state, "Idling...");
        assert_eq!(
            idle.started_at,
            Some(since / 1000),
            "timer counts idle time"
        );
        let done = Session {
            activity: "Waiting for input".into(),
            ..active(now)
        };
        assert_eq!(
            desired_payload(Some(&done), Some(since), &config, now),
            Some(idle.clone())
        );
        let stale = Session {
            updated: now - 180_000,
            ..active(now)
        };
        assert_eq!(
            desired_payload(Some(&stale), Some(since), &config, now),
            Some(idle)
        );
        let future = desired_payload(None, Some(now + 5_000), &config, now).unwrap();
        assert_eq!(future.started_at, None, "a clock skew never starts a timer");
    }

    #[test]
    fn idle_label_reads_idling_then_idling_for_a_duration() {
        let now = 1_790_500_000_000;
        assert_eq!(idle_label(None, now), "Idling...");
        assert_eq!(idle_label(Some(now - 20_000), now), "Idling...");
        assert_eq!(idle_label(Some(now - 12 * 60_000), now), "Idling for 12m");
        assert_eq!(
            idle_label(Some(now - (2 * 60 + 5) * 60_000), now),
            "Idling for 2h 5m"
        );
        assert_eq!(
            idle_label(Some(now - (26 * 60) * 60_000), now),
            "Idling for 1d 2h"
        );
    }

    #[test]
    fn only_running_subagents_reach_discord() {
        let now = 1_790_500_000_000;
        let text = |running| {
            let result = lines(
                Some(&Session {
                    subagent_count: 5,
                    running_subagents: running,
                    ..active(now)
                }),
                &Config::default(),
            );
            format!("{} {}", result.details, result.state)
        };
        assert!(!text(0).contains("subagent"), "{}", text(0));
        assert!(text(1).contains("1 subagent"), "{}", text(1));
        assert!(!text(1).contains("5 subagents"));
        assert!(text(3).contains("3 subagents"), "{}", text(3));
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
        assert_eq!(
            desired_payload(Some(&active(now)), None, &config, now),
            None
        );
        assert_eq!(
            desired_payload(None, Some(now - 60_000), &config, now),
            None
        );
    }
}
