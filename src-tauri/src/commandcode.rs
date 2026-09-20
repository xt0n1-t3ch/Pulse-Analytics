use crate::access::{
    AccessAvailability, AccessFreshness, AccessProof, AccessProvenance, AccessRouteSnapshot,
    AccessSource, AccessSourceKind, AccessWindow, AuthMethod,
};
use crate::commands::{DiscordDisplayPrefs, DiscordPresencePreview, DiscordSettings, SessionInfo};
use cc_discord_presence::commandcode::{ASSET_KEY, Config, NAME, Session, preferred_session};
use codex_presence_core::{CreditBalance, PresenceFieldId, QuotaWindow};

pub fn session_info(session: &Session) -> SessionInfo {
    let window = session.context_window.unwrap_or(0);
    SessionInfo {
        provider: "commandcode".into(),
        app_name: Some(NAME.into()),
        session_id: session.id.clone(),
        session_name: session.title.clone(),
        project: session.project.clone(),
        model: session.model.clone(),
        model_id: session.model.clone(),
        context_window: if window > 0 {
            cc_discord_presence::util::format_tokens(window)
        } else {
            "Not reported".into()
        },
        context_window_tokens: window,
        context_used_tokens: session.context_used.unwrap_or(0),
        cost_source: "commandcode_reported".into(),
        cost: session.known_cost.unwrap_or(0.0),
        cost_available: session.known_cost.is_some(),
        cost_basis: if session.known_cost.is_none() {
            "unavailable"
        } else if session.cost_complete {
            "exact"
        } else {
            "partial"
        }
        .into(),
        tokens: session.usage.total(),
        input_tokens: session
            .usage
            .input
            .saturating_add(session.usage.cache_read)
            .saturating_add(session.usage.cache_write),
        output_tokens: session.usage.output,
        cache_write_tokens: session.usage.cache_write,
        cache_read_tokens: session.usage.cache_read,
        branch: session.branch.clone(),
        activity: session.activity.clone(),
        effort: session
            .effort
            .clone()
            .unwrap_or_else(|| "Not reported".into()),
        effort_explicit: session.effort.is_some(),
        is_idle: !session.is_active(chrono::Utc::now().timestamp_millis()),
        started_at: chrono::DateTime::from_timestamp_millis(session.created)
            .map(|value| value.to_rfc3339()),
        duration_secs: session.updated.saturating_sub(session.created).max(0) as u64 / 1000,
        speed: "unknown".into(),
        ..Default::default()
    }
}

pub fn preview(sessions: &[Session], config: &Config) -> DiscordPresencePreview {
    let session = preferred_session(sessions);
    let (quotas, credits) = session
        .map(|session| crate::accounts::command_presence(&session.profile))
        .unwrap_or_default();
    let lines = cc_discord_presence::commandcode::presence::lines(
        session,
        config,
        quotas.as_deref(),
        credits.as_deref(),
    );
    DiscordPresencePreview {
        provider: "commandcode".into(),
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
        show_limits: has(PresenceFieldId::Quotas),
        show_credits: has(PresenceFieldId::Credits),
        show_context: has(PresenceFieldId::Context),
        show_systems: false,
    }
}
pub fn apply_prefs(config: &mut Config, prefs: &DiscordDisplayPrefs) {
    for field in &mut config.layout.fields {
        field.enabled = match field.field {
            PresenceFieldId::Project => prefs.show_project,
            PresenceFieldId::Branch => prefs.show_branch,
            PresenceFieldId::Model => prefs.show_model,
            PresenceFieldId::Activity => prefs.show_activity,
            PresenceFieldId::Tokens => prefs.show_tokens,
            PresenceFieldId::Cost => prefs.show_cost,
            PresenceFieldId::Quotas => prefs.show_limits,
            PresenceFieldId::Credits => prefs.show_credits,
            PresenceFieldId::Context => prefs.show_context,
            PresenceFieldId::Systems => false,
        };
    }
}
pub fn settings(config: &Config, status: &str, publisher: &str) -> DiscordSettings {
    DiscordSettings {
        provider: "commandcode".into(),
        enabled: config.enabled,
        status: status.into(),
        publisher: publisher.into(),
        display_prefs: prefs(config),
        desktop_design: None,
        supports_desktop_design: false,
        supports_field_order: true,
        supports_credits: true,
        field_order: config
            .layout
            .fields
            .iter()
            .map(|field| field.field.as_str().into())
            .collect(),
    }
}
pub fn access_route() -> AccessRouteSnapshot {
    let account = crate::accounts::command_account(&cc_discord_presence::commandcode::home());
    let source = AccessSource {
        id: account
            .as_ref()
            .map(|account| format!("commandcode:{}", account.id))
            .unwrap_or_else(|| "commandcode-local".into()),
        kind: if account.is_some() {
            AccessSourceKind::CommandCodeSubscription
        } else {
            AccessSourceKind::CommandCodeLocal
        },
        provider: "commandcode".into(),
        auth_method: AuthMethod::ApiKey,
        proof: AccessProof::None,
        plan: None,
    };
    let Some(account) = account else {
        return AccessRouteSnapshot::unavailable(source, "Command Code account is not connected");
    };
    let mut route = AccessRouteSnapshot::unavailable(source, "Command Code usage is not available");
    if account.status != "connected" {
        return route;
    }
    route.source.proof = AccessProof::QuotaResponse;
    route.source.plan = account.plan;
    route.availability = AccessAvailability::Available;
    route.freshness = AccessFreshness::Fresh;
    route.provenance = AccessProvenance::ProviderApi;
    route.error = None;
    route.unavailable_reason = None;
    route.observed_at = account
        .observed_at
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
        .map(|value| value.with_timezone(&chrono::Utc));
    route.fetched_at = route.observed_at;
    route.expires_at = account
        .expires_at
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
        .map(|value| value.with_timezone(&chrono::Utc));
    route.windows = account
        .windows
        .iter()
        .filter_map(|window| {
            Some(AccessWindow {
                key: window.id.clone(),
                label: Some(window.label.clone()),
                quota: QuotaWindow {
                    window_minutes: window.duration_minutes.unwrap_or(0),
                    used_percent: window.used_percent?,
                    remaining_percent: (100.0 - window.used_percent?).clamp(0.0, 100.0),
                    resets_at: window
                        .resets_at
                        .as_deref()
                        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                        .map(|value| value.with_timezone(&chrono::Utc)),
                },
            })
        })
        .collect();
    let total = account
        .balances
        .iter()
        .filter(|balance| balance.unit == "credits")
        .try_fold(0.0, |sum, balance| {
            balance.value.parse::<f64>().ok().map(|value| sum + value)
        });
    if let Some(total) = total.filter(|value| value.is_finite()) {
        route.credits = Some(CreditBalance {
            balance: Some(format!("{total:.2}")),
            has_credits: total > 0.0,
            unlimited: false,
        });
    }
    route
}
