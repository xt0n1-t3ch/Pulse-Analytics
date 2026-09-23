use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use discord_rich_presence::activity::{Activity, Assets, Button, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use serde::Deserialize;
use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::config::PresenceConfig;
use crate::session::{ActivityKind, ClaudeSessionSnapshot, RateLimits};
use crate::usage::UsageData;
use crate::util::{format_cost, format_tokens};

/// Structured payload for Discord presence change detection.
/// Enables proper dedup when session changes but text stays the same,
/// and when the start epoch changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PresencePayload {
    session_id: Option<String>,
    start_epoch: i64,
    details: String,
    state: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ResolvedAssets {
    large_image: Option<String>,
    large_text: Option<String>,
    small_image: Option<String>,
    small_text: Option<String>,
}

/// GitHub repo URL shown as a button on Discord profile popout.
const GITHUB_REPO_URL: &str = "https://github.com/xt0n1-t3ch/Pulse-Analytics";
const ACTIVE_PRESENTATION_SECS: u64 = 300;

pub struct DiscordPresence {
    client_id: Option<String>,
    client: Option<DiscordIpcClient>,
    last_status: String,
    last_sent: Option<PresencePayload>,
    last_publish_at: Option<Instant>,
    known_asset_keys: Option<HashSet<String>>,
    last_asset_refresh_at: Option<Instant>,
    last_heartbeat_at: Option<Instant>,
    reconnect_backoff: Duration,
    last_reconnect_attempt: Option<Instant>,
    consecutive_errors: u32,
    idle_start_epoch: Option<i64>,
}

const DISCORD_MIN_PUBLISH_INTERVAL: Duration = Duration::from_secs(2);
const DISCORD_ASSET_REFRESH_INTERVAL: Duration = Duration::from_secs(300);
const DISCORD_ASSET_FETCH_TIMEOUT: Duration = Duration::from_secs(2);

/// Force re-send activity at this interval even if payload unchanged,
/// preventing Discord from dropping the IPC connection due to inactivity.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Minimum backoff between reconnection attempts after IPC errors.
const RECONNECT_MIN_BACKOFF: Duration = Duration::from_secs(5);

/// Maximum backoff cap for exponential reconnection backoff.
const RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(60);

impl DiscordPresence {
    pub fn new(client_id: Option<String>) -> Self {
        let last_status = if client_id.is_some() {
            "Disconnected".to_string()
        } else {
            "Missing CC_DISCORD_CLIENT_ID".to_string()
        };
        Self {
            client_id,
            client: None,
            last_status,
            last_sent: None,
            last_publish_at: None,
            known_asset_keys: None,
            last_asset_refresh_at: None,
            last_heartbeat_at: None,
            reconnect_backoff: RECONNECT_MIN_BACKOFF,
            last_reconnect_attempt: None,
            consecutive_errors: 0,
            idle_start_epoch: None,
        }
    }

    pub fn status(&self) -> &str {
        &self.last_status
    }

    pub fn update(
        &mut self,
        active_session: Option<&ClaudeSessionSnapshot>,
        effective_limits: Option<&RateLimits>,
        api_usage: Option<&UsageData>,
        config: &PresenceConfig,
    ) -> Result<()> {
        // The persisted master switch is enforced here, in the one place that
        // publishes, so every publisher honours it: the Pulse GUI, the headless
        // daemon, and the `claude` wrapper. Checking it only in the GUI let an
        // external publisher keep broadcasting while the GUI reported "paused".
        if !config.presence_enabled {
            self.last_status = "Rich Presence disabled".to_string();
            if self.client.is_some() {
                let _ = self.clear_activity();
            }
            // Drop the dedup and heartbeat state along with the activity.
            // Keeping them would make the first update after re-enabling look
            // like an already-published payload, so `should_skip_publish` would
            // suppress it and Discord would stay blank until the 30s heartbeat
            // expired.
            self.last_sent = None;
            self.last_publish_at = None;
            self.last_heartbeat_at = None;
            self.idle_start_epoch = None;
            return Ok(());
        }

        if self.client_id.is_none() {
            self.last_status = "Missing CC_DISCORD_CLIENT_ID".to_string();
            return Ok(());
        }

        if let Err(_err) = self.ensure_connected() {
            return Ok(());
        }
        if self.client.is_none() {
            return Ok(());
        }

        self.refresh_asset_keys_if_needed();

        let needs_heartbeat = self
            .last_heartbeat_at
            .map(|t| t.elapsed() >= HEARTBEAT_INTERVAL)
            .unwrap_or(true);

        match active_session {
            Some(session) => {
                self.idle_start_epoch = None;
                let start_epoch = presence_start_epoch(session);
                let (details, state, tooltip) =
                    presence_lines(session, effective_limits, api_usage, config);
                let payload = PresencePayload {
                    session_id: Some(session.session_id.clone()),
                    start_epoch,
                    details: details.clone(),
                    state: state.clone(),
                };
                let tooltip_ref = if tooltip.is_empty() {
                    None
                } else {
                    Some(tooltip.as_str())
                };
                self.publish_presence(
                    payload,
                    &details,
                    &state,
                    start_epoch,
                    Some(session),
                    tooltip_ref,
                    config,
                    needs_heartbeat,
                    "Connected",
                )?;
            }
            None => {
                let idle_start = idle_start_epoch(&mut self.idle_start_epoch);
                let details = "Claude Code".to_string();
                let state = "Waiting for session".to_string();
                let payload = PresencePayload {
                    session_id: None,
                    start_epoch: idle_start,
                    details: details.clone(),
                    state: state.clone(),
                };
                self.publish_presence(
                    payload,
                    &details,
                    &state,
                    idle_start,
                    None,
                    None,
                    config,
                    needs_heartbeat,
                    "Connected (idle)",
                )?;
            }
        }

        Ok(())
    }

    /// Shared publish logic: dedup → rate limit → clear stale → resolve assets → set activity.
    #[allow(clippy::too_many_arguments)]
    fn publish_presence(
        &mut self,
        payload: PresencePayload,
        details: &str,
        state: &str,
        start_epoch: i64,
        session: Option<&ClaudeSessionSnapshot>,
        tooltip: Option<&str>,
        config: &PresenceConfig,
        needs_heartbeat: bool,
        status_label: &str,
    ) -> Result<()> {
        if should_skip_publish(self.last_sent.as_ref(), &payload, needs_heartbeat) {
            self.last_status = status_label.to_string();
            return Ok(());
        }
        if let Some(last_publish) = self.last_publish_at
            && last_publish.elapsed() < DISCORD_MIN_PUBLISH_INTERVAL
        {
            self.last_status = status_label.to_string();
            return Ok(());
        }

        if self.last_sent.as_ref() != Some(&payload)
            && let Some(client) = self.client.as_mut()
        {
            let _ = client.clear_activity();
        }

        let assets =
            resolve_presence_assets(session, config, self.known_asset_keys.as_ref(), tooltip);
        let activity = build_activity(
            details,
            state,
            start_epoch,
            assets.large_image.as_deref(),
            assets.large_text.as_deref(),
            assets.small_image.as_deref(),
            assets.small_text.as_deref(),
        );
        let client = self
            .client
            .as_mut()
            .ok_or_else(|| anyhow!("Discord IPC client unexpectedly missing"))?;
        if let Err(err) = client
            .set_activity(activity)
            .context("failed to set Discord activity")
        {
            self.handle_ipc_error(&err.to_string());
            return Err(err);
        }
        self.last_sent = Some(payload);
        self.last_publish_at = Some(Instant::now());
        self.last_heartbeat_at = Some(Instant::now());
        self.last_status = status_label.to_string();
        Ok(())
    }

    pub fn shutdown(&mut self) {
        let _ = self.clear_activity();
        if let Some(client) = self.client.as_mut() {
            let _ = client.close();
        }
        self.client = None;
        self.last_sent = None;
        self.last_publish_at = None;
        self.last_heartbeat_at = None;
        self.last_asset_refresh_at = None;
        self.idle_start_epoch = None;
        self.consecutive_errors = 0;
        self.reconnect_backoff = RECONNECT_MIN_BACKOFF;
        if self.client_id.is_some() {
            self.last_status = "Disconnected".to_string();
        }
    }

    fn clear_activity(&mut self) -> Result<()> {
        if let Some(client) = self.client.as_mut()
            && let Err(err) = client
                .clear_activity()
                .context("failed to clear Discord activity")
        {
            self.handle_ipc_error(&err.to_string());
            return Err(err);
        }
        Ok(())
    }

    fn ensure_connected(&mut self) -> Result<()> {
        if self.client.is_some() {
            return Ok(());
        }

        let Some(client_id) = self.client_id.clone() else {
            return Ok(());
        };

        if let Some(last_attempt) = self.last_reconnect_attempt
            && last_attempt.elapsed() < self.reconnect_backoff
        {
            return Ok(());
        }

        self.last_reconnect_attempt = Some(Instant::now());

        let mut client = DiscordIpcClient::new(&client_id);
        match client
            .connect()
            .context("failed to connect to Discord IPC (is Discord desktop open?)")
        {
            Ok(()) => {
                self.client = Some(client);
                self.consecutive_errors = 0;
                self.reconnect_backoff = RECONNECT_MIN_BACKOFF;
                self.last_sent = None;
                self.last_heartbeat_at = None;
                self.last_status = "Connected".to_string();
                Ok(())
            }
            Err(err) => {
                self.increase_backoff();
                self.last_status =
                    format!("Reconnecting in {}s...", self.reconnect_backoff.as_secs());
                Err(err)
            }
        }
    }

    fn refresh_asset_keys_if_needed(&mut self) {
        let Some(client_id) = self.client_id.as_deref() else {
            return;
        };
        if let Some(last_refresh) = self.last_asset_refresh_at
            && last_refresh.elapsed() < DISCORD_ASSET_REFRESH_INTERVAL
        {
            return;
        }

        self.last_asset_refresh_at = Some(Instant::now());
        if let Ok(asset_keys) = fetch_discord_asset_keys(client_id) {
            self.known_asset_keys = Some(asset_keys);
        }
    }

    fn handle_ipc_error(&mut self, message: &str) {
        self.client = None;
        self.increase_backoff();
        self.last_status = format!("Discord error: {}", compact_error(message));
    }

    fn increase_backoff(&mut self) {
        self.consecutive_errors = self.consecutive_errors.saturating_add(1);
        let backoff_secs = RECONNECT_MIN_BACKOFF
            .as_secs()
            .saturating_mul(1u64 << self.consecutive_errors.min(4));
        self.reconnect_backoff =
            Duration::from_secs(backoff_secs.min(RECONNECT_MAX_BACKOFF.as_secs()));
    }
}

fn compact_error(input: &str) -> String {
    const MAX: usize = 96;
    if input.len() <= MAX {
        return input.to_string();
    }
    let mut end = MAX.saturating_sub(3);
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &input[..end])
}

fn build_activity<'a>(
    details: &'a str,
    state: &'a str,
    start_epoch: i64,
    large_image_key: Option<&'a str>,
    large_text: Option<&'a str>,
    small_image_key: Option<&'a str>,
    small_text: Option<&'a str>,
) -> Activity<'a> {
    let mut activity = Activity::new()
        .details(details)
        .state(state)
        .timestamps(Timestamps::new().start(start_epoch))
        .buttons(vec![Button::new("View Source", GITHUB_REPO_URL)]);

    let mut assets = Assets::new();
    let mut has_assets = false;

    if let Some(image_key) = large_image_key {
        assets = assets.large_image(image_key);
        has_assets = true;
        if let Some(text) = large_text {
            assets = assets.large_text(text);
        }
    }

    if let Some(image_key) = small_image_key {
        assets = assets.small_image(image_key);
        has_assets = true;
        if let Some(text) = small_text {
            assets = assets.small_text(text);
        }
    }

    if has_assets {
        activity = activity.assets(assets);
    }

    activity
}

fn resolve_presence_assets(
    session: Option<&ClaudeSessionSnapshot>,
    config: &PresenceConfig,
    known_keys: Option<&HashSet<String>>,
    tooltip_override: Option<&str>,
) -> ResolvedAssets {
    let large_image = resolve_image_ref(&config.display.large_image_key, known_keys);
    let large_text = tooltip_override
        .and_then(non_empty_trimmed)
        .map(str::to_string)
        .or_else(|| non_empty_trimmed(&config.display.large_text).map(str::to_string));

    let (small_key, small_text) = session
        .map(|session| small_asset_for_activity(session, config))
        .unwrap_or_else(|| (String::new(), String::new()));
    let small_image = resolve_image_ref(&small_key, known_keys);
    let small_text = non_empty_trimmed(&small_text).map(str::to_string);
    let (large_image, small_image) = normalize_asset_pair(large_image, small_image);

    ResolvedAssets {
        large_image,
        large_text,
        small_image,
        small_text,
    }
}

/// Returns (details, state, tooltip) for the Discord presence.
/// - details: activity + project (line 1)
/// - state: essential metrics with `·` separator (line 2)
/// - tooltip: detailed breakdown shown on large image hover
pub fn presence_lines(
    session: &ClaudeSessionSnapshot,
    effective_limits: Option<&RateLimits>,
    api_usage: Option<&UsageData>,
    config: &PresenceConfig,
) -> (String, String, String) {
    if config.privacy.enabled {
        return (
            "Using Claude Code".to_string(),
            "In a coding session".to_string(),
            String::new(),
        );
    }

    let project_label = if config.privacy.show_project_name {
        if config.privacy.show_git_branch {
            if let Some(branch) = &session.git_branch {
                format!("{} ({branch})", session.project_name)
            } else {
                session.project_name.clone()
            }
        } else {
            session.project_name.clone()
        }
    } else {
        "private project".to_string()
    };

    let details = if config.privacy.show_activity {
        if let Some(activity) = &session.activity {
            format!(
                "{} \u{2022} {}",
                effective_activity_text(session, activity, config.privacy.show_activity_target),
                project_label
            )
        } else if config.privacy.show_project_name {
            format!("Working on {}", project_label)
        } else {
            "Working in Claude Code".to_string()
        }
    } else if config.privacy.show_project_name {
        project_label.clone()
    } else {
        "Using Claude Code".to_string()
    };
    let details = truncate_for_discord(&details);

    let mut state_parts: Vec<String> = Vec::new();

    if config.privacy.show_model {
        let model_id = session.model.as_deref().unwrap_or("");
        let is_fast = session.speed.is_fast() && crate::cost::is_fast_capable(model_id);
        let model_label = if let Some(display) = &session.model_display {
            let model_with_ctx = crate::cost::model_display_with_context(
                model_id,
                display,
                session.max_turn_api_input,
            );
            let stripped = crate::cost::strip_claude_prefix(&model_with_ctx);
            if stripped.is_empty() || stripped == "Claude" || stripped.starts_with('(') {
                session.model.as_deref().unwrap_or("Unknown").to_string()
            } else {
                stripped.to_string()
            }
        } else if let Some(model) = &session.model {
            model.clone()
        } else {
            "Unknown".to_string()
        };

        let model_label = if is_fast {
            format!("\u{26a1} {model_label}")
        } else {
            model_label
        };

        state_parts.push(model_label);
    }

    if config.privacy.show_plan
        && let Some(plan_name) = config.plan_badge_name()
    {
        state_parts.push(plan_name.to_string());
    }

    if config.privacy.show_model {
        state_parts.push(session.reasoning_effort.label().to_string());
        if session.is_ultrathinking() {
            state_parts.push("ULTRATHINK".to_string());
        }
    }

    if config.privacy.show_systems {
        render_systems_to_state(&mut state_parts, session);
    }

    if config.privacy.show_tokens
        && let Some(total) = session.session_total_tokens
        && total > 0
    {
        state_parts.push(format!("{} tokens", format_tokens(total)));
    }

    if config.privacy.show_cost && session.total_cost > 0.0 {
        state_parts.push(format_cost(session.total_cost));
    }

    if config.privacy.show_limits {
        render_limits_to_state(&mut state_parts, effective_limits, api_usage);
    }

    if config.privacy.show_cost {
        render_extra_usage_to_state(&mut state_parts, api_usage);
    }

    let state = compact_join_prioritized(&state_parts, 128);

    let mut tooltip_parts: Vec<String> = Vec::new();

    if config.privacy.show_tokens {
        if let Some(last) = session.last_turn_tokens
            && last > 0
        {
            tooltip_parts.push(format!("Last: {}", format_tokens(last)));
        }
        if let Some(total) = session.session_total_tokens
            && total > 0
        {
            tooltip_parts.push(format!("Total: {}", format_tokens(total)));
        }
    }

    if config.privacy.show_cost && session.total_cost > 0.0 {
        tooltip_parts.push(format!("Cost: {}", format_cost(session.total_cost)));
    }

    let tooltip = compact_join_prioritized(&tooltip_parts, 128);

    (truncate_for_discord(&details), state, tooltip)
}

fn effective_activity_text(
    session: &ClaudeSessionSnapshot,
    activity: &crate::session::ActivitySnapshot,
    show_target: bool,
) -> String {
    if matches!(activity.kind, ActivityKind::Idle) && session_is_recently_active(session) {
        return crate::session::ActivitySnapshot {
            kind: ActivityKind::Thinking,
            ..Default::default()
        }
        .to_text(false);
    }

    activity.to_text(show_target)
}

fn session_is_recently_active(session: &ClaudeSessionSnapshot) -> bool {
    let file_cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(ACTIVE_PRESENTATION_SECS))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    if session.last_activity >= file_cutoff {
        return true;
    }

    let token_cutoff = Utc::now() - chrono::Duration::seconds(ACTIVE_PRESENTATION_SECS as i64);
    session
        .last_token_event_at
        .is_some_and(|ts| ts >= token_cutoff)
}

fn render_systems_to_state(parts: &mut Vec<String>, session: &ClaudeSessionSnapshot) {
    if session.background_work.workflow_active {
        parts.push("ULTRACODE".to_string());
    }
    let count = session
        .subagents
        .len()
        .max(session.background_work.active_agent_count);
    if count > 0 {
        let label = if count == 1 { "agent" } else { "agents" };
        parts.push(format!("{count} {label}"));
    }
}

fn render_extra_usage_to_state(parts: &mut Vec<String>, api_usage: Option<&UsageData>) {
    let Some(usage) = api_usage else { return };
    let Some(ref extra) = usage.extra_usage else {
        return;
    };
    if !extra.is_enabled {
        return;
    }
    if let (Some(spent), Some(limit)) = (extra.used_credits, extra.monthly_limit) {
        let pct = extra.utilization.unwrap_or(0.0);
        parts.push(format!("Extra ${:.2}/${:.2} ({:.0}%)", spent, limit, pct));
    }
}

fn render_limits_to_state(
    parts: &mut Vec<String>,
    effective_limits: Option<&RateLimits>,
    api_usage: Option<&UsageData>,
) {
    if let Some(usage) = api_usage {
        let five_hr_used = usage.five_hour.utilization.clamp(0.0, 100.0);
        parts.push(format!("5h {:.0}% used", five_hr_used));
        let seven_day_used = usage.seven_day.utilization.clamp(0.0, 100.0);
        parts.push(format!("7d {:.0}% used", seven_day_used));
        return;
    }

    if let Some(limits) = effective_limits {
        if let Some(primary) = &limits.primary {
            parts.push(format!(
                "5h {:.0}% used",
                (100.0 - primary.remaining_percent).clamp(0.0, 100.0)
            ));
        }
        if let Some(secondary) = &limits.secondary {
            parts.push(format!(
                "7d {:.0}% used",
                (100.0 - secondary.remaining_percent).clamp(0.0, 100.0)
            ));
        }
    }
}

fn small_asset_for_activity(
    session: &ClaudeSessionSnapshot,
    config: &PresenceConfig,
) -> (String, String) {
    let activity_kind = session
        .activity
        .as_ref()
        .map(|a| &a.kind)
        .unwrap_or(&ActivityKind::Idle);

    let keys = &config.display.activity_small_image_keys;
    let override_key = match activity_kind {
        ActivityKind::Thinking => keys.thinking.as_deref(),
        ActivityKind::ReadingFile => keys.reading.as_deref(),
        ActivityKind::EditingFile => keys.editing.as_deref(),
        ActivityKind::RunningCommand => keys.running.as_deref(),
        ActivityKind::WaitingInput => keys.waiting.as_deref(),
        ActivityKind::Idle => keys.idle.as_deref(),
    };

    let image_key = override_key
        .unwrap_or(&config.display.small_image_key)
        .to_string();

    let text = match activity_kind {
        ActivityKind::Thinking => "Thinking",
        ActivityKind::ReadingFile => "Reading files",
        ActivityKind::EditingFile => "Editing files",
        ActivityKind::RunningCommand => "Running command",
        ActivityKind::WaitingInput => "Waiting for input",
        ActivityKind::Idle => &config.display.small_text,
    };

    (image_key, text.to_string())
}

fn truncate_for_discord(s: &str) -> String {
    if s.len() <= 128 {
        s.to_string()
    } else {
        // Walk back to a valid UTF-8 char boundary to avoid panicking on multi-byte chars
        let mut end = 125;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

fn compact_join_prioritized(parts: &[String], max_len: usize) -> String {
    if parts.is_empty() {
        return String::new();
    }

    let sep = " \u{b7} ";
    let joined = parts.join(sep);
    if joined.len() <= max_len {
        return joined;
    }

    for count in (1..parts.len()).rev() {
        let attempt = parts[..count].join(sep);
        if attempt.len() <= max_len {
            return attempt;
        }
    }

    truncate_for_discord(&parts[0])
}

fn non_empty_trimmed(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Resolve a configured image reference to a value Discord will actually render.
///
/// Discord Rich Presence accepts several forms for `large_image` / `small_image`:
///   - Asset keys uploaded to the Developer Portal (most reliable)
///   - `mp:external/...` Media Proxy URLs (undocumented; works inconsistently)
///   - Raw `https://` URLs (silently dropped by many Discord client versions)
///   - `mp:attachments/...` / `mp:avatars/...` (CDN-backed assets)
///
/// This resolver picks the best tier:
///   1. If `trimmed` is already an `mp:...` or CDN URL → pass through
///   2. If `known_keys` says Discord has this key on the app → use the key
///   3. If `trimmed` is a plain `https://` URL → wrap as `mp:external/https/<rest>`
///      so Discord's Media Proxy tries to render it
///   4. Otherwise → return the key as-is and let Discord resolve it (best effort)
fn resolve_image_ref(key: &str, known_keys: Option<&HashSet<String>>) -> Option<String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.starts_with("mp:") {
        return Some(trimmed.to_string());
    }

    if let Some(known) = known_keys
        && known.contains(trimmed)
    {
        return Some(trimmed.to_string());
    }

    if let Some(rest) = trimmed.strip_prefix("https://") {
        return Some(format!("mp:external/https/{rest}"));
    }
    if let Some(rest) = trimmed.strip_prefix("http://") {
        return Some(format!("mp:external/http/{rest}"));
    }

    Some(trimmed.to_string())
}

fn normalize_asset_pair(
    large: Option<String>,
    small: Option<String>,
) -> (Option<String>, Option<String>) {
    if small.is_some() && large.is_none() {
        return (small, None);
    }
    (large, small)
}

#[derive(Debug, Deserialize)]
struct DiscordAssetEntry {
    name: String,
}

fn fetch_discord_asset_keys(client_id: &str) -> Result<HashSet<String>> {
    let url = format!("https://discord.com/api/v10/oauth2/applications/{client_id}/assets");

    let agent = ureq::AgentBuilder::new()
        .timeout(DISCORD_ASSET_FETCH_TIMEOUT)
        .build();

    let response = agent.get(&url).call()?;
    let body = response.into_string()?;
    parse_discord_asset_keys(&body)
}

/// Parse Discord asset keys from the JSON response body.
/// Extracted for testability.
fn parse_discord_asset_keys(json_body: &str) -> Result<HashSet<String>> {
    let entries: Vec<DiscordAssetEntry> =
        serde_json::from_str(json_body).context("failed to parse Discord asset response")?;
    Ok(entries.into_iter().map(|e| e.name).collect())
}

/// Determine whether to skip publishing based on payload dedup and heartbeat.
fn should_skip_publish(
    previous: Option<&PresencePayload>,
    current: &PresencePayload,
    needs_heartbeat: bool,
) -> bool {
    if let Some(prev) = previous {
        prev == current && !needs_heartbeat
    } else {
        false
    }
}

/// Get or initialize the idle start epoch.
fn idle_start_epoch(slot: &mut Option<i64>) -> i64 {
    *slot.get_or_insert_with(|| Utc::now().timestamp().max(0))
}

/// Compute the presence start epoch from a session snapshot.
fn presence_start_epoch(session: &ClaudeSessionSnapshot) -> i64 {
    session
        .started_at
        .unwrap_or_else(Utc::now)
        .timestamp()
        .max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_for_discord_short() {
        let s = "Hello world";
        assert_eq!(truncate_for_discord(s), s);
    }

    #[test]
    fn test_truncate_for_discord_exact_limit() {
        let s = "a".repeat(128);
        assert_eq!(truncate_for_discord(&s), s);
    }

    #[test]
    fn test_truncate_for_discord_over_limit() {
        let s = "a".repeat(200);
        let result = truncate_for_discord(&s);
        assert!(result.len() <= 128);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_for_discord_multibyte_boundary() {
        let s = "a".repeat(124) + "\u{1f600}";
        let result = truncate_for_discord(&s);
        // Should not panic and should be valid UTF-8
        assert!(result.len() <= 128);
    }

    #[test]
    fn test_compact_join_prioritized_empty() {
        let result = compact_join_prioritized(&[], 128);
        assert_eq!(result, "");
    }

    #[test]
    fn test_compact_join_prioritized_fits() {
        let parts = vec![
            "Model".to_string(),
            "$1.50".to_string(),
            "5h 80%".to_string(),
        ];
        let result = compact_join_prioritized(&parts, 128);
        assert_eq!(result, "Model \u{b7} $1.50 \u{b7} 5h 80%");
    }

    #[test]
    fn test_compact_join_prioritized_drops_tail() {
        let parts = vec![
            "Claude Opus 4.6".to_string(),
            "Max ($200/mo)".to_string(),
            "$15.50".to_string(),
            "125K tokens".to_string(),
            "5h 45%".to_string(),
            "7d 80%".to_string(),
        ];
        let result = compact_join_prioritized(&parts, 50);
        assert!(result.len() <= 50);
        assert!(result.contains("Claude Opus 4.6"));
    }

    #[test]
    fn test_non_empty_trimmed() {
        assert_eq!(non_empty_trimmed("hello"), Some("hello"));
        assert_eq!(non_empty_trimmed("  hello  "), Some("hello"));
        assert_eq!(non_empty_trimmed(""), None);
        assert_eq!(non_empty_trimmed("   "), None);
    }

    #[test]
    fn test_resolve_image_key_empty() {
        assert_eq!(resolve_image_ref("", None), None);
        assert_eq!(resolve_image_ref("  ", None), None);
    }

    #[test]
    fn test_resolve_image_key_no_known_set() {
        let result = resolve_image_ref("claude-code", None);
        assert_eq!(result, Some("claude-code".to_string()));
    }

    #[test]
    fn test_resolve_image_key_known() {
        let mut known = HashSet::new();
        known.insert("claude-code".to_string());
        assert_eq!(
            resolve_image_ref("claude-code", Some(&known)),
            Some("claude-code".to_string())
        );
        assert_eq!(
            resolve_image_ref("unknown-key", Some(&known)),
            Some("unknown-key".to_string())
        );
    }

    #[test]
    fn test_resolve_image_key_direct_url_wraps_as_mp_external() {
        // https:// URLs are wrapped in Discord's Media Proxy format so the
        assert_eq!(
            resolve_image_ref(
                "https://example.com/claude-mascot.jpg",
                Some(&HashSet::new())
            ),
            Some("mp:external/https/example.com/claude-mascot.jpg".to_string())
        );
    }

    #[test]
    fn test_resolve_image_key_http_url_wraps_as_mp_external() {
        assert_eq!(
            resolve_image_ref("http://example.com/logo.png", None),
            Some("mp:external/http/example.com/logo.png".to_string())
        );
    }

    #[test]
    fn test_resolve_image_key_mp_prefix_passthrough() {
        assert_eq!(
            resolve_image_ref("mp:external/https/cdn.example/x.png", None),
            Some("mp:external/https/cdn.example/x.png".to_string())
        );
        assert_eq!(
            resolve_image_ref("mp:attachments/12345/image.png", None),
            Some("mp:attachments/12345/image.png".to_string())
        );
    }

    #[test]
    fn test_normalize_asset_pair_both_present() {
        let (large, small) = normalize_asset_pair(
            Some("claude-code".to_string()),
            Some("thinking".to_string()),
        );
        assert_eq!(large, Some("claude-code".to_string()));
        assert_eq!(small, Some("thinking".to_string()));
    }

    #[test]
    fn test_normalize_asset_pair_promote_small_to_large() {
        let (large, small) = normalize_asset_pair(None, Some("thinking".to_string()));
        assert_eq!(large, Some("thinking".to_string()));
        assert_eq!(small, None);
    }

    #[test]
    fn test_normalize_asset_pair_both_none() {
        let (large, small) = normalize_asset_pair(None, None);
        assert_eq!(large, None);
        assert_eq!(small, None);
    }

    #[test]
    fn test_should_skip_publish_no_previous() {
        let payload = PresencePayload {
            session_id: None,
            start_epoch: 1000,
            details: "test".to_string(),
            state: "state".to_string(),
        };
        assert!(!should_skip_publish(None, &payload, false));
    }

    #[test]
    fn test_should_skip_publish_same_payload() {
        let payload = PresencePayload {
            session_id: None,
            start_epoch: 1000,
            details: "test".to_string(),
            state: "state".to_string(),
        };
        assert!(should_skip_publish(Some(&payload), &payload, false));
        // Same payload, heartbeat needed → don't skip
        assert!(!should_skip_publish(Some(&payload), &payload, true));
    }

    #[test]
    fn test_should_skip_publish_different_payload() {
        let prev = PresencePayload {
            session_id: None,
            start_epoch: 1000,
            details: "old".to_string(),
            state: "state".to_string(),
        };
        let current = PresencePayload {
            session_id: None,
            start_epoch: 1000,
            details: "new".to_string(),
            state: "state".to_string(),
        };
        assert!(!should_skip_publish(Some(&prev), &current, false));
    }

    #[test]
    fn test_idle_start_epoch_initializes() {
        let mut slot: Option<i64> = None;
        let epoch = idle_start_epoch(&mut slot);
        assert!(epoch > 0);
        assert_eq!(slot, Some(epoch));
    }

    #[test]
    fn test_idle_start_epoch_preserves() {
        let mut slot: Option<i64> = Some(42);
        let epoch = idle_start_epoch(&mut slot);
        assert_eq!(epoch, 42);
    }

    #[test]
    fn test_parse_discord_asset_keys_valid() {
        let json =
            r#"[{"id":"1","name":"claude-code","type":1},{"id":"2","name":"thinking","type":1}]"#;
        let result = parse_discord_asset_keys(json).unwrap();
        assert!(result.contains("claude-code"));
        assert!(result.contains("thinking"));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_parse_discord_asset_keys_empty() {
        let result = parse_discord_asset_keys("[]").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_compact_error_short() {
        let msg = "short error";
        assert_eq!(compact_error(msg), msg);
    }

    #[test]
    fn test_compact_error_long() {
        let msg = "a".repeat(200);
        let result = compact_error(&msg);
        assert!(result.len() <= 96);
        assert!(result.ends_with("..."));
    }

    fn test_usage_window(utilization: f64) -> crate::usage::UsageWindow {
        crate::usage::UsageWindow {
            utilization,
            resets_at: Some(Utc::now()),
        }
    }

    fn test_usage_data(
        five_hr_util: f64,
        seven_day_util: f64,
        extra: Option<crate::usage::ExtraUsage>,
    ) -> UsageData {
        UsageData {
            five_hour: test_usage_window(five_hr_util),
            seven_day: test_usage_window(seven_day_util),
            sonnet_free: None,
            limits: Vec::new(),
            extra_usage: extra,
        }
    }

    #[test]
    fn test_render_limits_to_state_api_usage() {
        let mut parts = Vec::new();
        let usage = test_usage_data(40.0, 20.0, None);
        render_limits_to_state(&mut parts, None, Some(&usage));
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "5h 40% used");
        assert_eq!(parts[1], "7d 20% used");
    }

    #[test]
    fn test_render_extra_usage_enabled() {
        let mut parts = Vec::new();
        let usage = test_usage_data(
            0.0,
            0.0,
            Some(crate::usage::ExtraUsage {
                is_enabled: true,
                monthly_limit: Some(50.0),
                used_credits: Some(15.0),
                utilization: Some(30.0),
            }),
        );
        render_extra_usage_to_state(&mut parts, Some(&usage));
        assert_eq!(parts.len(), 1);
        assert!(parts[0].contains("Extra"));
        assert!(parts[0].contains("$15.00"));
        assert!(parts[0].contains("$50.00"));
    }

    #[test]
    fn test_render_extra_usage_disabled() {
        let mut parts = Vec::new();
        let usage = test_usage_data(
            0.0,
            0.0,
            Some(crate::usage::ExtraUsage {
                is_enabled: false,
                monthly_limit: None,
                used_credits: None,
                utilization: None,
            }),
        );
        render_extra_usage_to_state(&mut parts, Some(&usage));
        assert!(parts.is_empty());
    }

    fn mythos_class_session(model_id: &str) -> ClaudeSessionSnapshot {
        ClaudeSessionSnapshot {
            session_id: format!("{model_id}-session"),
            cwd: std::path::PathBuf::from("D:/X/Pulse"),
            project_name: "pulse".to_string(),
            git_branch: None,
            model: Some(model_id.to_string()),
            model_display: Some(crate::cost::model_display_name(model_id)),
            session_total_tokens: Some(120_000),
            last_turn_tokens: Some(2_000),
            session_delta_tokens: None,
            input_tokens: 100_000,
            output_tokens: 20_000,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            max_turn_api_input: 100_000,
            current_context_tokens: 100_000,
            reasoning_effort: crate::session::ReasoningEffort::High,
            reasoning_effort_explicit: true,
            has_thinking_blocks: false,
            speed: crate::session::Speed::Standard,
            service_tier: None,
            total_cost: 2.0,
            input_cost: 1.0,
            output_cost: 1.0,
            cache_write_cost: 0.0,
            cache_read_cost: 0.0,
            total_api_duration_ms: 0,
            limits: RateLimits::default(),
            activity: None,
            started_at: None,
            last_token_event_at: None,
            last_activity: std::time::SystemTime::now(),
            source: crate::session::DataSource::Jsonl,
            source_file: std::path::PathBuf::from("session.jsonl"),
            background_work: crate::workflow_state::BackgroundWorkInfo::default(),
            subagents: Vec::new(),
            is_subagent: false,
            parent_session_id: None,
        }
    }

    #[test]
    fn mythos_class_models_render_clean_rich_presence_labels() {
        let config = PresenceConfig::default();
        for (model_id, expected) in [
            ("claude-fable-5", "Fable 5 (1M)"),
            ("claude-mythos-5", "Mythos 5 (1M)"),
        ] {
            let session = mythos_class_session(model_id);
            let (_details, state, _tooltip) = presence_lines(&session, None, None, &config);
            assert!(state.contains(expected), "{model_id}: {state}");
            assert!(!state.contains("(claude-"), "{model_id}: {state}");
        }
    }

    #[test]
    fn opus_5_5_renders_a_clean_rich_presence_label() {
        let config = PresenceConfig::default();
        let session = mythos_class_session("claude-opus-5-5");
        let (_details, state, _tooltip) = presence_lines(&session, None, None, &config);
        assert!(state.contains("Opus 5.5 (1M)"), "{state}");
        assert!(!state.contains("Opus 5 (1M)"), "{state}");
    }

    #[test]
    fn branch_toggle_and_systems_toggle_control_claude_presence_lines() {
        let mut session = mythos_class_session("claude-opus-4-8");
        session.project_name = "PropertyAlpha-Agent".to_string();
        session.git_branch = Some("feat/marketplace-addtochat-liveview-management".to_string());
        session.has_thinking_blocks = true;
        session.background_work = crate::workflow_state::BackgroundWorkInfo {
            workflow_active: true,
            active_agent_count: 1,
            latest_signal_at: Some(std::time::SystemTime::now()),
        };
        session.activity = Some(crate::session::ActivitySnapshot {
            kind: crate::session::ActivityKind::ReadingFile,
            target: Some(
                "channel-events.ts - PropertyAlpha-Agent (feat/marketplace-addtochat-liveview-management)"
                    .to_string(),
            ),
            ..Default::default()
        });
        session.subagents.push(crate::session::SubagentInfo {
            agent_type: "researcher".to_string(),
            model: None,
            model_display: None,
            activity: None,
            tokens: 0,
            cost: 0.0,
        });

        let mut config = PresenceConfig::default();
        config.privacy.show_git_branch = false;
        let (details, state, _tooltip) = presence_lines(&session, None, None, &config);

        assert!(details.starts_with("Reading channel-events.ts"));
        assert!(details.contains("PropertyAlpha-Agent"));
        assert!(!details.contains("feat/marketplace"));
        assert!(!details.contains("researcher"));
        assert!(state.contains("ULTRACODE"));
        assert!(state.contains("1 agent"));

        config.privacy.show_systems = false;
        let (_details, state, _tooltip) = presence_lines(&session, None, None, &config);
        assert!(!state.contains("ULTRACODE"));
        assert!(!state.contains("agent"));
    }

    #[test]
    fn thinking_blocks_do_not_render_as_workflow_system_status() {
        let mut session = mythos_class_session("claude-opus-4-8");
        session.has_thinking_blocks = true;
        session.background_work = crate::workflow_state::BackgroundWorkInfo::default();

        let config = PresenceConfig::default();
        let (_details, state, _tooltip) = presence_lines(&session, None, None, &config);

        assert!(!state.contains("workflow active"));
        assert!(!state.contains("ULTRACODE"));
    }
}
