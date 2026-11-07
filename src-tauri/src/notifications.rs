//! Durable, deduplicated notifications shared by the Pulse backend and tray.
//!
//! The store deliberately owns only notification state. Provider polling stays
//! in `commands.rs`; callers pass observations here and get back one durable
//! record only when a state transition (or a bounded reminder) is actionable.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// Re-emit an unchanged condition at most once per day.
pub const REPEAT_COOLDOWN: Duration = Duration::from_secs(24 * 60 * 60);

const TABLE: &str = "pulse_notifications";
const DEDUPE_TABLE: &str = "pulse_notification_state";
const MIGRATION_TABLE: &str = "pulse_notification_migrations";
const LEGACY_RESET_MIGRATION: &str = "dismiss_legacy_quota_reset_rows_v2";
/// Before access-window keys included the duration, two windows from one
/// model-scoped envelope shared a ledger and could alternate into a false reset
/// on every poll. Those rows do not carry enough source identity to repair
/// retrospectively, so preserve them for audit while removing them from the
/// user-visible feed once the corrected producer starts.
const COLLIDED_RESET_MIGRATION: &str = "dismiss_collided_quota_reset_rows_v3";
/// One-time cleanup of provider-health / quota-threshold / Discord-connectivity
/// rows written by a build that alerted on every poll-cadence transition. Those
/// kinds are no longer emitted natively, so every existing row is spam; dismiss
/// them (keep for audit) so the bell and native toasts start clean.
const SPURIOUS_ALERT_MIGRATION: &str = "dismiss_spurious_poll_cadence_alerts_v1";
const MAX_DISPLAY_TEXT_CHARS: usize = 240;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    ProviderHealth,
    QuotaThreshold,
    QuotaReset,
    DiscordConnectivity,
}

impl NotificationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderHealth => "provider_health",
            Self::QuotaThreshold => "quota_threshold",
            Self::QuotaReset => "quota_reset",
            Self::DiscordConnectivity => "discord_connectivity",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "provider_health" => Ok(Self::ProviderHealth),
            "quota_threshold" => Ok(Self::QuotaThreshold),
            "quota_reset" => Ok(Self::QuotaReset),
            "discord_connectivity" => Ok(Self::DiscordConnectivity),
            other => bail!("unknown notification kind {other:?}"),
        }
    }
}

/// Input to the notification state machine.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NotificationSpec {
    pub kind: NotificationKind,
    pub provider: String,
    /// Stable source/window identity. Dynamic values belong in `body`, not
    /// here, so one provider/window has one transition ledger.
    pub key: String,
    pub title: String,
    pub body: String,
    pub action: Option<String>,
}

/// A persisted notification returned to the frontend or tray runtime.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NotificationRecord {
    pub id: i64,
    pub kind: NotificationKind,
    pub provider: String,
    pub key: String,
    pub title: String,
    pub body: String,
    pub action: Option<String>,
    pub created_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
    pub dismissed_at: Option<DateTime<Utc>>,
}

/// Public alias used by command adapters that prefer the shorter DTO name.
pub type Notification = NotificationRecord;

/// SQLite-backed notification center. A store borrows a connection so tests
/// can use an in-memory database and the runtime can choose its own lifecycle.
pub struct NotificationStore<'conn> {
    connection: &'conn Connection,
}

/// Descriptive alias for callers that treat the store as a notification center.
pub type NotificationCenter<'conn> = NotificationStore<'conn>;

/// Policy toggles for one deduplicated observation. Grouping them keeps the
/// state-machine boundary explicit without another argument-heavy call site.
#[derive(Clone, Copy)]
struct ObserveOptions {
    notify_initial: bool,
    repeat_unchanged: bool,
    suppress_unnotified_recovery: bool,
    notify_on_change: fn(&str, &str) -> bool,
}

/// One provider-window observation at the reset state-machine boundary.
pub struct QuotaResetObservation<'a> {
    pub provider: &'a str,
    pub window_identity: &'a str,
    pub window_label: &'a str,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub reset_at: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
}

impl<'conn> NotificationStore<'conn> {
    pub fn new(connection: &'conn Connection) -> Self {
        Self { connection }
    }

    /// Creates the notification tables without changing the analytics schema.
    pub fn initialize(connection: &Connection) -> Result<()> {
        connection.execute_batch(&format!(
            "PRAGMA busy_timeout = 5000;
             CREATE TABLE IF NOT EXISTS {TABLE} (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 kind TEXT NOT NULL,
                 provider TEXT NOT NULL,
                 notification_key TEXT NOT NULL,
                 title TEXT NOT NULL,
                 body TEXT NOT NULL,
                 action TEXT,
                 created_at TEXT NOT NULL,
                 read_at TEXT,
                 dismissed_at TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_{TABLE}_created
                 ON {TABLE}(created_at DESC, id DESC);
             CREATE INDEX IF NOT EXISTS idx_{TABLE}_unread
                 ON {TABLE}(dismissed_at, read_at, created_at DESC);
              CREATE TABLE IF NOT EXISTS {DEDUPE_TABLE} (
                  dedupe_key TEXT PRIMARY KEY,
                  state_fingerprint TEXT NOT NULL,
                  last_seen_at TEXT NOT NULL,
                  last_notified_at TEXT
              );
              CREATE TABLE IF NOT EXISTS {MIGRATION_TABLE} (
                  migration TEXT PRIMARY KEY,
                  applied_at TEXT NOT NULL
              );"
        ))?;
        // v2 also covers rows written by an older concurrent producer after the
        // v1 marker. Preserve them for audit/history, then leave future genuine
        // transition rows untouched once this migration commits.
        let transaction = connection.unchecked_transaction()?;
        let already_applied: Option<String> = transaction
            .query_row(
                &format!("SELECT migration FROM {MIGRATION_TABLE} WHERE migration = ?1"),
                [LEGACY_RESET_MIGRATION],
                |row| row.get(0),
            )
            .optional()?;
        if already_applied.is_none() {
            transaction.execute(
                &format!(
                    "UPDATE {TABLE}
                     SET dismissed_at = COALESCE(dismissed_at, ?1)
                     WHERE kind = 'quota_reset'"
                ),
                [Utc::now().to_rfc3339()],
            )?;
            transaction.execute(
                &format!(
                    "INSERT INTO {MIGRATION_TABLE} (migration, applied_at)
                     VALUES (?1, ?2)"
                ),
                params![LEGACY_RESET_MIGRATION, Utc::now().to_rfc3339()],
            )?;
        }

        let spurious_applied: Option<String> = transaction
            .query_row(
                &format!("SELECT migration FROM {MIGRATION_TABLE} WHERE migration = ?1"),
                [SPURIOUS_ALERT_MIGRATION],
                |row| row.get(0),
            )
            .optional()?;
        if spurious_applied.is_none() {
            transaction.execute(
                &format!(
                    "UPDATE {TABLE}
                     SET dismissed_at = COALESCE(dismissed_at, ?1)
                     WHERE kind IN ('provider_health', 'quota_threshold', 'discord_connectivity')"
                ),
                [Utc::now().to_rfc3339()],
            )?;
            transaction.execute(
                &format!(
                    "DELETE FROM {DEDUPE_TABLE}
                     WHERE dedupe_key LIKE 'provider_health:%'
                        OR dedupe_key LIKE 'quota_threshold:%'
                        OR dedupe_key LIKE 'discord_connectivity:%'"
                ),
                [],
            )?;
            transaction.execute(
                &format!(
                    "INSERT INTO {MIGRATION_TABLE} (migration, applied_at)
                     VALUES (?1, ?2)"
                ),
                params![SPURIOUS_ALERT_MIGRATION, Utc::now().to_rfc3339()],
            )?;
        }

        let collided_reset_applied: Option<String> = transaction
            .query_row(
                &format!("SELECT migration FROM {MIGRATION_TABLE} WHERE migration = ?1"),
                [COLLIDED_RESET_MIGRATION],
                |row| row.get(0),
            )
            .optional()?;
        if collided_reset_applied.is_none() {
            let applied_at = Utc::now().to_rfc3339();
            transaction.execute(
                &format!(
                    "UPDATE {TABLE}
                     SET dismissed_at = COALESCE(dismissed_at, ?1)
                     WHERE kind = 'quota_reset'"
                ),
                [&applied_at],
            )?;
            transaction.execute(
                &format!(
                    "INSERT INTO {MIGRATION_TABLE} (migration, applied_at)
                     VALUES (?1, ?2)"
                ),
                params![COLLIDED_RESET_MIGRATION, &applied_at],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Observe a condition and persist a record when it is new, changed, or
    /// older than [`REPEAT_COOLDOWN`].
    pub fn observe(
        &self,
        spec: NotificationSpec,
        at: DateTime<Utc>,
    ) -> Result<Option<NotificationRecord>> {
        self.observe_with_fingerprint(spec, fingerprint_from_spec, at, true)
    }

    /// Provider source health emits only on a state edge. The first sample,
    /// healthy or unavailable, establishes a silent baseline.
    pub fn observe_provider_health(
        &self,
        provider: &str,
        source: &str,
        healthy: bool,
        detail: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<Option<NotificationRecord>> {
        let state = if healthy { "healthy" } else { "unhealthy" };
        let body = if healthy {
            format!("{provider} source {source} recovered")
        } else {
            format!(
                "{provider} source {source} is unavailable{}",
                detail.map(|value| format!(": {value}")).unwrap_or_default()
            )
        };
        let spec = NotificationSpec {
            kind: NotificationKind::ProviderHealth,
            provider: provider.to_string(),
            key: source.to_string(),
            title: format!("{provider} provider source"),
            body,
            action: Some("Open provider settings".to_string()),
        };
        // An unavailable first sample is a diagnostic baseline (for example,
        // an optional Claude lane with no `.credentials.json`), not a native
        // alert. Provider health re-arms only after a real state edge; unlike
        // quota reminders, an unchanged outage never repeats after cooldown.
        self.observe_health_edge(spec, state, at)
    }

    /// Emits one notification when a quota window enters a new configured
    /// threshold bucket. Thresholds are supplied by the caller so provider and
    /// window policy stays centralized in the quota owner.
    pub fn observe_quota_threshold(
        &self,
        provider: &str,
        window: &str,
        used_percent: f64,
        thresholds: &[f64],
        reset_at: Option<DateTime<Utc>>,
        at: DateTime<Utc>,
    ) -> Result<Option<NotificationRecord>> {
        if !used_percent.is_finite() {
            return Ok(None);
        }
        let threshold = thresholds
            .iter()
            .copied()
            .filter(|value| value.is_finite() && *value >= 0.0 && *value <= 100.0)
            .filter(|value| used_percent >= *value)
            .max_by(f64::total_cmp);
        let Some(threshold) = threshold else {
            self.remember_state(
                &format!("quota_threshold:{provider}:{window}"),
                "below_threshold",
                at,
            )?;
            return Ok(None);
        };

        let state = format!("threshold:{threshold:.3}");
        let reset_suffix = reset_at
            .map(|value| format!("; resets {}", value.to_rfc3339()))
            .unwrap_or_default();
        let spec = NotificationSpec {
            kind: NotificationKind::QuotaThreshold,
            provider: provider.to_string(),
            key: window.to_string(),
            title: format!("{provider} quota threshold"),
            body: format!("{provider} {window} quota reached {threshold:.0}%{reset_suffix}"),
            action: Some("Open quota details".to_string()),
        };
        self.observe_transition(spec, &state, at, true)
    }

    /// Observe a provider quota and notify only on its genuine reset edge.
    ///
    /// Codex resets are `remaining < 100 -> 100`; Claude resets are
    /// `used > 0 -> 0`. The metric category, not `reset_at`, is persisted so
    /// timestamp drift, restarts, and repeated cache samples stay silent.
    pub fn observe_quota_reset_transition(
        &self,
        provider: &str,
        window: &str,
        used_percent: f64,
        remaining_percent: f64,
        reset_at: Option<DateTime<Utc>>,
        at: DateTime<Utc>,
    ) -> Result<Option<NotificationRecord>> {
        let display_window = humanize_window(window);
        self.observe_quota_reset_transition_for_window(QuotaResetObservation {
            provider,
            window_identity: window,
            window_label: &display_window,
            used_percent,
            remaining_percent,
            reset_at,
            observed_at: at,
        })
    }

    /// Variant used by access-route consumers that own a stable machine key
    /// and a separate human label. Dedupe always follows the machine identity;
    /// notification copy follows the provider-facing label.
    pub fn observe_quota_reset_transition_for_window(
        &self,
        observation: QuotaResetObservation<'_>,
    ) -> Result<Option<NotificationRecord>> {
        let QuotaResetObservation {
            provider,
            window_identity,
            window_label,
            used_percent,
            remaining_percent,
            reset_at,
            observed_at,
        } = observation;
        let provider = provider.trim().to_ascii_lowercase();
        let (state, notify_on_change) = match provider.as_str() {
            "codex"
                if remaining_percent.is_finite() && (0.0..=100.0).contains(&remaining_percent) =>
            {
                let state = if remaining_percent >= 100.0 {
                    "at_100"
                } else {
                    "below_100"
                };
                (state, codex_reset_transition as fn(&str, &str) -> bool)
            }
            "claude" if used_percent.is_finite() && (0.0..=100.0).contains(&used_percent) => {
                let state = if used_percent <= 0.0 { "zero" } else { "used" };
                (state, claude_reset_transition as fn(&str, &str) -> bool)
            }
            _ => return Ok(None),
        };
        let display_provider = humanize_provider(&provider);
        let display_window = sanitize_display_text(window_label);
        let display_window = if display_window.is_empty() {
            humanize_window(window_identity)
        } else {
            display_window
        };
        let spec = NotificationSpec {
            kind: NotificationKind::QuotaReset,
            provider: provider.to_string(),
            key: window_identity.to_string(),
            title: format!("{display_provider} limit reset"),
            body: reset_at
                .map(|reset| {
                    format!(
                        "Your {display_provider} {display_window} limit just reset. Next reset {}.",
                        humanize_reset_at(reset)
                    )
                })
                .unwrap_or_else(|| {
                    format!("Your {display_provider} {display_window} limit just reset.")
                }),
            action: Some("View quota details".to_string()),
        };
        let key = format!("quota_reset:{provider}:{window_identity}");
        self.observe_with_key(
            spec,
            &key,
            state,
            observed_at,
            ObserveOptions {
                notify_initial: false,
                repeat_unchanged: false,
                // A reset is itself the affirmative edge we care about. It
                // must notify after a silent below-100/used baseline even if
                // no earlier threshold or outage notification was emitted.
                suppress_unnotified_recovery: false,
                notify_on_change,
            },
        )
    }

    /// Compatibility shim for callers compiled against the old timestamp-only
    /// API. Timestamp presence alone is not reset proof and therefore remains
    /// silent; the poller uses [`Self::observe_quota_reset_transition`].
    #[deprecated(note = "use observe_quota_reset_transition")]
    pub fn observe_quota_reset(
        &self,
        _provider: &str,
        _window: &str,
        _reset_at: Option<DateTime<Utc>>,
        _at: DateTime<Utc>,
    ) -> Result<Option<NotificationRecord>> {
        Ok(None)
    }

    /// Discord connectivity uses the same edge-triggered ledger as provider
    /// health, with a distinct kind so the UI can route the action correctly.
    pub fn observe_discord_connectivity(
        &self,
        provider: &str,
        connected: bool,
        detail: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<Option<NotificationRecord>> {
        let state = if connected {
            "connected"
        } else {
            "disconnected"
        };
        let body = if connected {
            format!("Discord Rich Presence for {provider} recovered")
        } else {
            format!(
                "Discord Rich Presence for {provider} is unavailable{}",
                detail.map(|value| format!(": {value}")).unwrap_or_default()
            )
        };
        let spec = NotificationSpec {
            kind: NotificationKind::DiscordConnectivity,
            provider: provider.to_string(),
            key: "ipc".to_string(),
            title: "Discord connectivity".to_string(),
            body,
            action: Some("Open Discord settings".to_string()),
        };
        self.observe_transition(spec, state, at, !connected)
    }

    pub fn list(&self, limit: Option<usize>) -> Result<Vec<NotificationRecord>> {
        self.list_filtered(limit, false)
    }

    pub fn list_all(&self, limit: Option<usize>) -> Result<Vec<NotificationRecord>> {
        self.list_filtered(limit, true)
    }

    pub fn unread_count(&self) -> Result<u32> {
        let count: i64 = self.connection.query_row(
            &format!(
                "SELECT COUNT(*) FROM {TABLE}
                 WHERE dismissed_at IS NULL AND read_at IS NULL"
            ),
            [],
            |row| row.get(0),
        )?;
        Ok(count.clamp(0, u32::MAX as i64) as u32)
    }

    pub fn mark_read(&self, id: i64) -> Result<bool> {
        Ok(self.connection.execute(
            &format!(
                "UPDATE {TABLE} SET read_at = COALESCE(read_at, ?1)
                 WHERE id = ?2 AND dismissed_at IS NULL"
            ),
            params![Utc::now().to_rfc3339(), id],
        )? > 0)
    }

    pub fn mark_all_read(&self) -> Result<usize> {
        Ok(self.connection.execute(
            &format!(
                "UPDATE {TABLE} SET read_at = COALESCE(read_at, ?1)
                 WHERE dismissed_at IS NULL AND read_at IS NULL"
            ),
            params![Utc::now().to_rfc3339()],
        )?)
    }

    pub fn mark_unread(&self, id: i64) -> Result<bool> {
        Ok(self.connection.execute(
            &format!("UPDATE {TABLE} SET read_at = NULL WHERE id = ?1 AND dismissed_at IS NULL"),
            params![id],
        )? > 0)
    }

    pub fn mark_all_unread(&self) -> Result<usize> {
        Ok(self.connection.execute(
            &format!("UPDATE {TABLE} SET read_at = NULL WHERE dismissed_at IS NULL AND read_at IS NOT NULL"),
            [],
        )?)
    }

    pub fn dismiss_all(&self) -> Result<(usize, String)> {
        let token = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
        let count = self.connection.execute(
            &format!("UPDATE {TABLE} SET dismissed_at = ?1 WHERE dismissed_at IS NULL"),
            params![token],
        )?;
        Ok((count, token))
    }

    pub fn restore_batch(&self, token: &str) -> Result<usize> {
        DateTime::parse_from_rfc3339(token).context("invalid notification undo token")?;
        Ok(self.connection.execute(
            &format!("UPDATE {TABLE} SET dismissed_at = NULL WHERE dismissed_at = ?1"),
            params![token],
        )?)
    }

    pub fn dismiss(&self, id: i64) -> Result<bool> {
        Ok(self.connection.execute(
            &format!("UPDATE {TABLE} SET dismissed_at = COALESCE(dismissed_at, ?1) WHERE id = ?2"),
            params![Utc::now().to_rfc3339(), id],
        )? > 0)
    }

    pub fn undismiss(&self, id: i64) -> Result<bool> {
        Ok(self.connection.execute(
            &format!("UPDATE {TABLE} SET dismissed_at = NULL WHERE id = ?1"),
            params![id],
        )? > 0)
    }

    fn observe_transition(
        &self,
        spec: NotificationSpec,
        state: &str,
        at: DateTime<Utc>,
        notify_initial: bool,
    ) -> Result<Option<NotificationRecord>> {
        let key = format!(
            "{}:{}:{}",
            spec.kind.as_str(),
            spec.provider.trim(),
            spec.key.trim()
        );
        self.observe_transition_with_key(spec, &key, state, at, notify_initial, true)
    }

    fn observe_health_edge(
        &self,
        spec: NotificationSpec,
        state: &str,
        at: DateTime<Utc>,
    ) -> Result<Option<NotificationRecord>> {
        let key = format!(
            "{}:{}:{}",
            spec.kind.as_str(),
            spec.provider.trim(),
            spec.key.trim()
        );
        self.observe_with_key(
            spec,
            &key,
            state,
            at,
            ObserveOptions {
                notify_initial: false,
                repeat_unchanged: false,
                suppress_unnotified_recovery: true,
                notify_on_change: always_notify_on_change,
            },
        )
    }

    fn observe_transition_with_key(
        &self,
        spec: NotificationSpec,
        dedupe_key: &str,
        state: &str,
        at: DateTime<Utc>,
        notify_initial: bool,
        repeat_unchanged: bool,
    ) -> Result<Option<NotificationRecord>> {
        self.observe_with_key(
            spec,
            dedupe_key,
            state,
            at,
            ObserveOptions {
                notify_initial,
                repeat_unchanged,
                suppress_unnotified_recovery: false,
                notify_on_change: always_notify_on_change,
            },
        )
    }

    fn observe_with_fingerprint(
        &self,
        spec: NotificationSpec,
        fingerprint: fn(&NotificationSpec) -> String,
        at: DateTime<Utc>,
        notify_initial: bool,
    ) -> Result<Option<NotificationRecord>> {
        let key = format!(
            "{}:{}:{}",
            spec.kind.as_str(),
            spec.provider.trim(),
            spec.key.trim()
        );
        let state = fingerprint(&spec);
        self.observe_with_key(
            spec,
            &key,
            &state,
            at,
            ObserveOptions {
                notify_initial,
                repeat_unchanged: true,
                suppress_unnotified_recovery: false,
                notify_on_change: always_notify_on_change,
            },
        )
    }

    fn observe_with_key(
        &self,
        spec: NotificationSpec,
        dedupe_key: &str,
        state: &str,
        at: DateTime<Utc>,
        options: ObserveOptions,
    ) -> Result<Option<NotificationRecord>> {
        let provider = sanitize_display_text(&spec.provider);
        let key = sanitize_display_text(&spec.key);
        let title = sanitize_display_text(&spec.title);
        let body = sanitize_display_text(&spec.body);
        let action = spec.action.as_deref().map(sanitize_display_text);
        let safe_dedupe_key = sanitize_display_text(dedupe_key);
        let timestamp = at.to_rfc3339();
        let transaction = self.connection.unchecked_transaction()?;
        let previous = transaction
            .query_row(
                &format!(
                    "SELECT state_fingerprint, last_notified_at, last_seen_at
                     FROM {DEDUPE_TABLE} WHERE dedupe_key = ?1"
                ),
                params![&safe_dedupe_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((_, _, last_seen)) = previous.as_ref()
            && at < parse_timestamp(last_seen)?
        {
            return Ok(None);
        }

        let mut should_notify = options.notify_initial && previous.is_none();
        if let Some((previous_state, last_notified, _)) = previous.as_ref() {
            if previous_state != state {
                // A recovery from a silent baseline (e.g. missing optional
                // credentials on first launch) is not actionable; only a
                // previously delivered outage may produce a recovery toast.
                should_notify = (options.notify_on_change)(previous_state, state)
                    && (!options.suppress_unnotified_recovery
                        || state == "unhealthy"
                        || last_notified.is_some());
            } else if options.repeat_unchanged
                && let Some(last_notified) = last_notified.as_deref()
            {
                let last_notified = parse_timestamp(last_notified)?;
                should_notify = at >= last_notified
                    && at.signed_duration_since(last_notified)
                        >= chrono::Duration::from_std(REPEAT_COOLDOWN)?;
            }
        }

        let record = if should_notify {
            transaction.execute(
                &format!(
                    "INSERT INTO {TABLE}
                     (kind, provider, notification_key, title, body, action, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
                ),
                params![
                    spec.kind.as_str(),
                    &provider,
                    &key,
                    &title,
                    &body,
                    action.as_deref(),
                    &timestamp,
                ],
            )?;
            let id = transaction.last_insert_rowid();
            Some(NotificationRecord {
                id,
                kind: spec.kind,
                provider,
                key,
                title,
                body,
                action,
                created_at: at,
                read_at: None,
                dismissed_at: None,
            })
        } else {
            None
        };

        transaction.execute(
            &format!(
                "INSERT INTO {DEDUPE_TABLE}
                    (dedupe_key, state_fingerprint, last_seen_at, last_notified_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(dedupe_key) DO UPDATE SET
                    state_fingerprint = excluded.state_fingerprint,
                    last_seen_at = excluded.last_seen_at,
                    last_notified_at = COALESCE(excluded.last_notified_at,
                                                {DEDUPE_TABLE}.last_notified_at)"
            ),
            params![
                &safe_dedupe_key,
                state,
                &timestamp,
                should_notify.then_some(timestamp.as_str()),
            ],
        )?;
        transaction.commit()?;
        Ok(record)
    }

    fn remember_state(&self, dedupe_key: &str, state: &str, at: DateTime<Utc>) -> Result<()> {
        self.connection.execute(
            &format!(
                "INSERT INTO {DEDUPE_TABLE}
                    (dedupe_key, state_fingerprint, last_seen_at, last_notified_at)
                 VALUES (?1, ?2, ?3, NULL)
                 ON CONFLICT(dedupe_key) DO UPDATE SET
                    state_fingerprint = excluded.state_fingerprint,
                    last_seen_at = excluded.last_seen_at
                  WHERE excluded.last_seen_at > {DEDUPE_TABLE}.last_seen_at"
            ),
            params![dedupe_key, state, at.to_rfc3339()],
        )?;
        Ok(())
    }

    fn list_filtered(
        &self,
        limit: Option<usize>,
        include_dismissed: bool,
    ) -> Result<Vec<NotificationRecord>> {
        let limit = limit.unwrap_or(100).clamp(1, 500) as i64;
        let sql = if include_dismissed {
            format!(
                "SELECT id, kind, provider, notification_key, title, body, action,
                        created_at, read_at, dismissed_at
                 FROM {TABLE} ORDER BY created_at DESC, id DESC LIMIT ?1"
            )
        } else {
            format!(
                "SELECT id, kind, provider, notification_key, title, body, action,
                        created_at, read_at, dismissed_at
                 FROM {TABLE} WHERE dismissed_at IS NULL
                 ORDER BY created_at DESC, id DESC LIMIT ?1"
            )
        };
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params![limit], notification_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

fn always_notify_on_change(_: &str, _: &str) -> bool {
    true
}

fn codex_reset_transition(previous: &str, current: &str) -> bool {
    previous == "below_100" && current == "at_100"
}

fn claude_reset_transition(previous: &str, current: &str) -> bool {
    previous == "used" && current == "zero"
}

/// Present a provider id as a clean, capitalized name for notification copy.
fn humanize_provider(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "claude" => "Claude".to_string(),
        "codex" => "Codex".to_string(),
        "openai" => "OpenAI".to_string(),
        "anthropic" => "Anthropic".to_string(),
        other if !other.is_empty() => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => "Provider".to_string(),
            }
        }
        _ => "Provider".to_string(),
    }
}

/// Turn an internal window key into readable notification copy.
fn humanize_window(window: &str) -> String {
    match window.trim().to_ascii_lowercase().as_str() {
        "five_hour" => "5-hour".to_string(),
        "weekly" => "weekly".to_string(),
        "sonnet_free" | "sonnet" => "Sonnet".to_string(),
        "" => "usage".to_string(),
        other => other.replace('_', " "),
    }
}

/// A friendly local reset timestamp, e.g. "Aug 07, 10:39 PM", never the raw
/// RFC3339 string with microseconds and offset.
fn humanize_reset_at(reset: DateTime<Utc>) -> String {
    reset
        .with_timezone(&chrono::Local)
        .format("%b %d, %I:%M %p")
        .to_string()
}

fn fingerprint_from_spec(spec: &NotificationSpec) -> String {
    format!(
        "{}\u{0}{}\u{0}{}",
        spec.title.trim(),
        spec.body.trim(),
        spec.action.as_deref().unwrap_or_default().trim()
    )
}

/// Keep provider/Discord diagnostics safe for SQLite, native toasts, and the
/// tray without allowing control characters or unbounded remote text through.
fn sanitize_display_text(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len().min(MAX_DISPLAY_TEXT_CHARS));
    let mut pending_space = false;
    for character in value.chars() {
        if character.is_control() {
            pending_space = !sanitized.is_empty();
            continue;
        }
        if character.is_whitespace() {
            pending_space = !sanitized.is_empty();
            continue;
        }
        if pending_space && !sanitized.is_empty() {
            sanitized.push(' ');
        }
        pending_space = false;
        sanitized.push(character);
        if sanitized.chars().count() >= MAX_DISPLAY_TEXT_CHARS {
            break;
        }
    }
    sanitized
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("invalid notification timestamp {value:?}"))?
        .with_timezone(&Utc))
}

fn notification_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NotificationRecord> {
    let kind: String = row.get(1)?;
    let created_at: String = row.get(7)?;
    let read_at: Option<String> = row.get(8)?;
    let dismissed_at: Option<String> = row.get(9)?;
    let parse = |value: Option<String>| {
        value
            .as_deref()
            .map(parse_timestamp)
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        error.to_string(),
                    )),
                )
            })
    };
    let created_at = parse_timestamp(&created_at).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error.to_string(),
            )),
        )
    })?;
    Ok(NotificationRecord {
        id: row.get(0)?,
        kind: NotificationKind::parse(&kind).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    error.to_string(),
                )),
            )
        })?,
        provider: row.get(2)?,
        key: row.get(3)?,
        title: row.get(4)?,
        body: row.get(5)?,
        action: row.get(6)?,
        created_at,
        read_at: parse(read_at)?,
        dismissed_at: parse(dismissed_at)?,
    })
}

/// Opens the canonical Pulse analytics database for notification persistence.
/// Notifications use their own tables and therefore survive process restarts
/// without coupling the analytics migration version to this feature.
pub fn open_default_database() -> Result<Connection> {
    let path = default_database_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create notification directory {}",
                parent.display()
            )
        })?;
    }
    let connection = Connection::open(&path)
        .with_context(|| format!("failed to open notification database {}", path.display()))?;
    NotificationStore::initialize(&connection)?;
    Ok(connection)
}

pub fn default_database_path() -> PathBuf {
    cc_discord_presence::storage::database_path()
}

/// Native notification payload kept independent from platform APIs for tests.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NativeNotification {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub action: Option<String>,
}

impl From<&NotificationRecord> for NativeNotification {
    fn from(value: &NotificationRecord) -> Self {
        Self {
            id: value.id,
            title: value.title.clone(),
            body: value.body.clone(),
            action: value.action.clone(),
        }
    }
}

/// Capabilities are explicit because tray APIs are not symmetric across OSes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrayCapabilities {
    pub tooltip: bool,
    pub title: bool,
    pub numeric_badge: bool,
    pub fallback: String,
}

pub fn tray_capabilities() -> TrayCapabilities {
    let tooltip = !cfg!(target_os = "linux");
    let title = !cfg!(target_os = "windows");
    let numeric_badge = cfg!(target_os = "macos");
    TrayCapabilities {
        tooltip,
        title,
        numeric_badge,
        fallback: if numeric_badge {
            "Numeric badge uses the macOS window badge; the tray menu remains authoritative."
                .to_string()
        } else {
            "Numeric tray badges are not portable here; unread totals remain in the tray menu and supported title/tooltip surfaces."
                .to_string()
        },
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrayPresentation {
    pub unread_count: u32,
    pub menu_label: String,
    pub title: String,
    pub tooltip: String,
    pub badge_text: Option<String>,
    pub capabilities: TrayCapabilities,
}

pub fn tray_presentation(unread_count: u32) -> TrayPresentation {
    let capabilities = tray_capabilities();
    let notification_label = if unread_count == 1 {
        "notification"
    } else {
        "notifications"
    };
    let menu_label = if unread_count == 0 {
        "Notifications".to_string()
    } else {
        format!("Notifications ({unread_count})")
    };
    let title = if unread_count == 0 {
        "Pulse".to_string()
    } else {
        format!("Pulse · {unread_count} unread")
    };
    let tooltip = if unread_count == 0 {
        "Pulse — no unread notifications".to_string()
    } else {
        format!("Pulse — {unread_count} unread {notification_label}")
    };
    TrayPresentation {
        unread_count,
        menu_label,
        title,
        tooltip,
        badge_text: (capabilities.numeric_badge && unread_count > 0)
            .then(|| unread_count.to_string()),
        capabilities,
    }
}

/// Sends a notification through the Tauri notification plugin on supported
/// desktop targets. The plugin itself delegates to Windows, macOS, and Linux
/// native notification services; no platform shell command is required.
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub fn send_native_notification<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    notification: &NotificationRecord,
) -> Result<()> {
    use tauri_plugin_notification::NotificationExt;

    app.notification()
        .builder()
        .id(notification.id.clamp(0, i32::MAX as i64) as i32)
        .title(&notification.title)
        .body(&notification.body)
        .group("pulse-notifications")
        .show()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn send_native_notification<R>(_: &R, _: &NotificationRecord) -> Result<()> {
    bail!("native notifications are unavailable on this target")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().expect("sqlite");
        NotificationStore::initialize(&connection).expect("schema");
        connection
    }

    fn time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap()
    }

    #[test]
    fn bulk_read_unread_clear_and_undo_preserve_records_and_read_state() {
        let connection = connection();
        let store = NotificationStore::new(&connection);
        let make = |provider: &str, key: &str| {
            store
                .observe(
                    NotificationSpec {
                        kind: NotificationKind::QuotaReset,
                        provider: provider.into(),
                        key: key.into(),
                        title: "Quota reset".into(),
                        body: "Provider-reported reset".into(),
                        action: None,
                    },
                    time(),
                )
                .unwrap()
                .unwrap()
        };
        let first = make("codex", "first");
        let second = make("claude", "second");
        assert_eq!(store.mark_all_read().unwrap(), 2);
        assert_eq!(store.unread_count().unwrap(), 0);
        assert!(store.mark_unread(first.id).unwrap());
        assert_eq!(store.unread_count().unwrap(), 1);
        assert_eq!(store.mark_all_unread().unwrap(), 1);
        assert!(store.mark_read(second.id).unwrap());
        let (count, token) = store.dismiss_all().unwrap();
        assert_eq!(count, 2);
        assert!(store.list(None).unwrap().is_empty());
        assert_eq!(store.list_all(None).unwrap().len(), 2);
        assert_eq!(store.unread_count().unwrap(), 0);
        assert_eq!(store.restore_batch(&token).unwrap(), 2);
        assert_eq!(store.unread_count().unwrap(), 1);
        assert_eq!(store.restore_batch(&token).unwrap(), 0);
        assert!(store.restore_batch("not-a-token").is_err());
    }

    #[test]
    fn provider_health_uses_transition_edges_and_silent_baseline() {
        let connection = connection();
        let store = NotificationStore::new(&connection);
        let now = time();
        assert!(
            store
                .observe_provider_health("codex", "account", true, None, now)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .observe_provider_health("codex", "account", false, Some("timeout"), now)
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .observe_provider_health(
                    "codex",
                    "account",
                    false,
                    Some("timeout"),
                    now + chrono::Duration::seconds(5)
                )
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .observe_provider_health(
                    "codex",
                    "account",
                    true,
                    None,
                    now + chrono::Duration::seconds(6)
                )
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .observe_provider_health(
                    "codex",
                    "account",
                    false,
                    Some("timeout"),
                    now + chrono::Duration::seconds(7)
                )
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .observe_provider_health(
                    "codex",
                    "account",
                    false,
                    Some("timeout"),
                    now + chrono::Duration::hours(25)
                )
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn quota_thresholds_and_resets_are_edge_triggered() {
        let connection = connection();
        let store = NotificationStore::new(&connection);
        let now = time();
        assert!(
            store
                .observe_quota_threshold("claude", "weekly", 79.0, &[80.0, 90.0], None, now)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .observe_quota_threshold("claude", "weekly", 81.0, &[80.0, 90.0], None, now)
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .observe_quota_threshold(
                    "claude",
                    "weekly",
                    82.0,
                    &[80.0, 90.0],
                    None,
                    now + chrono::Duration::seconds(5)
                )
                .unwrap()
                .is_none()
        );
        let reset = now + chrono::Duration::hours(2);
        assert!(
            store
                .observe_quota_reset_transition("claude", "weekly", 12.0, 88.0, Some(reset), now)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .observe_quota_reset_transition(
                    "claude",
                    "weekly",
                    12.0,
                    88.0,
                    Some(reset),
                    now + chrono::Duration::seconds(5)
                )
                .unwrap()
                .is_none()
        );
        let reset_record = store
            .observe_quota_reset_transition(
                "claude",
                "weekly",
                0.0,
                100.0,
                Some(reset),
                now + chrono::Duration::seconds(6),
            )
            .unwrap()
            .expect("reset edge");
        assert!(reset_record.body.contains("just reset"));
        assert!(reset_record.body.contains("Next reset"));
    }

    #[test]
    fn below_threshold_baselines_never_create_blank_reminders() {
        let connection = connection();
        let store = NotificationStore::new(&connection);
        let now = time();
        assert!(
            store
                .observe_quota_threshold("codex", "primary", 10.0, &[80.0], None, now)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .observe_quota_threshold(
                    "codex",
                    "primary",
                    10.0,
                    &[80.0],
                    None,
                    now + chrono::Duration::hours(48)
                )
                .unwrap()
                .is_none()
        );
        assert!(store.list_all(Some(20)).unwrap().is_empty());
    }

    #[test]
    fn records_survive_store_rehydration_and_read_state_is_durable() {
        let connection = connection();
        let created = time();
        let first = {
            let store = NotificationStore::new(&connection);
            // Establish the healthy baseline before simulating a provider
            // outage; an initial unavailable lane is intentionally silent.
            store
                .observe_provider_health("claude", "oauth", true, None, created)
                .expect("healthy baseline");
            store
                .observe_provider_health(
                    "claude",
                    "oauth",
                    false,
                    Some("expired"),
                    created + chrono::Duration::seconds(1),
                )
                .unwrap()
                .expect("notification")
        };
        let rehydrated = NotificationStore::new(&connection);
        assert_eq!(rehydrated.list(None).unwrap()[0].id, first.id);
        assert_eq!(rehydrated.unread_count().unwrap(), 1);
        assert!(rehydrated.mark_read(first.id).unwrap());
        assert_eq!(
            NotificationStore::new(&connection).unread_count().unwrap(),
            0
        );
    }

    #[test]
    fn tray_presentation_never_claims_an_unsupported_numeric_badge() {
        let presentation = tray_presentation(3);
        if !presentation.capabilities.numeric_badge {
            assert!(presentation.badge_text.is_none());
            assert!(presentation.capabilities.fallback.contains("not portable"));
        }
        assert_eq!(presentation.menu_label, "Notifications (3)");
        if presentation.capabilities.numeric_badge {
            assert!(tray_presentation(0).badge_text.is_none());
        }
    }
}
