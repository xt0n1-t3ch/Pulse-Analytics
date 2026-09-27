use crate::access::{AccessProof, AccessRouteSnapshot, AccessSource, AccessSourceKind, AuthMethod};
use crate::commands::{DiscordDisplayPrefs, DiscordPresencePreview, DiscordSettings, SessionInfo};
use cc_discord_presence::orion::{ASSET_KEY, Config, NAME, Session, preferred_session};
use codex_presence_core::PresenceFieldId;

pub fn session_info(session: &Session) -> SessionInfo {
    let window = session.context_window.unwrap_or(0);
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
        cost_source: "api_equivalent".into(),
        cost: session.known_cost.unwrap_or(0.0),
        cost_available: session.known_cost.is_some(),
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
        speed: "unknown".into(),
        ..Default::default()
    }
}

pub fn preview(sessions: &[Session], config: &Config) -> DiscordPresencePreview {
    let session = preferred_session(sessions);
    let lines = cc_discord_presence::orion::presence::lines(session, config);
    DiscordPresencePreview {
        provider: "orion".into(),
        app_name: NAME.into(),
        details: lines.details,
        state: lines.state,
        large_image_key: ASSET_KEY.into(),
        large_text: NAME.into(),
        small_image_key: None,
        small_text: None,
        has_session: session.is_some(),
        duration_secs: session
            .map(|session| {
                (chrono::Utc::now().timestamp_millis() - session.created).max(0) as u64 / 1000
            })
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
    }
}
