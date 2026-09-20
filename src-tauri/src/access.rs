//! Provider-neutral access routes for Pulse.
//!
//! The quota payload remains owned by `codex_presence_core`.  This module only
//! adds runtime metadata that the UI needs to decide whether a route is
//! displayable: which product was authenticated, what proof was observed,
//! freshness, and the route's diagnostic state.

use cc_discord_presence::usage::{ExtraUsage, UsageData, UsageLimit};
use chrono::{DateTime, Duration, Utc};
use codex_presence_core::{
    CreditBalance, IndividualSpendLimit, QuotaScope, QuotaWindow, RateLimitResetCreditsSummary,
    RateLimitScope, UsageLane, UsageSignal, UsageSnapshot, UsageSource, format_window_label,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessSourceKind {
    CodexSubscription,
    OpenAiApi,
    ClaudeSubscription,
    AnthropicApi,
    OpenCodeLocal,
    OpenCodeGo,
    CommandCodeLocal,
    CommandCodeSubscription,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    AppServer,
    OAuth,
    ApiKey,
    None,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessProof {
    None,
    QuotaResponse,
    AuthenticatedProbe,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessAvailability {
    Available,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessFreshness {
    Fresh,
    Stale,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessProvenance {
    AppServer,
    ProviderApi,
    MemoryCache,
    SessionJsonl,
    None,
}

/// Why a route is unavailable, classified once from the backend diagnostic so
/// the UI can explain the state instead of re-parsing free-text `error`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessUnavailableReason {
    /// An authenticated session existed but its token/credential expired.
    Expired,
    /// No credential is configured for this lane at all.
    NotConfigured,
    /// A credential exists but the provider probe failed.
    ProbeFailed,
    /// Authenticated but the provider returned no usage data yet.
    NoData,
    /// Diagnostic did not match a known category.
    Other,
}

/// Map a backend diagnostic string to a stable reason. Substrings mirror the
/// exact hints emitted by `usage.rs` and the `probe`/`unavailable` call sites.
pub fn classify_unavailable_reason(error: &str) -> AccessUnavailableReason {
    let e = error.to_lowercase();
    if e.contains("expired") {
        AccessUnavailableReason::Expired
    } else if e.contains("not configured") || e.contains("missing") || e.contains("no credentials")
    {
        AccessUnavailableReason::NotConfigured
    } else if e.contains("probe failed") || e.contains("probe") {
        AccessUnavailableReason::ProbeFailed
    } else if e.contains("no usage data") || e.contains("pending") {
        AccessUnavailableReason::NoData
    } else {
        AccessUnavailableReason::Other
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct AccessSource {
    pub id: String,
    pub kind: AccessSourceKind,
    pub provider: String,
    pub auth_method: AuthMethod,
    pub proof: AccessProof,
    pub plan: Option<String>,
}

/// A display window carries only provider-facing labels around the canonical
/// core `QuotaWindow`; percentages, reset timestamps, and duration stay owned
/// by the core model rather than being copied into Pulse.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AccessWindow {
    pub key: String,
    pub label: Option<String>,
    #[serde(flatten)]
    pub quota: QuotaWindow,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ExtraUsageSnapshot {
    pub enabled: bool,
    pub limit: Option<f64>,
    pub used: Option<f64>,
    pub utilization: Option<f64>,
}

impl From<&ExtraUsage> for ExtraUsageSnapshot {
    fn from(value: &ExtraUsage) -> Self {
        Self {
            enabled: value.is_enabled,
            limit: value.monthly_limit,
            used: value.used_credits,
            utilization: value.utilization,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct LocalHistoryCapability {
    pub available: bool,
    pub sessions: u64,
}

/// One authenticated or attempted provider route.
///
/// `usage` is intentionally skipped from JSON.  It is the canonical core
/// snapshot used by backend consumers; the flattened `windows` and `credits`
/// fields are the stable UI DTO and are derived from that snapshot once.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AccessRouteSnapshot {
    pub source: AccessSource,
    pub availability: AccessAvailability,
    pub freshness: AccessFreshness,
    pub provenance: AccessProvenance,
    pub observed_at: Option<DateTime<Utc>>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub windows: Vec<AccessWindow>,
    pub credits: Option<CreditBalance>,
    /// Codex reset entitlements are a separate provider payload from spend
    /// credits. Keep the canonical structure intact and expose it only when
    /// the authenticated app-server route actually supplied it.
    #[serde(
        rename = "rateLimitResetCredits",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub rate_limit_reset_credits: Option<RateLimitResetCreditsSummary>,
    #[serde(
        rename = "individualSpendLimits",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub individual_limits: Vec<IndividualSpendLimit>,
    pub extra_usage: Option<ExtraUsageSnapshot>,
    /// Local analytics are independent from provider authentication. An
    /// expired subscription route may still own useful historical sessions.
    #[serde(default)]
    pub local_history: LocalHistoryCapability,
    pub error: Option<String>,
    /// Classified reason a route is unavailable, derived once from `error`.
    /// `None` when the route is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<AccessUnavailableReason>,
    #[serde(skip)]
    pub usage: Option<UsageSnapshot>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AccessSnapshot {
    pub routes: Vec<AccessRouteSnapshot>,
}

impl AccessSnapshot {
    /// Keep every lane's latest diagnostic in the transport snapshot. UI and
    /// selection consumers call [`visible_routes`] to proof-gate selectable
    /// routes; dropping failed API lanes here would erase the reason they are
    /// unavailable and make authentication failures indistinguishable from a
    /// missing provider.
    pub fn new(routes: Vec<AccessRouteSnapshot>) -> Self {
        Self { routes }
    }

    pub fn with_local_history(mut self, inventory: &BTreeMap<String, u64>) -> Self {
        for route in &mut self.routes {
            let sessions = inventory
                .get(&route.source.provider)
                .copied()
                .unwrap_or_default();
            route.local_history = LocalHistoryCapability {
                available: sessions > 0,
                sessions,
            };
        }
        self
    }
}

impl AccessRouteSnapshot {
    pub fn unavailable(mut source: AccessSource, error: impl Into<String>) -> Self {
        sanitize_unproved_plan(&mut source);
        let error = error.into();
        let unavailable_reason = Some(classify_unavailable_reason(&error));
        Self {
            source,
            availability: AccessAvailability::Unavailable,
            freshness: AccessFreshness::Unknown,
            provenance: AccessProvenance::None,
            observed_at: None,
            fetched_at: None,
            expires_at: None,
            windows: Vec::new(),
            credits: None,
            rate_limit_reset_credits: None,
            individual_limits: Vec::new(),
            extra_usage: None,
            local_history: LocalHistoryCapability::default(),
            error: Some(error),
            unavailable_reason,
            usage: None,
        }
    }

    pub fn is_authenticated(&self) -> bool {
        !matches!(self.source.proof, AccessProof::None)
    }

    pub fn displayable_usage(&self) -> Option<&UsageSnapshot> {
        (self.availability == AccessAvailability::Available
            && self.freshness == AccessFreshness::Fresh)
            .then_some(self.usage.as_ref())
            .flatten()
    }

    pub fn with_local_history(mut self, sessions: u64) -> Self {
        self.local_history = LocalHistoryCapability {
            available: sessions > 0,
            sessions,
        };
        self
    }
}

/// Build a route around a core snapshot while retaining the provider's native
/// window identity. No synthetic zero window is emitted for sparse responses.
pub fn access_route_from_usage(
    source: AccessSource,
    usage: UsageSnapshot,
    fetched_at: DateTime<Utc>,
    max_age: Duration,
    now: DateTime<Utc>,
) -> AccessRouteSnapshot {
    access_route_from_usage_with_account_details(
        source,
        usage,
        None,
        Vec::new(),
        fetched_at,
        max_age,
        now,
    )
}

/// Build a route from a canonical snapshot and, when present, the separate
/// Codex reset-credit summary returned by the same authenticated read.
pub fn access_route_from_usage_with_reset_credits(
    source: AccessSource,
    usage: UsageSnapshot,
    rate_limit_reset_credits: Option<RateLimitResetCreditsSummary>,
    fetched_at: DateTime<Utc>,
    max_age: Duration,
    now: DateTime<Utc>,
) -> AccessRouteSnapshot {
    access_route_from_usage_with_account_details(
        source,
        usage,
        rate_limit_reset_credits,
        Vec::new(),
        fetched_at,
        max_age,
        now,
    )
}

pub fn access_route_from_usage_with_account_details(
    mut source: AccessSource,
    usage: UsageSnapshot,
    rate_limit_reset_credits: Option<RateLimitResetCreditsSummary>,
    individual_limits: Vec<IndividualSpendLimit>,
    fetched_at: DateTime<Utc>,
    max_age: Duration,
    now: DateTime<Utc>,
) -> AccessRouteSnapshot {
    sanitize_unproved_plan(&mut source);
    if !usage_source_matches(&source, &usage.source) {
        return AccessRouteSnapshot::unavailable(
            source,
            "usage source is not selectable for this access route",
        );
    }
    let observed_at = usage.observed_at;
    let freshness = freshness(observed_at, max_age, now);
    let expires_at = observed_at.map(|observed| observed + max_age);
    let windows = usage
        .scopes
        .iter()
        .flat_map(scope_windows)
        .collect::<Vec<_>>();
    let availability = if windows.is_empty()
        && usage.credits.is_none()
        && rate_limit_reset_credits.is_none()
        && individual_limits.is_empty()
    {
        AccessAvailability::Unavailable
    } else {
        AccessAvailability::Available
    };
    AccessRouteSnapshot {
        source,
        availability,
        freshness,
        provenance: provenance_from_label(&usage.provenance_source),
        observed_at,
        fetched_at: Some(fetched_at),
        expires_at,
        windows,
        credits: usage.credits.clone(),
        rate_limit_reset_credits,
        individual_limits,
        extra_usage: None,
        local_history: LocalHistoryCapability::default(),
        error: None,
        unavailable_reason: None,
        usage: Some(usage),
    }
}

fn sanitize_unproved_plan(source: &mut AccessSource) {
    if matches!(source.proof, AccessProof::None) {
        source.plan = None;
    }
}

/// Map Claude's provider-specific response into the core snapshot once, then
/// attach the extra-usage billing metadata that is not a quota window.
pub fn claude_route_from_usage(
    source: AccessSource,
    usage: &UsageData,
    observed_at: DateTime<Utc>,
    fetched_at: DateTime<Utc>,
    max_age: Duration,
    now: DateTime<Utc>,
    provenance: impl Into<String>,
) -> AccessRouteSnapshot {
    let provenance_label = provenance.into();
    let structured_scopes = claude_structured_scopes(&usage.limits);
    let (scopes, sonnet_index) = if structured_scopes.is_empty() {
        let mut windows = vec![QuotaWindow {
            window_minutes: 300,
            used_percent: usage.five_hour.utilization,
            remaining_percent: (100.0 - usage.five_hour.utilization).clamp(0.0, 100.0),
            resets_at: usage.five_hour.resets_at,
        }];
        windows.push(QuotaWindow {
            window_minutes: 10_080,
            used_percent: usage.seven_day.utilization,
            remaining_percent: (100.0 - usage.seven_day.utilization).clamp(0.0, 100.0),
            resets_at: usage.seven_day.resets_at,
        });
        let sonnet_index = usage.sonnet_free.as_ref().map(|sonnet| {
            windows.push(QuotaWindow {
                window_minutes: 10_080,
                used_percent: sonnet.utilization,
                remaining_percent: (100.0 - sonnet.utilization).clamp(0.0, 100.0),
                resets_at: sonnet.resets_at,
            });
            windows.len() - 1
        });
        (
            vec![QuotaScope {
                id: Some("claude".to_string()),
                name: Some("Account quota".to_string()),
                kind: RateLimitScope::GlobalAccount,
                windows,
            }],
            sonnet_index,
        )
    } else {
        (merge_claude_legacy_scopes(structured_scopes, usage), None)
    };
    let core_usage = UsageSnapshot {
        source: UsageSource::new(
            "claude-subscription:default",
            [UsageSignal::ClaudeSubscriptionUsage],
        ),
        scopes,
        credits: None,
        observed_at: Some(observed_at),
        provenance_source: provenance_label.clone(),
    };
    let mut route = access_route_from_usage(source, core_usage, fetched_at, max_age, now);
    route.provenance = provenance_from_label(&provenance_label);
    if let Some(index) = sonnet_index
        && let Some(window) = route.windows.get_mut(index)
    {
        window.key = "sonnet_free".to_string();
        window.label = Some("Sonnet".to_string());
    }
    for window in &mut route.windows {
        if window.label.as_deref() == Some("Sonnet") {
            window.key = "sonnet_free".to_string();
        }
    }
    route.extra_usage = usage.extra_usage.as_ref().map(ExtraUsageSnapshot::from);
    route
}

fn merge_claude_legacy_scopes(mut scopes: Vec<QuotaScope>, usage: &UsageData) -> Vec<QuotaScope> {
    let global_index = scopes
        .iter()
        .position(|scope| scope.kind == RateLimitScope::GlobalAccount);
    let global_index = global_index.unwrap_or_else(|| {
        scopes.push(QuotaScope {
            id: Some("claude:legacy".to_string()),
            name: Some("Account quota".to_string()),
            kind: RateLimitScope::GlobalAccount,
            windows: Vec::new(),
        });
        scopes.len() - 1
    });
    let has_five_hour = scopes.iter().any(|scope| {
        scope.kind == RateLimitScope::GlobalAccount
            && scope
                .windows
                .iter()
                .any(|window| window.window_minutes == 300)
    });
    let has_weekly = scopes.iter().any(|scope| {
        scope.kind == RateLimitScope::GlobalAccount
            && scope
                .windows
                .iter()
                .any(|window| window.window_minutes == 10_080)
    });
    let global = &mut scopes[global_index];
    if !has_five_hour {
        global.windows.push(QuotaWindow {
            window_minutes: 300,
            used_percent: usage.five_hour.utilization,
            remaining_percent: (100.0 - usage.five_hour.utilization).clamp(0.0, 100.0),
            resets_at: usage.five_hour.resets_at,
        });
    }
    if !has_weekly {
        global.windows.push(QuotaWindow {
            window_minutes: 10_080,
            used_percent: usage.seven_day.utilization,
            remaining_percent: (100.0 - usage.seven_day.utilization).clamp(0.0, 100.0),
            resets_at: usage.seven_day.resets_at,
        });
    }

    if let Some(sonnet) = usage.sonnet_free.as_ref()
        && !scopes.iter().any(|scope| {
            scope.kind == RateLimitScope::ModelScoped
                && scope
                    .name
                    .as_deref()
                    .is_some_and(|name| name.trim().eq_ignore_ascii_case("sonnet"))
        })
    {
        scopes.push(QuotaScope {
            id: Some("claude:legacy:sonnet_free".to_string()),
            name: Some("Sonnet".to_string()),
            kind: RateLimitScope::ModelScoped,
            windows: vec![QuotaWindow {
                window_minutes: 10_080,
                used_percent: sonnet.utilization,
                remaining_percent: (100.0 - sonnet.utilization).clamp(0.0, 100.0),
                resets_at: sonnet.resets_at,
            }],
        });
    }
    scopes
}

/// Convert the provider's current structured limits response without assuming
/// a fixed model list. Unknown kinds remain visible as named scopes; entries
/// without a finite percentage are omitted instead of becoming fake zeros.
fn claude_structured_scopes(limits: &[UsageLimit]) -> Vec<QuotaScope> {
    limits
        .iter()
        .enumerate()
        .filter_map(|(index, limit)| {
            let used_percent = limit.percent?;
            if !used_percent.is_finite() {
                return None;
            }
            let used_percent = used_percent.clamp(0.0, 100.0);
            let kind = limit.kind.as_deref().unwrap_or("unknown");
            let (window_minutes, scope_kind, name) = match kind {
                "session" => (300, RateLimitScope::GlobalAccount, None),
                "weekly_all" => (10_080, RateLimitScope::GlobalAccount, None),
                "weekly_scoped" => {
                    let name = limit
                        .scope
                        .as_ref()
                        .and_then(|scope| scope.model.as_ref())
                        .and_then(|model| model.display_name.as_deref().or(model.id.as_deref()))
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(ToString::to_string)?;
                    (10_080, RateLimitScope::ModelScoped, Some(name))
                }
                other => (0, RateLimitScope::Other, Some(other.trim().to_string())),
            };
            Some(QuotaScope {
                id: Some(format!("claude:{kind}:{index}")),
                name,
                kind: scope_kind,
                windows: vec![QuotaWindow {
                    window_minutes,
                    used_percent,
                    remaining_percent: (100.0 - used_percent).clamp(0.0, 100.0),
                    resets_at: limit.resets_at,
                }],
            })
        })
        .collect()
}

pub fn subscription_source(provider: &str, plan: Option<String>) -> AccessSource {
    let (kind, id, auth_method) = match provider {
        "codex" => (
            AccessSourceKind::CodexSubscription,
            "codex-subscription:default",
            AuthMethod::AppServer,
        ),
        "claude" => (
            AccessSourceKind::ClaudeSubscription,
            "claude-subscription:default",
            AuthMethod::OAuth,
        ),
        other => panic!("unsupported subscription provider {other:?}"),
    };
    AccessSource {
        id: id.to_string(),
        kind,
        provider: provider.to_string(),
        auth_method,
        proof: AccessProof::None,
        plan,
    }
}

pub fn api_source(provider: &str, auth_method: AuthMethod, proof: AccessProof) -> AccessSource {
    let (kind, id, canonical_provider) = match provider {
        "anthropic" => (
            AccessSourceKind::AnthropicApi,
            "anthropic-api:configured",
            "anthropic",
        ),
        "openai" => (
            AccessSourceKind::OpenAiApi,
            "openai-api:configured",
            "openai",
        ),
        other => panic!("unsupported API provider {other:?}"),
    };
    AccessSource {
        id: id.to_string(),
        kind,
        provider: canonical_provider.to_string(),
        auth_method,
        proof,
        plan: None,
    }
}

/// API routes are fail-closed: a configured key is not evidence of a working
/// route until an authenticated probe or quota response has succeeded.
pub fn visible_routes(routes: Vec<AccessRouteSnapshot>) -> Vec<AccessRouteSnapshot> {
    routes
        .into_iter()
        .filter(|route| {
            !matches!(
                route.source.kind,
                AccessSourceKind::OpenAiApi | AccessSourceKind::AnthropicApi
            ) || route.is_authenticated()
        })
        .collect()
}

pub fn displayable_window_percent(
    route: &AccessRouteSnapshot,
    window: &AccessWindow,
) -> Option<f64> {
    (route.availability == AccessAvailability::Available
        && route.freshness == AccessFreshness::Fresh
        && window.quota.used_percent.is_finite()
        && (0.0..=100.0).contains(&window.quota.used_percent))
    .then_some(window.quota.used_percent)
}

pub fn window_label(window: &AccessWindow) -> String {
    window
        .label
        .clone()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| format_window_label(window.quota.window_minutes))
}

fn scope_windows(scope: &QuotaScope) -> Vec<AccessWindow> {
    let multiple_windows = scope.windows.len() > 1;
    let scope_label = semantic_scope_label(scope.name.as_deref());
    scope
        .windows
        .iter()
        .enumerate()
        .map(|(index, quota)| AccessWindow {
            key: window_key(scope, quota, index, multiple_windows),
            label: scope_label.as_ref().map(|label| {
                if multiple_windows {
                    format!("{label} · {}", format_window_label(quota.window_minutes))
                } else {
                    label.clone()
                }
            }),
            quota: quota.clone(),
        })
        .collect()
}

fn window_key(
    scope: &QuotaScope,
    quota: &QuotaWindow,
    index: usize,
    multiple_windows: bool,
) -> String {
    if let Some(scope_key) = semantic_scope_key(scope) {
        return if multiple_windows {
            format!("{scope_key}_{}", duration_key(quota.window_minutes, index))
        } else {
            scope_key
        };
    }
    duration_key(quota.window_minutes, index)
}

fn semantic_scope_key(scope: &QuotaScope) -> Option<String> {
    let candidates = if scope.kind == RateLimitScope::ModelScoped {
        [scope.name.as_deref(), scope.id.as_deref()]
    } else {
        [scope.name.as_deref(), None]
    };
    candidates.into_iter().flatten().find_map(|value| {
        let normalized = value.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        (!normalized.is_empty() && !is_generic_scope_name(&normalized)).then_some(normalized)
    })
}

fn duration_key(window_minutes: u64, index: usize) -> String {
    match window_minutes {
        300 => "five_hour".to_string(),
        10_080 => "weekly".to_string(),
        minutes => format!("window_{minutes}_{index}"),
    }
}

fn semantic_scope_label(name: Option<&str>) -> Option<String> {
    let name = name?.trim();
    if name.is_empty() {
        return None;
    }
    let normalized = name.to_ascii_lowercase().replace([' ', '-'], "_");
    (!is_generic_scope_name(&normalized)).then(|| name.to_string())
}

fn is_generic_scope_name(normalized: &str) -> bool {
    normalized == "account"
        || normalized == "global"
        || normalized == "global_quota"
        || normalized.starts_with("account_quota")
        || normalized.starts_with("global_account_quota")
}

fn freshness(
    observed_at: Option<DateTime<Utc>>,
    max_age: Duration,
    now: DateTime<Utc>,
) -> AccessFreshness {
    let Some(observed_at) = observed_at else {
        return AccessFreshness::Unknown;
    };
    if observed_at > now {
        return AccessFreshness::Unknown;
    }
    if now - observed_at <= max_age {
        AccessFreshness::Fresh
    } else {
        AccessFreshness::Stale
    }
}

fn provenance_from_label(label: &str) -> AccessProvenance {
    let normalized = label.trim().to_ascii_lowercase();
    if normalized.contains("app-server") || normalized.contains("app_server") {
        AccessProvenance::AppServer
    } else if normalized.contains("jsonl") {
        AccessProvenance::SessionJsonl
    } else if normalized.contains("cache") || normalized.contains("oauth") {
        // UsageManager's cache origin is intentionally passed as a diagnostic
        // label; callers that pass OAuth still identify it as provider API.
        if normalized.contains("cache") {
            AccessProvenance::MemoryCache
        } else {
            AccessProvenance::ProviderApi
        }
    } else if normalized.contains("api") || normalized.contains("anthropic") {
        AccessProvenance::ProviderApi
    } else {
        AccessProvenance::None
    }
}

fn usage_source_matches(source: &AccessSource, usage_source: &UsageSource) -> bool {
    usage_source.is_selectable()
        && usage_source.stream_id == source.id
        && source_lane(source.kind) == Some(usage_source.lane)
}

fn source_lane(kind: AccessSourceKind) -> Option<UsageLane> {
    match kind {
        AccessSourceKind::OpenCodeLocal
        | AccessSourceKind::OpenCodeGo
        | AccessSourceKind::CommandCodeLocal
        | AccessSourceKind::CommandCodeSubscription => None,
        AccessSourceKind::CodexSubscription => Some(UsageLane::CodexSubscription),
        AccessSourceKind::OpenAiApi => Some(UsageLane::OpenAiApi),
        AccessSourceKind::ClaudeSubscription => Some(UsageLane::ClaudeSubscription),
        AccessSourceKind::AnthropicApi => Some(UsageLane::AnthropicApi),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cc_discord_presence::usage::{UsageLimitModel, UsageLimitScope};

    #[test]
    fn api_source_ids_remain_provider_specific() {
        let openai = api_source("openai", AuthMethod::ApiKey, AccessProof::None);
        let anthropic = api_source("anthropic", AuthMethod::ApiKey, AccessProof::None);
        assert_eq!(openai.id, "openai-api:configured");
        assert_eq!(anthropic.id, "anthropic-api:configured");
        assert_ne!(openai.kind, anthropic.kind);
    }

    #[test]
    fn anonymous_weekly_scoped_limit_is_not_promoted_to_a_model_window() {
        let limits = [UsageLimit {
            kind: Some("weekly_scoped".to_string()),
            percent: Some(42.0),
            resets_at: None,
            scope: None,
        }];

        assert!(
            claude_structured_scopes(&limits).is_empty(),
            "a model-scoped window needs provider-reported model identity"
        );
    }

    #[test]
    fn structured_model_window_preserves_used_and_complement_percentages() {
        let limits = [UsageLimit {
            kind: Some("weekly_scoped".to_string()),
            percent: Some(68.0),
            resets_at: None,
            scope: Some(UsageLimitScope {
                model: Some(UsageLimitModel {
                    display_name: Some("Fable".to_string()),
                    id: Some("fable".to_string()),
                }),
            }),
        }];

        let scopes = claude_structured_scopes(&limits);
        let window = &scopes[0].windows[0];
        assert_eq!(window.used_percent, 68.0);
        assert_eq!(window.remaining_percent, 32.0);
    }
}
