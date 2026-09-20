use cc_discord_presence::opencode::{Config, Session, preferred_session};
use codex_presence_core::PresenceFieldId;

use crate::commands::{DiscordDisplayPrefs, DiscordPresencePreview, SessionInfo};

pub fn session_info(session: &Session) -> SessionInfo {
    let now = chrono::Utc::now().timestamp_millis();
    let window = session.context_window.unwrap_or(0);
    let fast = session.metadata.model_id.starts_with("gpt-")
        && session.metadata.model_id.ends_with("-fast");
    SessionInfo {
        provider: "opencode".into(),
        app_name: Some(session.metadata.surface.clone()),
        session_id: session.id.clone(),
        session_name: Some(session.title.clone()),
        project: session.project.clone(),
        model: if session.metadata.models.len() > 1 {
            "Mixed models".into()
        } else {
            session.metadata.model_name.clone()
        },
        model_id: session.metadata.model_id.clone(),
        context_window: if window > 0 {
            cc_discord_presence::util::format_tokens(window)
        } else {
            "Not reported".into()
        },
        cost: session.cost.unwrap_or(0.0),
        cost_available: session.cost.is_some(),
        cost_source: "opencode_reported".into(),
        cost_basis: if session.cost.is_some() {
            "exact"
        } else {
            "unavailable"
        }
        .into(),
        tokens: session.usage.total_tokens(),
        input_tokens: session
            .usage
            .input
            .saturating_add(session.usage.cache_read)
            .saturating_add(session.usage.cache_write),
        output_tokens: session.usage.output.saturating_add(session.usage.reasoning),
        cache_write_tokens: session.usage.cache_write,
        cache_read_tokens: session.usage.cache_read,
        context_used_tokens: session.context_used.unwrap_or(0),
        context_window_tokens: window,
        branch: session.branch.clone(),
        activity: session.activity.clone(),
        activity_target: session.activity_target.clone(),
        effort: session
            .metadata
            .variant
            .clone()
            .unwrap_or_else(|| "Not reported".into()),
        effort_explicit: session.metadata.variant.is_some(),
        is_idle: session.is_idle(now),
        started_at: chrono::DateTime::from_timestamp_millis(session.created)
            .map(|date| date.to_rfc3339()),
        duration_secs: session.updated.saturating_sub(session.created).max(0) as u64 / 1000,
        has_thinking: session.usage.reasoning > 0,
        speed: if fast { "fast" } else { "unknown" }.into(),
        fast,
        opencode: Some(session.metadata.clone()),
        ..Default::default()
    }
}

pub fn preview(
    sessions: &[Session],
    config: &Config,
    routes: &[crate::access::AccessRouteSnapshot],
) -> DiscordPresencePreview {
    let session = preferred_session(sessions);
    let quotas = crate::opencode_go::presence_text(routes, chrono::Utc::now());
    let lines = cc_discord_presence::opencode::presence::lines(session, config, quotas.as_deref());
    DiscordPresencePreview {
        provider: "opencode".into(),
        app_name: "OpenCode".into(),
        details: lines.details,
        state: lines.state,
        large_image_key: cc_discord_presence::opencode::presence::ASSET_KEY.into(),
        large_text: "OpenCode".into(),
        small_image_key: None,
        small_text: None,
        has_session: session.is_some(),
        duration_secs: session
            .map(|session| {
                chrono::Utc::now()
                    .timestamp_millis()
                    .saturating_sub(session.created)
                    .max(0) as u64
                    / 1000
            })
            .unwrap_or(0),
    }
}

pub fn prefs(config: &Config) -> DiscordDisplayPrefs {
    let enabled = |field| {
        config
            .layout
            .fields
            .iter()
            .any(|item| item.field == field && item.enabled)
    };
    DiscordDisplayPrefs {
        show_project: enabled(PresenceFieldId::Project),
        show_branch: enabled(PresenceFieldId::Branch),
        show_model: enabled(PresenceFieldId::Model),
        show_activity: enabled(PresenceFieldId::Activity),
        show_tokens: enabled(PresenceFieldId::Tokens),
        show_cost: enabled(PresenceFieldId::Cost),
        show_context: enabled(PresenceFieldId::Context),
        show_limits: enabled(PresenceFieldId::Quotas),
        show_credits: false,
        show_systems: false,
    }
}

pub fn apply_prefs(config: &mut Config, prefs: &DiscordDisplayPrefs) {
    for item in &mut config.layout.fields {
        item.enabled = match item.field {
            PresenceFieldId::Project => prefs.show_project,
            PresenceFieldId::Branch => prefs.show_branch,
            PresenceFieldId::Model => prefs.show_model,
            PresenceFieldId::Activity => prefs.show_activity,
            PresenceFieldId::Tokens => prefs.show_tokens,
            PresenceFieldId::Cost => prefs.show_cost,
            PresenceFieldId::Context => prefs.show_context,
            PresenceFieldId::Quotas => prefs.show_limits,
            _ => false,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quotas_toggle_roundtrips_without_changing_order_or_other_fields() {
        let mut config = Config::default();
        config.layout.fields.reverse();
        let order: Vec<_> = config.layout.fields.iter().map(|item| item.field).collect();
        let mut display = prefs(&config);
        for enabled in [true, false, true] {
            display.show_limits = enabled;
            apply_prefs(&mut config, &display);
            let saved = serde_json::to_string(&config).unwrap();
            config = serde_json::from_str(&saved).unwrap();
            assert_eq!(prefs(&config).show_limits, enabled);
            assert_eq!(
                config
                    .layout
                    .fields
                    .iter()
                    .map(|item| item.field)
                    .collect::<Vec<_>>(),
                order
            );
        }
    }

    #[test]
    fn native_provider_has_no_borrowed_quota_or_speed() {
        let info = session_info(&Session::default());
        assert_eq!(info.provider, "opencode");
        assert_eq!(info.speed, "unknown");
        assert!(!info.cost_available);
        assert_eq!(info.context_window_tokens, 0);
        let prefs = prefs(&Config::default());
        assert!(!prefs.show_credits && !prefs.show_systems);
    }
}
