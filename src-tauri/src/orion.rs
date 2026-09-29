use crate::access::{AccessProof, AccessRouteSnapshot, AccessSource, AccessSourceKind, AuthMethod};
use crate::commands::{DiscordDisplayPrefs, DiscordPresencePreview, DiscordSettings, SessionInfo};
use cc_discord_presence::orion::{ASSET_KEY, Config, NAME, Session, preferred_session};
use codex_presence_core::PresenceFieldId;

/// Orion stores `cost: 0` on every message, so Pulse prices the exact token
/// counts at published per-model rates, the same calculation Claude Code
/// sessions use. It is a calculation, not an invoice: `partial` when some model
/// has no known rate, `unavailable` when none does.
pub const COST_SOURCE: &str = "api_equivalent";

/// Live view of a session: the subagent badge counts only subagents that are
/// still working, so finished ones never linger on the card.
pub fn live_session_info(session: &Session) -> SessionInfo {
    SessionInfo {
        subagent_count: session.running_subagents,
        ..session_info(session)
    }
}

/// Stored view of a session: the subagent count is every subagent it started.
pub fn session_info(session: &Session) -> SessionInfo {
    let window = session.context_window.unwrap_or(0);
    let cost_available = session.known_cost.is_some();
    // Per-category costs describe the priced part only, and only when a cost exists.
    let category = |value: f64| if cost_available { value } else { 0.0 };
    // The marker belongs to the Claude tokenizer generation; other models never carry it.
    let model_id = session.model.rsplit('/').next().unwrap_or(&session.model);
    let has_inflated_tokenizer = model_id.to_ascii_lowercase().starts_with("claude-")
        && cc_discord_presence::cost::has_inflated_tokenizer(model_id);
    SessionInfo {
        provider: "orion".into(),
        app_name: Some(NAME.into()),
        session_id: session.id.clone(),
        session_name: session.title.clone(),
        project: session.project.clone(),
        model: session.model_label(),
        model_id: session.model.clone(),
        context_window: if window > 0 {
            cc_discord_presence::util::format_tokens(window)
        } else {
            "Not reported".into()
        },
        context_window_tokens: window,
        context_used_tokens: session.context_used.unwrap_or(0),
        cost_source: COST_SOURCE.into(),
        cost: session.known_cost.unwrap_or(0.0),
        cost_available,
        cost_basis: if session.known_cost.is_none() {
            "unavailable"
        } else if session.cost_complete {
            "estimated"
        } else {
            "partial"
        }
        .into(),
        tokens: session.usage.total(),
        input_tokens: session.usage.input,
        output_tokens: session.usage.output,
        cache_write_tokens: session.usage.cache_write,
        cache_read_tokens: session.usage.cache_read,
        branch: session.branch.clone(),
        activity: session.activity.clone(),
        activity_target: session.activity_target.clone(),
        effort: session
            .effort
            .clone()
            .unwrap_or_else(|| "Not reported".into()),
        effort_explicit: session.effort.is_some(),
        is_idle: !session.is_active(chrono::Utc::now().timestamp_millis()),
        started_at: chrono::DateTime::from_timestamp_millis(session.created)
            .map(|value| value.to_rfc3339()),
        duration_secs: session.updated.saturating_sub(session.created).max(0) as u64 / 1000,
        has_thinking: session.usage.reasoning > 0,
        subagent_count: session.subagent_count,
        tokens_per_sec: session.output_tokens_per_sec.unwrap_or(0.0),
        input_cost: category(session.cost_parts.input),
        output_cost: category(session.cost_parts.output),
        cache_write_cost: category(session.cost_parts.cache_write),
        cache_read_cost: category(session.cost_parts.cache_read),
        has_inflated_tokenizer,
        speed: "unknown".into(),
        ..Default::default()
    }
}

pub fn preview(
    sessions: &[Session],
    idle_since: Option<i64>,
    config: &Config,
) -> DiscordPresencePreview {
    let now = chrono::Utc::now().timestamp_millis();
    let session = preferred_session(sessions);
    let lines = cc_discord_presence::orion::presence::lines(session, config);
    // Discord shows idle time as its own timer; the preview spells it out.
    let state = if session.is_none() {
        cc_discord_presence::orion::presence::idle_label(idle_since, now)
    } else {
        lines.state
    };
    DiscordPresencePreview {
        provider: "orion".into(),
        app_name: NAME.into(),
        details: lines.details,
        state,
        large_image_key: ASSET_KEY.into(),
        large_text: NAME.into(),
        small_image_key: None,
        small_text: None,
        has_session: session.is_some(),
        duration_secs: session
            .map(|session| session.created)
            .or(idle_since)
            .filter(|since| *since > 0 && *since <= now)
            .map(|since| (now - since) as u64 / 1000)
            .unwrap_or(0),
    }
}

pub fn prefs(config: &Config) -> DiscordDisplayPrefs {
    let has = |id| {
        config
            .layout
            .fields
            .iter()
            .any(|field| field.field == id && field.enabled)
    };
    DiscordDisplayPrefs {
        show_project: has(PresenceFieldId::Project),
        show_branch: has(PresenceFieldId::Branch),
        show_model: has(PresenceFieldId::Model),
        show_activity: has(PresenceFieldId::Activity),
        show_tokens: has(PresenceFieldId::Tokens),
        show_cost: has(PresenceFieldId::Cost),
        show_limits: false,
        show_credits: false,
        show_context: has(PresenceFieldId::Context),
        show_systems: has(PresenceFieldId::Systems),
    }
}

/// Orion reports no account quota or credits, so those fields stay off.
pub fn apply_prefs(config: &mut Config, prefs: &DiscordDisplayPrefs) {
    for field in &mut config.layout.fields {
        field.enabled = match field.field {
            PresenceFieldId::Project => prefs.show_project,
            PresenceFieldId::Branch => prefs.show_branch,
            PresenceFieldId::Model => prefs.show_model,
            PresenceFieldId::Activity => prefs.show_activity,
            PresenceFieldId::Tokens => prefs.show_tokens,
            PresenceFieldId::Cost => prefs.show_cost,
            PresenceFieldId::Quotas | PresenceFieldId::Credits => false,
            PresenceFieldId::Context => prefs.show_context,
            PresenceFieldId::Systems => prefs.show_systems,
        };
    }
}

pub fn settings(config: &Config, status: &str, publisher: &str) -> DiscordSettings {
    DiscordSettings {
        provider: "orion".into(),
        enabled: config.enabled,
        status: status.into(),
        publisher: publisher.into(),
        display_prefs: prefs(config),
        desktop_design: None,
        supports_desktop_design: false,
        supports_field_order: true,
        supports_credits: false,
        field_order: config
            .layout
            .fields
            .iter()
            .map(|field| field.field.as_str().into())
            .collect(),
    }
}

pub fn access_route() -> AccessRouteSnapshot {
    AccessRouteSnapshot::unavailable(
        AccessSource {
            id: "orion-local".into(),
            kind: AccessSourceKind::OrionLocal,
            provider: "orion".into(),
            auth_method: AuthMethod::None,
            proof: AccessProof::None,
            plan: None,
        },
        "Orion App does not report an account quota",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cc_discord_presence::orion::CostParts;

    #[test]
    fn quota_and_credit_fields_stay_off_for_orion() {
        let mut config = Config::default();
        let prefs = DiscordDisplayPrefs {
            show_project: true,
            show_branch: true,
            show_model: true,
            show_activity: true,
            show_tokens: true,
            show_cost: true,
            show_limits: true,
            show_credits: true,
            show_context: true,
            show_systems: true,
        };
        apply_prefs(&mut config, &prefs);
        let read = super::prefs(&config);
        assert!(!read.show_limits && !read.show_credits);
        assert!(read.show_systems && read.show_cost && read.show_context);
        let settings = settings(&config, "Connected", "Pulse");
        assert_eq!(settings.provider, "orion");
        assert!(!settings.supports_credits);
    }

    #[test]
    fn session_info_keeps_missing_cost_unavailable() {
        let session = Session {
            id: "s".into(),
            model: "opencode-go/unknown".into(),
            ..Default::default()
        };
        let info = session_info(&session);
        assert_eq!(info.provider, "orion");
        assert!(!info.cost_available);
        assert_eq!(info.cost_basis, "unavailable");
        assert_eq!(info.cost, 0.0);
        assert_eq!(info.input_cost + info.output_cost, 0.0);
    }

    fn priced(known_cost: Option<f64>, complete: bool) -> Session {
        Session {
            id: "s".into(),
            model: "anthropic/claude-opus-5-5".into(),
            usage: cc_discord_presence::orion::Usage {
                input: 3_000_000,
                output: 1_000_000,
                cache_read: 1_000_000,
                cache_write: 1_000_000,
                ..Default::default()
            },
            known_cost,
            cost_complete: complete,
            cost_parts: CostParts {
                input: 4.0,
                output: 20.0,
                cache_write: 5.0,
                cache_read: 0.2,
            },
            output_tokens_per_sec: Some(120.0),
            created: 1_000_000,
            updated: 4_600_000,
            ..Default::default()
        }
    }

    #[test]
    fn session_info_carries_the_same_cost_grid_as_a_claude_session() {
        let info = session_info(&priced(Some(29.2), true));
        assert!(info.cost_available);
        assert_eq!(info.cost_basis, "estimated");
        assert_eq!(info.cost_source, COST_SOURCE);
        assert!((info.cost - 29.2).abs() < 1e-9);
        assert!(
            (info.input_cost + info.output_cost + info.cache_write_cost + info.cache_read_cost
                - info.cost)
                .abs()
                < 1e-9
        );
        assert_eq!(info.cache_write_cost, 5.0);
        assert_eq!(info.tokens_per_sec, 120.0);
        assert_eq!(info.duration_secs, 3_600);
        assert!(!info.has_inflated_tokenizer, "Opus 5.5 carries no marker");
    }

    #[test]
    fn a_lower_bound_stays_partial_with_its_priced_categories() {
        let info = session_info(&priced(Some(29.2), false));
        assert!(info.cost_available);
        assert_eq!(info.cost_basis, "partial");
        assert_eq!(info.output_cost, 20.0);
    }

    #[test]
    fn the_tokenizer_marker_only_follows_claude_models() {
        let mut session = priced(Some(1.0), true);
        session.model = "anthropic/claude-sonnet-5".into();
        assert!(session_info(&session).has_inflated_tokenizer);
        session.model = "gpt-5.6-sol".into();
        assert!(!session_info(&session).has_inflated_tokenizer);
    }

    #[test]
    fn live_cards_count_only_running_subagents_and_history_keeps_the_total() {
        let session = Session {
            subagent_count: 5,
            running_subagents: 1,
            ..priced(Some(1.0), true)
        };
        assert_eq!(live_session_info(&session).subagent_count, 1);
        assert_eq!(session_info(&session).subagent_count, 5);
    }

    #[test]
    fn idle_preview_spells_out_idle_time() {
        let now = chrono::Utc::now().timestamp_millis();
        let config = Config::default();
        let fresh = preview(&[], None, &config);
        assert_eq!(fresh.details, "Orion App");
        assert_eq!(fresh.state, "Idling...");
        assert!(!fresh.has_session);
        let idle = preview(&[], Some(now - 12 * 60_000 - 5_000), &config);
        assert_eq!(idle.state, "Idling for 12m");
        assert!(
            (720..=730).contains(&idle.duration_secs),
            "{}",
            idle.duration_secs
        );
    }
}
