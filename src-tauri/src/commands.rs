use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use cc_discord_presence::codex::account_usage::{
    AccountUsageManager, AccountUsageReading, effective_limits_from_envelopes, fresh_envelopes,
};
use cc_discord_presence::codex::config::{
    DesktopPresenceDesign, PresenceConfig as CodexPresenceConfig, PresenceSurface,
};
use cc_discord_presence::codex::discord::{
    DiscordPresence as CodexDiscordPresence, active_presence_presentation,
    idle_presence_presentation,
};
use cc_discord_presence::codex::model::SpeedMode as CodexSpeedMode;
use cc_discord_presence::codex::session::{
    self as codex_session, CodexSessionSnapshot, GitBranchCache as CodexGitBranchCache,
    SessionParseCache as CodexSessionParseCache,
};
use cc_discord_presence::codex::telemetry::limits::{RateLimits, UsageWindow};
use cc_discord_presence::codex::telemetry::plan::{DetectedPlanTier, PlanDetector};
use cc_discord_presence::codex::telemetry::service_tier::resolve_service_tier;
use cc_discord_presence::commandcode::{self, Config as CommandCodeConfig};
use cc_discord_presence::config::PresenceConfig;
use cc_discord_presence::cost;
use cc_discord_presence::discord::DiscordPresence as ClaudeDiscordPresence;
use cc_discord_presence::discord::presence_lines as claude_presence_lines;
use cc_discord_presence::opencode::{self, Config as OpenCodeConfig};
use cc_discord_presence::provider::Provider;
use cc_discord_presence::session::{
    self, ClaudeSessionSnapshot, GitBranchCache, RateLimits as ClaudeRateLimits, SessionParseCache,
    UsageWindowLimits as ClaudeUsageWindowLimits, merge_statusline_into_sessions,
    preferred_active_session, read_statusline_data,
};
use cc_discord_presence::usage::{UsageAuth, UsageManager};
use codex_presence_core::{
    CreditBalance, PresenceFieldId, QuotaScope, QuotaWindow, RateLimitScope,
    RateLimits as CoreRateLimits, UsageSignal, UsageSnapshot, UsageSource,
    usage_snapshot_from_envelopes,
};
use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::access::{
    AccessAvailability, AccessFreshness, AccessProof, AccessProvenance, AccessRouteSnapshot,
    AccessSnapshot, AccessWindow, AuthMethod, access_route_from_usage,
    access_route_from_usage_with_account_details, api_source, claude_route_from_usage,
    subscription_source, window_label,
};
use crate::live::PublisherLease;
use crate::notifications::{NotificationRecord, NotificationStore, QuotaResetObservation};

const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const STALE_THRESHOLD: Duration = Duration::from_secs(120);
const STICKY_WINDOW: Duration = Duration::from_secs(120);
const ACTIVE_CUTOFF: Duration = Duration::from_secs(600);
const IDLE_CUTOFF: Duration = Duration::from_secs(300);
const APP_SNAPSHOT_CACHE_SCHEMA: u32 = 1;
const APP_SNAPSHOT_CACHE_MAX_BYTES: u64 = 16 * 1024 * 1024;
const DISCORD_USER_CACHE_TTL: Duration = Duration::from_secs(60);
const REPORTS_BUNDLE_CACHE_TTL: Duration = Duration::from_secs(60);

static START_TIME: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
static DISCORD_USER_CACHE: Mutex<Option<(Instant, Option<DiscordUserInfo>)>> = Mutex::new(None);
#[allow(clippy::type_complexity)]
static REPORTS_BUNDLE_CACHE: Mutex<
    Option<(Instant, (u32, Option<String>, String), ReportsBundle)>,
> = Mutex::new(None);
static SNAPSHOT_POLLER_STARTED: AtomicBool = AtomicBool::new(false);
static INITIAL_SNAPSHOT_READY: AtomicBool = AtomicBool::new(false);
static SESSION_FINGERPRINTS: std::sync::OnceLock<Mutex<HashMap<String, u64>>> =
    std::sync::OnceLock::new();
fn uptime_secs() -> u64 {
    START_TIME.get_or_init(Instant::now).elapsed().as_secs()
}

fn publish_notification(app: Option<&tauri::AppHandle>, record: Option<NotificationRecord>) {
    let Some(record) = record else {
        return;
    };
    let Some(app) = app else {
        // The standalone browser bridge keeps the durable notification record
        // but has no Tauri runtime to deliver native events through.
        return;
    };
    if let Err(error) = crate::notifications::send_native_notification(app, &record) {
        tracing::debug!(error = %error, id = record.id, "native notification delivery unavailable");
    }
    if let Err(error) = app.emit("pulse://notification", &record) {
        tracing::warn!(error = %error, id = record.id, "failed to emit Pulse notification");
    }
}

fn observe_access_notifications(
    app: Option<&tauri::AppHandle>,
    store: Option<&NotificationStore<'_>>,
    provider: Provider,
    route: &AccessRouteSnapshot,
) {
    let Some(store) = store else {
        return;
    };
    let provider_name = provider.as_str();
    // Native notifications are intentionally limited to genuine quota-reset
    // edges. Provider health, quota thresholds, and Discord connectivity are
    // observed at the 5-second poll cadence on signals that legitimately flap
    // (a "pending" account read, a percentage hovering at a threshold, a
    // reconnecting IPC), so alerting on each transition spams the user with
    // duplicate, incoherent toasts. They stay diagnostics in the access
    // snapshot and never become native alerts.
    let now = chrono::Utc::now();
    if route.availability != AccessAvailability::Available
        || route.freshness != AccessFreshness::Fresh
        || !route.is_authenticated()
    {
        return;
    }
    for window in &route.windows {
        let label = window_label(window);
        match store.observe_quota_reset_transition_for_window(QuotaResetObservation {
            provider: provider_name,
            window_identity: &window.key,
            window_label: &label,
            used_percent: window.quota.used_percent,
            remaining_percent: window.quota.remaining_percent,
            reset_at: window.quota.resets_at,
            observed_at: now,
        }) {
            Ok(record) => publish_notification(app, record),
            Err(error) => {
                tracing::warn!(provider = provider_name, window = %window.key, error = %error, "failed to record quota reset notification")
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct DiscordDisplayPrefs {
    pub show_project: bool,
    pub show_branch: bool,
    pub show_model: bool,
    pub show_activity: bool,
    pub show_tokens: bool,
    pub show_cost: bool,
    pub show_limits: bool,
    pub show_credits: bool,
    pub show_context: bool,
    pub show_systems: bool,
}

impl Default for DiscordDisplayPrefs {
    fn default() -> Self {
        Self {
            show_project: true,
            show_branch: true,
            show_model: true,
            show_activity: true,
            show_tokens: false,
            show_cost: false,
            show_limits: true,
            show_credits: true,
            show_context: true,
            show_systems: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiscordSettings {
    pub provider: String,
    pub enabled: bool,
    pub status: String,
    pub publisher: String,
    pub display_prefs: DiscordDisplayPrefs,
    pub desktop_design: Option<String>,
    pub supports_desktop_design: bool,
    pub supports_field_order: bool,
    /// False for Claude: "Credits available" is a Codex account-balance field
    /// with no Claude equivalent, so the backend pins it off. The UI disables the
    /// row instead of offering a switch that silently snaps back.
    pub supports_credits: bool,
    pub field_order: Vec<String>,
}

#[derive(Default, Clone)]
enum ActiveSessions {
    #[default]
    None,
    Claude(Vec<ClaudeSessionSnapshot>),
    Codex(Vec<CodexSessionSnapshot>),
}

#[derive(Default, Clone)]
struct CachedData {
    active_provider: Provider,
    opencode_sessions: Vec<opencode::Session>,
    commandcode_sessions: Vec<commandcode::Session>,
    commandcode_diagnostics: Vec<String>,
    live_claude: Vec<ClaudeSessionSnapshot>,
    live_codex: Vec<CodexSessionSnapshot>,
    opencode_diagnostics: Vec<String>,
    sessions: ActiveSessions,
    claude_usage: Option<CachedUsage>,
    claude_usage_data: Option<cc_discord_presence::usage::UsageData>,
    claude_usage_error: Option<String>,
    codex_usage: Option<UsageSnapshot>,
    codex_plan_envelopes: Vec<cc_discord_presence::codex::telemetry::limits::RateLimitEnvelope>,
    codex_limits: Option<codex_session::EffectiveLimitSelection>,
    codex_usage_error: Option<String>,
    /// Canonical provider access routes. All quota DTOs and presence gates
    /// derive from this snapshot rather than reconstructing windows per seam.
    access: AccessSnapshot,
    discord_status: String,
    discord_publisher: String,
    discord_enabled: bool,
    discord_prefs: DiscordDisplayPrefs,
    codex_opencode_running: bool,
    codex_desktop_surface_running: bool,
    /// One-shot flag: when set by `refresh_usage` command, the background
    /// poller invalidates its usage cache on the next tick and forces a
    /// fresh API call. The flag is cleared after handling.
    usage_refresh_requested: bool,
}

#[derive(Clone)]
struct CachedUsage {
    five_hour_pct: f64,
    seven_day_pct: f64,
    extra_enabled: bool,
    extra_limit: Option<f64>,
    extra_used: Option<f64>,
    extra_pct: Option<f64>,
    /// Provenance observed by `UsageManager` when these numbers were produced —
    /// the OAuth handshake or the on-disk cache. Carried through so the UI can
    /// state how it knows, instead of the old hard-coded `api` label.
    source: String,
}

/// A bounded API probe is deliberately separate from subscription polling.
/// A provider key's presence is configuration, not authentication proof; the
/// route only becomes proofed after the provider accepts the request.
const API_PROBE_TTL: Duration = Duration::from_secs(300);
const API_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Default)]
struct ApiProbeCache {
    openai: ApiProbeEntry,
    anthropic: ApiProbeEntry,
}

#[derive(Default)]
struct ApiProbeEntry {
    key_fingerprint: Option<u64>,
    checked_at: Option<Instant>,
    route: Option<AccessRouteSnapshot>,
}

/// Build a stable, non-secret cache key for an API credential. The key itself
/// never enters logs, snapshots, or diagnostics.
fn api_key_fingerprint(key: Option<&str>) -> Option<u64> {
    let key = key?.trim();
    if key.is_empty() {
        return None;
    }
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    Some(hasher.finish())
}

fn api_probe_route(
    provider: &'static str,
    key: Option<&str>,
    cache: &mut ApiProbeEntry,
) -> AccessRouteSnapshot {
    let fingerprint = api_key_fingerprint(key);
    if cache.key_fingerprint == fingerprint
        && cache
            .checked_at
            .is_some_and(|checked_at| checked_at.elapsed() < API_PROBE_TTL)
        && let Some(route) = cache.route.clone()
    {
        return route;
    }

    let route = match key.map(str::trim).filter(|key| !key.is_empty()) {
        None => AccessRouteSnapshot::unavailable(
            api_source(provider, AuthMethod::ApiKey, AccessProof::None),
            "API key not configured",
        ),
        Some(key) => probe_api_endpoint(provider, key),
    };
    cache.key_fingerprint = fingerprint;
    cache.checked_at = Some(Instant::now());
    cache.route = Some(route.clone());
    route
}

/// Probe only the provider's authentication surface. `/models` intentionally
/// supplies no fabricated quota windows: an API route is proofed when the
/// request succeeds, while allowance windows remain absent until that API
/// actually reports them.
fn probe_api_endpoint(provider: &'static str, key: &str) -> AccessRouteSnapshot {
    let agent = ureq::AgentBuilder::new().timeout(API_PROBE_TIMEOUT).build();
    let mut request = match provider {
        "anthropic" => agent
            .get("https://api.anthropic.com/v1/models")
            .set("x-api-key", key)
            .set("anthropic-version", "2023-06-01"),
        _ => agent
            .get("https://api.openai.com/v1/models")
            .set("Authorization", &format!("Bearer {key}")),
    };
    request = request.set("User-Agent", "cc-discord-presence");

    match request.call() {
        Ok(response) if (200..300).contains(&response.status()) => {
            let source = api_source(
                provider,
                AuthMethod::ApiKey,
                AccessProof::AuthenticatedProbe,
            );
            AccessRouteSnapshot::unavailable(
                source,
                "authenticated probe succeeded; provider reported no quota windows",
            )
        }
        Ok(response) => AccessRouteSnapshot::unavailable(
            api_source(provider, AuthMethod::ApiKey, AccessProof::None),
            format!("provider rejected API probe (HTTP {})", response.status()),
        ),
        Err(ureq::Error::Status(status, _)) => AccessRouteSnapshot::unavailable(
            api_source(provider, AuthMethod::ApiKey, AccessProof::None),
            format!("provider rejected API probe (HTTP {status})"),
        ),
        Err(_) => AccessRouteSnapshot::unavailable(
            api_source(provider, AuthMethod::ApiKey, AccessProof::None),
            "provider API probe failed",
        ),
    }
}

fn merge_access_routes(
    base: &[AccessRouteSnapshot],
    replacements: impl IntoIterator<Item = AccessRouteSnapshot>,
) -> AccessSnapshot {
    let mut routes = base.to_vec();
    for replacement in replacements {
        if let Some(existing) = routes
            .iter_mut()
            .find(|route| route.source.id == replacement.source.id)
        {
            *existing = replacement;
        } else {
            routes.push(replacement);
        }
    }
    // The provider lanes are a small fixed inventory. Keep one route per source
    // so a failed probe replaces, rather than leaves behind, old windows.
    routes.sort_by(|left, right| left.source.id.cmp(&right.source.id));
    AccessSnapshot { routes }
}

fn route_for_provider(data: &CachedData, provider: Provider) -> Option<AccessRouteSnapshot> {
    data.access
        .routes
        .iter()
        .find(|route| {
            route.source.provider == provider.as_str()
                && matches!(
                    (provider, route.source.kind),
                    (
                        Provider::Codex,
                        crate::access::AccessSourceKind::CodexSubscription
                    ) | (
                        Provider::Claude,
                        crate::access::AccessSourceKind::ClaudeSubscription
                    )
                )
        })
        .cloned()
}

fn claude_route_from_probe(
    usage_mgr: &mut UsageManager,
    force_refresh: bool,
) -> (
    AccessRouteSnapshot,
    Option<cc_discord_presence::usage::UsageData>,
    Option<CachedUsage>,
    Option<String>,
    Option<String>,
) {
    if force_refresh {
        usage_mgr.invalidate_cache();
        let usage_cache_path = cc_discord_presence::config::usage_cache_path();
        if let Err(error) = std::fs::remove_file(&usage_cache_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                path = %usage_cache_path.display(),
                error = %error,
                "failed to remove usage cache"
            );
        }
    }

    let usage = trusted_claude_usage(usage_mgr);
    let usage_source = usage_mgr
        .last_usage_origin()
        .map(|origin| origin.label())
        .unwrap_or_else(|| "unknown source".to_string());
    let detected_plan_key = usage_mgr.detected_plan_key();
    let cached_usage = usage.as_ref().map(|usage| CachedUsage {
        five_hour_pct: usage.five_hour.utilization,
        seven_day_pct: usage.seven_day.utilization,
        extra_enabled: usage
            .extra_usage
            .as_ref()
            .is_some_and(|extra| extra.is_enabled),
        extra_limit: usage
            .extra_usage
            .as_ref()
            .and_then(|extra| extra.monthly_limit),
        extra_used: usage
            .extra_usage
            .as_ref()
            .and_then(|extra| extra.used_credits),
        extra_pct: usage
            .extra_usage
            .as_ref()
            .and_then(|extra| extra.utilization),
        source: usage_source.clone(),
    });
    let usage_error = usage_mgr.error_hint_with_countdown();
    let observed_at = usage_mgr
        .last_usage_observed_at()
        .unwrap_or_else(chrono::Utc::now);
    let route = match usage.as_ref() {
        Some(usage) => {
            let mut source = subscription_source("claude", detected_plan_key.clone());
            let origin_is_oauth = usage_mgr
                .last_usage_origin()
                .is_some_and(|origin| matches!(origin.auth, UsageAuth::OAuth));
            if origin_is_oauth {
                source.proof = AccessProof::QuotaResponse;
            } else if usage_mgr.has_valid_oauth_credentials() {
                // Numbers came from Pulse's local cache, but the account is still
                // authenticated — report authenticated rather than signed out.
                source.proof = AccessProof::AuthenticatedProbe;
            }
            claude_route_from_usage(
                source,
                usage,
                observed_at,
                chrono::Utc::now(),
                chrono::Duration::seconds(300),
                chrono::Utc::now(),
                usage_source,
            )
        }
        None => AccessRouteSnapshot::unavailable(
            subscription_source("claude", detected_plan_key.clone()),
            usage_error.clone().unwrap_or_else(|| {
                "Claude quota requires an authenticated OAuth response; local credentials are missing or invalid"
                    .to_string()
            }),
        ),
    };
    (route, usage, cached_usage, usage_error, detected_plan_key)
}

/// Cache/session data can be useful diagnostics but is not provider proof. The
/// Claude subscription route therefore exposes metrics only when the usage
/// manager recorded a successful OAuth response for those exact figures.
fn trusted_claude_usage(
    usage_mgr: &mut UsageManager,
) -> Option<cc_discord_presence::usage::UsageData> {
    let usage = usage_mgr.get_usage();
    let origin_is_oauth = usage_mgr
        .last_usage_origin()
        .is_some_and(|origin| matches!(origin.auth, UsageAuth::OAuth));
    // Pulse's own short-lived usage cache is written only from a live OAuth 200,
    // so it stays trustworthy while the same account's credentials remain valid.
    // Only a fully unauthenticated read — no OAuth origin and no unexpired token
    // — is dropped, which keeps a missing/expired credential at "sign in
    // required" instead of collapsing a validly signed-in account to it.
    let authenticated = origin_is_oauth || usage_mgr.has_valid_oauth_credentials();
    usage.filter(|_| authenticated)
}

fn codex_route_from_probe(
    latest: &Arc<Mutex<Option<Result<AccountUsageReading, String>>>>,
    plan_detector: &mut PlanDetector,
    plan_config: &cc_discord_presence::codex::config::OpenAiPlanDisplayConfig,
) -> (AccessRouteSnapshot, Option<UsageSnapshot>, Option<String>) {
    let account_usage = latest
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_else(|| Err("Codex account quota read is pending".to_string()));
    match account_usage {
        Ok(reading) => {
            let resolved_plan =
                plan_detector.resolve_from_envelopes(&reading.plan_envelopes(), plan_config);
            let plan_key = codex_plan_key_from_tier(resolved_plan.tier);
            let usage = reading.usage_snapshot();
            let reset_credits = reading.rate_limit_reset_credits.clone();
            let individual_limits = reading.individual_limits.clone();
            let mut source = subscription_source(
                "codex",
                (!plan_key.is_empty()).then(|| plan_key.to_string()),
            );
            source.proof = if reading.envelopes.is_empty() {
                AccessProof::AuthenticatedProbe
            } else {
                AccessProof::QuotaResponse
            };
            let route = access_route_from_usage_with_account_details(
                source,
                usage.clone(),
                reset_credits,
                individual_limits,
                chrono::Utc::now(),
                chrono::Duration::seconds(30),
                chrono::Utc::now(),
            );
            (route, Some(usage), None)
        }
        Err(error) => {
            let message = format!("Codex account API unavailable: {error}");
            (
                AccessRouteSnapshot::unavailable(subscription_source("codex", None), &message),
                None,
                Some(message),
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn independently_probe_access(
    usage_mgr: &mut UsageManager,
    codex_usage_trigger: &std::sync::mpsc::SyncSender<()>,
    codex_usage_force: &AtomicBool,
    codex_usage_latest: &Arc<Mutex<Option<Result<AccountUsageReading, String>>>>,
    api_probe_cache: &mut ApiProbeCache,
    force_refresh: bool,
    active_provider: Provider,
    codex_plan_detector: &mut PlanDetector,
    codex_plan_config: &cc_discord_presence::codex::config::OpenAiPlanDisplayConfig,
) -> Vec<AccessRouteSnapshot> {
    if force_refresh {
        codex_usage_force.store(true, Ordering::Release);
    }
    // Trigger the app-server lane on every tick, regardless of which provider
    // currently owns the UI/presence surface.
    let _ = codex_usage_trigger.try_send(());

    let refresh_claude = force_refresh && active_provider != Provider::Claude;
    let (claude_route, _, _, _, _) = claude_route_from_probe(usage_mgr, refresh_claude);
    let (codex_route, _, _) =
        codex_route_from_probe(codex_usage_latest, codex_plan_detector, codex_plan_config);
    let openai_route = api_probe_route(
        "openai",
        std::env::var("OPENAI_API_KEY").ok().as_deref(),
        &mut api_probe_cache.openai,
    );
    let anthropic_route = api_probe_route(
        "anthropic",
        std::env::var("ANTHROPIC_API_KEY").ok().as_deref(),
        &mut api_probe_cache.anthropic,
    );
    vec![codex_route, claude_route, openai_route, anthropic_route]
}

static SHARED: std::sync::OnceLock<Arc<Mutex<CachedData>>> = std::sync::OnceLock::new();

fn shared() -> &'static Arc<Mutex<CachedData>> {
    SHARED.get_or_init(|| Arc::new(Mutex::new(CachedData::default())))
}

fn current_provider() -> Provider {
    shared()
        .lock()
        .ok()
        .map(|d| d.active_provider)
        .unwrap_or_else(cc_discord_presence::provider::load_active_provider)
}

fn active_access_route(data: &CachedData) -> AccessRouteSnapshot {
    if let Some(route) = route_for_provider(data, data.active_provider) {
        return route.clone();
    }

    match data.active_provider {
        Provider::CommandCode => crate::commandcode::access_route(),
        Provider::OpenCode => AccessRouteSnapshot::unavailable(
            crate::access::AccessSource {
                id: "opencode-local".into(),
                kind: crate::access::AccessSourceKind::OpenCodeLocal,
                provider: "opencode".into(),
                auth_method: AuthMethod::None,
                proof: AccessProof::None,
                plan: None,
            },
            "OpenCode does not report an account quota",
        ),
        Provider::Claude => {
            let source = subscription_source("claude", None);
            let Some(usage) = data.claude_usage.as_ref() else {
                return AccessRouteSnapshot::unavailable(source, "no usage data yet");
            };
            let now = chrono::Utc::now();
            let snapshot = UsageSnapshot {
                source: UsageSource::new(
                    "claude-subscription:default",
                    [UsageSignal::ClaudeSubscriptionUsage],
                ),
                scopes: vec![QuotaScope {
                    id: Some("claude".to_string()),
                    name: Some("Account quota".to_string()),
                    kind: RateLimitScope::GlobalAccount,
                    windows: vec![
                        QuotaWindow {
                            window_minutes: 300,
                            used_percent: usage.five_hour_pct,
                            remaining_percent: (100.0 - usage.five_hour_pct).clamp(0.0, 100.0),
                            resets_at: None,
                        },
                        QuotaWindow {
                            window_minutes: 10_080,
                            used_percent: usage.seven_day_pct,
                            remaining_percent: (100.0 - usage.seven_day_pct).clamp(0.0, 100.0),
                            resets_at: None,
                        },
                    ],
                }],
                credits: None,
                observed_at: Some(now),
                provenance_source: usage.source.clone(),
            };
            let mut route =
                access_route_from_usage(source, snapshot, now, chrono::Duration::seconds(300), now);
            route.extra_usage = Some(crate::access::ExtraUsageSnapshot {
                enabled: usage.extra_enabled,
                limit: usage.extra_limit,
                used: usage.extra_used,
                utilization: usage.extra_pct,
            });
            route
        }
        Provider::Codex => data
            .codex_usage
            .clone()
            .map(|usage| {
                let now = chrono::Utc::now();
                access_route_from_usage(
                    subscription_source("codex", None),
                    usage,
                    now,
                    chrono::Duration::seconds(30),
                    now,
                )
            })
            .unwrap_or_else(|| {
                AccessRouteSnapshot::unavailable(
                    subscription_source("codex", None),
                    data.codex_usage_error
                        .clone()
                        .unwrap_or_else(|| "Codex account quota unavailable".to_string()),
                )
            }),
    }
}

fn route_window_slots(
    route: &AccessRouteSnapshot,
) -> (Option<&AccessWindow>, Option<&AccessWindow>) {
    let primary = route
        .windows
        .iter()
        .find(|window| window.key == "five_hour" || window.key == "primary");
    let secondary = route.windows.iter().find(|window| {
        matches!(window.key.as_str(), "weekly" | "seven_day" | "secondary")
            || window.quota.window_minutes == 10_080
    });
    if primary.is_none()
        && secondary.is_none()
        && route.windows.len() == 1
        && route.windows[0].label.is_none()
    {
        let only = &route.windows[0];
        if only.key.contains("week") || only.key.contains("seven") {
            return (None, Some(only));
        }
        return (Some(only), None);
    }
    // Do not promote a known weekly window into the legacy primary/5h slot
    // merely because another model-scoped weekly window is also present.
    let primary = primary.or_else(|| secondary.is_none().then(|| route.windows.first()).flatten());
    (primary, secondary)
}

/// Adapt the canonical access route to the legacy presence compositor without
/// re-reading session envelopes. A supplied route always yields `Some`: an
/// unavailable or stale route intentionally becomes an empty limit object so
/// Codex's compositor cannot fall back to the session's older limits.
fn route_limits_for_presence(route: Option<&AccessRouteSnapshot>) -> Option<RateLimits> {
    let route = route?;
    let live = route.availability == AccessAvailability::Available
        && route.freshness == AccessFreshness::Fresh;
    if !live {
        return Some(RateLimits::default());
    }
    let (primary, secondary) = route_window_slots(route);
    Some(CoreRateLimits::new(
        [
            primary.and_then(presence_window_from_access),
            secondary.and_then(presence_window_from_access),
        ]
        .into_iter()
        .flatten()
        .collect(),
    ))
}

fn route_limits_for_claude_presence(
    route: Option<&AccessRouteSnapshot>,
) -> Option<ClaudeRateLimits> {
    let route = route?;
    let live = route.availability == AccessAvailability::Available
        && route.freshness == AccessFreshness::Fresh;
    if !live {
        return Some(ClaudeRateLimits::default());
    }
    let (primary, secondary) = route_window_slots(route);
    Some(ClaudeRateLimits {
        primary: primary.and_then(claude_window_from_access),
        secondary: secondary.and_then(claude_window_from_access),
    })
}

fn presence_window_from_access(window: &AccessWindow) -> Option<UsageWindow> {
    let used_percent = window.quota.used_percent;
    let remaining_percent = window.quota.remaining_percent;
    (used_percent.is_finite()
        && remaining_percent.is_finite()
        && (0.0..=100.0).contains(&used_percent)
        && (0.0..=100.0).contains(&remaining_percent))
    .then_some(UsageWindow {
        used_percent,
        remaining_percent,
        window_minutes: window.quota.window_minutes,
        resets_at: window.quota.resets_at,
    })
}

fn claude_window_from_access(window: &AccessWindow) -> Option<ClaudeUsageWindowLimits> {
    let used_percent = window.quota.used_percent;
    let remaining_percent = window.quota.remaining_percent;
    (used_percent.is_finite()
        && remaining_percent.is_finite()
        && (0.0..=100.0).contains(&used_percent)
        && (0.0..=100.0).contains(&remaining_percent))
    .then_some(ClaudeUsageWindowLimits {
        used_percent,
        remaining_percent,
        window_minutes: window.quota.window_minutes,
        resets_at: window.quota.resets_at,
    })
}

fn access_source_label(route: &AccessRouteSnapshot) -> String {
    match route.provenance {
        AccessProvenance::AppServer => "Codex account API".to_string(),
        AccessProvenance::ProviderApi => "Provider API".to_string(),
        AccessProvenance::MemoryCache => "Cached provider response".to_string(),
        AccessProvenance::SessionJsonl => "Session JSONL".to_string(),
        AccessProvenance::None => route
            .error
            .clone()
            .unwrap_or_else(|| "no usage data yet".to_string()),
    }
}

fn rate_limit_info_from_access(route: &AccessRouteSnapshot) -> RateLimitInfo {
    let (primary, secondary) = route_window_slots(route);
    let sonnet = route
        .windows
        .iter()
        .find(|window| window.key == "sonnet_free");
    let live = route.availability == AccessAvailability::Available
        && route.freshness == AccessFreshness::Fresh;
    let usage = live
        .then(|| route.usage.clone())
        .flatten()
        .map(|snapshot| UsageSnapshotTransport::from_snapshot(&route.source.provider, snapshot));
    let extra = route.extra_usage.as_ref();
    RateLimitInfo {
        provider: route.source.provider.clone(),
        usage,
        five_hour_pct: if live {
            primary.map_or(0.0, |window| window.quota.used_percent)
        } else {
            0.0
        },
        five_hour_resets: live
            .then(|| primary.and_then(|window| window.quota.resets_at))
            .flatten()
            .map_or_else(|| "N/A".to_string(), |reset| reset.to_rfc3339()),
        five_hour_label: primary.map_or_else(|| "5-hour window".to_string(), window_label),
        five_hour_window_minutes: primary.map(|window| window.quota.window_minutes),
        seven_day_pct: if live {
            secondary.map_or(0.0, |window| window.quota.used_percent)
        } else {
            0.0
        },
        seven_day_resets: live
            .then(|| secondary.and_then(|window| window.quota.resets_at))
            .flatten()
            .map_or_else(|| "N/A".to_string(), |reset| reset.to_rfc3339()),
        seven_day_label: secondary.map_or_else(|| "All Models".to_string(), window_label),
        seven_day_window_minutes: secondary.map(|window| window.quota.window_minutes),
        sonnet_pct: live
            .then_some(sonnet.map(|window| window.quota.used_percent))
            .flatten(),
        sonnet_resets: live
            .then(|| sonnet.and_then(|window| window.quota.resets_at))
            .flatten()
            .map(|reset| reset.to_rfc3339()),
        extra_enabled: extra.is_some_and(|usage| usage.enabled),
        extra_limit: extra.and_then(|usage| usage.limit),
        extra_used: extra.and_then(|usage| usage.used),
        extra_pct: extra.and_then(|usage| usage.utilization),
        source: access_source_label(route),
        availability: route.availability,
        freshness: route.freshness,
        provenance: route.provenance,
        error: route.error.clone(),
    }
}

fn plan_name_from_key(key: &str) -> String {
    cc_discord_presence::plan::name_from_key(key)
}

fn plan_key_from_override(name: &str) -> Option<&'static str> {
    cc_discord_presence::plan::key_from_override(name)
}

fn codex_plan_key_from_tier(tier: DetectedPlanTier) -> &'static str {
    match tier {
        DetectedPlanTier::Free => "free",
        DetectedPlanTier::Go => "go",
        DetectedPlanTier::Plus => "plus",
        DetectedPlanTier::Business => "business",
        DetectedPlanTier::Enterprise => "enterprise",
        DetectedPlanTier::Edu => "edu",
        DetectedPlanTier::Pro5x => "pro_5x",
        DetectedPlanTier::Pro20x => "pro_20x",
        DetectedPlanTier::Unknown => "",
    }
}

/// Loads the master Rich Presence switch and the field visibility flags from the
/// active provider's on-disk config into the shared cache.
///
/// This is the only thing standing between a saved preference and a fresh
/// process: the Claude arm used to hard-code `discord_enabled = true`, so
/// pausing Rich Presence never survived a restart.
pub(crate) fn seed_discord_state_from_disk() {
    seed_discord_state_for(cc_discord_presence::provider::load_active_provider());
}

/// Seeds the cache for an explicitly named provider.
///
/// A provider switch must seed for the provider the user just chose. Re-reading
/// `pulse-provider.json` here would silently revert to the previous provider
/// whenever that file could not be written, leaving the UI branded for one
/// provider while presence and snapshots served another.
pub(crate) fn seed_discord_state_for(provider: Provider) {
    let Ok(mut data) = shared().lock() else {
        return;
    };
    data.active_provider = provider;
    // Access routes are provider-lane state, not UI selection state. Keep the
    // independently probed lanes while the active provider changes; the
    // selector below still chooses only the current subscription route.
    match data.active_provider {
        Provider::CommandCode => {
            if let Ok(config) = CommandCodeConfig::load() {
                data.discord_enabled = config.enabled;
                data.discord_prefs = crate::commandcode::prefs(&config);
            }
        }
        Provider::OpenCode => {
            if let Ok(config) = OpenCodeConfig::load() {
                data.discord_enabled = config.enabled;
                data.discord_prefs = crate::opencode::prefs(&config);
            }
        }
        Provider::Claude => {
            if let Ok(config) = PresenceConfig::load_or_init() {
                data.discord_enabled = config.presence_enabled;
                data.discord_prefs = claude_display_prefs(&config);
            }
        }
        Provider::Codex => {
            if let Ok(config) = CodexPresenceConfig::load_or_init() {
                data.discord_enabled = config.presence_enabled;
                data.discord_prefs = codex_display_prefs(&config);
            }
        }
    }
}

pub fn start_background_poller(app: tauri::AppHandle) {
    start_background_poller_inner(Some(app));
}

/// Starts the same live session/usage poller for the standalone debug bridge.
///
/// The bridge has no Tauri `AppHandle`, so notification delivery and snapshot
/// events are intentionally omitted; the shared state and durable analytics
/// writes remain identical to the native Pulse process.
#[cfg(debug_assertions)]
pub fn start_background_poller_without_app() {
    start_background_poller_inner(None);
}

fn start_background_poller_inner(app: Option<tauri::AppHandle>) {
    INITIAL_SNAPSHOT_READY.store(false, Ordering::Release);
    SNAPSHOT_POLLER_STARTED.store(true, Ordering::Release);
    let data = Arc::clone(shared());
    let (codex_usage_trigger, codex_usage_requests) = std::sync::mpsc::sync_channel::<()>(1);
    let codex_usage_force = Arc::new(AtomicBool::new(false));
    let codex_usage_latest = Arc::new(Mutex::new(None::<Result<AccountUsageReading, String>>));

    {
        let force = Arc::clone(&codex_usage_force);
        let latest = Arc::clone(&codex_usage_latest);
        thread::spawn(move || {
            let mut manager = AccountUsageManager::read_only();
            while codex_usage_requests.recv().is_ok() {
                let result = manager
                    .get_usage(force.swap(false, Ordering::AcqRel))
                    .map_err(|error| error.to_string());
                if let Ok(mut slot) = latest.lock() {
                    *slot = Some(result);
                }
            }
        });
    }

    seed_discord_state_from_disk();
    let _ = crate::accounts::discover();

    thread::spawn(move || {
        let notification_connection = crate::notifications::open_default_database().ok();
        let notification_store = notification_connection.as_ref().map(NotificationStore::new);
        let mut claude_git = GitBranchCache::new(Duration::from_secs(30));
        let mut claude_parse = SessionParseCache::default();
        let mut codex_git = CodexGitBranchCache::new(Duration::from_secs(30));
        let mut codex_parse = CodexSessionParseCache::default();
        let mut usage_mgr = UsageManager::read_only();
        let mut claude_config = PresenceConfig::load_or_init().unwrap_or_default();
        let mut codex_config = CodexPresenceConfig::load_or_init().unwrap_or_default();
        let mut claude_discord = ClaudeDiscordPresence::new(claude_config.effective_client_id());
        let mut codex_discord = CodexDiscordPresence::new(codex_config.effective_client_id());
        let mut codex_plan_detector = PlanDetector::new();
        let mut claude_publisher = PublisherLease::new(cc_discord_presence::config::lock_path());
        let mut codex_publisher =
            PublisherLease::new(cc_discord_presence::codex::config::lock_path());
        let mut api_probe_cache = ApiProbeCache::default();
        let mut last_snapshot_hash = None;

        let go_usage = crate::opencode_go::start();
        let mut opencode_collector = opencode::Collector::default();
        let mut commandcode_collector = commandcode::Collector::default();
        let mut commandcode_publisher = commandcode::presence::Publisher::default();
        let mut commandcode_lease = PublisherLease::new(
            cc_discord_presence::storage::home().join("pulse-commandcode.lock"),
        );
        let mut commandcode_git = CodexGitBranchCache::new(Duration::from_secs(30));
        let mut opencode_runtime = opencode::process::RuntimeDetector::default();
        let mut opencode_publisher = opencode::presence::Publisher::default();
        let mut opencode_lease =
            PublisherLease::new(cc_discord_presence::storage::home().join("pulse-opencode.lock"));
        let mut opencode_git = CodexGitBranchCache::new(Duration::from_secs(30));
        loop {
            let provider = current_provider();
            let (discord_enabled, prefs, force_refresh) = data
                .lock()
                .ok()
                .map(|mut d| {
                    d.active_provider = provider;
                    let req = d.usage_refresh_requested;
                    if req {
                        d.usage_refresh_requested = false;
                    }
                    (d.discord_enabled, d.discord_prefs.clone(), req)
                })
                .unwrap_or((true, DiscordDisplayPrefs::default(), false));
            let discord_enabled =
                discord_enabled && std::env::var("PULSE_DISABLE_DISCORD").as_deref() != Ok("1");

            if let Ok(fresh) = CodexPresenceConfig::load_or_init() {
                codex_config = fresh;
            }
            let mut independently_probed_routes = independently_probe_access(
                &mut usage_mgr,
                &codex_usage_trigger,
                &codex_usage_force,
                &codex_usage_latest,
                &mut api_probe_cache,
                force_refresh,
                provider,
                &mut codex_plan_detector,
                &codex_config.openai_plan,
            );
            if let Some(mut route) = go_usage.lock().ok().and_then(|latest| latest.clone()) {
                if route
                    .expires_at
                    .is_some_and(|expires| expires < chrono::Utc::now())
                {
                    route.freshness = AccessFreshness::Stale;
                }
                independently_probed_routes.push(route);
            }
            if let Some(Ok(reading)) = codex_usage_latest.lock().ok().and_then(|slot| slot.clone())
                && let Ok(mut d) = data.lock()
            {
                d.codex_plan_envelopes = reading.plan_envelopes();
            }
            for route in &independently_probed_routes {
                let observed_provider = match route.source.provider.as_str() {
                    "claude" => Some(Provider::Claude),
                    "codex" => Some(Provider::Codex),
                    _ => None,
                };
                if let Some(observed_provider) = observed_provider {
                    observe_access_notifications(
                        app.as_ref(),
                        notification_store.as_ref(),
                        observed_provider,
                        route,
                    );
                }
            }
            let commandcode_config_result = CommandCodeConfig::load();
            let mut commandcode_live = Vec::new();
            let mut commandcode_diagnostics = Vec::new();
            if let Ok(config) = &commandcode_config_result {
                let mut batch = commandcode_collector.poll(config);
                let mut persisted = true;
                for session in &mut batch.changed {
                    if session.branch.is_none()
                        && session.is_recent(chrono::Utc::now().timestamp_millis())
                    {
                        session.branch = commandcode_git.get(&session.directory);
                    }
                    persisted &= crate::db::upsert_external_session(
                        &crate::commandcode::session_info(session),
                        session.updated,
                    );
                }
                if persisted {
                    commandcode_collector.acknowledge(&batch);
                }
                for session in &mut batch.live {
                    if session.branch.is_none() {
                        session.branch = commandcode_git.get(&session.directory);
                    }
                }
                let active_ids: Vec<_> = batch
                    .live
                    .iter()
                    .filter(|session| session.is_active(chrono::Utc::now().timestamp_millis()))
                    .map(|session| session.id.clone())
                    .collect();
                crate::db::mark_inactive("commandcode", &active_ids);
                commandcode_live = batch.live;
                commandcode_diagnostics = batch.diagnostics;
            } else {
                commandcode_diagnostics.push("Command Code configuration is unavailable".into());
            }
            independently_probed_routes.push(crate::commandcode::access_route());
            if let Ok(mut d) = data.lock() {
                d.commandcode_sessions = commandcode_live.clone();
                d.commandcode_diagnostics = commandcode_diagnostics;
            }
            if provider != Provider::CommandCode {
                commandcode_publisher.shutdown();
                commandcode_lease.release();
            }
            let open_config_result = OpenCodeConfig::load();
            let mut open_diagnostics = Vec::new();
            let mut open_live = Vec::new();
            if let Ok(config) = &open_config_result {
                let mut batch = opencode_collector.poll(config);
                let mut persisted = true;
                for session in &mut batch.sessions {
                    opencode_runtime.enrich(session);
                    if session.is_recent(chrono::Utc::now().timestamp_millis()) {
                        session.branch = opencode_git.get(&session.directory);
                    }
                    let info = crate::opencode::session_info(session);
                    persisted &= crate::db::upsert_opencode_session(&info, session.updated);
                }
                if persisted {
                    opencode_collector.acknowledge(&batch);
                }
                if !persisted {
                    batch
                        .diagnostics
                        .push("OpenCode analytics write failed; retrying this batch".into());
                }
                for session in &mut batch.live {
                    opencode_runtime.enrich(session);
                    session.branch = opencode_git.get(&session.directory);
                }
                let active_ids: Vec<_> = batch
                    .live
                    .iter()
                    .filter(|session| !session.is_idle(chrono::Utc::now().timestamp_millis()))
                    .map(|session| session.id.clone())
                    .collect();
                crate::db::mark_inactive("opencode", &active_ids);
                open_diagnostics = batch.diagnostics;
                open_live = batch.live;
            } else if let Err(error) = &open_config_result {
                open_diagnostics.push(error.to_string());
            }
            if let Ok(mut d) = data.lock() {
                d.opencode_sessions = open_live.clone();
                d.opencode_diagnostics = open_diagnostics;
            }
            if provider != Provider::OpenCode {
                opencode_publisher.shutdown();
                opencode_lease.release();
            }
            let codex_sessions_roots = cc_discord_presence::codex::config::sessions_paths();
            let active_codex = codex_session::collect_active_sessions_multi(
                &codex_sessions_roots,
                STALE_THRESHOLD,
                STICKY_WINDOW,
                &mut codex_git,
                &mut codex_parse,
                &codex_config.pricing,
            )
            .unwrap_or_default();
            // Analytics ingestion is provider-independent: switching the visible
            // workspace to Claude must not stop current Codex JSONLs advancing.
            persist_live_codex_snapshots(&active_codex, &codex_config, PresenceSurface::Cli);

            if let Ok(fresh) = PresenceConfig::load_or_init() {
                claude_config = fresh;
            }
            let now = SystemTime::now();
            let cutoff = now
                .checked_sub(ACTIVE_CUTOFF)
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let mut all_claude = session::collect_active_sessions(
                &mut claude_git,
                &mut claude_parse,
                STALE_THRESHOLD,
                STICKY_WINDOW,
            )
            .unwrap_or_default();
            if let Some(statusline) = read_statusline_data(&mut claude_git) {
                merge_statusline_into_sessions(&mut all_claude, statusline);
            }
            let cutoff_chrono =
                chrono::Utc::now() - chrono::Duration::seconds(ACTIVE_CUTOFF.as_secs() as i64);
            let active_claude: Vec<_> = all_claude
                .into_iter()
                .filter(|session| is_claude_presence_candidate(session, cutoff, cutoff_chrono))
                .collect();
            // Claude analytics advance independently from the provider that
            // currently owns the visible workspace and Discord publisher.
            persist_live_claude_snapshots(&active_claude);
            if let Ok(mut d) = data.lock() {
                d.live_claude = active_claude.clone();
                d.live_codex = active_codex.clone();
            }

            let (discord_status, discord_publisher) = match provider {
                Provider::CommandCode => {
                    claude_publisher.release();
                    codex_publisher.release();
                    opencode_lease.release();
                    claude_discord.shutdown();
                    codex_discord.shutdown();
                    opencode_publisher.shutdown();
                    let owned = commandcode_lease.try_acquire().unwrap_or(false);
                    if let Ok(mut config) = commandcode_config_result {
                        config.enabled &= discord_enabled;
                        let session = commandcode::preferred_session(&commandcode_live);
                        let (quotas, credits) = session
                            .map(|session| crate::accounts::command_presence(&session.profile))
                            .unwrap_or_default();
                        if owned {
                            commandcode_publisher.update(
                                session,
                                &config,
                                quotas.as_deref(),
                                credits.as_deref(),
                            );
                        } else {
                            commandcode_publisher.shutdown();
                        }
                    } else {
                        commandcode_publisher.shutdown();
                    }
                    (
                        if owned {
                            commandcode_publisher.status().to_string()
                        } else {
                            "Controlled by external publisher".into()
                        },
                        if owned {
                            "Pulse".into()
                        } else {
                            "external".into()
                        },
                    )
                }
                Provider::OpenCode => {
                    claude_publisher.release();
                    codex_publisher.release();
                    let publisher_owned = opencode_lease.try_acquire().unwrap_or(false);
                    claude_discord.shutdown();
                    codex_discord.shutdown();
                    let status = if let Ok(mut config) = open_config_result {
                        config.enabled &= discord_enabled;
                        if publisher_owned {
                            let quotas = crate::opencode_go::presence_text(
                                &independently_probed_routes,
                                chrono::Utc::now(),
                            );
                            opencode_publisher.update(
                                opencode::preferred_session(&open_live),
                                &config,
                                quotas.as_deref(),
                            );
                        } else {
                            opencode_publisher.shutdown();
                        }
                        if publisher_owned {
                            opencode_publisher.status().to_string()
                        } else {
                            "Controlled by external daemon".into()
                        }
                    } else {
                        opencode_publisher.shutdown();
                        "OpenCode configuration unavailable".into()
                    };
                    if let Ok(mut d) = data.lock() {
                        d.sessions = ActiveSessions::None;
                        d.access = merge_access_routes(&independently_probed_routes, []);
                    }
                    (
                        status,
                        if publisher_owned {
                            "pulse"
                        } else {
                            "external_daemon"
                        }
                        .to_string(),
                    )
                }
                Provider::Claude => {
                    codex_publisher.release();
                    let publisher_owned = claude_publisher.try_acquire().unwrap_or_else(|error| {
                        tracing::warn!(error = %error, "failed to acquire Claude publisher lease");
                        false
                    });
                    let active = active_claude;

                    if force_refresh {
                        usage_mgr.invalidate_cache();
                        let usage_cache_path = cc_discord_presence::config::usage_cache_path();
                        if let Err(err) = std::fs::remove_file(&usage_cache_path)
                            && err.kind() != std::io::ErrorKind::NotFound
                        {
                            tracing::warn!(
                                path = %usage_cache_path.display(),
                                error = %err,
                                "failed to remove usage cache"
                            );
                        }
                    }

                    let usage = trusted_claude_usage(&mut usage_mgr);
                    let usage_source = usage_mgr
                        .last_usage_origin()
                        .map(|origin| origin.label())
                        .unwrap_or_else(|| "unknown source".to_string());
                    let detected_plan_key = usage_mgr.detected_plan_key();
                    let cached_usage = usage.as_ref().map(|u| CachedUsage {
                        five_hour_pct: u.five_hour.utilization,
                        seven_day_pct: u.seven_day.utilization,
                        extra_enabled: u.extra_usage.as_ref().is_some_and(|e| e.is_enabled),
                        extra_limit: u.extra_usage.as_ref().and_then(|e| e.monthly_limit),
                        extra_used: u.extra_usage.as_ref().and_then(|e| e.used_credits),
                        extra_pct: u.extra_usage.as_ref().and_then(|e| e.utilization),
                        source: usage_source.clone(),
                    });
                    let usage_error = usage_mgr.error_hint_with_countdown();
                    let observed_at = usage_mgr
                        .last_usage_observed_at()
                        .unwrap_or_else(chrono::Utc::now);
                    let access_route = match usage.as_ref() {
                        Some(usage) => {
                            let mut source =
                                subscription_source("claude", detected_plan_key.clone());
                            let origin_is_oauth = usage_mgr
                                .last_usage_origin()
                                .is_some_and(|origin| matches!(origin.auth, UsageAuth::OAuth));
                            if origin_is_oauth {
                                source.proof = crate::access::AccessProof::QuotaResponse;
                            } else if usage_mgr.has_valid_oauth_credentials() {
                                // Cached numbers from a prior OAuth read, but the
                                // account is still authenticated — not signed out.
                                source.proof = crate::access::AccessProof::AuthenticatedProbe;
                            }
                            claude_route_from_usage(
                                source,
                                usage,
                                observed_at,
                                chrono::Utc::now(),
                                chrono::Duration::seconds(300),
                                chrono::Utc::now(),
                                usage_source.clone(),
                            )
                        }
                        None => AccessRouteSnapshot::unavailable(
                            subscription_source("claude", detected_plan_key.clone()),
                            usage_error
                                .clone()
                                .unwrap_or_else(|| "no usage data yet".to_string()),
                        ),
                    };
                    // A stale cache remains useful as a diagnostic route but
                    // must not put old percentages back into Rich Presence.
                    let usage_for_discord = (access_route.availability
                        == AccessAvailability::Available
                        && access_route.freshness == AccessFreshness::Fresh)
                        .then_some(usage.as_ref())
                        .flatten();

                    apply_claude_display_prefs(&mut claude_config, &prefs);
                    let manual_plan = PresenceConfig::load_or_init()
                        .ok()
                        .and_then(|cfg| cfg.plan)
                        .filter(|p| {
                            !p.trim().is_empty()
                                && cc_discord_presence::plan::plan_from_key(p).is_some()
                        });
                    claude_config.plan = manual_plan.or_else(|| detected_plan_key.clone());

                    let status = if discord_enabled && publisher_owned {
                        let active_session = preferred_active_session(&active);
                        let limits = route_limits_for_claude_presence(Some(&access_route));
                        if let Err(err) = claude_discord.update(
                            active_session,
                            limits.as_ref(),
                            usage_for_discord,
                            &claude_config,
                        ) {
                            tracing::warn!(error = %err, "failed to update Claude Discord presence");
                        }
                        codex_discord.shutdown();
                        claude_discord.status().to_string()
                    } else if !discord_enabled {
                        claude_discord.shutdown();
                        codex_discord.shutdown();
                        "Disabled".to_string()
                    } else {
                        claude_discord.shutdown();
                        codex_discord.shutdown();
                        "Controlled by external daemon".to_string()
                    };
                    if let Ok(mut d) = data.lock() {
                        d.sessions = ActiveSessions::Claude(active);
                        d.claude_usage = cached_usage;
                        d.claude_usage_data = usage.clone();
                        d.claude_usage_error = usage_error;
                        d.access =
                            merge_access_routes(&independently_probed_routes, [access_route]);
                    }
                    (
                        status,
                        if publisher_owned {
                            "pulse"
                        } else {
                            "external_daemon"
                        }
                        .to_string(),
                    )
                }
                Provider::Codex => {
                    claude_publisher.release();
                    let publisher_owned = codex_publisher.try_acquire().unwrap_or_else(|error| {
                        tracing::warn!(error = %error, "failed to acquire Codex publisher lease");
                        false
                    });
                    let discord_enabled = codex_config.presence_enabled;

                    apply_codex_display_prefs(&mut codex_config, &prefs, CreditsMirror::Apply);

                    let active = active_codex;
                    let jsonl_envelopes = fresh_envelopes(
                        codex_parse.rate_limit_envelopes(),
                        chrono::Utc::now(),
                        chrono::Duration::minutes(15),
                    );
                    if force_refresh {
                        codex_usage_force.store(true, Ordering::Release);
                    }
                    // The account protocol has independent response timeouts;
                    // never hold session persistence or Discord updates on it.
                    let _ = codex_usage_trigger.try_send(());
                    let account_usage = codex_usage_latest
                        .lock()
                        .ok()
                        .and_then(|slot| slot.clone())
                        .unwrap_or_else(|| Err("Codex account quota read is pending".to_string()));
                    let account_quota_reported = account_usage
                        .as_ref()
                        .is_ok_and(|reading| !reading.envelopes.is_empty());
                    let (
                        usage_envelopes,
                        codex_usage,
                        codex_usage_error,
                        codex_reset_credits,
                        codex_individual_limits,
                    ) = match account_usage {
                        Ok(reading) => {
                            let snapshot = reading.usage_snapshot();
                            (
                                reading.envelopes,
                                Some(snapshot),
                                None,
                                reading.rate_limit_reset_credits,
                                reading.individual_limits,
                            )
                        }
                        Err(error) if !jsonl_envelopes.is_empty() => {
                            let snapshot = usage_snapshot_from_envelopes(
                                UsageSource::new(
                                    "codex-subscription:jsonl",
                                    [
                                        UsageSignal::CodexSessionJsonl,
                                        UsageSignal::CodexSubscriptionUsage,
                                    ],
                                ),
                                "Codex JSONL rate_limits · fallback",
                                &jsonl_envelopes,
                            );
                            (
                                jsonl_envelopes,
                                Some(snapshot),
                                Some(format!("Codex account API unavailable: {error}")),
                                None,
                                Vec::new(),
                            )
                        }
                        Err(error) => (
                            Vec::new(),
                            None,
                            Some(format!("Codex account API unavailable: {error}")),
                            None,
                            Vec::new(),
                        ),
                    };
                    let effective_limits = effective_limits_from_envelopes(&usage_envelopes);
                    let plan_envelopes = data
                        .lock()
                        .map(|d| d.codex_plan_envelopes.clone())
                        .unwrap_or_default();
                    let resolved_plan = codex_plan_detector.resolve_from_envelopes(
                        if plan_envelopes.is_empty() {
                            &usage_envelopes
                        } else {
                            &plan_envelopes
                        },
                        &codex_config.openai_plan,
                    );
                    let resolved_plan_key = codex_plan_key_from_tier(resolved_plan.tier);
                    let access_plan =
                        (!resolved_plan_key.is_empty()).then(|| resolved_plan_key.to_string());
                    let access_route =
                        match codex_usage.clone() {
                            Some(usage) => {
                                let max_age =
                                    if usage.source.signals.iter().any(|signal| {
                                        matches!(signal, UsageSignal::CodexSessionJsonl)
                                    }) {
                                        chrono::Duration::minutes(15)
                                    } else {
                                        chrono::Duration::seconds(30)
                                    };
                                let mut source = subscription_source("codex", access_plan.clone());
                                if usage.source.signals.iter().any(|signal| {
                                    matches!(signal, UsageSignal::CodexSubscriptionUsage)
                                }) {
                                    source.proof = if account_quota_reported {
                                        crate::access::AccessProof::QuotaResponse
                                    } else {
                                        crate::access::AccessProof::AuthenticatedProbe
                                    };
                                }
                                access_route_from_usage_with_account_details(
                                    source,
                                    usage,
                                    codex_reset_credits,
                                    codex_individual_limits,
                                    chrono::Utc::now(),
                                    max_age,
                                    chrono::Utc::now(),
                                )
                            }
                            None => AccessRouteSnapshot::unavailable(
                                subscription_source("codex", access_plan),
                                codex_usage_error.clone().unwrap_or_else(|| {
                                    "Codex account quota unavailable".to_string()
                                }),
                            ),
                        };
                    let resolved_service_tier = resolve_service_tier();
                    let opencode_running =
                        cc_discord_presence::codex::process::is_opencode_running();
                    let codex_desktop_running =
                        cc_discord_presence::codex::process::is_codex_app_running();
                    let surface_override = codex_fallback_surface(codex_desktop_running);

                    let status = if discord_enabled && publisher_owned {
                        let active_session = codex_session::preferred_active_session(&active);
                        let limits = route_limits_for_presence(Some(&access_route));
                        if let Err(err) = codex_discord.update(
                            active_session,
                            limits.as_ref(),
                            &resolved_plan,
                            &resolved_service_tier,
                            &codex_config,
                            surface_override,
                        ) {
                            tracing::warn!(error = %err, "failed to update Codex Discord presence");
                        }
                        claude_discord.shutdown();
                        codex_discord.status().to_string()
                    } else if !discord_enabled {
                        claude_discord.shutdown();
                        codex_discord.shutdown();
                        "Disabled".to_string()
                    } else {
                        claude_discord.shutdown();
                        codex_discord.shutdown();
                        "Controlled by external daemon".to_string()
                    };
                    if let Ok(mut d) = data.lock() {
                        d.discord_enabled = discord_enabled;
                        d.sessions = ActiveSessions::Codex(active);
                        d.codex_opencode_running = opencode_running;
                        d.codex_desktop_surface_running = codex_desktop_running;
                        d.claude_usage = None;
                        d.claude_usage_data = None;
                        d.claude_usage_error = None;
                        d.codex_usage = codex_usage;
                        d.codex_limits = effective_limits;
                        d.codex_usage_error = codex_usage_error;
                        d.access =
                            merge_access_routes(&independently_probed_routes, [access_route]);
                    }
                    (
                        status,
                        if publisher_owned {
                            "pulse"
                        } else {
                            "external_daemon"
                        }
                        .to_string(),
                    )
                }
            };

            if let Ok(mut d) = data.lock() {
                d.discord_status = discord_status;
                d.discord_publisher = discord_publisher;
            }
            INITIAL_SNAPSHOT_READY.store(true, Ordering::Release);

            if let Ok(snapshot) = build_app_snapshot("live") {
                let snapshot_hash = app_snapshot_fingerprint(&snapshot);
                if last_snapshot_hash != Some(snapshot_hash) {
                    if let Err(error) = persist_app_snapshot(&snapshot) {
                        tracing::warn!(error = %error, "failed to persist Pulse app snapshot");
                    }
                    if let Some(app) = app.as_ref()
                        && let Err(error) = app.emit("pulse://snapshot", &snapshot)
                    {
                        tracing::warn!(error = %error, "failed to emit Pulse snapshot");
                    }
                    last_snapshot_hash = Some(snapshot_hash);
                }
            }

            thread::sleep(REFRESH_INTERVAL);
        }
    });
}

fn is_claude_presence_candidate(
    session: &ClaudeSessionSnapshot,
    cutoff: SystemTime,
    cutoff_chrono: chrono::DateTime<chrono::Utc>,
) -> bool {
    if session.is_subagent {
        return false;
    }

    session
        .last_token_event_at
        .is_some_and(|ts| ts >= cutoff_chrono)
        || session.last_activity >= cutoff
}

fn read_claude_sessions() -> Vec<ClaudeSessionSnapshot> {
    shared().lock().ok().map_or_else(Vec::new, |d| {
        if !d.live_claude.is_empty() {
            d.live_claude.clone()
        } else {
            match &d.sessions {
                ActiveSessions::Claude(sessions) => sessions.clone(),
                _ => Vec::new(),
            }
        }
    })
}

fn read_codex_sessions() -> Vec<CodexSessionSnapshot> {
    shared().lock().ok().map_or_else(Vec::new, |d| {
        if !d.live_codex.is_empty() {
            d.live_codex.clone()
        } else {
            match &d.sessions {
                ActiveSessions::Codex(sessions) => sessions.clone(),
                _ => Vec::new(),
            }
        }
    })
}

fn read_codex_desktop_surface_running() -> bool {
    shared()
        .lock()
        .ok()
        .is_some_and(|d| d.codex_desktop_surface_running)
}

fn codex_fallback_surface(desktop_surface_running: bool) -> PresenceSurface {
    if desktop_surface_running {
        PresenceSurface::Desktop
    } else {
        PresenceSurface::Cli
    }
}

fn codex_session_surface(
    session: &CodexSessionSnapshot,
    fallback_surface: PresenceSurface,
) -> PresenceSurface {
    session.detected_surface().unwrap_or(fallback_surface)
}

fn current_live_session_infos() -> Vec<SessionInfo> {
    match current_provider() {
        Provider::CommandCode => shared()
            .lock()
            .map(|data| {
                data.commandcode_sessions
                    .iter()
                    .map(crate::commandcode::session_info)
                    .collect()
            })
            .unwrap_or_default(),
        Provider::OpenCode => shared()
            .lock()
            .map(|data| {
                data.opencode_sessions
                    .iter()
                    .map(crate::opencode::session_info)
                    .collect()
            })
            .unwrap_or_default(),
        Provider::Claude => build_claude_session_infos(&read_claude_sessions()),
        Provider::Codex => {
            let config = CodexPresenceConfig::load_or_init().unwrap_or_default();
            build_codex_session_infos(
                &read_codex_sessions(),
                &config,
                codex_fallback_surface(read_codex_desktop_surface_running()),
            )
        }
    }
}

#[derive(Serialize)]
pub struct HealthResponse {
    pub version: &'static str,
    pub uptime_seconds: u64,
    pub discord_status: String,
    pub discord_enabled: bool,
}

#[derive(Serialize)]
pub struct AppSnapshot {
    pub revision: u32,
    pub opencode_diagnostics: Vec<String>,
    pub commandcode_diagnostics: Vec<String>,
    pub sync_state: &'static str,
    pub snapshot_captured_at: String,
    pub health: HealthResponse,
    pub metrics: MetricsResponse,
    pub sessions: Vec<SessionInfo>,
    pub access: AccessSnapshot,
    pub rate_limits: Option<RateLimitInfo>,
    /// Config-dependent payloads are absent together when the persisted
    /// Discord configuration cannot be read. The frontend may retain a
    /// same-provider last-good value, but the backend never fabricates one.
    pub discord_preview: Option<DiscordPresencePreview>,
    pub discord_settings: Option<DiscordSettings>,
    pub discord_settings_error: Option<String>,
    pub plan: PlanInfo,
}

#[derive(Deserialize, Serialize)]
struct PersistedAppSnapshot {
    schema_version: u32,
    provider: String,
    saved_at: String,
    snapshot: serde_json::Value,
}

fn build_app_snapshot(sync_state: &'static str) -> Result<AppSnapshot, String> {
    // Shared-state failure is a snapshot-wide trust-boundary failure. Check it
    // before any legacy getter can turn a poisoned mutex into Unknown/empty
    // defaults and accidentally publish fabricated core telemetry.
    let cached = shared()
        .lock()
        .map_err(|_| "Pulse shared snapshot state is unavailable".to_string())?
        .clone();
    let access = get_access_snapshot();
    let (discord_preview, discord_settings, discord_settings_error) =
        match build_discord_snapshot_payload(&cached) {
            Ok((preview, settings)) => (Some(preview), Some(settings), None),
            Err(error) => (None, None, Some(error)),
        };
    Ok(AppSnapshot {
        revision: 1,
        opencode_diagnostics: cached.opencode_diagnostics.clone(),
        commandcode_diagnostics: cached.commandcode_diagnostics.clone(),
        sync_state,
        snapshot_captured_at: chrono::Utc::now().to_rfc3339(),
        health: get_health(),
        metrics: get_metrics(),
        sessions: get_live_sessions(),
        access,
        rate_limits: get_rate_limits(),
        discord_preview,
        discord_settings,
        discord_settings_error,
        plan: get_plan_info(),
    })
}

fn build_discord_snapshot_payload(
    cached: &CachedData,
) -> Result<(DiscordPresencePreview, DiscordSettings), String> {
    let provider = cached.active_provider;
    match provider {
        Provider::CommandCode => {
            let config = CommandCodeConfig::load().map_err(|error| error.to_string())?;
            Ok((
                crate::commandcode::preview(&cached.commandcode_sessions, &config),
                crate::commandcode::settings(
                    &config,
                    &cached.discord_status,
                    &cached.discord_publisher,
                ),
            ))
        }
        Provider::OpenCode => {
            let config = OpenCodeConfig::load().map_err(|error| error.to_string())?;
            Ok((
                crate::opencode::preview(&cached.opencode_sessions, &config, &cached.access.routes),
                build_opencode_settings(cached, &config),
            ))
        }
        Provider::Claude => {
            let mut config = PresenceConfig::load_or_init().map_err(|error| error.to_string())?;
            apply_claude_display_prefs(&mut config, &cached.discord_prefs);
            let sessions = match &cached.sessions {
                ActiveSessions::Claude(sessions) => sessions.as_slice(),
                _ => &[],
            };
            let preview = build_claude_discord_preview_with_access(
                sessions,
                &config,
                Some(&active_access_route(cached)),
                cached.claude_usage_data.as_ref(),
            );
            let settings = build_discord_settings(provider, cached, Some(&config), None);
            Ok((preview, settings))
        }
        Provider::Codex => {
            let mut config =
                CodexPresenceConfig::load_or_init().map_err(|error| error.to_string())?;
            apply_codex_display_prefs(&mut config, &cached.discord_prefs, CreditsMirror::Apply);
            let sessions = match &cached.sessions {
                ActiveSessions::Codex(sessions) => sessions.as_slice(),
                _ => &[],
            };
            let plan = resolve_codex_plan(sessions, &config, &cached.codex_plan_envelopes);
            let preview = build_codex_discord_preview_with_plan(
                sessions,
                &config,
                cached.codex_desktop_surface_running,
                Some(&active_access_route(cached)),
                &plan,
            );
            let settings = build_discord_settings(provider, cached, None, Some(&config));
            Ok((preview, settings))
        }
    }
}

fn app_snapshot_cache_path() -> PathBuf {
    cc_discord_presence::storage::home().join("pulse-last-app-snapshot.json")
}

fn persist_app_snapshot(snapshot: &AppSnapshot) -> Result<(), String> {
    let value = serde_json::to_value(snapshot).map_err(|error| error.to_string())?;
    persist_app_snapshot_value_at(&app_snapshot_cache_path(), current_provider(), value)
}

fn persist_app_snapshot_value_at(
    path: &std::path::Path,
    provider: Provider,
    snapshot: serde_json::Value,
) -> Result<(), String> {
    let envelope = PersistedAppSnapshot {
        schema_version: APP_SNAPSHOT_CACHE_SCHEMA,
        provider: provider.as_str().to_string(),
        saved_at: chrono::Utc::now().to_rfc3339(),
        snapshot,
    };
    cc_discord_presence::codex::util::write_json_pretty_atomic(path, &envelope)
        .map_err(|error| error.to_string())
}

fn load_persisted_app_snapshot_at(
    path: &std::path::Path,
    provider: Provider,
) -> Result<Option<serde_json::Value>, String> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.len() > APP_SNAPSHOT_CACHE_MAX_BYTES {
        return Err(format!(
            "persisted app snapshot is too large: {} bytes",
            metadata.len()
        ));
    }
    let raw = std::fs::read(path).map_err(|error| error.to_string())?;
    let mut persisted: PersistedAppSnapshot =
        serde_json::from_slice(&raw).map_err(|error| error.to_string())?;
    if persisted.schema_version != APP_SNAPSHOT_CACHE_SCHEMA
        || persisted.provider != provider.as_str()
    {
        return Ok(None);
    }
    let Some(root) = persisted.snapshot.as_object_mut() else {
        return Err("persisted app snapshot root is not an object".to_string());
    };
    let required = [
        "revision",
        "health",
        "metrics",
        "sessions",
        "access",
        "rate_limits",
        "discord_preview",
        "discord_settings",
        "plan",
    ];
    if root.get("revision").and_then(serde_json::Value::as_u64) != Some(1)
        || required.iter().any(|field| !root.contains_key(*field))
    {
        return Err("persisted app snapshot has an invalid contract".to_string());
    }
    root.insert(
        "sync_state".to_string(),
        serde_json::Value::String("syncing".to_string()),
    );
    root.entry("snapshot_captured_at".to_string())
        .or_insert(serde_json::Value::String(persisted.saved_at));
    Ok(Some(persisted.snapshot))
}

#[tauri::command]
pub fn get_app_snapshot() -> Result<serde_json::Value, String> {
    let poller_is_syncing = SNAPSHOT_POLLER_STARTED.load(Ordering::Acquire)
        && !INITIAL_SNAPSHOT_READY.load(Ordering::Acquire);
    if poller_is_syncing {
        match load_persisted_app_snapshot_at(&app_snapshot_cache_path(), current_provider()) {
            Ok(Some(snapshot)) => return Ok(snapshot),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(error = %error, "ignoring invalid persisted Pulse app snapshot");
            }
        }
    }
    serde_json::to_value(build_app_snapshot(if poller_is_syncing {
        "syncing"
    } else {
        "live"
    })?)
    .map_err(|error| error.to_string())
}

/// Returns the canonical access routes used to build quota DTOs, previews, and
/// live provider state. Unavailable lanes remain in the diagnostic transport;
/// proof-gated consumers select only routes with authenticated evidence.
#[tauri::command]
pub fn get_access_snapshot() -> AccessSnapshot {
    let inventory = crate::db::get_provider_history_inventory(None);
    shared()
        .lock()
        .ok()
        .map(|data| {
            if data.access.routes.is_empty() && data.active_provider == Provider::OpenCode {
                AccessSnapshot::default()
            } else if data.access.routes.is_empty() {
                AccessSnapshot::new(vec![active_access_route(&data)])
            } else {
                AccessSnapshot::new(data.access.routes.clone())
            }
        })
        .map(|snapshot| snapshot.with_local_history(&inventory))
        .unwrap_or_default()
}

fn app_snapshot_fingerprint(snapshot: &AppSnapshot) -> u64 {
    semantic_snapshot_fingerprint(serde_json::to_value(snapshot).unwrap_or_default())
}

fn semantic_snapshot_fingerprint(mut value: serde_json::Value) -> u64 {
    if let Some(root) = value.as_object_mut() {
        root.remove("sync_state");
        root.remove("snapshot_captured_at");
        if let Some(health) = root
            .get_mut("health")
            .and_then(serde_json::Value::as_object_mut)
        {
            health.remove("uptime_seconds");
        }
        if let Some(preview) = root
            .get_mut("discord_preview")
            .and_then(serde_json::Value::as_object_mut)
        {
            preview.remove("duration_secs");
        }
        if let Some(sessions) = root
            .get_mut("sessions")
            .and_then(serde_json::Value::as_array_mut)
        {
            for session in sessions {
                if let Some(session) = session.as_object_mut() {
                    session.remove("duration_secs");
                    session.remove("tokens_per_sec");
                }
            }
        }
    }
    let mut hasher = DefaultHasher::new();
    value.to_string().hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiscordPresencePreview {
    pub provider: String,
    pub app_name: String,
    pub details: String,
    pub state: String,
    pub large_image_key: String,
    pub large_text: String,
    pub small_image_key: Option<String>,
    pub small_text: Option<String>,
    pub has_session: bool,
    pub duration_secs: u64,
}

#[derive(Serialize)]
pub struct MetricsResponse {
    pub total_cost: f64,
    /// False when one or more sessions have no exact cost basis. The numeric
    /// total then covers exact observations only and is not a complete bill.
    pub cost_available: bool,
    pub cost_basis: String,
    pub input_tokens: u64,
    pub pure_input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub session_count: usize,
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_write_cost: f64,
    pub cache_read_cost: f64,
    pub cache_hit_ratio: f64,
    pub models: Vec<ModelMetric>,
}

#[derive(Serialize)]
pub struct ModelMetric {
    pub model: String,
    pub sessions: usize,
    pub cost: f64,
    pub tokens: u64,
}

#[derive(Serialize)]
pub struct SubagentDetail {
    pub agent_type: String,
    pub model: String,
    pub tokens: u64,
    pub cost: f64,
    pub activity: String,
}

fn read_session_name(session_id: &str) -> Option<String> {
    let meta_dir = cc_discord_presence::config::claude_home()
        .join("usage-data")
        .join("session-meta");
    let meta_file = meta_dir.join(format!("{}.json", session_id));
    let data = std::fs::read_to_string(meta_file).ok()?;
    let json: serde_json::Value = serde_json::from_str(&data).ok()?;
    let prompt = json.get("first_prompt")?.as_str()?;
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return None;
    }
    let truncated = if trimmed.len() > 80 {
        let end = trimmed
            .char_indices()
            .take(80)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(80);
        format!("{}...", &trimmed[..end])
    } else {
        trimmed.to_string()
    };
    Some(truncated)
}

#[derive(Default, Serialize)]
pub struct SessionInfo {
    pub opencode: Option<opencode::Metadata>,
    pub provider: String,
    pub app_name: Option<String>,
    pub session_id: String,
    pub session_name: Option<String>,
    pub project: String,
    pub model: String,
    pub model_id: String,
    pub context_window: String,
    pub cost: f64,
    /// Distinguishes a true observed zero from an unavailable/partial cost.
    pub cost_available: bool,
    pub cost_basis: String,
    pub cost_source: String,
    pub tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    pub context_used_tokens: u64,
    pub context_window_tokens: u64,
    pub branch: Option<String>,
    pub activity: String,
    pub activity_target: Option<String>,
    pub effort: String,
    /// True when the effort came from an explicit JSONL injection. False means
    /// we only have the `settings.json` default and the live Claude Desktop
    /// composer selection may differ (Claude Desktop keeps it in memory).
    pub effort_explicit: bool,
    pub is_idle: bool,
    pub started_at: Option<String>,
    pub duration_secs: u64,
    pub has_thinking: bool,
    pub workflow_label: Option<String>,
    pub subagent_count: usize,
    pub subagents: Vec<SubagentDetail>,
    pub tokens_per_sec: f64,
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_write_cost: f64,
    pub cache_read_cost: f64,
    /// Speed tier of the most recent turn ("fast"/"standard").
    pub speed: String,
    /// True when the most recent turn ran in fast mode (priority speed).
    pub fast: bool,
    /// Service tier of the most recent turn ("priority"/"standard"), display only.
    pub service_tier: Option<String>,
    /// True when this session's model uses a newer tokenizer that bills more
    /// tokens than its predecessor for the same input text at an unchanged
    /// per-token rate (currently: Opus 4.7+, Claude Sonnet 5).
    pub has_inflated_tokenizer: bool,
}

#[derive(Serialize)]
pub struct UsageSnapshotTransport {
    /// Legacy v1.6 discriminator retained for existing IPC consumers.
    pub provider: String,
    /// Legacy v1.6 provenance string retained under its original key.
    pub source: String,
    /// Structured v1.7 source identity.
    pub usage_source: UsageSource,
    pub scopes: Vec<QuotaScope>,
    pub credits: Option<CreditBalance>,
    pub observed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub provenance_source: String,
}

impl UsageSnapshotTransport {
    fn from_snapshot(provider: &str, snapshot: UsageSnapshot) -> Self {
        Self {
            provider: provider.to_string(),
            source: snapshot.provenance_source.clone(),
            usage_source: snapshot.source,
            scopes: snapshot.scopes,
            credits: snapshot.credits,
            observed_at: snapshot.observed_at,
            provenance_source: snapshot.provenance_source,
        }
    }
}

#[derive(Serialize)]
pub struct RateLimitInfo {
    pub provider: String,
    pub usage: Option<UsageSnapshotTransport>,
    pub five_hour_pct: f64,
    pub five_hour_resets: String,
    pub five_hour_label: String,
    pub five_hour_window_minutes: Option<u64>,
    pub seven_day_pct: f64,
    pub seven_day_resets: String,
    pub seven_day_label: String,
    pub seven_day_window_minutes: Option<u64>,
    pub sonnet_pct: Option<f64>,
    pub sonnet_resets: Option<String>,
    pub extra_enabled: bool,
    pub extra_limit: Option<f64>,
    pub extra_used: Option<f64>,
    pub extra_pct: Option<f64>,
    pub source: String,
    pub availability: AccessAvailability,
    pub freshness: AccessFreshness,
    pub provenance: AccessProvenance,
    pub error: Option<String>,
}

pub(crate) fn build_claude_session_infos(snapshots: &[ClaudeSessionSnapshot]) -> Vec<SessionInfo> {
    let now = SystemTime::now();
    let idle = now
        .checked_sub(IDLE_CUTOFF)
        .unwrap_or(SystemTime::UNIX_EPOCH);

    snapshots
        .iter()
        .map(|s| {
            let is_idle = s.last_activity < idle;
            let duration_secs = s
                .started_at
                .map(|st| (chrono::Utc::now() - st).num_seconds().max(0) as u64)
                .unwrap_or(0);
            let model_id_for_speed = s.model.clone().unwrap_or_default();
            let fast = s.speed.is_fast() && cost::is_fast_capable(&model_id_for_speed);
            let ic = s.input_cost;
            let oc = s.output_cost;
            let cwc = s.cache_write_cost;
            let crc = s.cache_read_cost;
            let tps = if s.total_api_duration_ms > 0 {
                s.output_tokens as f64 / (s.total_api_duration_ms as f64 / 1000.0)
            } else {
                0.0
            };
            let model_id_raw = s.model.clone().unwrap_or_default();
            let has_inflated_tokenizer = cost::has_inflated_tokenizer(&model_id_raw);
            let has_1m = cost::is_ga_1m_context(&model_id_raw)
                || model_id_raw.contains("[1m]")
                || s.max_turn_api_input > 200_000;
            let ctx_window_tokens = if has_1m { 1_000_000 } else { 200_000 };
            let ctx_used_tokens = s.current_context_tokens.min(ctx_window_tokens);
            let ctx_window = if has_1m { "1M" } else { "200K" }.to_string();
            let activity = s
                .activity
                .as_ref()
                .map_or("Idle".into(), |a| a.action_text().to_string());
            let activity = if !is_idle && activity == "Idle" {
                "Thinking".to_string()
            } else {
                activity
            };
            let subagent_details: Vec<SubagentDetail> = s
                .subagents
                .iter()
                .map(|sa| SubagentDetail {
                    agent_type: sa.agent_type.clone(),
                    model: sa
                        .model_display
                        .clone()
                        .or(sa.model.as_ref().map(|m| cost::model_display_name(m)))
                        .unwrap_or_else(|| "Unknown".into()),
                    tokens: sa.tokens,
                    cost: sa.cost,
                    activity: sa
                        .activity
                        .as_ref()
                        .map_or("Idle".into(), |a| a.action_text().to_string()),
                })
                .collect();
            let background_agent_count = s.background_work.active_agent_count;
            let subagent_count = subagent_details.len().max(background_agent_count);
            let workflow_label = if s.background_work.workflow_active {
                Some("ULTRACODE".to_string())
            } else {
                None
            };
            let session_name = read_session_name(&s.session_id);
            SessionInfo {
                opencode: None,
                provider: Provider::Claude.as_str().to_string(),
                app_name: None,
                session_id: s.session_id.clone(),
                session_name,
                project: s.project_name.clone(),
                model: s
                    .model_display
                    .clone()
                    .or(s.model.as_ref().map(|m| cost::model_display_name(m)))
                    .unwrap_or_else(|| "Unknown".into()),
                model_id: model_id_raw,
                context_window: ctx_window,
                cost_source: match s.source {
                    session::DataSource::Statusline => "claude_statusline_api_equivalent",
                    session::DataSource::Jsonl => "api_equivalent",
                }
                .into(),
                cost: s.total_cost,
                cost_available: s.total_cost.is_finite()
                    && s.total_cost >= 0.0
                    && (s.total_cost > 0.0 || s.session_total_tokens.unwrap_or(0) == 0),
                cost_basis: if s.total_cost.is_finite()
                    && s.total_cost >= 0.0
                    && (s.total_cost > 0.0 || s.session_total_tokens.unwrap_or(0) == 0)
                {
                    match s.source {
                        session::DataSource::Statusline => "exact".to_string(),
                        session::DataSource::Jsonl => "estimated".to_string(),
                    }
                } else {
                    "unavailable".to_string()
                },
                tokens: s.session_total_tokens.unwrap_or(0),
                input_tokens: s.input_tokens,
                output_tokens: s.output_tokens,
                cache_write_tokens: s.cache_creation_tokens,
                cache_read_tokens: s.cache_read_tokens,
                context_used_tokens: ctx_used_tokens,
                context_window_tokens: ctx_window_tokens,
                branch: s.git_branch.clone(),
                activity,
                activity_target: s.activity.as_ref().and_then(|a| a.target.clone()),
                effort: s.reasoning_effort.label().to_string(),
                effort_explicit: s.reasoning_effort_explicit,
                is_idle,
                started_at: s.started_at.map(|t| t.to_rfc3339()),
                duration_secs,
                has_thinking: s.has_thinking_blocks,
                workflow_label,
                subagent_count,
                subagents: subagent_details,
                tokens_per_sec: tps,
                input_cost: ic,
                output_cost: oc,
                cache_write_cost: cwc,
                cache_read_cost: crc,
                speed: s.speed.as_str().to_string(),
                fast,
                service_tier: s.service_tier.clone(),
                has_inflated_tokenizer,
            }
        })
        .collect()
}

pub(crate) fn build_codex_session_infos(
    snapshots: &[CodexSessionSnapshot],
    config: &CodexPresenceConfig,
    fallback_surface: PresenceSurface,
) -> Vec<SessionInfo> {
    let now = SystemTime::now();
    let idle = now
        .checked_sub(IDLE_CUTOFF)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    snapshots
        .iter()
        .map(|s| {
            let is_idle = s.last_activity < idle;
            let duration_secs = s
                .started_at
                .map(|st| (chrono::Utc::now() - st).num_seconds().max(0) as u64)
                .unwrap_or(0);
            let model_id_raw = s.model.clone().unwrap_or_default();
            let model_key = model_id_raw.clone();
            let fast = s.speed.mode == CodexSpeedMode::Fast;
            let display_name = if model_key.is_empty() {
                "Unknown".to_string()
            } else {
                cc_discord_presence::codex::util::format_model_display(
                    &model_key,
                    s.reasoning_effort,
                    fast,
                )
            };
            let context_window = s
                .context_window
                .as_ref()
                .map(|snapshot| snapshot.window_tokens)
                .or_else(|| {
                    cc_discord_presence::codex::cost::default_model_context_window(&model_key)
                })
                .unwrap_or(0);
            let context_window_label = if context_window == 0 {
                "Unknown".to_string()
            } else {
                cc_discord_presence::codex::util::format_tokens(context_window)
            };
            let input_total = codex_total_input_tokens(s);
            let cached_input = s.cached_input_tokens_total;
            let context_used_tokens = s
                .context_window
                .as_ref()
                .map(|snapshot| snapshot.used_tokens.min(context_window))
                .unwrap_or(0);
            let activity = s
                .activity
                .as_ref()
                .map_or("Idle".into(), |a| a.action_text().to_string());
            let activity = if !is_idle && activity == "Idle" {
                "Thinking".to_string()
            } else {
                activity
            };
            let activity_target = s.activity.as_ref().and_then(|a| a.target.clone());
            let surface = codex_session_surface(s, fallback_surface);
            let known_cost = (s.pricing_status
                != cc_discord_presence::codex::cost::PricingStatus::Unavailable)
                .then_some(s.known_cost_usd)
                .flatten()
                .filter(|value| value.is_finite() && *value >= 0.0);
            let cost_breakdown = &s.cost_breakdown;
            SessionInfo {
                opencode: None,
                provider: Provider::Codex.as_str().to_string(),
                app_name: Some(
                    surface
                        .label(config.display.desktop_presence_design)
                        .to_string(),
                ),
                session_id: s.session_id.clone(),
                session_name: None,
                project: s.project_name.clone(),
                model: display_name,
                model_id: model_key,
                context_window: context_window_label,
                cost_source: if s.pricing_source
                    == cc_discord_presence::codex::cost::PricingSource::ProviderReported
                {
                    "codex_reported"
                } else {
                    "api_equivalent"
                }
                .into(),
                cost: known_cost.unwrap_or(0.0),
                cost_available: known_cost.is_some(),
                cost_basis: match (known_cost, s.pricing_status) {
                    (Some(_), cc_discord_presence::codex::cost::PricingStatus::Exact) => "exact",
                    (Some(_), _) => "partial",
                    _ => "unavailable",
                }
                .to_string(),
                tokens: s
                    .session_total_tokens
                    .unwrap_or(input_total + s.output_tokens_total),
                input_tokens: input_total,
                output_tokens: s.output_tokens_total,
                cache_write_tokens: s.cache_write_tokens_total.unwrap_or(0),
                cache_read_tokens: cached_input,
                context_used_tokens,
                context_window_tokens: context_window,
                branch: s.git_branch.clone(),
                activity,
                activity_target,
                effort: s
                    .reasoning_effort
                    .map(|effort| effort.label().to_string())
                    .unwrap_or_else(|| "Unknown".to_string()),
                effort_explicit: s.reasoning_effort.is_some(),
                is_idle,
                started_at: s.started_at.map(|t| t.to_rfc3339()),
                duration_secs,
                has_thinking: s.reasoning_effort.is_some(),
                workflow_label: None,
                subagent_count: 0,
                subagents: Vec::new(),
                tokens_per_sec: 0.0,
                input_cost: known_cost
                    .map(|_| cost_breakdown.input_cost_usd)
                    .unwrap_or(0.0),
                output_cost: known_cost
                    .map(|_| cost_breakdown.output_cost_usd)
                    .unwrap_or(0.0),
                cache_write_cost: known_cost
                    .map(|_| cost_breakdown.cache_write_cost_usd)
                    .unwrap_or(0.0),
                cache_read_cost: known_cost
                    .map(|_| cost_breakdown.cached_input_cost_usd)
                    .unwrap_or(0.0),
                speed: s.speed.mode.label().to_ascii_lowercase(),
                fast,
                service_tier: s.speed.known.then(|| {
                    if fast {
                        "priority".to_string()
                    } else {
                        "standard".to_string()
                    }
                }),
                has_inflated_tokenizer: false,
            }
        })
        .collect()
}

fn codex_total_input_tokens(snapshot: &CodexSessionSnapshot) -> u64 {
    snapshot
        .input_tokens_total
        .max(snapshot.cached_input_tokens_total)
}

fn persist_live_session_infos(provider: Provider, result: &[SessionInfo]) {
    let active_ids: Vec<String> = result.iter().map(|s| s.session_id.clone()).collect();
    let active_keys: HashSet<String> = active_ids
        .iter()
        .map(|id| format!("{}:{id}", provider.as_str()))
        .collect();
    let fingerprints = SESSION_FINGERPRINTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut fingerprints = fingerprints.lock().ok();
    let mut changed_count = 0;
    for s in result {
        let key = format!("{}:{}", provider.as_str(), s.session_id);
        let fingerprint = persistent_session_fingerprint(s);
        let unchanged = fingerprints
            .as_ref()
            .and_then(|items| items.get(&key))
            .is_some_and(|previous| *previous == fingerprint);
        if !unchanged && crate::db::upsert_session(s) {
            changed_count += 1;
            if let Some(items) = fingerprints.as_mut() {
                items.insert(key, fingerprint);
            }
        }
    }
    if let Some(items) = fingerprints.as_mut() {
        let provider_prefix = format!("{}:", provider.as_str());
        items.retain(|key, _| !key.starts_with(&provider_prefix) || active_keys.contains(key));
    }
    crate::db::mark_inactive(provider.as_str(), &active_ids);
    crate::db::checkpoint_wal_after_writes(changed_count);
}

fn persistent_session_fingerprint(session: &SessionInfo) -> u64 {
    let mut value = serde_json::to_value(session).unwrap_or_default();
    if let Some(session) = value.as_object_mut() {
        // These values advance with wall-clock time even when no telemetry changed.
        // They are presentation-only and must not force SQLite writes every poll.
        session.remove("duration_secs");
        session.remove("tokens_per_sec");
    }
    let mut hasher = DefaultHasher::new();
    value.to_string().hash(&mut hasher);
    hasher.finish()
}

fn persist_live_claude_snapshots(snapshots: &[ClaudeSessionSnapshot]) {
    let result = build_claude_session_infos(snapshots);
    persist_live_session_infos(Provider::Claude, &result);
}

fn persist_live_codex_snapshots(
    snapshots: &[CodexSessionSnapshot],
    config: &CodexPresenceConfig,
    fallback_surface: PresenceSurface,
) {
    let result = build_codex_session_infos(snapshots, config, fallback_surface);
    persist_live_session_infos(Provider::Codex, &result);
}

#[tauri::command]
pub fn get_health() -> HealthResponse {
    let (discord_status, discord_enabled) = shared()
        .lock()
        .ok()
        .map(|d| (d.discord_status.clone(), d.discord_enabled))
        .unwrap_or_else(|| ("Unknown".into(), true));
    HealthResponse {
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds: uptime_secs(),
        discord_status,
        discord_enabled,
    }
}

#[tauri::command]
pub fn get_notifications(limit: Option<usize>) -> Result<Vec<NotificationRecord>, String> {
    let connection =
        crate::notifications::open_default_database().map_err(|error| error.to_string())?;
    NotificationStore::new(&connection)
        .list(limit)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_unread_notification_count() -> u32 {
    let Ok(connection) = crate::notifications::open_default_database() else {
        return 0;
    };
    NotificationStore::new(&connection)
        .unread_count()
        .unwrap_or(0)
}

#[tauri::command]
pub fn mark_notification_read(id: i64) -> Result<bool, String> {
    let connection =
        crate::notifications::open_default_database().map_err(|error| error.to_string())?;
    NotificationStore::new(&connection)
        .mark_read(id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn mark_all_notifications_read() -> Result<usize, String> {
    let connection =
        crate::notifications::open_default_database().map_err(|error| error.to_string())?;
    NotificationStore::new(&connection)
        .mark_all_read()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn mark_notification_unread(id: i64) -> Result<bool, String> {
    let connection =
        crate::notifications::open_default_database().map_err(|error| error.to_string())?;
    NotificationStore::new(&connection)
        .mark_unread(id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn mark_all_notifications_unread() -> Result<usize, String> {
    let connection =
        crate::notifications::open_default_database().map_err(|error| error.to_string())?;
    NotificationStore::new(&connection)
        .mark_all_unread()
        .map_err(|error| error.to_string())
}

#[derive(Serialize)]
pub struct ClearedNotifications {
    pub count: usize,
    pub undo_token: String,
}

#[tauri::command]
pub fn dismiss_all_notifications() -> Result<ClearedNotifications, String> {
    let connection =
        crate::notifications::open_default_database().map_err(|error| error.to_string())?;
    let (count, undo_token) = NotificationStore::new(&connection)
        .dismiss_all()
        .map_err(|error| error.to_string())?;
    Ok(ClearedNotifications { count, undo_token })
}

#[tauri::command]
pub fn restore_notifications(token: String) -> Result<usize, String> {
    let connection =
        crate::notifications::open_default_database().map_err(|error| error.to_string())?;
    NotificationStore::new(&connection)
        .restore_batch(&token)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dismiss_notification(id: i64) -> Result<bool, String> {
    let connection =
        crate::notifications::open_default_database().map_err(|error| error.to_string())?;
    NotificationStore::new(&connection)
        .dismiss(id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_discord_enabled(enabled: bool) -> Result<DiscordSettings, String> {
    // Persist for whichever provider is active. Claude used to skip this branch
    // entirely, so the master switch lived only in `shared()` and every restart
    // silently turned Rich Presence back on.
    match current_provider() {
        Provider::CommandCode => {
            let mut config = CommandCodeConfig::load().map_err(|error| error.to_string())?;
            config.enabled = enabled;
            config.save().map_err(|error| error.to_string())?;
        }
        Provider::OpenCode => {
            let mut config = OpenCodeConfig::load().map_err(|error| error.to_string())?;
            config.enabled = enabled;
            config.save().map_err(|error| error.to_string())?;
        }
        Provider::Codex => {
            let mut config =
                CodexPresenceConfig::load_or_init().map_err(|error| error.to_string())?;
            config.presence_enabled = enabled;
            config.save().map_err(|error| error.to_string())?;
        }
        Provider::Claude => {
            let mut config = PresenceConfig::load_or_init().map_err(|error| error.to_string())?;
            config.presence_enabled = enabled;
            config.save().map_err(|error| error.to_string())?;
        }
    }

    let mut data = shared()
        .lock()
        .map_err(|_| "Discord settings state is unavailable".to_string())?;
    data.discord_enabled = enabled;
    drop(data);
    get_discord_settings()
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn set_discord_display_prefs(
    show_project: bool,
    show_branch: bool,
    show_model: bool,
    show_activity: bool,
    show_tokens: bool,
    show_cost: bool,
    show_limits: bool,
    show_credits: bool,
    show_context: bool,
    show_systems: bool,
) -> Result<DiscordSettings, String> {
    let prefs = DiscordDisplayPrefs {
        show_project,
        show_branch,
        show_model,
        show_activity,
        show_tokens,
        show_cost,
        show_limits,
        show_credits,
        show_context,
        show_systems,
    };
    // Both provider configs are kept in sync so the field switches mean the same
    // thing after a provider swap, but the order matters. The ACTIVE provider is
    // written first and its failure is fatal; the mirror is only attempted once
    // the user's own config has landed. Writing both up front meant a failed
    // active write still persisted the mirror, so the change reappeared after a
    // provider swap even though the UI had reported it as failed and rolled back.
    let save_claude = || -> Result<(), String> {
        let mut config = PresenceConfig::load_or_init().map_err(|error| error.to_string())?;
        apply_claude_display_prefs(&mut config, &prefs);
        config.save().map_err(|error| error.to_string())
    };
    let credits = if current_provider() == Provider::Codex {
        CreditsMirror::Apply
    } else {
        CreditsMirror::Preserve
    };
    let save_codex = || -> Result<(), String> {
        let mut config = CodexPresenceConfig::load_or_init().map_err(|error| error.to_string())?;
        apply_codex_display_prefs(&mut config, &prefs, credits);
        config.save().map_err(|error| error.to_string())
    };

    match current_provider() {
        Provider::CommandCode => {
            let mut config = CommandCodeConfig::load().map_err(|error| error.to_string())?;
            crate::commandcode::apply_prefs(&mut config, &prefs);
            config.save().map_err(|error| error.to_string())?;
        }
        Provider::OpenCode => {
            let mut config = OpenCodeConfig::load().map_err(|error| error.to_string())?;
            crate::opencode::apply_prefs(&mut config, &prefs);
            config.save().map_err(|error| error.to_string())?;
        }
        Provider::Claude => {
            save_claude()?;
            log_mirror_error(Provider::Codex, save_codex());
        }
        Provider::Codex => {
            save_codex()?;
            log_mirror_error(Provider::Claude, save_claude());
        }
    }

    shared()
        .lock()
        .map_err(|_| "Discord settings state is unavailable".to_string())?
        .discord_prefs = prefs;
    get_discord_settings()
}

/// Reports a failed write to the *inactive* provider's config without failing the
/// user's action. The active provider is the one on screen; losing the mirror is
/// a degraded sync, not a lost setting.
fn log_mirror_error(provider: Provider, result: Result<(), String>) {
    if let Err(error) = result {
        tracing::warn!(
            provider = provider.as_str(),
            error = %error,
            "failed to mirror Discord display preferences to the inactive provider"
        );
    }
}

#[tauri::command]
pub fn set_codex_desktop_design(design: String) -> Result<DiscordSettings, String> {
    let design = match design.trim().to_ascii_lowercase().as_str() {
        "codex_app" => DesktopPresenceDesign::CodexApp,
        "chatgpt_app" => DesktopPresenceDesign::ChatGptApp,
        _ => return Err("Desktop design must be codex_app or chatgpt_app".to_string()),
    };
    let mut config = CodexPresenceConfig::load_or_init().map_err(|error| error.to_string())?;
    config.display.desktop_presence_design = design;
    config.save().map_err(|error| error.to_string())?;
    get_discord_settings()
}

/// Ask the background poller to drop its usage cache and hit the API on the
/// next tick. The UI's refresh button is wired to this — real data within ~5s.
#[tauri::command]
pub fn refresh_usage() {
    if let Ok(mut d) = shared().lock() {
        d.usage_refresh_requested = true;
    }
}

#[tauri::command]
pub fn get_metrics() -> MetricsResponse {
    let sessions = current_live_session_infos();
    let (mut cost, mut inp, mut out, mut cw, mut cr, mut tot) = (0.0, 0u64, 0u64, 0u64, 0u64, 0u64);
    for s in &sessions {
        cost += s.cost;
        inp += s.input_tokens;
        out += s.output_tokens;
        cw += s.cache_write_tokens;
        cr += s.cache_read_tokens;
        tot += s.tokens;
    }

    let (mut ic, mut oc, mut cwc, mut crc) = (0.0, 0.0, 0.0, 0.0);
    let mut model_map: std::collections::HashMap<String, (usize, f64, u64)> =
        std::collections::HashMap::new();
    for s in &sessions {
        ic += s.input_cost;
        oc += s.output_cost;
        cwc += s.cache_write_cost;
        crc += s.cache_read_cost;

        let entry = model_map.entry(s.model.clone()).or_insert((0, 0.0, 0));
        entry.0 += 1;
        entry.1 += s.cost;
        entry.2 += s.tokens;
    }

    let pure_inp_total = inp.saturating_sub(cw).saturating_sub(cr);
    let cache_total = cr as f64 + pure_inp_total as f64;
    let cache_hit_ratio = if cache_total > 0.0 {
        cr as f64 / cache_total * 100.0
    } else {
        0.0
    };

    let mut models: Vec<ModelMetric> = model_map
        .into_iter()
        .map(|(model, (sessions, cost, tokens))| ModelMetric {
            model,
            sessions,
            cost,
            tokens,
        })
        .collect();
    models.sort_by(|a, b| {
        b.cost
            .partial_cmp(&a.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let (cost_available, cost_basis) = cost_availability(&sessions);
    MetricsResponse {
        total_cost: cost,
        cost_available,
        cost_basis,
        input_tokens: inp,
        pure_input_tokens: inp.saturating_sub(cw).saturating_sub(cr),
        output_tokens: out,
        cache_write_tokens: cw,
        cache_read_tokens: cr,
        total_tokens: tot,
        session_count: sessions.len(),
        input_cost: ic,
        output_cost: oc,
        cache_write_cost: cwc,
        cache_read_cost: crc,
        cache_hit_ratio,
        models,
    }
}

fn cost_availability(sessions: &[SessionInfo]) -> (bool, String) {
    if sessions.is_empty() {
        return (false, "unavailable".to_string());
    }
    if !sessions.iter().any(|session| session.cost_available) {
        return (false, "unavailable".into());
    }
    let complete = sessions
        .iter()
        .all(|session| session.cost_available && session.cost_basis != "partial");
    let basis = if !complete {
        "partial"
    } else if sessions
        .iter()
        .any(|session| session.cost_basis == "estimated")
    {
        "estimated"
    } else {
        "exact"
    };
    (true, basis.into())
}

#[tauri::command]
pub fn get_live_sessions() -> Vec<SessionInfo> {
    let mut sessions = build_claude_session_infos(&read_claude_sessions());
    sessions.extend(build_codex_session_infos(
        &read_codex_sessions(),
        &CodexPresenceConfig::load_or_init().unwrap_or_default(),
        PresenceSurface::Cli,
    ));
    if let Ok(data) = shared().lock() {
        sessions.extend(
            data.commandcode_sessions
                .iter()
                .map(crate::commandcode::session_info),
        );
        sessions.extend(
            data.opencode_sessions
                .iter()
                .map(crate::opencode::session_info),
        );
    }
    sessions
}

#[tauri::command]
pub fn get_discord_preview() -> Result<DiscordPresencePreview, String> {
    let data = shared()
        .lock()
        .map_err(|_| "Pulse shared snapshot state is unavailable".to_string())?
        .clone();
    build_discord_snapshot_payload(&data).map(|(preview, _settings)| preview)
}

fn apply_claude_display_prefs(config: &mut PresenceConfig, prefs: &DiscordDisplayPrefs) {
    config.privacy.show_project_name = prefs.show_project;
    config.privacy.show_git_branch = prefs.show_branch;
    config.privacy.show_model = prefs.show_model;
    config.privacy.show_activity = prefs.show_activity;
    config.privacy.show_tokens = prefs.show_tokens;
    config.privacy.show_cost = prefs.show_cost;
    config.privacy.show_limits = prefs.show_limits;
    config.privacy.show_context = prefs.show_context;
    config.privacy.show_systems = prefs.show_systems;
}

/// Applies the shared field switches to the Codex config.
///
/// `credits` is passed separately because it is Codex-only. A payload composed
/// while Claude is active always carries `show_credits = false` — Claude has no
/// such field — so copying it here silently disabled Credits on the Codex side
/// every time an unrelated Claude toggle was saved.
fn apply_codex_display_prefs(
    config: &mut CodexPresenceConfig,
    prefs: &DiscordDisplayPrefs,
    credits: CreditsMirror,
) {
    config.privacy.show_project_name = prefs.show_project;
    config.privacy.show_git_branch = prefs.show_branch;
    config.privacy.show_model = prefs.show_model;
    config.privacy.show_activity = prefs.show_activity;
    config.privacy.show_tokens = prefs.show_tokens;
    config.privacy.show_cost = prefs.show_cost;
    config.privacy.show_limits = prefs.show_limits;
    if credits == CreditsMirror::Apply {
        config.privacy.show_credits = prefs.show_credits;
    }
    config.privacy.show_context = prefs.show_context;
    config.privacy.show_systems = prefs.show_systems;
}

/// Whether the incoming payload actually carries a meaningful `show_credits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreditsMirror {
    /// Composed while Codex was active — the value is the user's choice.
    Apply,
    /// Composed while a provider without Credits was active — keep what Codex has.
    Preserve,
}

fn claude_display_prefs(config: &PresenceConfig) -> DiscordDisplayPrefs {
    DiscordDisplayPrefs {
        show_project: config.privacy.show_project_name,
        show_branch: config.privacy.show_git_branch,
        show_model: config.privacy.show_model,
        show_activity: config.privacy.show_activity,
        show_tokens: config.privacy.show_tokens,
        show_cost: config.privacy.show_cost,
        show_limits: config.privacy.show_limits,
        show_credits: false,
        show_context: config.privacy.show_context,
        show_systems: config.privacy.show_systems,
    }
}

fn codex_display_prefs(config: &CodexPresenceConfig) -> DiscordDisplayPrefs {
    DiscordDisplayPrefs {
        show_project: config.privacy.show_project_name,
        show_branch: config.privacy.show_git_branch,
        show_model: config.privacy.show_model,
        show_activity: config.privacy.show_activity,
        show_tokens: config.privacy.show_tokens,
        show_cost: config.privacy.show_cost,
        show_limits: config.privacy.show_limits,
        show_credits: config.privacy.show_credits,
        show_context: config.privacy.show_context,
        show_systems: config.privacy.show_systems,
    }
}

fn desktop_design_key(design: DesktopPresenceDesign) -> &'static str {
    match design {
        DesktopPresenceDesign::CodexApp => "codex_app",
        DesktopPresenceDesign::ChatGptApp => "chatgpt_app",
    }
}

fn build_discord_settings(
    provider: Provider,
    cached: &CachedData,
    claude_config: Option<&PresenceConfig>,
    codex_config: Option<&CodexPresenceConfig>,
) -> DiscordSettings {
    let (enabled, display_prefs, desktop_design, supports_desktop_design, field_order) =
        match provider {
            Provider::OpenCode | Provider::CommandCode => (
                cached.discord_enabled,
                cached.discord_prefs.clone(),
                None,
                false,
                Vec::new(),
            ),
            Provider::Claude => (
                claude_config
                    .map(|config| config.presence_enabled)
                    .unwrap_or(cached.discord_enabled),
                claude_config
                    .map(claude_display_prefs)
                    .unwrap_or_else(|| cached.discord_prefs.clone()),
                None,
                false,
                Vec::new(),
            ),
            Provider::Codex => (
                codex_config
                    .map(|config| config.presence_enabled)
                    .unwrap_or(cached.discord_enabled),
                codex_config
                    .map(codex_display_prefs)
                    .unwrap_or_else(|| cached.discord_prefs.clone()),
                codex_config.map(|config| {
                    desktop_design_key(config.display.desktop_presence_design).to_string()
                }),
                true,
                codex_config
                    .map(|config| {
                        config
                            .display
                            .presence_layout
                            .fields
                            .iter()
                            .map(|item| item.field.as_str().to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
        };

    DiscordSettings {
        provider: provider.as_str().to_string(),
        enabled,
        status: cached.discord_status.clone(),
        publisher: cached.discord_publisher.clone(),
        display_prefs,
        desktop_design,
        supports_desktop_design,
        supports_field_order: provider == Provider::Codex,
        supports_credits: provider == Provider::Codex,
        field_order,
    }
}

#[tauri::command]
pub fn set_discord_field_order(order: Vec<String>) -> Result<DiscordSettings, String> {
    if current_provider() == Provider::Claude {
        return Err(
            "Field ordering is currently available for Codex and OpenCode presence".to_string(),
        );
    }
    let parsed: Vec<PresenceFieldId> = order
        .iter()
        .map(|value| {
            PresenceFieldId::parse(value).ok_or_else(|| format!("Unknown presence field: {value}"))
        })
        .collect::<Result<_, _>>()?;
    let unique: HashSet<PresenceFieldId> = parsed.iter().copied().collect();
    if parsed.len() != PresenceFieldId::ALL.len() || unique.len() != PresenceFieldId::ALL.len() {
        return Err("Field order must contain every field exactly once".to_string());
    }
    if current_provider() == Provider::CommandCode {
        let mut config = CommandCodeConfig::load().map_err(|error| error.to_string())?;
        config.layout.fields.sort_by_key(|item| {
            parsed
                .iter()
                .position(|field| *field == item.field)
                .unwrap_or(usize::MAX)
        });
        config.save().map_err(|error| error.to_string())?;
        return get_discord_settings();
    }
    if current_provider() == Provider::OpenCode {
        let mut config = OpenCodeConfig::load().map_err(|error| error.to_string())?;
        config.layout.fields.sort_by_key(|item| {
            parsed
                .iter()
                .position(|field| *field == item.field)
                .unwrap_or(usize::MAX)
        });
        config.save().map_err(|error| error.to_string())?;
        return get_discord_settings();
    }
    let mut config = CodexPresenceConfig::load_or_init().map_err(|error| error.to_string())?;
    config.display.presence_layout.fields.sort_by_key(|item| {
        parsed
            .iter()
            .position(|field| *field == item.field)
            .unwrap_or(usize::MAX)
    });
    config.save().map_err(|error| error.to_string())?;
    get_discord_settings()
}

#[tauri::command]
pub fn get_discord_settings() -> Result<DiscordSettings, String> {
    let provider = current_provider();
    let cached = shared()
        .lock()
        .map_err(|_| "Discord settings state is unavailable".to_string())?
        .clone();
    match provider {
        Provider::CommandCode => {
            let config = CommandCodeConfig::load().map_err(|error| error.to_string())?;
            Ok(crate::commandcode::settings(
                &config,
                &cached.discord_status,
                &cached.discord_publisher,
            ))
        }
        Provider::OpenCode => {
            let config = OpenCodeConfig::load().map_err(|error| error.to_string())?;
            Ok(build_opencode_settings(&cached, &config))
        }
        Provider::Claude => {
            let config = PresenceConfig::load_or_init().map_err(|error| error.to_string())?;
            Ok(build_discord_settings(
                provider,
                &cached,
                Some(&config),
                None,
            ))
        }
        Provider::Codex => {
            let config = CodexPresenceConfig::load_or_init().map_err(|error| error.to_string())?;
            Ok(build_discord_settings(
                provider,
                &cached,
                None,
                Some(&config),
            ))
        }
    }
}

#[allow(dead_code)]
fn build_claude_discord_preview(
    sessions: &[ClaudeSessionSnapshot],
    config: &PresenceConfig,
) -> DiscordPresencePreview {
    build_claude_discord_preview_with_access(sessions, config, None, None)
}

fn build_claude_discord_preview_with_access(
    sessions: &[ClaudeSessionSnapshot],
    config: &PresenceConfig,
    access: Option<&AccessRouteSnapshot>,
    api_usage: Option<&cc_discord_presence::usage::UsageData>,
) -> DiscordPresencePreview {
    let Some(session) = preferred_active_session(sessions) else {
        return DiscordPresencePreview {
            provider: Provider::Claude.as_str().to_string(),
            app_name: "Claude Code".to_string(),
            details: "Claude Code".to_string(),
            state: "Waiting for session".to_string(),
            large_image_key: config.display.large_image_key.clone(),
            large_text: config.display.large_text.clone(),
            small_image_key: None,
            small_text: None,
            has_session: false,
            duration_secs: 0,
        };
    };

    let limits = route_limits_for_claude_presence(access);
    let route_is_live = access.is_some_and(|route| {
        route.availability == AccessAvailability::Available
            && route.freshness == AccessFreshness::Fresh
    });
    let live_usage = route_is_live.then_some(api_usage).flatten();
    let (details, state, _tooltip) =
        claude_presence_lines(session, limits.as_ref(), live_usage, config);

    DiscordPresencePreview {
        provider: Provider::Claude.as_str().to_string(),
        app_name: "Claude Code".to_string(),
        details,
        state,
        large_image_key: config.display.large_image_key.clone(),
        large_text: config.display.large_text.clone(),
        small_image_key: None,
        small_text: None,
        has_session: true,
        duration_secs: claude_duration_secs(session),
    }
}

#[allow(dead_code)]
fn build_codex_discord_preview(
    sessions: &[CodexSessionSnapshot],
    config: &CodexPresenceConfig,
    desktop_surface_running: bool,
) -> DiscordPresencePreview {
    build_codex_discord_preview_with_access(sessions, config, desktop_surface_running, None)
}

fn build_codex_discord_preview_with_access(
    sessions: &[CodexSessionSnapshot],
    config: &CodexPresenceConfig,
    desktop_surface_running: bool,
    access: Option<&AccessRouteSnapshot>,
) -> DiscordPresencePreview {
    let plan = resolve_codex_plan(sessions, config, &[]);
    build_codex_discord_preview_with_plan(sessions, config, desktop_surface_running, access, &plan)
}

fn build_codex_discord_preview_with_plan(
    sessions: &[CodexSessionSnapshot],
    config: &CodexPresenceConfig,
    desktop_surface_running: bool,
    access: Option<&AccessRouteSnapshot>,
    resolved_plan: &cc_discord_presence::codex::telemetry::plan::ResolvedPlan,
) -> DiscordPresencePreview {
    let fallback_surface = codex_fallback_surface(desktop_surface_running);
    let Some(session) = codex_session::preferred_active_session(sessions) else {
        let presentation = idle_presence_presentation(fallback_surface, config);
        return DiscordPresencePreview {
            provider: Provider::Codex.as_str().to_string(),
            app_name: presentation.app_name,
            details: presentation.details,
            state: presentation.state,
            large_image_key: presentation.large_image_key,
            large_text: presentation.large_text,
            small_image_key: presentation.small_image_key,
            small_text: presentation.small_text,
            has_session: false,
            duration_secs: 0,
        };
    };

    let resolved_service_tier = resolve_service_tier();
    let limits = route_limits_for_presence(access);
    let presentation = active_presence_presentation(
        codex_session_surface(session, fallback_surface),
        session,
        limits.as_ref(),
        resolved_plan,
        &resolved_service_tier,
        config,
    );

    DiscordPresencePreview {
        provider: Provider::Codex.as_str().to_string(),
        app_name: presentation.app_name,
        details: presentation.details,
        state: presentation.state,
        large_image_key: presentation.large_image_key,
        large_text: presentation.large_text,
        small_image_key: presentation.small_image_key,
        small_text: presentation.small_text,
        has_session: true,
        duration_secs: codex_duration_secs(session),
    }
}

fn claude_duration_secs(session: &ClaudeSessionSnapshot) -> u64 {
    session
        .started_at
        .map(|started_at| (chrono::Utc::now() - started_at).num_seconds().max(0) as u64)
        .unwrap_or(0)
}

fn codex_duration_secs(session: &CodexSessionSnapshot) -> u64 {
    session
        .started_at
        .map(|started_at| (chrono::Utc::now() - started_at).num_seconds().max(0) as u64)
        .unwrap_or(0)
}

#[tauri::command]
pub fn get_rate_limits() -> Option<RateLimitInfo> {
    let data = shared().lock().ok()?;
    Some(rate_limit_info_from_access(&active_access_route(&data)))
}

#[derive(Clone, Serialize)]
pub struct DiscordUserInfo {
    pub user_id: String,
    pub username: String,
    pub global_name: Option<String>,
    pub discriminator: String,
    pub avatar_hash: String,
    pub avatar_url: String,
    /// Discord's built-in default avatar for this account. Always resolves
    /// (HTTP 200), so the UI can fall back to it when `avatar_url` points at a
    /// stale hash the CDN no longer serves.
    pub avatar_default_url: String,
    pub banner_hash: Option<String>,
    pub banner_url: Option<String>,
}

fn discord_leveldb_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let variants = ["discord", "discordcanary", "discordptb"];

    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            for v in &variants {
                dirs.push(
                    PathBuf::from(&appdata)
                        .join(v)
                        .join("Local Storage/leveldb"),
                );
            }
        }
        if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
            let pascal = ["Discord", "DiscordCanary", "DiscordPTB"];
            for v in &pascal {
                dirs.push(
                    PathBuf::from(&localappdata)
                        .join(v)
                        .join("Local Storage/leveldb"),
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(&home);
        let variants_mac = [
            "discord",
            "discordcanary",
            "discordptb",
            "Discord",
            "Discord Canary",
            "Discord PTB",
        ];
        for v in &variants_mac {
            dirs.push(
                home_path
                    .join("Library/Application Support")
                    .join(v)
                    .join("Local Storage/leveldb"),
            );
        }
    }

    #[cfg(target_os = "linux")]
    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(&home);
        for v in &variants {
            dirs.push(
                home_path
                    .join(".config")
                    .join(v)
                    .join("Local Storage/leveldb"),
            );
        }
        let flatpak_ids = [
            "com.discordapp.Discord",
            "com.discordapp.DiscordCanary",
            "com.discordapp.DiscordPTB",
        ];
        for id in &flatpak_ids {
            dirs.push(
                home_path
                    .join(".var/app")
                    .join(id)
                    .join("config/discord/Local Storage/leveldb"),
            );
        }
        for v in &variants {
            dirs.push(
                home_path
                    .join("snap")
                    .join(v)
                    .join("current/.config")
                    .join(v)
                    .join("Local Storage/leveldb"),
            );
        }
    }

    dirs
}

#[tauri::command]
pub fn get_discord_user() -> Option<DiscordUserInfo> {
    let mut cache = DISCORD_USER_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((cached_at, cached_user)) = cache.as_ref()
        && cached_at.elapsed() < DISCORD_USER_CACHE_TTL
    {
        return cached_user.clone();
    }

    let user = cc_discord_presence::discord_identity::current_user()
        .map(discord_user_from_rpc)
        .or_else(|| scan_discord_leveldb_dirs(&discord_leveldb_dirs()));
    *cache = Some((Instant::now(), user.clone()));
    user
}

fn discord_user_from_rpc(
    identity: cc_discord_presence::discord_identity::Identity,
) -> DiscordUserInfo {
    let user_id = identity.id;
    let discriminator = if identity.discriminator.is_empty() {
        "0".into()
    } else {
        identity.discriminator
    };
    let avatar_hash = identity.avatar.unwrap_or_default();
    let avatar_default_url = default_avatar_url(&user_id, &discriminator);
    let avatar_url = if avatar_hash.is_empty() {
        avatar_default_url.clone()
    } else {
        format!(
            "https://cdn.discordapp.com/avatars/{user_id}/{avatar_hash}.{}?size=256",
            if avatar_hash.starts_with("a_") {
                "gif"
            } else {
                "png"
            }
        )
    };
    let banner_hash = identity
        .banner
        .or_else(|| local_discord_banner_hash(&user_id));
    let banner_url = banner_hash.as_ref().map(|hash| {
        format!(
            "https://cdn.discordapp.com/banners/{user_id}/{hash}.{}?size=600",
            if hash.starts_with("a_") { "gif" } else { "png" }
        )
    });
    DiscordUserInfo {
        user_id,
        username: identity.username,
        global_name: identity.global_name,
        discriminator,
        avatar_hash,
        avatar_url,
        avatar_default_url,
        banner_hash,
        banner_url,
    }
}

fn local_discord_banner_hash(user_id: &str) -> Option<String> {
    use std::io::Read;
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from)?;
    let needle = format!("/banners/{user_id}/");
    let mut files = Vec::new();
    for variant in ["discord", "discordcanary", "discordptb"] {
        let root = appdata.join(variant).join("Cache/Cache_Data");
        if let Ok(entries) = std::fs::read_dir(root) {
            files.extend(entries.flatten().filter_map(|entry| {
                let metadata = entry.metadata().ok()?;
                metadata
                    .is_file()
                    .then(|| (metadata.modified().ok(), entry.path()))
            }));
        }
    }
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, path) in files.into_iter().take(512) {
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        let mut bytes = Vec::new();
        if file.take(16_384).read_to_end(&mut bytes).is_err() {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        if let Some(start) = text.find(&needle) {
            let hash: String = text[start + needle.len()..]
                .chars()
                .take_while(|c| c.is_ascii_hexdigit() || *c == '_')
                .collect();
            if hash.len() == 32 || (hash.len() == 34 && hash.starts_with("a_")) {
                return Some(hash);
            }
        }
    }
    None
}

fn scan_discord_leveldb_dirs(dirs: &[PathBuf]) -> Option<DiscordUserInfo> {
    let mut candidates = Vec::new();
    for (dir_priority, leveldb_dir) in dirs.iter().enumerate() {
        let Ok(read_dir) = std::fs::read_dir(leveldb_dir) else {
            continue;
        };
        candidates.extend(
            read_dir
                .filter_map(Result::ok)
                .filter(|entry| {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    name.ends_with(".ldb") || name.ends_with(".log")
                })
                .filter_map(|entry| {
                    let modified = entry.metadata().ok()?.modified().ok()?;
                    Some((modified, dir_priority, entry.file_name(), entry.path()))
                }),
        );
    }

    candidates.sort_by(
        |(modified_a, priority_a, name_a, _), (modified_b, priority_b, name_b, _)| {
            modified_b
                .cmp(modified_a)
                .then_with(|| priority_a.cmp(priority_b))
                .then_with(|| name_a.cmp(name_b))
        },
    );
    for (_, _, _, path) in candidates {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        if let Some(user) = extract_discord_user(&data) {
            return Some(user);
        }
    }
    None
}

/// End (exclusive) of the JSON object owning the member that starts at
/// `start`. The walk honors quoted strings so a `}` inside a display value
/// never truncates the record, and it stops at the object's own closing brace
/// so a neighboring account profile never leaks fields into this id's chunk.
fn discord_user_object_end(data: &[u8], start: usize) -> Option<usize> {
    let mut in_string = false;
    let mut escaped = false;
    let mut depth: usize = 0;
    for (index, &byte) in data.iter().enumerate().skip(start).take(4096) {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                if depth == 0 {
                    return Some(index + 1);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// Read a JSON string field whose value may carry escape sequences such as
/// quotes or backslashes (display names do), unlike the strict-charset
/// identifier fields.
fn extract_json_escaped_field(chunk: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let value_start = chunk.find(&needle)? + needle.len();
    let mut chars = chunk[value_start..].chars();
    let mut value = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(value),
            '\\' => match chars.next()? {
                '"' => value.push('"'),
                '\\' => value.push('\\'),
                '/' => value.push('/'),
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                other => {
                    value.push('\\');
                    value.push(other);
                }
            },
            ch => value.push(ch),
        }
    }
    None
}

fn pick_best_discord_user(candidates: Vec<(usize, DiscordUserInfo)>) -> Option<DiscordUserInfo> {
    candidates
        .into_iter()
        .max_by_key(|(offset, user)| {
            let score = u8::from(!user.avatar_hash.is_empty()) * 2
                + u8::from(user.global_name.is_some())
                + u8::from(user.banner_url.is_some());
            (score, *offset)
        })
        .map(|(_, user)| user)
}

fn extract_discord_user(data: &[u8]) -> Option<DiscordUserInfo> {
    let needle = b"\"id\":\"";
    let mut pos = 0;
    let mut candidates = Vec::new();
    while pos < data.len().saturating_sub(100) {
        if let Some(offset) = data[pos..].windows(needle.len()).position(|w| w == needle) {
            let start = pos + offset;
            let id_start = start + needle.len();
            if let Some(id_end) = data[id_start..].iter().position(|&b| b == b'"') {
                let id_bytes = &data[id_start..id_start + id_end];
                if id_bytes.len() >= 17 && id_bytes.iter().all(|b| b.is_ascii_digit()) {
                    let user_id = String::from_utf8_lossy(id_bytes).to_string();
                    // The owning object's real end (or the byte cap when LevelDB
                    // text is truncated) keeps fields from a neighboring account
                    // profile out of this id's record.
                    let chunk_end = discord_user_object_end(data, start)
                        .unwrap_or((start + 1200).min(data.len()));
                    let chunk = &data[start..chunk_end];
                    let chunk_str = String::from_utf8_lossy(chunk);

                    let username = match extract_json_field(&chunk_str, "username") {
                        Some(u) if !u.is_empty() => u,
                        _ => {
                            pos = start + 1;
                            continue;
                        }
                    };

                    let global_name = extract_json_escaped_field(&chunk_str, "global_name")
                        .filter(|name| !name.is_empty());

                    let discriminator = extract_json_field(&chunk_str, "discriminator")
                        .filter(|d| !d.is_empty())
                        .unwrap_or_else(|| "0".to_string());

                    let avatar_hash = extract_json_field(&chunk_str, "avatar")
                        .filter(|h| !h.is_empty())
                        .unwrap_or_default();

                    let avatar_url = if avatar_hash.is_empty() {
                        default_avatar_url(&user_id, &discriminator)
                    } else {
                        let ext = if avatar_hash.starts_with("a_") {
                            "gif"
                        } else {
                            "png"
                        };
                        format!(
                            "https://cdn.discordapp.com/avatars/{}/{}.{}?size=256",
                            user_id, avatar_hash, ext
                        )
                    };

                    // Always resolvable fallback. When the custom avatar hash is
                    // stale (the CDN returns 404 after the user changes it), the
                    // UI swaps to this on the img error event.
                    let avatar_default_url = default_avatar_url(&user_id, &discriminator);

                    let (banner_hash, banner_url) = match extract_json_field(&chunk_str, "banner") {
                        Some(bh) if !bh.is_empty() => {
                            let ext = if bh.starts_with("a_") { "gif" } else { "png" };
                            let url = format!(
                                "https://cdn.discordapp.com/banners/{}/{}.{}?size=600",
                                user_id, bh, ext
                            );
                            (Some(bh), Some(url))
                        }
                        _ => (None, None),
                    };

                    candidates.push((
                        start,
                        DiscordUserInfo {
                            user_id,
                            username,
                            global_name,
                            discriminator,
                            avatar_hash,
                            avatar_url,
                            avatar_default_url,
                            banner_hash,
                            banner_url,
                        },
                    ));
                }
            }
            pos = start + 1;
        } else {
            break;
        }
    }
    pick_best_discord_user(candidates)
}

/// Build the CDN URL for Discord's built-in default avatars.
///
/// - New username system (discriminator "0"): index = (user_id >> 22) % 6
/// - Legacy discriminator system: index = discriminator % 5
fn default_avatar_url(user_id: &str, discriminator: &str) -> String {
    let index = if discriminator == "0" {
        user_id.parse::<u64>().map(|id| (id >> 22) % 6).unwrap_or(0)
    } else {
        discriminator
            .parse::<u32>()
            .map(|d| u64::from(d % 5))
            .unwrap_or(0)
    };
    format!("https://cdn.discordapp.com/embed/avatars/{index}.png")
}

fn extract_json_field(text: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{}\":\"", field);
    let start = text.find(&pattern)? + pattern.len();
    let end = start + text[start..].find('"')?;
    Some(text[start..end].to_string())
}

#[derive(Serialize)]
pub struct PlanInfo {
    pub provider: String,
    pub plan_key: String,
    pub plan_name: String,
    pub detected: bool,
}

#[tauri::command]
pub fn get_plan_info() -> PlanInfo {
    match current_provider() {
        Provider::CommandCode => {
            let plan = crate::accounts::command_account(&commandcode::home())
                .filter(|account| account.status == "connected")
                .and_then(|account| account.plan);
            PlanInfo {
                provider: "commandcode".into(),
                plan_key: plan.clone().unwrap_or_default(),
                plan_name: plan.clone().unwrap_or_else(|| "Not reported".into()),
                detected: plan.is_some(),
            }
        }
        Provider::OpenCode => PlanInfo {
            provider: "opencode".into(),
            plan_key: String::new(),
            plan_name: "No account quota reported".into(),
            detected: false,
        },
        Provider::Claude => {
            if let Ok(cfg) = PresenceConfig::load_or_init()
                && let Some(plan) = cfg.plan.as_deref().filter(|plan| !plan.trim().is_empty())
                && cc_discord_presence::plan::plan_from_key(plan).is_some()
            {
                return PlanInfo {
                    provider: Provider::Claude.as_str().to_string(),
                    plan_key: plan.to_string(),
                    plan_name: plan_name_from_key(plan),
                    detected: false,
                };
            }

            let mut usage_mgr = UsageManager::new();
            let plan_key = usage_mgr.detected_plan_key().unwrap_or_default();
            let plan_name = if plan_key.is_empty() {
                "Unknown".to_string()
            } else {
                plan_name_from_key(&plan_key)
            };
            let detected = !plan_key.is_empty();

            PlanInfo {
                provider: Provider::Claude.as_str().to_string(),
                plan_key,
                plan_name,
                detected,
            }
        }
        Provider::Codex => {
            let config = CodexPresenceConfig::load_or_init().unwrap_or_default();
            let sessions = read_codex_sessions();
            let envelopes = shared()
                .lock()
                .map(|data| data.codex_plan_envelopes.clone())
                .unwrap_or_default();
            let resolved = resolve_codex_plan(&sessions, &config, &envelopes);
            let plan_key = codex_plan_key_from_tier(resolved.tier).to_string();
            PlanInfo {
                provider: Provider::Codex.as_str().to_string(),
                plan_key: plan_key.clone(),
                plan_name: resolved.label(config.openai_plan.show_price),
                detected: !plan_key.is_empty()
                    && !matches!(
                        resolved.source,
                        cc_discord_presence::codex::telemetry::plan::DetectedPlanSource::Manual
                    ),
            }
        }
    }
}

#[tauri::command]
pub fn set_plan_override(plan: String, provider: Option<String>) -> Result<(), String> {
    let provider = match provider {
        Some(provider) => Provider::parse(&provider)
            .ok_or_else(|| format!("unsupported provider {provider:?}"))?,
        None => current_provider(),
    };
    match provider {
        Provider::OpenCode | Provider::CommandCode => {
            return Err("This provider has no subscription plan override".into());
        }
        Provider::Claude => {
            let mut cfg = PresenceConfig::load_or_init().map_err(|error| error.to_string())?;
            cfg.plan = plan_key_from_override(&plan).map(str::to_string);
            cfg.save().map_err(|error| error.to_string())?;
        }
        Provider::Codex => {
            let mut cfg = CodexPresenceConfig::load_or_init().map_err(|error| error.to_string())?;
            let normalized = plan.trim().to_ascii_lowercase();
            let tier = match normalized.as_str() {
                "" | "auto" => None,
                "free" => Some(cc_discord_presence::codex::config::OpenAiPlanTier::Free),
                "go" => Some(cc_discord_presence::codex::config::OpenAiPlanTier::Go),
                "plus" => Some(cc_discord_presence::codex::config::OpenAiPlanTier::Plus),
                "team" => Some(cc_discord_presence::codex::config::OpenAiPlanTier::Business),
                "pro_5x" | "pro5x" => {
                    Some(cc_discord_presence::codex::config::OpenAiPlanTier::Pro5x)
                }
                "pro_20x" | "pro20x" => {
                    Some(cc_discord_presence::codex::config::OpenAiPlanTier::Pro20x)
                }
                "business" => Some(cc_discord_presence::codex::config::OpenAiPlanTier::Business),
                "enterprise" => {
                    Some(cc_discord_presence::codex::config::OpenAiPlanTier::Enterprise)
                }
                "edu" => Some(cc_discord_presence::codex::config::OpenAiPlanTier::Edu),
                _ => return Err(format!("Unsupported Codex plan override: {normalized}")),
            };
            if let Some(tier) = tier {
                cfg.openai_plan.mode = cc_discord_presence::codex::config::OpenAiPlanMode::Manual;
                cfg.openai_plan.tier = tier;
            } else {
                cfg.openai_plan.mode = cc_discord_presence::codex::config::OpenAiPlanMode::Auto;
            }
            cfg.save().map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
pub struct ProviderInfo {
    pub active_provider: String,
}

#[derive(Serialize)]
pub struct ProviderCopyInfo {
    pub provider: String,
    pub provider_label: String,
    pub instruction_file: String,
    pub home_dir: String,
    pub sessions_store: String,
    pub fix_label: String,
    pub global_state_source: String,
}

#[tauri::command]
pub fn get_active_provider() -> ProviderInfo {
    ProviderInfo {
        active_provider: current_provider().as_str().to_string(),
    }
}

#[tauri::command]
pub fn get_provider_copy() -> ProviderCopyInfo {
    let provider = current_provider();
    ProviderCopyInfo {
        provider: provider.as_str().to_string(),
        provider_label: provider.display_name().to_string(),
        instruction_file: provider.instruction_file_name().to_string(),
        home_dir: provider.home_dir_name().to_string(),
        sessions_store: provider.sessions_glob_label().to_string(),
        fix_label: provider.fix_action_label().to_string(),
        global_state_source: provider.global_state_label().to_string(),
    }
}

#[tauri::command]
pub fn set_active_provider(provider: String) -> Result<(), String> {
    let provider =
        Provider::parse(&provider).ok_or_else(|| format!("unsupported provider {provider:?}"))?;
    cc_discord_presence::provider::save_active_provider(provider)
        .map_err(|error| error.to_string())?;
    // The switch flags and field visibility belong to the provider, not to the
    // session — re-read them only after persistence succeeds.
    seed_discord_state_for(provider);
    Ok(())
}

#[tauri::command]
pub fn get_app_settings() -> crate::app_settings::AppSettings {
    crate::app_settings::load()
}

#[tauri::command]
pub fn set_close_to_tray(enabled: bool) -> Result<crate::app_settings::AppSettings, String> {
    crate::app_settings::set_close_to_tray(enabled).map_err(|error| error.to_string())
}

#[derive(Serialize)]
pub struct DashboardBundle {
    pub summary: crate::db::AnalyticsSummary,
    pub sessions: Vec<crate::db::HistoricalSession>,
    pub forecast: crate::db::CostForecast,
    pub hourly_activity: Vec<crate::db::HourlyActivity>,
}

#[tauri::command]
pub async fn get_dashboard_bundle(provider: Option<String>) -> DashboardBundle {
    offload(move || dashboard_bundle_blocking(provider)).await
}

pub fn dashboard_bundle_blocking(provider: Option<String>) -> DashboardBundle {
    let data = crate::db::get_dashboard_data_scoped(provider.as_deref(), 7, 5000);
    DashboardBundle {
        summary: data.summary,
        sessions: data.sessions,
        forecast: data.forecast,
        hourly_activity: data.hourly_activity,
    }
}

#[tauri::command]
pub fn get_session_history(
    days: Option<i64>,
    project: Option<String>,
    limit: Option<i64>,
    provider: Option<String>,
) -> Vec<crate::db::HistoricalSession> {
    crate::db::get_session_history_scoped(provider.as_deref(), days, project.as_deref(), limit)
}

#[tauri::command]
// Tauri exposes these filters as flat invoke arguments; nesting them would break the public IPC shape.
#[allow(clippy::too_many_arguments)]
pub fn get_session_history_filtered(
    from_iso: Option<String>,
    to_iso: Option<String>,
    project: Option<String>,
    model: Option<String>,
    min_cost: Option<f64>,
    max_cost: Option<f64>,
    limit: Option<i64>,
    provider: Option<String>,
) -> Vec<crate::db::HistoricalSession> {
    crate::db::get_session_history_filtered_scoped(
        provider.as_deref(),
        from_iso.as_deref(),
        to_iso.as_deref(),
        project.as_deref(),
        model.as_deref(),
        min_cost,
        max_cost,
        limit,
    )
}

#[tauri::command]
pub fn get_sessions_by_hour_range(
    start_hour: i64,
    end_hour: i64,
    days: Option<i64>,
    provider: Option<String>,
) -> Vec<crate::db::HistoricalSession> {
    crate::db::get_sessions_by_hour_range_scoped(provider.as_deref(), start_hour, end_hour, days)
}

#[tauri::command]
pub fn search_sessions(
    query: String,
    limit: Option<i64>,
    provider: Option<String>,
) -> Vec<crate::db::HistoricalSession> {
    crate::db::search_sessions_scoped(provider.as_deref(), &query, limit)
}

#[tauri::command]
pub fn get_daily_stats(days: Option<i64>, provider: Option<String>) -> Vec<crate::db::DailyStat> {
    crate::db::get_daily_stats_scoped(provider.as_deref(), days)
}

#[tauri::command]
pub fn get_analytics_summary(provider: Option<String>) -> crate::db::AnalyticsSummary {
    crate::db::get_analytics_summary_scoped(provider.as_deref())
}

#[derive(Serialize, Clone)]
pub struct ContextFileEntry {
    pub name: String,
    pub tokens: u64,
}

#[derive(Serialize, Clone)]
pub struct ContextBreakdown {
    pub model: String,
    pub context_window: u64,
    pub used_tokens: u64,
    pub free_space: u64,
    pub autocompact_buffer: u64,
    pub system_prompt: u64,
    pub system_tools: u64,
    pub memory_files: Vec<ContextFileEntry>,
    pub memory_total: u64,
    pub skills: Vec<ContextFileEntry>,
    pub skills_total: u64,
    pub messages: u64,
    pub mcp_tools: Vec<ContextFileEntry>,
    pub mcp_total: u64,
}

#[derive(Serialize)]
pub struct SessionContextBreakdown {
    pub session_id: String,
    pub project: String,
    pub model_id: String,
    pub is_idle: bool,
    pub activity: String,
    pub breakdown: ContextBreakdown,
}

#[derive(Serialize)]
pub struct SessionContextUsage {
    pub session_id: String,
    pub project: String,
    pub model: String,
    pub model_display: String,
    pub used_tokens: u64,
    pub window_tokens: u64,
    pub utilization_pct: f64,
    pub recommendation: String,
}

const CONTEXT_WATCH_PCT: f64 = 50.0;
const CONTEXT_COMPACT_SOON_PCT: f64 = 80.0;
const CONTEXT_COMPACT_NOW_PCT: f64 = 95.0;

fn context_utilization_pct(used_tokens: u64, window_tokens: u64) -> f64 {
    if window_tokens == 0 {
        0.0
    } else {
        ((used_tokens as f64 / window_tokens as f64) * 100.0).clamp(0.0, 100.0)
    }
}

fn context_recommendation(utilization_pct: f64) -> String {
    if utilization_pct >= CONTEXT_COMPACT_NOW_PCT {
        "Context is nearly full — compact now or start a fresh session before the next turn.".into()
    } else if utilization_pct >= CONTEXT_COMPACT_SOON_PCT {
        "Context is filling up — plan to compact soon to avoid an autocompact mid-task.".into()
    } else if utilization_pct >= CONTEXT_WATCH_PCT {
        "Context is past half — keep an eye on it and compact when you shift topics.".into()
    } else {
        "Context is healthy — plenty of headroom for this session.".into()
    }
}

fn estimate_tokens(text: &str) -> u64 {
    (text.len() as f64 / 3.5).ceil() as u64
}

fn estimate_tokens_from_file(path: &std::path::Path) -> u64 {
    std::fs::read_to_string(path)
        .map(|s| estimate_tokens(&s))
        .unwrap_or(0)
}

fn resolve_instruction_include(
    base_file: &std::path::Path,
    raw: &str,
) -> Option<std::path::PathBuf> {
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'');
    if trimmed.is_empty() {
        return None;
    }
    let candidate = std::path::PathBuf::from(trimmed);
    if candidate.is_absolute() {
        Some(candidate)
    } else {
        base_file.parent().map(|parent| parent.join(candidate))
    }
}

fn discover_instruction_includes(
    base_file: &std::path::Path,
    content: &str,
) -> Vec<std::path::PathBuf> {
    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            if let Some(include) = trimmed.strip_prefix('@') {
                return resolve_instruction_include(base_file, include);
            }
            if let Some(include) = trimmed.strip_prefix("file:") {
                return resolve_instruction_include(base_file, include);
            }
            None
        })
        .collect()
}

fn label_context_file(
    path: &std::path::Path,
    provider: Provider,
    project_root: Option<&std::path::Path>,
    project_name: Option<&str>,
) -> String {
    let provider_home = provider.home_path();
    if path.starts_with(&provider_home)
        && let Ok(relative) = path.strip_prefix(&provider_home)
    {
        let relative = relative.to_string_lossy().replace('\\', "/");
        return format!("{}/{}", provider.home_dir_name(), relative);
    }
    if let (Some(root), Some(name)) = (project_root, project_name)
        && path.starts_with(root)
        && let Ok(relative) = path.strip_prefix(root)
    {
        let relative = relative.to_string_lossy().replace('\\', "/");
        if relative.is_empty() {
            return name.to_string();
        }
        return format!("{name}/{relative}");
    }
    path.to_string_lossy().replace('\\', "/")
}

fn collect_instruction_tree(
    provider: Provider,
    root_file: &std::path::Path,
    project_root: Option<&std::path::Path>,
    project_name: Option<&str>,
    seen: &mut HashSet<std::path::PathBuf>,
    out: &mut Vec<ContextFileEntry>,
) {
    let canonical = std::fs::canonicalize(root_file).unwrap_or_else(|_| root_file.to_path_buf());
    if !seen.insert(canonical.clone()) {
        return;
    }
    let Ok(content) = std::fs::read_to_string(&canonical) else {
        return;
    };
    let tokens = estimate_tokens(&content);
    if tokens > 0 {
        let name = label_context_file(&canonical, provider, project_root, project_name);
        out.push(ContextFileEntry { name, tokens });
    }
    for include in discover_instruction_includes(&canonical, &content) {
        if include.exists() {
            collect_instruction_tree(provider, &include, project_root, project_name, seen, out);
        }
    }
}

fn collect_skills_from_dir(skills_dir: &std::path::Path) -> Vec<ContextFileEntry> {
    let mut skills = Vec::new();
    if skills_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(skills_dir)
    {
        let mut dirs: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        dirs.sort_by_key(|e| e.file_name());
        for entry in dirs {
            let skill_file = entry.path().join("SKILL.md");
            if skill_file.exists() {
                let tokens = estimate_tokens_from_file(&skill_file);
                if tokens > 0 {
                    skills.push(ContextFileEntry {
                        name: entry.file_name().to_string_lossy().to_string(),
                        tokens,
                    });
                }
            }
        }
    }
    skills.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));
    skills
}

fn collect_codex_mcp_tools(config_path: &std::path::Path) -> Vec<ContextFileEntry> {
    let Ok(content) = std::fs::read_to_string(config_path) else {
        return Vec::new();
    };

    let mut tools = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_block = String::new();

    let flush = |name: &mut Option<String>, block: &mut String, out: &mut Vec<ContextFileEntry>| {
        if let Some(name) = name.take() {
            let tokens = estimate_tokens(block).max(20);
            out.push(ContextFileEntry { name, tokens });
        }
        block.clear();
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            flush(&mut current_name, &mut current_block, &mut tools);

            if let Some(name) = trimmed
                .strip_prefix("[mcp_servers.")
                .and_then(|value| value.strip_suffix(']'))
                .map(|value| value.trim_matches('"').trim_matches('\'').to_string())
                .filter(|value| !value.is_empty())
            {
                current_name = Some(name);
                current_block.push_str(line);
                current_block.push('\n');
            }
            continue;
        }

        if current_name.is_some() {
            current_block.push_str(line);
            current_block.push('\n');
        }
    }

    flush(&mut current_name, &mut current_block, &mut tools);
    tools.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));
    tools
}

fn empty_context_breakdown(model: &str, context_window: u64) -> ContextBreakdown {
    ContextBreakdown {
        model: model.to_string(),
        context_window,
        used_tokens: 0,
        free_space: context_window,
        autocompact_buffer: 0,
        system_prompt: 0,
        system_tools: 0,
        memory_files: Vec::new(),
        memory_total: 0,
        skills: Vec::new(),
        skills_total: 0,
        messages: 0,
        mcp_tools: Vec::new(),
        mcp_total: 0,
    }
}

fn is_claude_session_idle(session: &ClaudeSessionSnapshot, idle: SystemTime) -> bool {
    session.last_activity < idle
}

fn is_codex_session_idle(session: &CodexSessionSnapshot, idle: SystemTime) -> bool {
    session.last_activity < idle
}

fn claude_context_window(session: &ClaudeSessionSnapshot) -> u64 {
    let model_id = session.model.as_deref().unwrap_or("");
    if cost::is_ga_1m_context(model_id)
        || model_id.contains("[1m]")
        || session.max_turn_api_input > 200_000
    {
        1_000_000
    } else {
        200_000
    }
}

fn claude_context_model(session: &ClaudeSessionSnapshot) -> String {
    session
        .model_display
        .clone()
        .or(session.model.as_ref().map(|m| cost::model_display_name(m)))
        .unwrap_or_else(|| "Unknown".into())
}

fn collect_claude_memory_files(
    claude_home: &std::path::Path,
    selected: Option<&ClaudeSessionSnapshot>,
) -> Vec<ContextFileEntry> {
    let mut memory_files = Vec::new();
    let global_claude_md = claude_home.join("CLAUDE.md");
    if global_claude_md.exists() {
        let tokens = estimate_tokens_from_file(&global_claude_md);
        if tokens > 0 {
            memory_files.push(ContextFileEntry {
                name: ".claude/CLAUDE.md".into(),
                tokens,
            });
        }
    }

    let rules_dir = claude_home.join("rules");
    if rules_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&rules_dir)
    {
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext == "md" || ext == "txt")
            })
            .collect();
        files.sort_by_key(|e| e.file_name());
        for entry in files {
            let path = entry.path();
            let tokens = estimate_tokens_from_file(&path);
            if tokens > 0 {
                memory_files.push(ContextFileEntry {
                    name: format!(
                        ".claude/rules/{}",
                        path.file_name().unwrap().to_string_lossy()
                    ),
                    tokens,
                });
            }
        }
    }

    if let Some(session) = selected {
        let project_claude = session.cwd.join("CLAUDE.md");
        if project_claude.exists() {
            let tokens = estimate_tokens_from_file(&project_claude);
            let name = format!("{}/CLAUDE.md", session.project_name);
            if tokens > 0 && !memory_files.iter().any(|f| f.name == name) {
                memory_files.push(ContextFileEntry { name, tokens });
            }
        }
    }

    memory_files.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));
    memory_files
}

fn collect_claude_mcp_tools(settings_file: &std::path::Path) -> Vec<ContextFileEntry> {
    let mut mcp_tools = Vec::new();
    if settings_file.exists()
        && let Ok(data) = std::fs::read_to_string(settings_file)
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&data)
        && let Some(servers) = json.get("mcpServers").and_then(|v| v.as_object())
    {
        for (name, config) in servers {
            let config_str = serde_json::to_string(config).unwrap_or_default();
            let tokens = estimate_tokens(&config_str).max(20);
            mcp_tools.push(ContextFileEntry {
                name: name.clone(),
                tokens,
            });
        }
    }
    mcp_tools.sort_by_key(|f| std::cmp::Reverse(f.tokens));
    mcp_tools
}

fn build_claude_context_breakdown(selected: Option<&ClaudeSessionSnapshot>) -> ContextBreakdown {
    let claude_home = cc_discord_presence::config::claude_home();
    let model = selected.map_or_else(|| "Unknown".into(), claude_context_model);
    let ctx_window = selected.map_or(200_000, claude_context_window);
    let current_context_tokens = selected.map_or(0, |s| s.current_context_tokens);
    let memory_files = collect_claude_memory_files(&claude_home, selected);
    let memory_total: u64 = memory_files.iter().map(|f| f.tokens).sum();
    let skills = collect_skills_from_dir(&claude_home.join("skills"));
    let skills_total: u64 = skills.iter().map(|f| f.tokens).sum();
    let mcp_tools = collect_claude_mcp_tools(&claude_home.join("settings.json"));
    let mcp_total: u64 = mcp_tools.iter().map(|f| f.tokens).sum();
    let used_tokens = current_context_tokens.min(ctx_window);
    let free_space = ctx_window.saturating_sub(used_tokens);

    ContextBreakdown {
        model,
        context_window: ctx_window,
        used_tokens,
        free_space,
        autocompact_buffer: 0,
        system_prompt: 0,
        system_tools: 0,
        memory_files,
        memory_total,
        skills,
        skills_total,
        messages: used_tokens,
        mcp_tools,
        mcp_total,
    }
}

fn build_codex_context_breakdown(selected: Option<&CodexSessionSnapshot>) -> ContextBreakdown {
    let codex_home = cc_discord_presence::codex::config::codex_home();
    let model = selected
        .and_then(|s| s.model.clone())
        .map(|model| {
            cc_discord_presence::codex::util::format_model_display(
                &model,
                selected.and_then(|session| session.reasoning_effort),
                resolve_service_tier().is_fast(),
            )
        })
        .unwrap_or_else(|| "Codex".to_string());
    let ctx_window = selected
        .and_then(|s| s.context_window.as_ref().map(|w| w.window_tokens))
        .or_else(|| {
            selected
                .and_then(|s| s.model.as_deref())
                .and_then(cc_discord_presence::codex::cost::default_model_context_window)
        })
        .unwrap_or(400_000);
    let used_tokens = selected
        .and_then(|s| {
            s.context_window
                .as_ref()
                .map(|w| w.used_tokens.min(ctx_window))
        })
        .unwrap_or(0);
    let mut memory_files = Vec::new();
    let mut seen_instruction_files = HashSet::new();
    let global_agents = codex_home.join("AGENTS.md");
    if global_agents.exists() {
        collect_instruction_tree(
            Provider::Codex,
            &global_agents,
            None,
            None,
            &mut seen_instruction_files,
            &mut memory_files,
        );
    }
    let generated_instructions = codex_home.join("generated-model-instructions.md");
    if generated_instructions.exists() {
        collect_instruction_tree(
            Provider::Codex,
            &generated_instructions,
            None,
            None,
            &mut seen_instruction_files,
            &mut memory_files,
        );
    }
    if let Some(session) = selected {
        let project_agents = session.cwd.join("AGENTS.md");
        if project_agents.exists() {
            collect_instruction_tree(
                Provider::Codex,
                &project_agents,
                Some(&session.cwd),
                Some(&session.project_name),
                &mut seen_instruction_files,
                &mut memory_files,
            );
        }
    }
    memory_files.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));
    let memory_total: u64 = memory_files.iter().map(|f| f.tokens).sum();
    let skills = collect_skills_from_dir(&codex_home.join("skills"));
    let skills_total: u64 = skills.iter().map(|f| f.tokens).sum();
    let mcp_tools = collect_codex_mcp_tools(&codex_home.join("config.toml"));
    let mcp_total: u64 = mcp_tools.iter().map(|f| f.tokens).sum();
    let free_space = ctx_window.saturating_sub(used_tokens);

    ContextBreakdown {
        model,
        context_window: ctx_window,
        used_tokens,
        free_space,
        autocompact_buffer: 0,
        system_prompt: 0,
        system_tools: 0,
        memory_files,
        memory_total,
        skills,
        skills_total,
        messages: used_tokens,
        mcp_tools,
        mcp_total,
    }
}

fn selected_claude_context_sessions<'a>(
    sessions: &'a [ClaudeSessionSnapshot],
    session_ids: Option<&[String]>,
) -> Vec<&'a ClaudeSessionSnapshot> {
    if let Some(ids) = session_ids
        && !ids.is_empty()
    {
        let mut seen = HashSet::new();
        return ids
            .iter()
            .filter(|id| seen.insert((*id).clone()))
            .filter_map(|id| sessions.iter().find(|s| s.session_id == *id))
            .collect();
    }

    let idle = SystemTime::now()
        .checked_sub(IDLE_CUTOFF)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let active: Vec<_> = sessions
        .iter()
        .filter(|session| !is_claude_session_idle(session, idle))
        .collect();
    if active.is_empty() {
        preferred_active_session(sessions).into_iter().collect()
    } else {
        active
    }
}

fn selected_codex_context_sessions<'a>(
    sessions: &'a [CodexSessionSnapshot],
    session_ids: Option<&[String]>,
) -> Vec<&'a CodexSessionSnapshot> {
    if let Some(ids) = session_ids
        && !ids.is_empty()
    {
        let mut seen = HashSet::new();
        return ids
            .iter()
            .filter(|id| seen.insert((*id).clone()))
            .filter_map(|id| sessions.iter().find(|s| s.session_id == *id))
            .collect();
    }

    let idle = SystemTime::now()
        .checked_sub(IDLE_CUTOFF)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let active: Vec<_> = sessions
        .iter()
        .filter(|session| !is_codex_session_idle(session, idle))
        .collect();
    if active.is_empty() {
        codex_session::preferred_active_session(sessions)
            .into_iter()
            .collect()
    } else {
        active
    }
}

fn claude_context_entry(
    session: &ClaudeSessionSnapshot,
    idle: SystemTime,
) -> SessionContextBreakdown {
    SessionContextBreakdown {
        session_id: session.session_id.clone(),
        project: session.project_name.clone(),
        model_id: session.model.clone().unwrap_or_default(),
        is_idle: is_claude_session_idle(session, idle),
        activity: session
            .activity
            .as_ref()
            .map_or("Idle".into(), |a| a.action_text().to_string()),
        breakdown: build_claude_context_breakdown(Some(session)),
    }
}

fn codex_context_entry(
    session: &CodexSessionSnapshot,
    idle: SystemTime,
) -> SessionContextBreakdown {
    SessionContextBreakdown {
        session_id: session.session_id.clone(),
        project: session.project_name.clone(),
        model_id: session.model.clone().unwrap_or_default(),
        is_idle: is_codex_session_idle(session, idle),
        activity: session
            .activity
            .as_ref()
            .map_or("Idle".into(), |a| a.action_text().to_string()),
        breakdown: build_codex_context_breakdown(Some(session)),
    }
}

#[tauri::command]
pub fn get_context_breakdown(
    session_id: Option<String>,
    provider: Option<String>,
) -> ContextBreakdown {
    get_context_breakdowns(session_id.map(|id| vec![id]), provider.clone())
        .into_iter()
        .next()
        .map(|entry| entry.breakdown)
        .unwrap_or_else(|| match provider.as_deref() {
            Some("codex") => empty_context_breakdown("Codex", 400_000),
            Some("claude") => empty_context_breakdown("Unknown", 200_000),
            Some("openai" | "anthropic" | "all") => empty_context_breakdown("Unknown", 0),
            _ => match current_provider() {
                Provider::Claude => empty_context_breakdown("Unknown", 200_000),
                Provider::Codex => empty_context_breakdown("Codex", 400_000),
                Provider::OpenCode | Provider::CommandCode => {
                    empty_context_breakdown("Not reported", 0)
                }
            },
        })
}

#[tauri::command]
pub fn get_context_breakdowns(
    session_ids: Option<Vec<String>>,
    provider: Option<String>,
) -> Vec<SessionContextBreakdown> {
    let idle = SystemTime::now()
        .checked_sub(IDLE_CUTOFF)
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let scope = provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| current_provider().as_str());
    match scope {
        "claude" => {
            let sessions = read_claude_sessions();
            selected_claude_context_sessions(&sessions, session_ids.as_deref())
                .into_iter()
                .map(|session| claude_context_entry(session, idle))
                .collect()
        }
        "codex" => {
            let sessions = read_codex_sessions();
            selected_codex_context_sessions(&sessions, session_ids.as_deref())
                .into_iter()
                .map(|session| codex_context_entry(session, idle))
                .collect()
        }
        "opencode" => opencode_context_entries(session_ids.as_deref()),
        "all" => {
            let claude_sessions = read_claude_sessions();
            let codex_sessions = read_codex_sessions();
            let mut entries: Vec<_> =
                selected_claude_context_sessions(&claude_sessions, session_ids.as_deref())
                    .into_iter()
                    .map(|session| claude_context_entry(session, idle))
                    .collect();
            entries.extend(
                selected_codex_context_sessions(&codex_sessions, session_ids.as_deref())
                    .into_iter()
                    .map(|session| codex_context_entry(session, idle)),
            );
            entries.extend(opencode_context_entries(session_ids.as_deref()));
            entries
        }
        // API access lanes have quota and spend identity but no local CLI
        // session store. Returning no context is honest; borrowing subscription
        // sessions here would cross the selected provider boundary.
        _ => Vec::new(),
    }
}

#[tauri::command]
pub fn get_sessions_context_usage(
    days: Option<i64>,
    provider: Option<String>,
) -> Vec<SessionContextUsage> {
    let mut seen = HashSet::new();
    let mut rows: Vec<SessionContextUsage> = get_context_breakdowns(None, provider.clone())
        .into_iter()
        .filter(|entry| seen.insert(entry.session_id.clone()))
        .map(|entry| {
            let breakdown = entry.breakdown;
            let used_tokens = breakdown.used_tokens.min(breakdown.context_window);
            let utilization_pct = context_utilization_pct(used_tokens, breakdown.context_window);
            SessionContextUsage {
                session_id: entry.session_id,
                project: entry.project,
                model: entry.model_id,
                model_display: breakdown.model,
                used_tokens,
                window_tokens: breakdown.context_window,
                utilization_pct,
                recommendation: context_recommendation(utilization_pct),
            }
        })
        .collect();

    rows.extend(
        crate::db::get_session_history_scoped(
            provider.as_deref(),
            Some(days.unwrap_or(30)),
            None,
            Some(5000),
        )
        .into_iter()
        .filter(|s| seen.insert(s.id.clone()))
        .map(|s| {
            let window_tokens = if s.window_tokens > 0 {
                s.window_tokens as u64
            } else if s.provider == "opencode" {
                0
            } else if cost::is_ga_1m_context(&s.model_id) || s.context_window == "1M" {
                1_000_000
            } else {
                200_000
            };
            let used_tokens = (s.used_tokens.max(0) as u64).min(window_tokens);
            let utilization_pct = context_utilization_pct(used_tokens, window_tokens);
            SessionContextUsage {
                session_id: s.id,
                project: s.project,
                model: s.model_id,
                model_display: s.model,
                used_tokens,
                window_tokens,
                utilization_pct,
                recommendation: context_recommendation(utilization_pct),
            }
        }),
    );
    rows
}

#[tauri::command]
pub fn get_project_stats(
    days: Option<i64>,
    provider: Option<String>,
) -> Vec<crate::db::ProjectStat> {
    crate::db::get_project_stats_scoped(provider.as_deref(), days)
}

#[tauri::command]
pub fn get_hourly_activity(
    days: Option<i64>,
    provider: Option<String>,
) -> Vec<crate::db::HourlyActivity> {
    crate::db::get_hourly_activity_scoped(provider.as_deref(), days)
}

#[tauri::command]
pub fn get_top_sessions(
    limit: Option<i64>,
    days: Option<i64>,
    provider: Option<String>,
) -> Vec<crate::db::HistoricalSession> {
    crate::db::get_top_sessions_scoped(provider.as_deref(), limit, days)
}

#[tauri::command]
pub fn get_cost_forecast(provider: Option<String>) -> crate::db::CostForecast {
    crate::db::get_cost_forecast_scoped(provider.as_deref())
}

#[tauri::command]
pub fn get_budget_status(provider: Option<String>) -> crate::db::BudgetStatus {
    crate::db::get_budget_status_scoped(provider.as_deref())
}

#[tauri::command]
pub fn set_budget(monthly_budget: f64, alert_threshold_pct: Option<f64>) -> Result<(), String> {
    if !monthly_budget.is_finite() || monthly_budget < 0.0 {
        return Err("monthlyBudget must be a finite, non-negative number".to_string());
    }
    if alert_threshold_pct
        .is_some_and(|threshold| !threshold.is_finite() || !(0.0..=100.0).contains(&threshold))
    {
        return Err("alertThresholdPct must be between 0 and 100".to_string());
    }
    crate::db::set_budget(monthly_budget, alert_threshold_pct)
}

#[tauri::command]
pub fn get_model_distribution(days: Option<i64>) -> Vec<(String, i64, f64)> {
    legacy_model_distribution(crate::db::get_model_distribution(days))
}

fn legacy_model_distribution(rows: Vec<crate::db::ModelStat>) -> Vec<(String, i64, f64)> {
    rows.into_iter()
        .map(|row| (row.model, row.session_count, row.total_cost))
        .collect()
}

#[tauri::command]
pub fn get_model_distribution_v2(
    days: Option<i64>,
    provider: Option<String>,
) -> Vec<crate::db::ModelStat> {
    crate::db::get_model_distribution_scoped(provider.as_deref(), days)
}

#[tauri::command]
pub fn export_all_data(provider: Option<String>) -> serde_json::Value {
    crate::db::export_all_data_scoped(provider.as_deref())
}

#[tauri::command]
pub fn clear_history(provider: Option<String>) -> Result<i64, String> {
    crate::db::clear_history_scoped(provider.as_deref()).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_db_size() -> u64 {
    crate::db::get_db_size_bytes()
}

#[tauri::command]
pub async fn generate_html_report(
    days: Option<i64>,
    project: Option<String>,
    provider: Option<String>,
) -> String {
    offload(move || {
        crate::report::generate_html_report_scoped(provider.as_deref(), days, project.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn generate_markdown_report(
    days: Option<i64>,
    project: Option<String>,
    provider: Option<String>,
) -> String {
    offload(move || {
        crate::report::generate_markdown_report_scoped(
            provider.as_deref(),
            days,
            project.as_deref(),
        )
    })
    .await
}

async fn offload<T, F>(work: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .expect("analyzer blocking task panicked")
}

fn analyzer_roots() -> (Vec<PathBuf>, Vec<PathBuf>) {
    (
        cc_discord_presence::config::projects_paths(),
        cc_discord_presence::codex::config::sessions_paths(),
    )
}

fn analyzer_provider() -> Provider {
    current_provider()
}

#[tauri::command]
pub async fn get_cache_health(
    days: Option<i64>,
    provider: Option<String>,
) -> crate::analyzers::cache_health::CacheHealthReport {
    offload(move || reports_bundle_blocking(days.unwrap_or(30), None, provider).cache_health).await
}

#[tauri::command]
pub async fn get_inflection_points(
    days: Option<i64>,
    provider: Option<String>,
) -> Vec<crate::analyzers::inflection::InflectionPoint> {
    offload(move || reports_bundle_blocking(days.unwrap_or(30), None, provider).inflection_points)
        .await
}

#[tauri::command]
pub async fn get_model_routing(
    days: Option<i64>,
    provider: Option<String>,
) -> Option<crate::analyzers::model_routing::ModelRoutingReport> {
    offload(move || reports_bundle_blocking(days.unwrap_or(30), None, provider).model_routing).await
}

#[tauri::command]
pub async fn get_tool_frequency(
    days: Option<i64>,
    provider: Option<String>,
) -> crate::analyzers::tool_frequency::ToolFrequencyReport {
    offload(move || reports_bundle_blocking(days.unwrap_or(30), None, provider).tool_frequency)
        .await
}

#[tauri::command]
pub async fn get_prompt_complexity(
    days: Option<i64>,
    provider: Option<String>,
) -> crate::analyzers::prompt_complexity::PromptComplexityReport {
    offload(move || reports_bundle_blocking(days.unwrap_or(30), None, provider).prompt_complexity)
        .await
}

#[tauri::command]
pub async fn get_session_health(
    days: Option<i64>,
    provider: Option<String>,
) -> crate::analyzers::session_health::SessionHealthReport {
    offload(move || reports_bundle_blocking(days.unwrap_or(30), None, provider).session_health)
        .await
}

#[tauri::command]
pub async fn get_trace_overview(
    days: Option<i64>,
    provider: Option<String>,
) -> crate::analyzers::trace_overview::TraceOverview {
    offload(move || reports_bundle_blocking(days.unwrap_or(30), None, provider).trace_overview)
        .await
}

#[tauri::command]
pub async fn get_recommendations(
    days: Option<i64>,
    provider: Option<String>,
) -> Vec<crate::analyzers::recommendations::Recommendation> {
    offload(move || reports_bundle_blocking(days.unwrap_or(30), None, provider).recommendations)
        .await
}

/// Look up a recommendation by id and return its `fix_prompt` so the frontend
/// can `navigator.clipboard.writeText(...)` it. Returns an empty string if
/// no matching recommendation exists for the requested data window.
#[tauri::command]
pub async fn copy_fix_prompt(
    rec_id: String,
    days: Option<i64>,
    provider: Option<String>,
) -> String {
    offload(move || {
        reports_bundle_blocking(days.unwrap_or(30), None, provider)
            .recommendations
            .into_iter()
            .find(|r| r.id == rec_id)
            .map(|r| r.fix_prompt)
            .unwrap_or_default()
    })
    .await
}

#[derive(Serialize, Clone)]
pub struct ReportsBundle {
    pub provider: String,
    pub capabilities: cc_discord_presence::provider::ProviderCapabilities,
    pub days: i64,
    pub total_sessions: usize,
    pub priced_sessions: usize,
    pub cost_basis: crate::db::CostBasis,
    pub cost_sources: Vec<String>,
    pub recommendations: Vec<crate::analyzers::recommendations::Recommendation>,
    pub trace_overview: crate::analyzers::trace_overview::TraceOverview,
    pub tool_frequency: crate::analyzers::tool_frequency::ToolFrequencyReport,
    pub prompt_complexity: crate::analyzers::prompt_complexity::PromptComplexityReport,
    pub session_health: crate::analyzers::session_health::SessionHealthReport,
    pub cache_health: crate::analyzers::cache_health::CacheHealthReport,
    pub model_routing: Option<crate::analyzers::model_routing::ModelRoutingReport>,
    pub inflection_points: Vec<crate::analyzers::inflection::InflectionPoint>,
    /// Daily spend series for the requested window, oldest first, with every
    /// day in range present (zero-filled). The Reports timeline plots this
    /// directly, so gaps must be explicit days rather than missing points.
    pub daily_costs: Vec<DailyCostPoint>,
}

/// One day on the Reports cost timeline.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct DailyCostPoint {
    pub date: String,
    pub cost: f64,
    pub sessions: i64,
    pub priced_sessions: i64,
    pub cost_basis: crate::db::CostBasis,
    pub cost_sources: Vec<String>,
}

fn cached_reports_bundle(
    days: u32,
    project: Option<String>,
    provider: String,
    build: impl FnOnce() -> ReportsBundle,
) -> ReportsBundle {
    let key = (days, project, provider);
    let mut cache = REPORTS_BUNDLE_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((cached_at, cached_key, cached_bundle)) = cache.as_ref()
        && cached_at.elapsed() < REPORTS_BUNDLE_CACHE_TTL
        && cached_key == &key
    {
        return cached_bundle.clone();
    }

    let bundle = build();
    *cache = Some((Instant::now(), key, bundle.clone()));
    bundle
}

#[tauri::command]
pub async fn get_reports_bundle(
    days: Option<i64>,
    project: Option<String>,
    provider: Option<String>,
) -> ReportsBundle {
    let scope = provider.unwrap_or_else(|| analyzer_provider().as_str().to_string());
    let cache_days = u32::try_from(days.unwrap_or(30)).unwrap_or(30);
    let (claude_roots, codex_roots) = analyzer_roots();
    offload(move || {
        cached_reports_bundle(cache_days, project.clone(), scope.clone(), || {
            let sessions = crate::db::get_session_history_scoped(
                Some(&scope),
                Some(days.unwrap_or(30)),
                project.as_deref(),
                Some(5000),
            );
            let daily_costs =
                window_daily_costs_scoped(&scope, days.unwrap_or(30), project.as_deref());
            build_reports_bundle_for_scope_from_roots(
                &scope,
                days,
                sessions,
                daily_costs,
                claude_roots,
                codex_roots,
            )
        })
    })
    .await
}

/// Synchronous form of [`get_reports_bundle`] for callers that are already off
/// the UI thread (currently the debug-only dev bridge).
pub fn reports_bundle_blocking(
    days: i64,
    project: Option<String>,
    provider: Option<String>,
) -> ReportsBundle {
    let scope = provider.unwrap_or_else(|| analyzer_provider().as_str().to_string());
    let (claude_roots, codex_roots) = analyzer_roots();
    let sessions = crate::db::get_session_history_scoped(
        Some(&scope),
        Some(days),
        project.as_deref(),
        Some(5000),
    );
    let daily_costs = window_daily_costs_scoped(&scope, days, project.as_deref());
    build_reports_bundle_for_scope_from_roots(
        &scope,
        Some(days),
        sessions,
        daily_costs,
        claude_roots,
        codex_roots,
    )
}

/// Window-wide monetary totals for the Cost Analysis KPIs.
///
/// The view lists a capped page of recent sessions for its table, but a KPI
/// that describes a 30-day value has to cover the whole window. Summing the
/// visible page instead understates the value by exactly the sessions that
/// did not fit — which is how the screen came to report $7.73 against an
/// actual $7,371.35.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct CostTotals {
    pub days: i64,
    pub sessions: usize,
    pub priced_sessions: usize,
    pub cost_basis: crate::db::CostBasis,
    pub cost_sources: Vec<String>,
    /// Provider billing readback only. `None` means no such observation exists.
    pub billed_spend_usd: Option<f64>,
    /// Token usage priced with documented API rates, never provider-billed spend.
    pub api_equivalent_usd: Option<f64>,
    pub billed_sessions: usize,
    pub api_equivalent_sessions: usize,
    pub total_cost: f64,
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_write_cost: f64,
    pub cache_read_cost: f64,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_write_tokens: i64,
    pub cache_read_tokens: i64,
    /// Input tokens that were neither written to nor read from cache. Paired
    /// with `input_cost` this yields the window's true per-token input rate,
    /// which is what the cache-savings estimate needs.
    pub pure_input_tokens: i64,
    /// Window-wide spend per model, highest first. Computed here so the
    /// breakdown always reconciles with `total_cost` instead of describing
    /// only the page of sessions the table happens to show.
    pub by_model: Vec<CostSlice>,
    /// Window-wide spend per project, highest first.
    pub by_project: Vec<CostSlice>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct CostSlice {
    pub label: String,
    pub cost: f64,
    pub sessions: usize,
}

#[tauri::command]
pub async fn get_cost_totals(
    days: Option<i64>,
    project: Option<String>,
    provider: Option<String>,
) -> CostTotals {
    offload(move || cost_totals_blocking(days.unwrap_or(30), project, provider)).await
}

pub fn cost_totals_blocking(
    days: i64,
    project: Option<String>,
    provider: Option<String>,
) -> CostTotals {
    // No limit: these are aggregates over the whole window by definition.
    let sessions = crate::db::get_session_history_scoped(
        provider.as_deref(),
        Some(days),
        project.as_deref(),
        None,
    );
    aggregate_cost_totals(days, &sessions)
}

pub(crate) fn aggregate_cost_totals(
    days: i64,
    sessions: &[crate::db::HistoricalSession],
) -> CostTotals {
    use std::collections::HashMap;

    let coverage = crate::db::summarize_cost_provenance(sessions.iter().map(|session| {
        (
            session.cost_basis,
            session.cost_source.as_str(),
            session.known_cost,
        )
    }));
    let mut totals = CostTotals {
        days,
        sessions: sessions.len(),
        priced_sessions: coverage.priced_sessions,
        cost_basis: coverage.cost_basis,
        cost_sources: coverage.cost_sources,
        ..Default::default()
    };
    let mut by_model: HashMap<&str, (f64, usize)> = HashMap::new();
    let mut by_project: HashMap<&str, (f64, usize)> = HashMap::new();

    for s in sessions {
        totals.total_tokens += s.total_tokens;
        totals.input_tokens += s.input_tokens;
        totals.output_tokens += s.output_tokens;
        totals.cache_write_tokens += s.cache_write_tokens;
        totals.cache_read_tokens += s.cache_read_tokens;
        totals.pure_input_tokens +=
            (s.input_tokens - s.cache_write_tokens - s.cache_read_tokens).max(0);
        let Some(known_cost) = s.known_cost else {
            continue;
        };
        if s.cost_basis == crate::db::CostBasis::Unavailable {
            continue;
        }
        if !known_cost.is_finite() || known_cost < 0.0 {
            continue;
        }
        match crate::db::monetary_provenance(&s.cost_source) {
            crate::db::MonetaryProvenance::ProviderBilled => {
                totals.billed_sessions += 1;
                *totals.billed_spend_usd.get_or_insert(0.0) += known_cost;
            }
            crate::db::MonetaryProvenance::ApiEquivalent => {
                totals.api_equivalent_sessions += 1;
                *totals.api_equivalent_usd.get_or_insert(0.0) += known_cost;
            }
            crate::db::MonetaryProvenance::Other => {}
        }
        totals.total_cost += known_cost;
        totals.input_cost += s.input_cost;
        totals.output_cost += s.output_cost;
        totals.cache_write_cost += s.cache_write_cost;
        totals.cache_read_cost += s.cache_read_cost;

        let m = by_model.entry(s.model.as_str()).or_insert((0.0, 0));
        m.0 += known_cost;
        m.1 += 1;
        let p = by_project.entry(s.project.as_str()).or_insert((0.0, 0));
        p.0 += known_cost;
        p.1 += 1;
    }

    totals.by_model = into_sorted_slices(by_model);
    totals.by_project = into_sorted_slices(by_project);
    totals
}

/// Sorts a label -> (cost, sessions) map into descending-cost slices.
fn into_sorted_slices(map: std::collections::HashMap<&str, (f64, usize)>) -> Vec<CostSlice> {
    let mut slices: Vec<CostSlice> = map
        .into_iter()
        .map(|(label, (cost, sessions))| CostSlice {
            label: label.to_string(),
            cost,
            sessions,
        })
        .collect();
    slices.sort_by(|a, b| {
        b.cost
            .partial_cmp(&a.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
            // Stable tie-break so equal-cost rows do not reshuffle between polls.
            .then_with(|| a.label.cmp(&b.label))
    });
    slices
}

#[derive(Serialize)]
pub struct CostsBundle {
    pub history: Vec<crate::db::HistoricalSession>,
    pub forecast: crate::db::CostForecast,
    pub budget: crate::db::BudgetStatus,
    pub totals: CostTotals,
    pub daily_usage: Vec<crate::db::DailyStat>,
}

#[tauri::command]
pub async fn get_costs_bundle(project: Option<String>, provider: Option<String>) -> CostsBundle {
    offload(move || costs_bundle_blocking(project, provider)).await
}

pub fn costs_bundle_blocking(project: Option<String>, provider: Option<String>) -> CostsBundle {
    let data = crate::db::get_costs_data_scoped(provider.as_deref(), 30, project.as_deref(), 200);
    CostsBundle {
        history: data.history,
        forecast: data.forecast,
        budget: data.budget,
        totals: aggregate_cost_totals(30, &data.aggregate_sessions),
        daily_usage: data.daily_usage,
    }
}

/// First calendar date plotted for a `days`-long window, inclusive.
///
/// The series spans today plus `days - 1` earlier dates. Aggregating from this
/// date keeps the boundary day whole, so a session that lands earlier in the
/// day than the rolling `now - days` cutoff still appears on the curve that
/// describes it.
fn window_start_date(days: i64) -> chrono::NaiveDate {
    let span = days.max(1);
    chrono::Utc::now().date_naive() - chrono::Duration::days(span - 1)
}

/// Reads window-wide daily spend straight from SQLite.
///
/// Separate from the analyzer session page on purpose: the analyzers accept a
/// capped page of recent rows, but a timeline built from that page would
/// zero-fill days whose sessions were discarded.
fn window_daily_costs_scoped(
    provider: &str,
    days: i64,
    project: Option<&str>,
) -> Vec<crate::db::DailyCostRow> {
    let from = window_start_date(days).format("%Y-%m-%d").to_string();
    crate::db::get_daily_costs_scoped(Some(provider), &from, project)
}

/// Zero-fills SQL daily totals into one point per day, oldest first.
///
/// Zero-filling matters for the timeline: a day with no sessions is a real
/// observation (you did not spend), not a missing sample. Plotting only the
/// days that exist would silently compress idle stretches and distort the
/// shape of the curve.
fn daily_cost_series(rows: &[crate::db::DailyCostRow], days: i64) -> Vec<DailyCostPoint> {
    use std::collections::HashMap;

    let by_date: HashMap<&str, &crate::db::DailyCostRow> =
        rows.iter().map(|row| (row.date.as_str(), row)).collect();

    let span = days.max(1);
    let start = window_start_date(days);
    (0..span)
        .map(|offset| {
            let date = (start + chrono::Duration::days(offset))
                .format("%Y-%m-%d")
                .to_string();
            if let Some(row) = by_date.get(date.as_str()).copied() {
                DailyCostPoint {
                    date,
                    cost: row.cost,
                    sessions: row.sessions,
                    priced_sessions: row.priced_sessions,
                    cost_basis: row.cost_basis,
                    cost_sources: row.cost_sources.clone(),
                }
            } else {
                DailyCostPoint {
                    date,
                    cost: 0.0,
                    sessions: 0,
                    priced_sessions: 0,
                    cost_basis: crate::db::CostBasis::Unavailable,
                    cost_sources: Vec::new(),
                }
            }
        })
        .collect()
}

pub fn build_reports_bundle_from_roots(
    provider: Provider,
    days: Option<i64>,
    sessions: Vec<crate::db::HistoricalSession>,
    daily_costs: Vec<crate::db::DailyCostRow>,
    claude_roots: Vec<PathBuf>,
    codex_roots: Vec<PathBuf>,
) -> ReportsBundle {
    build_reports_bundle_for_scope_from_roots(
        provider.as_str(),
        days,
        sessions,
        daily_costs,
        claude_roots,
        codex_roots,
    )
}

fn build_reports_bundle_for_scope_from_roots(
    scope: &str,
    days: Option<i64>,
    sessions: Vec<crate::db::HistoricalSession>,
    daily_costs: Vec<crate::db::DailyCostRow>,
    claude_roots: Vec<PathBuf>,
    codex_roots: Vec<PathBuf>,
) -> ReportsBundle {
    let provider = Provider::parse(scope);
    let cost_coverage = crate::db::summarize_cost_provenance(sessions.iter().map(|session| {
        (
            session.cost_basis,
            session.cost_source.as_str(),
            session.known_cost,
        )
    }));
    let traces = crate::analyzers::session_trace::load_session_traces_from_roots(
        &sessions,
        claude_roots,
        codex_roots,
    );

    let diagnostic_provider = provider.unwrap_or(Provider::Codex);
    let mut cache_health =
        crate::analyzers::cache_health::analyze_for_provider(diagnostic_provider, &sessions);
    let capabilities = provider.map_or(
        cc_discord_presence::provider::ProviderCapabilities {
            cache_health: true,
            model_routing: false,
            extra_usage: false,
        },
        Provider::capabilities,
    );
    let model_routing = provider
        .filter(|provider| provider.capabilities().model_routing)
        .map(|_| crate::analyzers::model_routing::analyze(&sessions));
    let inflection_points = provider.map_or_else(Vec::new, |provider| {
        crate::analyzers::inflection::detect_for_provider(provider, &sessions)
    });
    let tool_frequency = crate::analyzers::tool_frequency::analyze(&sessions, &traces);
    let prompt_complexity = crate::analyzers::prompt_complexity::analyze(&sessions, &traces);
    let session_health = crate::analyzers::session_health::analyze(
        &sessions,
        &traces,
        &tool_frequency,
        &prompt_complexity,
    );
    let mut trace_overview = crate::analyzers::trace_overview::build(
        diagnostic_provider,
        &sessions,
        &traces,
        cache_health.trend_weighted_ratio,
    );
    let recommendations = provider
        .filter(|provider| !matches!(provider, Provider::OpenCode | Provider::CommandCode))
        .map_or_else(Vec::new, |provider| {
            let ctx = crate::analyzers::recommendations::AnalysisContext {
                provider,
                sessions: &sessions,
                cache: &cache_health,
                routing: model_routing.as_ref(),
                inflections: &inflection_points,
                tool_frequency: Some(&tool_frequency),
                prompt_complexity: Some(&prompt_complexity),
                session_health: Some(&session_health),
            };
            crate::analyzers::recommendations::generate(&ctx)
        });

    if provider.is_none() {
        let provider_display = match scope {
            "all" => "All providers",
            "openai" => "OpenAI API",
            "anthropic" => "Anthropic API",
            _ => "Selected provider",
        };
        cache_health.diagnosis = if sessions.is_empty() {
            format!("No {provider_display} session telemetry is available for this window.")
        } else {
            format!(
                "Cache health is aggregated across {provider_display}; select one subscription provider for instruction-file guidance."
            )
        };
        trace_overview.provider = scope.to_string();
        trace_overview.provider_display = provider_display.to_string();
        trace_overview.instruction_file = "Provider-specific".to_string();
        trace_overview.fix_button_label = "Select one provider for fixes".to_string();
        trace_overview.session_store = "Provider-scoped session stores".to_string();
        trace_overview.global_state_source = "Provider-scoped telemetry".to_string();
        trace_overview.telemetry_mermaid = format!(
            "flowchart LR\n  Sources[\"Provider-scoped session stores\"] --> Pulse[\"Pulse analytics\\n{} sessions\"]\n  Pulse --> Reports[\"Aggregated reports\"]",
            sessions.len()
        );
        trace_overview.cache_mermaid = format!(
            "flowchart LR\n  Input[\"Selected provider input\"] --> Cache[\"Cache reuse\\n{:.1}% hit ratio\"]\n  Cache --> Reports[\"Aggregated reports\"]",
            cache_health.trend_weighted_ratio
        );
    }

    ReportsBundle {
        provider: scope.to_string(),
        capabilities,
        days: days.unwrap_or(30),
        total_sessions: sessions.len(),
        priced_sessions: cost_coverage.priced_sessions,
        cost_basis: cost_coverage.cost_basis,
        cost_sources: cost_coverage.cost_sources,
        daily_costs: daily_cost_series(&daily_costs, days.unwrap_or(30)),
        recommendations,
        trace_overview,
        tool_frequency,
        prompt_complexity,
        session_health,
        cache_health,
        model_routing,
        inflection_points,
    }
}

fn build_opencode_settings(cached: &CachedData, config: &OpenCodeConfig) -> DiscordSettings {
    DiscordSettings {
        provider: "opencode".into(),
        enabled: config.enabled,
        status: cached.discord_status.clone(),
        publisher: cached.discord_publisher.clone(),
        display_prefs: crate::opencode::prefs(config),
        desktop_design: None,
        supports_desktop_design: false,
        supports_field_order: true,
        supports_credits: false,
        field_order: config
            .layout
            .fields
            .iter()
            .map(|item| item.field.as_str().into())
            .collect(),
    }
}

fn opencode_context_entries(ids: Option<&[String]>) -> Vec<SessionContextBreakdown> {
    shared()
        .lock()
        .map(|data| {
            data.opencode_sessions
                .iter()
                .filter(|session| ids.is_none_or(|ids| ids.contains(&session.id)))
                .map(|session| {
                    let window = session.context_window.unwrap_or(0);
                    let used = session.context_used.unwrap_or(0).min(window);
                    let mut breakdown =
                        empty_context_breakdown(&session.metadata.model_name, window);
                    breakdown.used_tokens = used;
                    breakdown.free_space = window.saturating_sub(used);
                    SessionContextBreakdown {
                        session_id: session.id.clone(),
                        project: session.project.clone(),
                        model_id: session.metadata.model_id.clone(),
                        is_idle: session.is_idle(chrono::Utc::now().timestamp_millis()),
                        activity: session.activity.clone(),
                        breakdown,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn resolve_codex_plan(
    sessions: &[CodexSessionSnapshot],
    config: &CodexPresenceConfig,
    envelopes: &[cc_discord_presence::codex::telemetry::limits::RateLimitEnvelope],
) -> cc_discord_presence::codex::telemetry::plan::ResolvedPlan {
    let mut detector = PlanDetector::new();
    if envelopes.is_empty() {
        detector.resolve_from_sessions(sessions, &config.openai_plan)
    } else {
        detector.resolve_from_envelopes(envelopes, &config.openai_plan)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        REPORTS_BUNDLE_CACHE, aggregate_cost_totals, build_reports_bundle_for_scope_from_roots,
        cached_reports_bundle,
    };
    use super::{
        active_access_route, active_presence_presentation, api_probe_route,
        build_claude_context_breakdown, build_claude_session_infos, build_codex_discord_preview,
        build_codex_discord_preview_with_access, build_codex_session_infos,
        claude_route_from_usage, codex_fallback_surface, codex_plan_key_from_tier,
        codex_session_surface, codex_total_input_tokens, cost_availability, daily_cost_series,
        dismiss_notification, extract_discord_user, get_notifications,
        get_unread_notification_count, mark_all_notifications_read, mark_notification_read,
        merge_access_routes, plan_key_from_override, route_for_provider,
        route_limits_for_claude_presence, route_limits_for_presence, scan_discord_leveldb_dirs,
        semantic_snapshot_fingerprint,
    };
    use crate::access::{
        AccessAvailability, AccessFreshness, AccessProof, AccessProvenance, AccessRouteSnapshot,
        AccessSnapshot, AccessSourceKind, AccessWindow, AuthMethod, access_route_from_usage,
        subscription_source,
    };
    use crate::notifications::NotificationStore;
    use cc_discord_presence::codex::config::{
        DesktopPresenceDesign, DisplayConfig, OpenAiPlanMode,
        PresenceConfig as TestCodexPresenceConfig, PresenceSurface as TestPresenceSurface,
        PrivacyConfig,
    };
    use cc_discord_presence::codex::cost::{
        CostAttribution, PricingSource, PricingStatus, TokenCostBreakdown,
    };
    use cc_discord_presence::codex::model::{SessionSpeed, SpeedMode, SpeedSource};
    use cc_discord_presence::codex::session::{
        CodexSessionSnapshot, ContextWindowSnapshot, ContextWindowSource,
    };
    use cc_discord_presence::codex::telemetry::limits::RateLimits;
    use cc_discord_presence::codex::telemetry::plan::DetectedPlanTier;
    use cc_discord_presence::codex::telemetry::plan::PlanDetector;
    use cc_discord_presence::codex::telemetry::service_tier::resolve_service_tier;
    use cc_discord_presence::config::PresenceConfig as TestClaudePresenceConfig;
    use cc_discord_presence::cost;
    use cc_discord_presence::provider::Provider;

    use cc_discord_presence::session::{ClaudeSessionSnapshot, DataSource, ReasoningEffort, Speed};
    use cc_discord_presence::usage::{ExtraUsage, UsageData, UsageWindow};
    use codex_presence_core::{
        QuotaScope, QuotaWindow, RateLimitScope, UsageSignal, UsageSnapshot, UsageSource,
    };
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{Duration, SystemTime};

    static REPORTS_BUNDLE_CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn quota_usage(
        provider: &str,
        signal: UsageSignal,
        windows: Vec<QuotaWindow>,
    ) -> UsageSnapshot {
        UsageSnapshot {
            source: UsageSource::new(format!("{provider}-subscription:default"), [signal]),
            scopes: vec![QuotaScope {
                id: Some(provider.to_string()),
                name: None,
                kind: RateLimitScope::GlobalAccount,
                windows,
            }],
            credits: None,
            observed_at: Some(chrono::Utc::now()),
            provenance_source: "deterministic fixture".to_string(),
        }
    }

    fn discord_user_record(
        user_id: &str,
        username: &str,
        global_name: Option<&str>,
        avatar_hash: &str,
        padding: usize,
    ) -> Vec<u8> {
        let global_name = global_name
            .map(|name| format!(r#", "global_name":"{name}""#))
            .unwrap_or_default();
        format!(
            r#"{{"id":"{user_id}","username":"{username}"{}{},"discriminator":"0","avatar":"{avatar_hash}"}}{}"#,
            " ".repeat(padding),
            global_name,
            " ".repeat(120)
        )
        .into_bytes()
    }

    fn discord_user_fixture(global_name: Option<&str>, padding: usize) -> Vec<u8> {
        discord_user_record(
            "123456789012345678",
            "pulse-user",
            global_name,
            "avatar-hash",
            padding,
        )
    }

    fn discord_test_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pulse-discord-user-{tag}-{}-{:?}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create discord test dir");
        dir
    }

    #[test]
    fn discord_user_extracts_global_name_from_bounded_record_region() {
        let data = discord_user_fixture(Some("Pulse Display"), 700);

        let user = extract_discord_user(&data).expect("discord user");

        assert_eq!(user.global_name.as_deref(), Some("Pulse Display"));
    }

    #[test]
    fn discord_user_treats_empty_or_missing_global_name_as_absent() {
        let empty = extract_discord_user(&discord_user_fixture(Some(""), 0)).expect("discord user");
        let missing = extract_discord_user(&discord_user_fixture(None, 0)).expect("discord user");

        assert_eq!(empty.global_name, None);
        assert_eq!(missing.global_name, None);
    }

    #[test]
    fn discord_user_scan_skips_unreadable_entry_and_uses_later_file() {
        let dir = discord_test_dir("unreadable");
        std::fs::create_dir(dir.join("newest.ldb")).expect("create unreadable directory entry");
        std::fs::write(dir.join("older.log"), discord_user_fixture(None, 0))
            .expect("write valid discord record");

        let user = scan_discord_leveldb_dirs(std::slice::from_ref(&dir)).expect("discord user");

        assert_eq!(user.username, "pulse-user");
        std::fs::remove_dir_all(dir).expect("remove discord test dir");
    }

    #[test]
    fn discord_user_scan_falls_back_to_second_candidate_directory() {
        let first = discord_test_dir("first-empty");
        let second = discord_test_dir("second-valid");
        std::fs::write(first.join("000001.log"), b"not a discord user")
            .expect("write empty candidate");
        std::fs::write(second.join("000002.ldb"), discord_user_fixture(None, 0))
            .expect("write valid candidate");

        let user = scan_discord_leveldb_dirs(&[first.clone(), second.clone()])
            .expect("discord user from second candidate");

        assert_eq!(user.user_id, "123456789012345678");
        std::fs::remove_dir_all(first).expect("remove first discord test dir");
        std::fs::remove_dir_all(second).expect("remove second discord test dir");
    }

    #[test]
    fn discord_user_scan_uses_newest_file_across_candidate_directories() {
        let first = discord_test_dir("first-older");
        let second = discord_test_dir("second-newer");
        let older_path = first.join("000001.ldb");
        let newer_path = second.join("000002.ldb");
        std::fs::write(
            &older_path,
            discord_user_record("111111111111111111", "old-user", None, "old-avatar", 0),
        )
        .expect("write older discord record");
        std::fs::write(
            &newer_path,
            discord_user_record("222222222222222222", "current-user", None, "new-avatar", 0),
        )
        .expect("write newer discord record");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&older_path)
            .expect("open older discord record")
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
            .expect("set older modification time");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&newer_path)
            .expect("open newer discord record")
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_100))
            .expect("set newer modification time");

        let user = scan_discord_leveldb_dirs(&[first.clone(), second.clone()])
            .expect("discord user from globally newest file");

        assert_eq!(user.user_id, "222222222222222222");
        assert_eq!(user.username, "current-user");
        std::fs::remove_dir_all(first).expect("remove first discord test dir");
        std::fs::remove_dir_all(second).expect("remove second discord test dir");
    }

    #[test]
    fn discord_user_extract_prefers_richer_later_record() {
        let mut data = discord_user_record("111111111111111111", "bare-user", None, "", 0);
        data.extend_from_slice(&discord_user_record(
            "222222222222222222",
            "current-user",
            Some("Current Display"),
            "current-avatar",
            0,
        ));

        let user = extract_discord_user(&data).expect("best discord user");

        assert_eq!(user.user_id, "222222222222222222");
        assert_eq!(user.global_name.as_deref(), Some("Current Display"));
        assert_eq!(user.avatar_hash, "current-avatar");
    }

    #[test]
    fn discord_user_extract_resolves_single_clean_record() {
        let user = extract_discord_user(&discord_user_fixture(Some("Pulse Display"), 0))
            .expect("single discord user");

        assert_eq!(user.user_id, "123456789012345678");
        assert_eq!(user.username, "pulse-user");
    }

    #[test]
    fn discord_user_never_mixes_fields_from_a_following_profile() {
        // Trailing filler mirrors real LevelDB bulk: the extractor's outer loop
        // only keeps scanning while a record-sized lookahead remains.
        let mut data = br#"{"id":"123456789012345678"}"#.to_vec();
        data.extend_from_slice(
            br#"{"id":"987654321098765432","username":"next-account","avatar":"hashB"}"#,
        );
        data.extend_from_slice(b" ".repeat(120).as_slice());

        let user = extract_discord_user(&data).expect("second profile resolves");

        assert_eq!(user.user_id, "987654321098765432");
        assert_eq!(user.username, "next-account");
        assert_eq!(user.avatar_hash, "hashB");
    }

    #[test]
    fn discord_user_global_name_decodes_escapes_and_survives_braces_in_values() {
        let mut data =
            br#"{"id":"123456789012345678","username":"brace-user","note":"evil } tail","global_name":"Pulse \"Dev\""}"#
                .to_vec();
        data.extend_from_slice(b" ".repeat(120).as_slice());

        let user = extract_discord_user(&data).expect("discord user");

        assert_eq!(user.global_name.as_deref(), Some("Pulse \"Dev\""));
        assert_eq!(user.username, "brace-user");
    }

    fn proofed_route(
        provider: &str,
        signal: UsageSignal,
        windows: Vec<QuotaWindow>,
    ) -> AccessRouteSnapshot {
        let mut source = subscription_source(provider, None);
        source.proof = AccessProof::QuotaResponse;
        access_route_from_usage(
            source,
            quota_usage(provider, signal, windows),
            chrono::Utc::now(),
            chrono::Duration::seconds(30),
            chrono::Utc::now(),
        )
    }

    #[test]
    fn simultaneous_codex_and_claude_routes_are_retained() {
        let codex = proofed_route(
            "codex",
            UsageSignal::CodexSubscriptionUsage,
            vec![QuotaWindow {
                window_minutes: 7_777,
                used_percent: 12.0,
                remaining_percent: 88.0,
                resets_at: None,
            }],
        );
        let claude = proofed_route(
            "claude",
            UsageSignal::ClaudeSubscriptionUsage,
            vec![QuotaWindow {
                window_minutes: 2_345,
                used_percent: 34.0,
                remaining_percent: 66.0,
                resets_at: None,
            }],
        );
        let snapshot = merge_access_routes(&[], [codex, claude]);

        assert_eq!(snapshot.routes.len(), 2);
        assert!(
            snapshot
                .routes
                .iter()
                .any(|route| route.source.kind == AccessSourceKind::CodexSubscription)
        );
        assert!(
            snapshot
                .routes
                .iter()
                .any(|route| route.source.kind == AccessSourceKind::ClaudeSubscription)
        );
    }

    #[test]
    fn active_provider_selection_does_not_depend_on_route_order() {
        let codex = proofed_route(
            "codex",
            UsageSignal::CodexSubscriptionUsage,
            vec![QuotaWindow {
                window_minutes: 17,
                used_percent: 1.0,
                remaining_percent: 99.0,
                resets_at: None,
            }],
        );
        let claude = proofed_route(
            "claude",
            UsageSignal::ClaudeSubscriptionUsage,
            vec![QuotaWindow {
                window_minutes: 4_321,
                used_percent: 2.0,
                remaining_percent: 98.0,
                resets_at: None,
            }],
        );
        let mut data = super::CachedData {
            active_provider: Provider::Claude,
            access: AccessSnapshot {
                routes: vec![codex, claude],
            },
            ..Default::default()
        };
        assert_eq!(active_access_route(&data).source.provider, "claude");
        data.active_provider = Provider::Codex;
        assert_eq!(active_access_route(&data).source.provider, "codex");
        assert_eq!(
            route_for_provider(&data, Provider::Codex)
                .unwrap()
                .source
                .kind,
            AccessSourceKind::CodexSubscription
        );
    }

    #[test]
    fn failed_api_auth_is_unavailable_and_unproofed() {
        let mut cache = super::ApiProbeEntry::default();
        let route = api_probe_route("openai", None, &mut cache);
        assert_eq!(route.availability, AccessAvailability::Unavailable);
        assert_eq!(route.source.proof, AccessProof::None);
        assert!(route.windows.is_empty());
        assert_eq!(route.source.kind, AccessSourceKind::OpenAiApi);
        assert_eq!(route.source.auth_method, AuthMethod::ApiKey);
        assert_eq!(AccessSnapshot::new(vec![route]).routes.len(), 1);
    }

    #[test]
    fn failed_refresh_replaces_old_windows_instead_of_leaking_stale_usage() {
        let old = proofed_route(
            "codex",
            UsageSignal::CodexSubscriptionUsage,
            vec![QuotaWindow {
                window_minutes: 300,
                used_percent: 75.0,
                remaining_percent: 25.0,
                resets_at: None,
            }],
        );
        let replacement = AccessRouteSnapshot::unavailable(
            subscription_source("codex", None),
            "account probe failed",
        );
        let snapshot = merge_access_routes(&[old], [replacement]);
        assert_eq!(snapshot.routes.len(), 1);
        assert!(snapshot.routes[0].windows.is_empty());
        assert_eq!(
            snapshot.routes[0].availability,
            AccessAvailability::Unavailable
        );
    }

    #[test]
    fn provider_windows_keep_arbitrary_durations_without_synthetic_buckets() {
        let route = proofed_route(
            "claude",
            UsageSignal::ClaudeSubscriptionUsage,
            vec![
                QuotaWindow {
                    window_minutes: 17,
                    used_percent: 11.0,
                    remaining_percent: 89.0,
                    resets_at: None,
                },
                QuotaWindow {
                    window_minutes: 4_321,
                    used_percent: 22.0,
                    remaining_percent: 78.0,
                    resets_at: None,
                },
            ],
        );
        assert_eq!(
            route
                .windows
                .iter()
                .map(|window| window.quota.window_minutes)
                .collect::<Vec<_>>(),
            vec![17, 4_321]
        );
    }

    #[test]
    fn snapshot_fingerprint_ignores_wall_clock_fields_but_not_telemetry() {
        let first = serde_json::json!({
            "sync_state": "syncing",
            "snapshot_captured_at": "2026-08-12T10:00:00Z",
            "health": {"uptime_seconds": 10, "status": "ok"},
            "discord_preview": {"duration_secs": 10, "state": "7d 96%"},
            "sessions": [{"duration_secs": 10, "tokens_per_sec": 4.5, "tokens": 100}]
        });
        let second = serde_json::json!({
            "sync_state": "live",
            "snapshot_captured_at": "2026-08-12T10:00:05Z",
            "health": {"uptime_seconds": 15, "status": "ok"},
            "discord_preview": {"duration_secs": 15, "state": "7d 96%"},
            "sessions": [{"duration_secs": 15, "tokens_per_sec": 3.0, "tokens": 100}]
        });
        let changed = serde_json::json!({
            "health": {"uptime_seconds": 15, "status": "ok"},
            "discord_preview": {"duration_secs": 15, "state": "7d 95%"},
            "sessions": [{"duration_secs": 15, "tokens_per_sec": 3.0, "tokens": 101}]
        });

        assert_eq!(
            semantic_snapshot_fingerprint(first),
            semantic_snapshot_fingerprint(second)
        );
        assert_ne!(
            semantic_snapshot_fingerprint(serde_json::json!({
                "health": {"uptime_seconds": 15, "status": "ok"},
                "discord_preview": {"duration_secs": 15, "state": "7d 96%"},
                "sessions": [{"duration_secs": 15, "tokens_per_sec": 3.0, "tokens": 100}]
            })),
            semantic_snapshot_fingerprint(changed)
        );
    }

    fn persisted_snapshot_fixture() -> serde_json::Value {
        serde_json::json!({
            "revision": 1,
            "sync_state": "live",
            "snapshot_captured_at": "2026-08-12T10:00:00Z",
            "health": {},
            "metrics": {},
            "sessions": [],
            "access": {},
            "rate_limits": null,
            "discord_preview": {},
            "discord_settings": {},
            "plan": {}
        })
    }

    #[test]
    fn persisted_snapshot_is_provider_scoped_and_returns_as_syncing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("snapshot.json");
        super::persist_app_snapshot_value_at(&path, Provider::Claude, persisted_snapshot_fixture())
            .expect("persist snapshot");

        let loaded = super::load_persisted_app_snapshot_at(&path, Provider::Claude)
            .expect("load snapshot")
            .expect("matching provider snapshot");
        assert_eq!(loaded["sync_state"], "syncing");
        assert_eq!(loaded["snapshot_captured_at"], "2026-08-12T10:00:00Z");
        assert!(
            super::load_persisted_app_snapshot_at(&path, Provider::Codex)
                .expect("provider mismatch is not an I/O failure")
                .is_none()
        );
    }

    #[test]
    fn corrupt_or_wrong_contract_persisted_snapshot_is_never_promoted() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("snapshot.json");
        std::fs::write(&path, b"not-json").expect("write corrupt cache");
        assert!(super::load_persisted_app_snapshot_at(&path, Provider::Claude).is_err());

        super::persist_app_snapshot_value_at(
            &path,
            Provider::Claude,
            serde_json::json!({"revision": 1}),
        )
        .expect("persist invalid contract");
        assert!(super::load_persisted_app_snapshot_at(&path, Provider::Claude).is_err());
    }

    #[test]
    fn usage_snapshot_transport_uses_public_scope_labels() {
        let snapshot = codex_presence_core::UsageSnapshot {
            source: UsageSource::new("codex-session-jsonl", [UsageSignal::CodexSessionJsonl]),
            scopes: vec![codex_presence_core::QuotaScope {
                id: Some("codex".to_string()),
                name: None,
                kind: codex_presence_core::RateLimitScope::GlobalAccount,
                windows: Vec::new(),
            }],
            credits: None,
            observed_at: None,
            provenance_source: "fixture".to_string(),
        };
        let value = serde_json::to_value(super::UsageSnapshotTransport::from_snapshot(
            "codex", snapshot,
        ))
        .expect("usage snapshot JSON");
        assert_eq!(value["provider"], "codex");
        assert_eq!(value["source"], "fixture");
        assert_eq!(value["usage_source"]["stream_id"], "codex-session-jsonl");
        assert_eq!(value["scopes"][0]["kind"], "global_account");
    }

    #[test]
    fn legacy_model_distribution_transport_remains_tuple_shaped() {
        let rows = vec![crate::db::ModelStat {
            model: "gpt-5.6-sol".to_string(),
            session_count: 2,
            priced_sessions: 1,
            cost_basis: crate::db::CostBasis::Partial,
            cost_sources: vec!["provider_billed".to_string()],
            total_cost: 4.25,
        }];

        assert_eq!(
            super::legacy_model_distribution(rows),
            vec![("gpt-5.6-sol".to_string(), 2, 4.25)]
        );
    }

    #[test]
    fn budget_validation_rejects_invalid_values_before_persistence() {
        assert!(super::set_budget(-1.0, Some(80.0)).is_err());
        assert!(super::set_budget(f64::NAN, Some(80.0)).is_err());
        assert!(super::set_budget(10.0, Some(101.0)).is_err());
        assert!(super::set_budget(10.0, Some(f64::INFINITY)).is_err());
    }

    fn sample_claude_snapshot(model_id: &str) -> ClaudeSessionSnapshot {
        ClaudeSessionSnapshot {
            session_id: format!("{model_id}-session"),
            cwd: PathBuf::from("D:/X/Pulse"),
            project_name: "pulse".into(),
            git_branch: None,
            model: Some(model_id.to_string()),
            model_display: Some(cost::model_display_name(model_id)),
            session_total_tokens: Some(120_000),
            last_turn_tokens: Some(2_000),
            session_delta_tokens: None,
            input_tokens: 100_000,
            output_tokens: 20_000,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            max_turn_api_input: 100_000,
            current_context_tokens: 100_000,
            reasoning_effort: ReasoningEffort::High,
            reasoning_effort_explicit: true,
            has_thinking_blocks: false,
            speed: Speed::Standard,
            service_tier: None,
            total_cost: 2.0,
            input_cost: 1.0,
            output_cost: 1.0,
            cache_write_cost: 0.0,
            cache_read_cost: 0.0,
            total_api_duration_ms: 0,
            limits: cc_discord_presence::session::RateLimits::default(),
            activity: None,
            started_at: None,
            last_token_event_at: None,
            last_activity: SystemTime::now(),
            source: DataSource::Jsonl,
            source_file: PathBuf::from("session.jsonl"),
            background_work: cc_discord_presence::workflow_state::BackgroundWorkInfo::default(),
            subagents: Vec::new(),
            is_subagent: false,
            parent_session_id: None,
        }
    }

    /// `CLAUDE_HOME` / `CODEX_HOME` are process-global, so every test that
    /// redirects them must hold this lock or they clobber each other under
    /// libtest's default multi-threaded runner.
    static HOME_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Redirects both provider homes at a fresh temp tree and returns the guard
    /// that keeps other home-mutating tests out until the caller is done.
    fn isolated_homes(tag: &str) -> (std::sync::MutexGuard<'static, ()>, PathBuf, PathBuf) {
        let guard = HOME_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = std::env::temp_dir().join(format!("pulse-{tag}-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let claude_home = temp.join("claude");
        let codex_home = temp.join("codex");
        std::fs::create_dir_all(&claude_home).expect("claude home");
        std::fs::create_dir_all(&codex_home).expect("codex home");
        unsafe {
            std::env::set_var("CLAUDE_HOME", &claude_home);
            std::env::set_var("CODEX_HOME", &codex_home);
            std::env::set_var("PULSE_HOME", &temp);
        }
        (guard, claude_home, codex_home)
    }

    #[test]
    fn claude_rich_presence_toggle_survives_a_restart() {
        let (_guard, _claude_home, _codex_home) = isolated_homes("claude-rp-toggle");
        super::set_active_provider("claude".to_string()).expect("save Claude provider");

        let settings = super::set_discord_enabled(false).expect("disable Claude presence");
        assert!(!settings.enabled, "the command must report the new state");

        // Restart equivalent: nothing but the on-disk config survives, and the
        // startup path is what seeds the in-memory cache.
        let claude = TestClaudePresenceConfig::load_or_init().expect("claude config");
        assert!(
            !claude.presence_enabled,
            "turning Rich Presence off must be written to the Claude config"
        );

        if let Ok(mut data) = super::shared().lock() {
            data.discord_enabled = true; // pretend a fresh process default
        }
        super::seed_discord_state_from_disk();

        // The poller decides whether to publish to Discord from this cached flag,
        // so seeding it wrong is what makes a paused presence come back to life.
        let cached_enabled = super::shared()
            .lock()
            .expect("shared state")
            .discord_enabled;
        assert!(
            !cached_enabled,
            "startup must seed the cached presence flag from disk, not force it on"
        );

        let after_restart = super::get_discord_settings().expect("settings after restart");
        assert!(
            !after_restart.enabled,
            "startup must honour the persisted Claude presence flag instead of forcing it on"
        );
    }

    #[test]
    fn claude_display_prefs_persist_even_when_the_codex_mirror_write_fails() {
        let (_guard, _claude_home, codex_home) = isolated_homes("mirror-failure");
        super::set_active_provider("claude".to_string()).expect("save Claude provider");

        // Make the Codex home unusable: a regular file where a directory is
        // expected, so `CodexPresenceConfig::load_or_init` cannot succeed.
        std::fs::remove_dir_all(&codex_home).expect("clear codex home");
        std::fs::write(&codex_home, b"not a directory").expect("occupy codex home");

        let settings = super::set_discord_display_prefs(
            true, false, true, true, true, true, true, false, true, true,
        )
        .expect("a broken Codex mirror must not fail the active provider's save");

        assert!(
            !settings.display_prefs.show_branch,
            "the returned canonical payload must reflect the change the user made"
        );
        let claude = TestClaudePresenceConfig::load_or_init().expect("claude config");
        assert!(
            !claude.privacy.show_git_branch,
            "the Claude config must keep the change even though the mirror write failed"
        );
    }

    #[test]
    fn saving_claude_toggles_preserves_the_codex_credits_preference() {
        let (_guard, _claude_home, _codex_home) = isolated_homes("credits-mirror");

        // Codex owns Credits and has it on.
        super::set_active_provider("codex".to_string()).expect("save Codex provider");
        super::set_discord_display_prefs(
            true, true, true, true, true, true, true, true, true, true,
        )
        .expect("save under codex");
        assert!(
            TestCodexPresenceConfig::load_or_init()
                .expect("codex config")
                .privacy
                .show_credits
        );

        // Claude has no Credits field, so its payload always carries `false`.
        // Saving an unrelated Claude toggle must not disable Codex Credits.
        super::set_active_provider("claude".to_string()).expect("save Claude provider");
        super::set_discord_display_prefs(
            true, false, true, true, true, true, true, false, true, true,
        )
        .expect("save under claude");

        let codex = TestCodexPresenceConfig::load_or_init().expect("codex config");
        assert!(
            codex.privacy.show_credits,
            "a Claude-side save must not clobber a Codex-only preference"
        );
        assert!(
            !codex.privacy.show_git_branch,
            "shared fields still mirror across providers"
        );
    }

    #[test]
    fn discord_controls_are_saved_for_claude_and_codex_together() {
        let (_guard, _claude_home, _codex_home) = isolated_homes("display-prefs");

        super::set_discord_display_prefs(
            true, false, true, true, true, true, false, true, false, true,
        )
        .expect("save display preferences");
        super::set_active_provider("codex".to_string()).expect("save Codex provider");
        let settings = super::set_discord_enabled(false).expect("disable Codex presence");

        let claude = TestClaudePresenceConfig::load_or_init().expect("claude config");
        let codex = TestCodexPresenceConfig::load_or_init().expect("codex config");

        assert!(!claude.privacy.show_git_branch);
        assert!(!codex.privacy.show_git_branch);
        assert!(claude.privacy.show_cost);
        assert!(codex.privacy.show_cost);
        assert!(!claude.privacy.show_limits);
        assert!(!codex.privacy.show_limits);
        assert!(codex.privacy.show_credits);
        assert!(!claude.privacy.show_context);
        assert!(!codex.privacy.show_context);
        assert!(claude.privacy.show_systems);
        assert!(codex.privacy.show_systems);
        assert!(!codex.presence_enabled);
        assert!(!settings.enabled);
    }

    #[test]
    fn switching_provider_keeps_lane_diagnostics_without_selecting_old_provider() {
        let (_guard, _claude_home, _codex_home) = isolated_homes("provider-switch-access");
        if let Ok(mut data) = super::shared().lock() {
            data.access = super::AccessSnapshot {
                routes: vec![crate::access::AccessRouteSnapshot::unavailable(
                    crate::access::subscription_source("codex", None),
                    "fixture",
                )],
            };
        }

        super::seed_discord_state_for(Provider::Claude);

        let routes = super::shared()
            .lock()
            .expect("shared state")
            .access
            .routes
            .clone();
        assert_eq!(routes.len(), 1, "independent lane diagnostics stay cached");
        assert_eq!(routes[0].source.provider, "codex");
        let data = super::shared().lock().expect("shared state");
        assert_eq!(super::active_access_route(&data).source.provider, "claude");
    }

    #[test]
    fn switching_to_claude_does_not_leak_a_stale_codex_plan_info() {
        let (_guard, _claude_home, _codex_home) = isolated_homes("provider-switch-plan");
        let mut codex = TestCodexPresenceConfig::load_or_init().expect("codex config");
        codex.openai_plan.mode = OpenAiPlanMode::Manual;
        codex.openai_plan.tier = cc_discord_presence::codex::config::OpenAiPlanTier::Pro20x;
        codex.save().expect("codex plan override");

        super::set_active_provider("codex".to_string()).expect("save Codex provider");
        assert_eq!(super::get_plan_info().plan_key, "pro_20x");

        super::set_active_provider("claude".to_string()).expect("save Claude provider");
        let claude_plan = super::get_plan_info();
        assert_eq!(claude_plan.provider, "claude");
        assert_ne!(claude_plan.plan_key, "pro_20x");
        assert_eq!(claude_plan.plan_name, "Unknown");
        assert!(
            !claude_plan.detected,
            "no Claude plan proof must be Unknown"
        );
    }

    #[test]
    fn invalid_claude_max_override_is_not_reported_as_a_plan() {
        let (_guard, _claude_home, _codex_home) = isolated_homes("invalid-claude-plan");
        super::set_active_provider("claude".to_string()).expect("save Claude provider");

        let mut config = TestClaudePresenceConfig::load_or_init().expect("Claude config");
        config.plan = Some("max".to_string());
        config.save().expect("save invalid plan fixture");

        let plan = super::get_plan_info();
        assert_eq!(plan.provider, "claude");
        assert_eq!(plan.plan_key, "");
        assert_eq!(plan.plan_name, "Unknown");
        assert!(!plan.detected);
    }

    #[test]
    fn notification_commands_share_the_durable_store_lifecycle() {
        let (_guard, _claude_home, _codex_home) = isolated_homes("notification-commands");
        let connection = crate::notifications::open_default_database().expect("notification db");
        let record = NotificationStore::new(&connection)
            .observe(
                crate::notifications::NotificationSpec {
                    kind: crate::notifications::NotificationKind::ProviderHealth,
                    provider: "codex".to_string(),
                    key: "account".to_string(),
                    title: "Codex provider".to_string(),
                    body: "Provider unavailable".to_string(),
                    action: None,
                },
                chrono::Utc::now(),
            )
            .expect("record notification")
            .expect("new notification");

        assert_eq!(get_unread_notification_count(), 1);
        assert_eq!(
            get_notifications(Some(10))
                .expect("list notifications")
                .len(),
            1
        );
        assert!(mark_notification_read(record.id).expect("mark notification read"));
        assert_eq!(get_unread_notification_count(), 0);
        assert_eq!(
            mark_all_notifications_read().expect("mark all notifications read"),
            0
        );
        assert!(super::mark_notification_unread(record.id).unwrap());
        assert_eq!(get_unread_notification_count(), 1);
        assert_eq!(mark_all_notifications_read().unwrap(), 1);
        assert_eq!(super::mark_all_notifications_unread().unwrap(), 1);
        let cleared = super::dismiss_all_notifications().unwrap();
        assert_eq!(cleared.count, 1);
        assert!(get_notifications(Some(10)).unwrap().is_empty());
        assert_eq!(super::restore_notifications(cleared.undo_token).unwrap(), 1);
        assert!(dismiss_notification(record.id).expect("dismiss notification"));
        assert!(
            get_notifications(Some(10))
                .expect("list notifications after dismiss")
                .is_empty()
        );
    }

    #[test]
    fn access_diagnostics_never_become_native_health_or_threshold_alerts() {
        let connection = rusqlite::Connection::open_in_memory().expect("notification sqlite");
        NotificationStore::initialize(&connection).expect("notification schema");
        let store = NotificationStore::new(&connection);

        let mut authenticated = subscription_source("claude", None);
        authenticated.proof = AccessProof::QuotaResponse;
        let healthy = AccessRouteSnapshot {
            source: authenticated,
            availability: AccessAvailability::Available,
            freshness: AccessFreshness::Fresh,
            provenance: AccessProvenance::ProviderApi,
            observed_at: None,
            fetched_at: None,
            expires_at: None,
            windows: Vec::new(),
            credits: None,
            rate_limit_reset_credits: None,
            individual_limits: Vec::new(),
            extra_usage: None,
            local_history: Default::default(),
            error: None,
            unavailable_reason: None,
            usage: None,
        };

        // A proofed healthy sample is still only a baseline.
        super::observe_access_notifications(None, Some(&store), Provider::Claude, &healthy);
        assert!(store.list_all(Some(20)).expect("baseline rows").is_empty());

        // A displayable cache with no provider proof (or an unavailable route)
        // must not emit a native quota or health alert at the poll cadence —
        // only genuine quota-reset edges do.
        let mut cached = healthy.clone();
        cached.source.proof = AccessProof::None;
        cached.provenance = AccessProvenance::MemoryCache;
        cached.windows = vec![AccessWindow {
            key: "weekly".to_string(),
            label: None,
            quota: QuotaWindow {
                window_minutes: 10_080,
                used_percent: 95.0,
                remaining_percent: 5.0,
                resets_at: None,
            },
        }];
        super::observe_access_notifications(None, Some(&store), Provider::Claude, &cached);
        assert!(store.list_all(Some(20)).expect("cached rows").is_empty());

        let unavailable = AccessRouteSnapshot::unavailable(
            subscription_source("claude", None),
            "no credentials — check .credentials.json",
        );
        super::observe_access_notifications(None, Some(&store), Provider::Claude, &unavailable);
        assert!(store.list_all(Some(20)).expect("edge rows").is_empty());
    }

    #[test]
    fn codex_discord_settings_reflect_persisted_privacy_design_and_publisher() {
        let config = TestCodexPresenceConfig {
            presence_enabled: false,
            privacy: PrivacyConfig {
                show_git_branch: false,
                ..PrivacyConfig::default()
            },
            display: DisplayConfig {
                desktop_presence_design: DesktopPresenceDesign::ChatGptApp,
                ..DisplayConfig::default()
            },
            ..TestCodexPresenceConfig::default()
        };
        let cached = super::CachedData {
            active_provider: cc_discord_presence::provider::Provider::Codex,
            discord_status: "Controlled by external daemon".to_string(),
            discord_publisher: "external_daemon".to_string(),
            discord_enabled: true,
            ..Default::default()
        };

        let settings = super::build_discord_settings(
            cc_discord_presence::provider::Provider::Codex,
            &cached,
            None,
            Some(&config),
        );

        assert_eq!(settings.provider, "codex");
        assert!(!settings.enabled);
        assert_eq!(settings.publisher, "external_daemon");
        assert!(!settings.display_prefs.show_branch);
        assert_eq!(settings.desktop_design.as_deref(), Some("chatgpt_app"));
        assert!(settings.supports_desktop_design);
    }

    #[test]
    fn claude_session_info_surfaces_ultracode_background_work_safely() {
        let mut snapshot = sample_claude_snapshot("claude-opus-4-8");
        snapshot.background_work = cc_discord_presence::workflow_state::BackgroundWorkInfo {
            workflow_active: true,
            active_agent_count: 2,
            latest_signal_at: Some(SystemTime::now()),
        };

        let infos = build_claude_session_infos(&[snapshot]);

        assert_eq!(infos[0].workflow_label.as_deref(), Some("ULTRACODE"));
        assert_eq!(infos[0].subagent_count, 2);
        assert!(infos[0].subagents.is_empty());
    }

    #[test]
    fn claude_session_info_does_not_call_plain_thinking_a_workflow() {
        let mut snapshot = sample_claude_snapshot("claude-opus-4-8");
        snapshot.has_thinking_blocks = true;
        snapshot.background_work =
            cc_discord_presence::workflow_state::BackgroundWorkInfo::default();

        let infos = build_claude_session_infos(&[snapshot]);

        assert_eq!(infos[0].workflow_label, None);
    }

    #[test]
    fn claude_session_cost_provenance_distinguishes_jsonl_estimates_from_statusline() {
        let jsonl = sample_claude_snapshot("claude-opus-4-8");
        let mut statusline = jsonl.clone();
        statusline.source = DataSource::Statusline;

        let estimated = build_claude_session_infos(&[jsonl]);
        let exact = build_claude_session_infos(&[statusline]);

        assert_eq!(estimated[0].cost_basis, "estimated");
        assert_eq!(exact[0].cost_basis, "exact");
    }

    #[test]
    fn discord_preview_uses_the_same_claude_presence_lines_as_publish() {
        let mut snapshot = sample_claude_snapshot("claude-opus-4-8");
        snapshot.project_name = "PropertyAlpha-Agent".to_string();
        snapshot.git_branch = Some("feat/marketplace-addtochat-liveview-management".to_string());
        snapshot.background_work = cc_discord_presence::workflow_state::BackgroundWorkInfo {
            workflow_active: true,
            active_agent_count: 1,
            latest_signal_at: Some(SystemTime::now()),
        };

        let mut config = TestClaudePresenceConfig {
            plan: Some("max_20x".to_string()),
            ..Default::default()
        };
        super::apply_claude_display_prefs(
            &mut config,
            &super::DiscordDisplayPrefs {
                show_project: true,
                show_branch: false,
                show_model: true,
                show_activity: true,
                show_tokens: true,
                show_cost: true,
                show_limits: false,
                show_credits: false,
                show_context: false,
                show_systems: true,
            },
        );

        let preview = super::build_claude_discord_preview(&[snapshot], &config);

        assert!(preview.has_session);
        assert!(preview.details.contains("PropertyAlpha-Agent"));
        assert!(!preview.details.contains("feat/marketplace"));
        assert!(preview.state.contains("ULTRACODE"));
        assert!(preview.state.contains("1 agent"));
        assert!(!preview.state.contains("5h"));
        assert!(!preview.state.contains("7d"));
        assert!(!preview.state.contains("Ctx"));
    }

    #[test]
    fn claude_preview_and_live_share_extra_usage_from_the_access_route() {
        let now = chrono::Utc::now();
        let snapshot = sample_claude_snapshot("claude-opus-4-8");
        let usage = UsageData {
            five_hour: UsageWindow {
                utilization: 20.0,
                resets_at: None,
            },
            seven_day: UsageWindow {
                utilization: 35.0,
                resets_at: None,
            },
            sonnet_free: None,
            limits: Vec::new(),
            extra_usage: Some(ExtraUsage {
                is_enabled: true,
                monthly_limit: Some(75.0),
                used_credits: Some(10.0),
                utilization: Some(13.333),
            }),
        };
        let route = claude_route_from_usage(
            subscription_source("claude", None),
            &usage,
            now,
            now,
            chrono::Duration::seconds(300),
            now,
            "OAuth usage API",
        );
        let mut config = TestClaudePresenceConfig::default();
        config.privacy.show_cost = true;
        config.privacy.show_limits = true;

        let preview = super::build_claude_discord_preview_with_access(
            std::slice::from_ref(&snapshot),
            &config,
            Some(&route),
            Some(&usage),
        );
        let limits = route_limits_for_claude_presence(Some(&route));
        let (_details, live_state, _tooltip) =
            super::claude_presence_lines(&snapshot, limits.as_ref(), Some(&usage), &config);

        assert_eq!(preview.state, live_state);
        assert!(preview.state.contains("5h 20% used"));
        assert!(preview.state.contains("7d 35% used"));
        assert!(preview.state.contains("Extra $10.00/$75.00"));
    }

    #[test]
    fn claude_presence_candidate_keeps_background_work_when_token_event_is_stale() {
        let mut snapshot = sample_claude_snapshot("claude-opus-4-8");
        snapshot.last_token_event_at = Some(chrono::Utc::now() - chrono::Duration::minutes(20));
        snapshot.last_activity = SystemTime::now();
        snapshot.background_work = cc_discord_presence::workflow_state::BackgroundWorkInfo {
            workflow_active: true,
            active_agent_count: 1,
            latest_signal_at: Some(snapshot.last_activity),
        };

        let cutoff = SystemTime::now()
            .checked_sub(Duration::from_secs(600))
            .expect("cutoff");
        let cutoff_chrono = chrono::Utc::now() - chrono::Duration::seconds(600);

        assert!(super::is_claude_presence_candidate(
            &snapshot,
            cutoff,
            cutoff_chrono
        ));
    }

    #[test]
    fn claude_session_info_flags_inflated_tokenizer_for_sonnet_5_and_opus_4_7_plus() {
        for model_id in ["claude-sonnet-5", "claude-opus-4-8"] {
            let snapshots = [sample_claude_snapshot(model_id)];
            let infos = build_claude_session_infos(&snapshots);
            assert!(infos[0].has_inflated_tokenizer, "{model_id}");
        }
    }

    #[test]
    fn claude_session_info_does_not_flag_inflated_tokenizer_for_sonnet_4_6() {
        let snapshots = [sample_claude_snapshot("claude-sonnet-4-6")];
        let infos = build_claude_session_infos(&snapshots);

        assert!(!infos[0].has_inflated_tokenizer);
    }

    #[test]
    fn codex_session_info_never_carries_an_inflated_tokenizer_flag() {
        let standard = build_codex_session_infos(
            &[sample_codex_snapshot()],
            &TestCodexPresenceConfig::default(),
            TestPresenceSurface::Cli,
        );

        assert!(!standard[0].has_inflated_tokenizer);
    }

    #[test]
    fn codex_session_info_does_not_invent_subagent_count() {
        let infos = build_codex_session_infos(
            &[sample_codex_snapshot()],
            &TestCodexPresenceConfig::default(),
            TestPresenceSurface::Cli,
        );

        assert_eq!(infos[0].subagent_count, 0);
        assert!(infos[0].subagents.is_empty());
        assert_eq!(infos[0].workflow_label, None);
    }

    #[test]
    fn session_info_context_used_tokens_reflects_current_fill_not_the_historical_peak() {
        let snapshot = ClaudeSessionSnapshot {
            max_turn_api_input: 999_486,
            current_context_tokens: 25_500,
            ..sample_claude_snapshot("claude-sonnet-5")
        };
        let infos = build_claude_session_infos(&[snapshot]);

        assert_eq!(
            infos[0].context_used_tokens, 25_500,
            "the live-session ctx-1m badge must show current fill, not the all-time peak"
        );
        assert_eq!(
            infos[0].context_window_tokens, 1_000_000,
            "the 1M-vs-200K window-size decision is unaffected -- it stays keyed off the \
             historical peak (max_turn_api_input), which correctly never decreases"
        );
    }

    #[test]
    fn context_breakdown_used_tokens_reflects_current_fill_not_the_historical_peak() {
        let snapshot = ClaudeSessionSnapshot {
            max_turn_api_input: 999_486,
            current_context_tokens: 25_500,
            ..sample_claude_snapshot("claude-sonnet-5")
        };
        let breakdown = build_claude_context_breakdown(Some(&snapshot));

        assert_eq!(
            breakdown.used_tokens, 25_500,
            "the Context Window view must show current fill, not the session's all-time peak"
        );
        assert_eq!(breakdown.context_window, 1_000_000);
        assert_eq!(breakdown.system_prompt, 0);
        assert_eq!(breakdown.system_tools, 0);
        assert_eq!(breakdown.autocompact_buffer, 0);
        assert_eq!(breakdown.messages, breakdown.used_tokens);
        assert_eq!(breakdown.free_space, 1_000_000 - 25_500);
        assert!(
            breakdown.free_space > 0,
            "a session that genuinely emptied out after compaction must show real free space, \
             not 0 (the exact symptom Tony reported: a CRITICAL \"100% full\" recommendation \
             for a session that isn't actually full right now)"
        );

        let empty = super::empty_context_breakdown("Claude", 200_000);
        assert_eq!(empty.used_tokens, 0);
        assert_eq!(empty.autocompact_buffer, 0);
        assert_eq!(empty.free_space, 200_000);
    }

    #[test]
    fn plan_key_from_override_accepts_display_labels_and_auto() {
        assert_eq!(plan_key_from_override("Max 20x ($200/mo)"), Some("max_20x"));
        assert_eq!(plan_key_from_override("Max 5x ($100/mo)"), Some("max_5x"));
        assert_eq!(plan_key_from_override("  Teams plan  "), Some("team"));
        assert_eq!(plan_key_from_override("enterprise"), Some("enterprise"));
        assert_eq!(plan_key_from_override("pro monthly"), Some("pro"));
        assert_eq!(plan_key_from_override("free"), Some("free"));
        assert_eq!(plan_key_from_override("Max"), None);
        assert_eq!(plan_key_from_override("auto"), None);
        assert_eq!(plan_key_from_override(""), None);
    }

    #[test]
    fn codex_plan_key_maps_detected_tiers_to_frontend_contract() {
        assert_eq!(codex_plan_key_from_tier(DetectedPlanTier::Free), "free");
        assert_eq!(codex_plan_key_from_tier(DetectedPlanTier::Go), "go");
        assert_eq!(codex_plan_key_from_tier(DetectedPlanTier::Plus), "plus");
        assert_eq!(
            codex_plan_key_from_tier(DetectedPlanTier::Business),
            "business"
        );
        assert_eq!(
            codex_plan_key_from_tier(DetectedPlanTier::Enterprise),
            "enterprise"
        );
        assert_eq!(codex_plan_key_from_tier(DetectedPlanTier::Pro5x), "pro_5x");
        assert_eq!(
            codex_plan_key_from_tier(DetectedPlanTier::Pro20x),
            "pro_20x"
        );
        assert_eq!(codex_plan_key_from_tier(DetectedPlanTier::Unknown), "");
    }

    fn sample_codex_snapshot() -> CodexSessionSnapshot {
        CodexSessionSnapshot {
            session_id: "session".into(),
            cwd: PathBuf::from("D:/X/Web Development/MCP Servers/cc-discord-presence"),
            project_name: "pulse".into(),
            git_branch: None,
            model: Some("gpt-5.4".into()),
            originator: None,
            source: None,
            reasoning_effort: None,
            speed: SessionSpeed::default(),
            approval_policy: None,
            sandbox_policy: None,
            session_total_tokens: Some(54_764_083),
            last_turn_tokens: None,
            session_delta_tokens: None,
            input_tokens_total: 54_626_018,
            cached_input_tokens_total: 52_219_136,
            cache_write_tokens_total: None,
            output_tokens_total: 138_065,
            last_input_tokens: None,
            last_cached_input_tokens: None,
            last_output_tokens: None,
            total_cost_usd: 0.0,
            known_cost_usd: None,
            cost_breakdown: TokenCostBreakdown {
                input_cost_usd: 0.0,
                cache_write_cost_usd: 0.0,
                cached_input_cost_usd: 0.0,
                output_cost_usd: 0.0,
                cached_input_savings_usd: 0.0,
            },
            pricing_source: PricingSource::Fallback,
            pricing_status: PricingStatus::Unavailable,
            cost_attribution: CostAttribution::SingleModel,
            cost_breakdown_reconciled: false,
            context_window: None,
            limits: RateLimits::default(),
            rate_limit_envelopes: Vec::new(),
            activity: None,
            started_at: None,
            last_token_event_at: None,
            last_activity: SystemTime::UNIX_EPOCH,
            source_file: PathBuf::from("C:/Users/xt0n1/.codex/sessions/sample.jsonl"),
        }
    }

    #[test]
    fn codex_total_input_tokens_uses_telemetry_total_without_double_counting_cache() {
        let snapshot = sample_codex_snapshot();
        assert_eq!(codex_total_input_tokens(&snapshot), 54_626_018);
    }

    #[test]
    fn codex_session_info_uses_the_canonical_effective_context_for_gpt_5_4() {
        // The snapshot carries no observed context window, so resolution falls
        // back to the model catalogue. Pin it here: without an explicit window
        // the resolver also consults ~/.codex/models_cache.json, which makes
        // the expected value depend on whichever Codex build last wrote that
        // file on the developer's machine.
        let mut snapshot = sample_codex_snapshot();
        snapshot.context_window = Some(ContextWindowSnapshot {
            raw_window_tokens: 272_000,
            window_tokens: 258_400,
            effective_percent: Some(95),
            used_tokens: 0,
            remaining_tokens: 258_400,
            remaining_percent: 100.0,
            source: ContextWindowSource::Event,
            raw_source: ContextWindowSource::Event,
        });

        let infos = build_codex_session_infos(
            &[snapshot],
            &TestCodexPresenceConfig::default(),
            TestPresenceSurface::Cli,
        );

        assert_eq!(infos[0].context_window, "258.4K");
        assert_eq!(infos[0].context_window_tokens, 258_400);
    }

    #[test]
    fn codex_discord_preview_idles_as_codex_app_when_desktop_is_running() {
        let config = TestCodexPresenceConfig::default();
        let preview = build_codex_discord_preview(&[], &config, true);

        assert_eq!(preview.app_name, "Codex App");
        assert_eq!(preview.details, "Codex App");
        assert_eq!(preview.state, "Idling...");
        assert!(!preview.has_session);
    }

    #[test]
    fn codex_discord_preview_uses_selected_chatgpt_identity_and_hides_branch() {
        let mut snapshot = sample_codex_snapshot();
        snapshot.model = Some("gpt-5.6-sol".into());
        snapshot.reasoning_effort = Some(cc_discord_presence::codex::model::ReasoningEffort::Max);
        snapshot.git_branch = Some("main".into());
        snapshot.originator = Some("Codex Desktop".into());
        let mut config = TestCodexPresenceConfig::default();
        config.display.desktop_presence_design = DesktopPresenceDesign::ChatGptApp;
        config.privacy.show_git_branch = false;
        config.openai_plan.mode = OpenAiPlanMode::Manual;

        let preview = build_codex_discord_preview(&[snapshot], &config, true);

        assert_eq!(preview.app_name, "ChatGPT App");
        assert_eq!(preview.large_image_key, "codex-logo");
        assert_eq!(preview.large_text, "ChatGPT App");
        assert!(!preview.details.contains("main"));
        assert!(
            preview
                .state
                .starts_with("GPT-5.6 Sol · Max | Pro 20x ($200/month)")
        );
    }

    #[test]
    fn discord_preview_and_live_share_access_route_for_quota_and_cost() {
        let now = chrono::Utc::now();
        let mut snapshot = sample_codex_snapshot();
        snapshot.known_cost_usd = Some(4.25);
        snapshot.pricing_status = PricingStatus::Exact;
        snapshot.cost_breakdown = TokenCostBreakdown {
            input_cost_usd: 1.25,
            cache_write_cost_usd: 0.0,
            cached_input_cost_usd: 1.0,
            output_cost_usd: 2.0,
            cached_input_savings_usd: 0.0,
        };
        snapshot.limits = RateLimits::new(vec![
            cc_discord_presence::codex::telemetry::limits::UsageWindow {
                used_percent: 99.0,
                remaining_percent: 1.0,
                window_minutes: 300,
                resets_at: None,
            },
            cc_discord_presence::codex::telemetry::limits::UsageWindow {
                used_percent: 88.0,
                remaining_percent: 12.0,
                window_minutes: 10_080,
                resets_at: None,
            },
        ]);
        let route = access_route_from_usage(
            subscription_source("codex", Some("Pro 20x".to_string())),
            UsageSnapshot {
                source: UsageSource::new(
                    "codex-subscription:default",
                    [UsageSignal::CodexSubscriptionUsage],
                ),
                scopes: vec![QuotaScope {
                    id: Some("codex".to_string()),
                    name: Some("Global account quota".to_string()),
                    kind: RateLimitScope::GlobalAccount,
                    windows: vec![
                        QuotaWindow {
                            window_minutes: 300,
                            used_percent: 17.0,
                            remaining_percent: 83.0,
                            resets_at: None,
                        },
                        QuotaWindow {
                            window_minutes: 10_080,
                            used_percent: 42.0,
                            remaining_percent: 58.0,
                            resets_at: None,
                        },
                    ],
                }],
                credits: None,
                observed_at: Some(now),
                provenance_source: "app_server".to_string(),
            },
            now,
            chrono::Duration::seconds(30),
            now,
        );
        let mut config = TestCodexPresenceConfig::default();
        config.privacy.show_limits = true;
        config.privacy.show_cost = true;
        let preview = build_codex_discord_preview_with_access(
            &[snapshot.clone()],
            &config,
            false,
            Some(&route),
        );
        let plan =
            PlanDetector::with_cache_path(tempfile::tempdir().unwrap().path().join("plan.json"))
                .resolve_from_sessions(&[snapshot.clone()], &config.openai_plan);
        let service_tier = resolve_service_tier();
        let live = active_presence_presentation(
            codex_session_surface(&snapshot, codex_fallback_surface(false)),
            &snapshot,
            route_limits_for_presence(Some(&route)).as_ref(),
            &plan,
            &service_tier,
            &config,
        );

        assert_eq!(preview.details, live.details);
        assert_eq!(preview.state, live.state);
        assert!(preview.state.contains("5h 83%"));
        assert!(preview.state.contains("7d 58%"));
        assert!(preview.state.contains("$4.25"));

        let mut stale = route.clone();
        stale.freshness = AccessFreshness::Stale;
        let stale_preview =
            build_codex_discord_preview_with_access(&[snapshot], &config, false, Some(&stale));
        assert!(!stale_preview.state.contains("5h"));
        assert!(!stale_preview.state.contains("7d"));
        assert!(stale_preview.state.contains("$4.25"));
    }

    #[test]
    fn codex_weekly_and_model_weekly_do_not_duplicate_the_global_window() {
        let now = chrono::Utc::now();
        let route = access_route_from_usage(
            subscription_source("codex", None),
            UsageSnapshot {
                source: UsageSource::new(
                    "codex-subscription:default",
                    [UsageSignal::CodexSubscriptionUsage],
                ),
                scopes: vec![
                    QuotaScope {
                        id: Some("codex".to_string()),
                        name: None,
                        kind: RateLimitScope::GlobalAccount,
                        windows: vec![QuotaWindow {
                            window_minutes: 10_080,
                            used_percent: 52.0,
                            remaining_percent: 48.0,
                            resets_at: Some(now + chrono::Duration::days(5)),
                        }],
                    },
                    QuotaScope {
                        id: Some("codex_bengalfox".to_string()),
                        name: Some("GPT-5.3-Codex-Spark".to_string()),
                        kind: RateLimitScope::ModelScoped,
                        windows: vec![QuotaWindow {
                            window_minutes: 10_080,
                            used_percent: 0.0,
                            remaining_percent: 100.0,
                            resets_at: Some(now + chrono::Duration::days(7)),
                        }],
                    },
                ],
                credits: None,
                observed_at: Some(now),
                provenance_source: "app_server".to_string(),
            },
            now,
            chrono::Duration::seconds(30),
            now,
        );

        let (primary, secondary) = super::route_window_slots(&route);
        assert!(primary.is_none(), "weekly must not be reused as a 5h slot");
        assert_eq!(secondary.map(|window| window.key.as_str()), Some("weekly"));

        let legacy = super::rate_limit_info_from_access(&route);
        assert_eq!(legacy.five_hour_window_minutes, None);
        assert_eq!(legacy.seven_day_window_minutes, Some(10_080));
    }

    #[test]
    fn codex_model_scoped_five_hour_is_not_promoted_to_global_discord() {
        let now = chrono::Utc::now();
        let route = access_route_from_usage(
            subscription_source("codex", None),
            UsageSnapshot {
                source: UsageSource::new(
                    "codex-subscription:default",
                    [UsageSignal::CodexSubscriptionUsage],
                ),
                scopes: vec![
                    QuotaScope {
                        id: Some("codex".to_string()),
                        name: None,
                        kind: RateLimitScope::GlobalAccount,
                        windows: vec![QuotaWindow {
                            window_minutes: 10_080,
                            used_percent: 5.0,
                            remaining_percent: 95.0,
                            resets_at: Some(now + chrono::Duration::days(7)),
                        }],
                    },
                    QuotaScope {
                        id: Some("codex_bengalfox".to_string()),
                        name: Some("GPT-5.3-Codex-Spark".to_string()),
                        kind: RateLimitScope::ModelScoped,
                        windows: vec![
                            QuotaWindow {
                                window_minutes: 300,
                                used_percent: 0.0,
                                remaining_percent: 100.0,
                                resets_at: Some(now + chrono::Duration::hours(5)),
                            },
                            QuotaWindow {
                                window_minutes: 10_080,
                                used_percent: 0.0,
                                remaining_percent: 100.0,
                                resets_at: Some(now + chrono::Duration::days(7)),
                            },
                        ],
                    },
                ],
                credits: None,
                observed_at: Some(now),
                provenance_source: "app_server".to_string(),
            },
            now,
            chrono::Duration::seconds(30),
            now,
        );

        let limits = route_limits_for_presence(Some(&route)).expect("fresh global quota");
        assert_eq!(
            limits.primary().map(|window| window.window_minutes),
            Some(10_080),
            "a model-scoped 5h window must not become the global Discord primary"
        );
        assert_eq!(
            limits.primary().map(|window| window.remaining_percent),
            Some(95.0)
        );
        assert!(limits.secondary().is_none());

        let mut config = TestCodexPresenceConfig::default();
        config.privacy.show_limits = true;
        let preview = build_codex_discord_preview_with_access(
            &[sample_codex_snapshot()],
            &config,
            false,
            Some(&route),
        );
        assert!(
            preview.state.contains("7d 95% remaining"),
            "{}",
            preview.state
        );
        assert!(!preview.state.contains("5h"), "{}", preview.state);
    }

    #[test]
    fn context_recommendation_maps_each_tier_at_boundaries() {
        use super::{
            CONTEXT_COMPACT_NOW_PCT, CONTEXT_COMPACT_SOON_PCT, CONTEXT_WATCH_PCT,
            context_recommendation,
        };

        assert_eq!(CONTEXT_WATCH_PCT, 50.0);
        assert_eq!(CONTEXT_COMPACT_SOON_PCT, 80.0);
        assert_eq!(CONTEXT_COMPACT_NOW_PCT, 95.0);

        let healthy = context_recommendation(49.0);
        let watch = context_recommendation(50.0);
        let soon = context_recommendation(80.0);
        let now = context_recommendation(95.0);
        let full = context_recommendation(100.0);

        assert!(healthy.contains("healthy"));
        assert!(watch.contains("half"));
        assert!(soon.contains("compact soon"));
        assert!(now.contains("compact now"));
        assert!(full.contains("compact now"));

        assert_ne!(healthy, watch);
        assert_ne!(watch, soon);
        assert_ne!(soon, now);
    }

    #[test]
    fn codex_session_info_does_not_multiply_canonical_fast_cost_twice() {
        use super::build_codex_session_infos;

        let mut snapshot = sample_codex_snapshot();
        snapshot.model = Some("gpt-5.5".into());
        snapshot.speed = SessionSpeed::explicit(SpeedMode::Fast, SpeedSource::ThreadSettings);
        snapshot.total_cost_usd = 4.0;
        snapshot.known_cost_usd = Some(4.0);
        snapshot.pricing_status = PricingStatus::Exact;
        snapshot.cost_breakdown = TokenCostBreakdown {
            input_cost_usd: 1.0,
            cache_write_cost_usd: 0.0,
            cached_input_cost_usd: 0.5,
            output_cost_usd: 2.5,
            cached_input_savings_usd: 4.5,
        };

        let info = build_codex_session_infos(
            &[snapshot],
            &TestCodexPresenceConfig::default(),
            TestPresenceSurface::Cli,
        );

        assert!((info[0].cost - 4.0).abs() < 0.0001);
        assert!((info[0].input_cost - 1.0).abs() < 0.0001);
        assert!((info[0].output_cost - 2.5).abs() < 0.0001);
        assert!((info[0].cache_read_cost - 0.5).abs() < 0.0001);
        assert!(info[0].cost_available);
        assert_eq!(info[0].cost_basis, "exact");
        assert!(info[0].fast);
    }

    #[test]
    fn codex_session_info_preserves_partial_cost_from_shared_metrics() {
        let mut snapshot = sample_codex_snapshot();
        snapshot.known_cost_usd = Some(0.454);
        snapshot.total_cost_usd = 0.454;
        snapshot.pricing_status = PricingStatus::Partial;
        snapshot.cost_breakdown = TokenCostBreakdown {
            input_cost_usd: 0.2,
            output_cost_usd: 0.254,
            ..TokenCostBreakdown::default()
        };

        let info = build_codex_session_infos(
            &[snapshot],
            &TestCodexPresenceConfig::default(),
            TestPresenceSurface::Cli,
        );

        assert_eq!(info[0].cost, 0.454);
        assert_eq!(info[0].input_cost, 0.2);
        assert_eq!(info[0].output_cost, 0.254);
        assert!(info[0].cost_available);
        assert_eq!(info[0].cost_basis, "partial");
    }

    // ---- Reports cost timeline ---------------------------------------

    /// Builds a history row that landed `days_ago` days before today.
    /// A SQL daily total placed `days_ago` calendar days before today.
    fn cost_row(days_ago: i64, cost: f64, sessions: i64) -> crate::db::DailyCostRow {
        let date = chrono::Utc::now().date_naive() - chrono::Duration::days(days_ago);
        crate::db::DailyCostRow {
            date: date.format("%Y-%m-%d").to_string(),
            cost,
            sessions,
            priced_sessions: sessions,
            cost_basis: crate::db::CostBasis::Exact,
            cost_sources: vec!["fixture".to_string()],
        }
    }

    /// A stored session dated `days_ago` calendar days before today.
    fn session_on_day(days_ago: i64, cost: f64) -> crate::db::HistoricalSession {
        let date = chrono::Utc::now().date_naive() - chrono::Duration::days(days_ago);
        crate::db::HistoricalSession {
            started_at: Some(format!("{}T12:00:00Z", date.format("%Y-%m-%d"))),
            total_cost: cost,
            cost_basis: crate::db::CostBasis::Exact,
            cost_source: "fixture".to_string(),
            known_cost: Some(cost),
            ..Default::default()
        }
    }

    #[test]
    fn daily_cost_series_returns_one_point_per_day_oldest_first() {
        let series = daily_cost_series(&[], 7);
        assert_eq!(series.len(), 7);
        let dates: Vec<&str> = series.iter().map(|p| p.date.as_str()).collect();
        let mut sorted = dates.clone();
        sorted.sort_unstable();
        assert_eq!(dates, sorted, "series must run oldest to newest");
    }

    /// A day without sessions is a real "you spent nothing" observation, so it
    /// must appear as a zero point rather than vanish from the timeline.
    #[test]
    fn daily_cost_series_zero_fills_days_without_sessions() {
        let series = daily_cost_series(&[cost_row(1, 12.5, 1)], 5);
        assert_eq!(series.len(), 5);
        let zero_days = series.iter().filter(|p| p.cost == 0.0).count();
        assert_eq!(zero_days, 4);
        let spend_day = series.iter().find(|p| p.cost > 0.0).expect("spend day");
        assert!((spend_day.cost - 12.5).abs() < 0.000_001);
        assert_eq!(spend_day.sessions, 1);
    }

    #[test]
    fn daily_cost_series_carries_the_aggregated_day_total() {
        let series = daily_cost_series(&[cost_row(2, 8.0, 3)], 5);
        let day = series.iter().find(|p| p.sessions > 0).expect("busy day");
        assert!((day.cost - 8.0).abs() < 0.000_001);
        assert_eq!(day.sessions, 3);
        assert_eq!(day.priced_sessions, 3);
        assert_eq!(day.cost_basis, crate::db::CostBasis::Exact);
        assert_eq!(day.cost_sources, vec!["fixture".to_string()]);
    }

    #[test]
    fn report_windows_preserve_cost_coverage_for_7_30_90_and_365_days() {
        for days in [7, 30, 90, 365] {
            let series = daily_cost_series(&[cost_row(0, 4.25, 2)], days);
            assert_eq!(series.len(), days as usize);
            let current = series.last().expect("current day");
            assert_eq!(current.cost, 4.25);
            assert_eq!(current.sessions, 2);
            assert_eq!(current.priced_sessions, 2);
            assert_eq!(current.cost_basis, crate::db::CostBasis::Exact);
        }
    }

    #[test]
    fn cached_reports_bundle_reuses_fresh_bundle_for_same_key() {
        let _test_guard = REPORTS_BUNDLE_CACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *REPORTS_BUNDLE_CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let builds = Cell::new(0);
        let project = Some("cache-test-same-key-7717".to_string());
        let provider = "all-cache-test-7717".to_string();

        let first = cached_reports_bundle(7717, project.clone(), provider.clone(), || {
            builds.set(builds.get() + 1);
            build_reports_bundle_for_scope_from_roots(
                &provider,
                Some(7717),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
        });
        let second = cached_reports_bundle(7717, project, provider, || {
            builds.set(builds.get() + 1);
            unreachable!("fresh matching cache entry should skip the builder")
        });

        assert_eq!(builds.get(), 1);
        assert_eq!(
            serde_json::to_value(first).expect("serialize first bundle"),
            serde_json::to_value(second).expect("serialize cached bundle")
        );
    }

    #[test]
    fn cached_reports_bundle_rebuilds_for_different_key() {
        let _test_guard = REPORTS_BUNDLE_CACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *REPORTS_BUNDLE_CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let builds = Cell::new(0);

        for (days, project) in [
            (7717, Some("cache-test-key-a-7717".to_string())),
            (7718, Some("cache-test-key-b-7718".to_string())),
        ] {
            cached_reports_bundle(
                days,
                project,
                "all-cache-test-key-change".to_string(),
                || {
                    builds.set(builds.get() + 1);
                    build_reports_bundle_for_scope_from_roots(
                        "all-cache-test-key-change",
                        Some(i64::from(days)),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                    )
                },
            );
        }

        assert_eq!(builds.get(), 2);
    }

    #[test]
    fn all_provider_report_bundle_aggregates_without_cross_provider_guidance() {
        let mut codex = session_on_day(0, 2.0);
        codex.id = "codex:scope".to_string();
        codex.provider = "codex".to_string();
        codex.model = "GPT-5.6 Sol".to_string();
        let mut claude = session_on_day(0, 7.0);
        claude.id = "claude:scope".to_string();
        claude.provider = "claude".to_string();
        claude.model = "Claude Opus 5".to_string();

        let bundle = build_reports_bundle_for_scope_from_roots(
            "all",
            Some(7),
            vec![codex, claude],
            vec![cost_row(0, 9.0, 2)],
            Vec::new(),
            Vec::new(),
        );

        assert_eq!(bundle.provider, "all");
        assert_eq!(bundle.total_sessions, 2);
        assert_eq!(bundle.priced_sessions, 2);
        assert_eq!(bundle.cost_basis, crate::db::CostBasis::Exact);
        assert_eq!(bundle.daily_costs.last().expect("today").cost, 9.0);
        assert!(bundle.model_routing.is_none());
        assert!(bundle.recommendations.is_empty());
        assert_eq!(bundle.trace_overview.provider_display, "All providers");
        assert!(
            bundle
                .cache_health
                .diagnosis
                .contains("aggregated across All providers")
        );
    }

    #[test]
    fn daily_cost_series_ignores_sessions_outside_the_window() {
        // 60 days ago is outside a 7-day window; the series stays flat.
        let series = daily_cost_series(&[cost_row(60, 40.0, 1)], 7);
        assert_eq!(series.len(), 7);
        assert!(series.iter().all(|p| p.cost == 0.0));
    }

    /// The first plotted date is the same day the SQL aggregate starts from,
    /// so a session on the boundary day is charted instead of silently
    /// disagreeing with the analyzers that counted it.
    #[test]
    fn daily_cost_series_plots_the_boundary_day_of_the_window() {
        let series = daily_cost_series(&[cost_row(6, 7.25, 2)], 7);
        let first = series.first().expect("first day");
        assert_eq!(first.date, cost_row(6, 0.0, 0).date);
        assert!((first.cost - 7.25).abs() < 0.000_001);
        assert_eq!(first.sessions, 2);
    }

    // ---- Cost Analysis KPI totals ------------------------------------

    /// KPIs must cover every session in the window, not just the page the
    /// table happens to display.
    #[test]
    fn cost_totals_sum_every_session_in_the_window() {
        let sessions: Vec<_> = (0..250).map(|i| session_on_day(i % 30, 2.0)).collect();
        let totals = aggregate_cost_totals(30, &sessions);
        assert_eq!(totals.sessions, 250);
        assert!((totals.total_cost - 500.0).abs() < 0.000_001);
    }

    /// The four cost categories must reconcile with the headline total, or the
    /// "Cost by type" breakdown silently disagrees with "Total spent".
    #[test]
    fn cost_totals_categories_reconcile_with_the_headline() {
        let mut a = session_on_day(1, 0.0);
        a.input_cost = 1.5;
        a.output_cost = 2.0;
        a.cache_write_cost = 0.75;
        a.cache_read_cost = 0.25;
        a.total_cost = 4.5;
        a.known_cost = Some(4.5);

        let totals = aggregate_cost_totals(30, &[a]);
        let by_category = totals.input_cost
            + totals.output_cost
            + totals.cache_write_cost
            + totals.cache_read_cost;
        assert!((by_category - totals.total_cost).abs() < 0.000_001);
    }

    #[test]
    fn cost_totals_are_zero_for_an_empty_window() {
        let totals = aggregate_cost_totals(7, &[]);
        assert_eq!(totals.sessions, 0);
        assert_eq!(totals.total_cost, 0.0);
        assert_eq!(totals.days, 7);
        assert_eq!(
            totals.cost_basis,
            crate::db::CostBasis::Unavailable,
            "an empty window is not an exact zero-cost observation"
        );
    }

    #[test]
    fn cost_totals_keep_partial_coverage_and_ignore_unavailable_raw_costs() {
        let mut exact = session_on_day(1, 2.5);
        exact.cost_basis = crate::db::CostBasis::Exact;
        exact.cost_source = "session-calculated".to_string();
        exact.known_cost = Some(2.5);

        let mut unavailable = session_on_day(1, 99.0);
        unavailable.cost_basis = crate::db::CostBasis::Unavailable;
        unavailable.cost_source = "unknown".to_string();
        unavailable.known_cost = None;

        let totals = aggregate_cost_totals(30, &[exact, unavailable]);

        assert_eq!(totals.cost_basis, crate::db::CostBasis::Partial);
        assert_eq!(totals.sessions, 2);
        assert_eq!(totals.priced_sessions, 1);
        assert_eq!(totals.total_cost, 2.5);
        assert_eq!(totals.cost_sources, vec!["session-calculated".to_string()]);
        assert_eq!(totals.billed_spend_usd, None);
        assert_eq!(totals.api_equivalent_usd, Some(2.5));
        assert_eq!(totals.billed_sessions, 0);
        assert_eq!(totals.api_equivalent_sessions, 1);
    }

    #[test]
    fn cost_totals_keep_billed_and_api_equivalent_values_in_separate_fields() {
        let mut billed = session_on_day(1, 10.0);
        billed.cost_basis = crate::db::CostBasis::Exact;
        billed.cost_source = "provider_billed".to_string();
        billed.known_cost = Some(10.0);

        let mut estimated = session_on_day(1, 90.0);
        estimated.cost_basis = crate::db::CostBasis::Estimated;
        estimated.cost_source = "api_equivalent".to_string();
        estimated.known_cost = Some(90.0);

        let totals = aggregate_cost_totals(30, &[billed, estimated]);

        assert_eq!(totals.total_cost, 100.0);
        assert_eq!(totals.billed_spend_usd, Some(10.0));
        assert_eq!(totals.api_equivalent_usd, Some(90.0));
        assert_eq!(totals.billed_sessions, 1);
        assert_eq!(totals.api_equivalent_sessions, 1);
    }

    #[test]
    fn cost_totals_keep_usage_breakdown_when_cost_is_unavailable() {
        let mut unavailable = session_on_day(1, 99.0);
        unavailable.cost_basis = crate::db::CostBasis::Unavailable;
        unavailable.cost_source = "unknown".to_string();
        unavailable.known_cost = None;
        unavailable.input_tokens = 100_000;
        unavailable.output_tokens = 20_000;
        unavailable.cache_write_tokens = 5_000;
        unavailable.cache_read_tokens = 75_000;
        unavailable.total_tokens = 200_000;

        let totals = aggregate_cost_totals(30, &[unavailable]);

        assert_eq!(totals.cost_basis, crate::db::CostBasis::Unavailable);
        assert_eq!(totals.input_tokens, 100_000);
        assert_eq!(totals.output_tokens, 20_000);
        assert_eq!(totals.cache_write_tokens, 5_000);
        assert_eq!(totals.cache_read_tokens, 75_000);
        assert_eq!(totals.total_tokens, 200_000);
    }

    #[test]
    fn metrics_cost_availability_distinguishes_empty_partial_and_exact_zero() {
        assert_eq!(
            cost_availability(&[]),
            (false, "unavailable".to_string()),
            "an empty session set has no cost observation"
        );

        let mut exact = build_claude_session_infos(&[sample_claude_snapshot("claude-opus-4-8")]);
        exact[0].cost = 0.0;
        exact[0].cost_basis = "exact".into();
        assert_eq!(cost_availability(&exact), (true, "exact".to_string()));

        let mut mixed = build_claude_session_infos(&[sample_claude_snapshot("claude-opus-4-8")]);
        mixed[0].cost_available = false;
        mixed[0].cost_basis = "unavailable".to_string();
        assert_eq!(
            cost_availability(&mixed),
            (false, "unavailable".to_string())
        );
        mixed[0].cost_available = true;
        mixed[0].cost_basis = "partial".into();
        assert_eq!(cost_availability(&mixed), (true, "partial".to_string()));
    }

    /// The cache-savings estimate multiplies a token count by a per-token rate.
    /// Both must come from this aggregate, so `pure_input_tokens` has to be the
    /// denominator that matches `input_cost` over the same sessions.
    #[test]
    fn cost_totals_expose_pure_input_tokens_for_the_input_rate() {
        let mut s = session_on_day(1, 0.0);
        s.input_tokens = 100_000;
        s.cache_write_tokens = 30_000;
        s.cache_read_tokens = 50_000;
        s.input_cost = 0.1;

        let totals = aggregate_cost_totals(30, &[s]);
        // 100k total input minus 30k written minus 50k read = 20k pure.
        assert_eq!(totals.pure_input_tokens, 20_000);
        assert_eq!(totals.cache_read_tokens, 50_000);

        let rate = totals.input_cost / totals.pure_input_tokens as f64;
        assert!((rate - 0.000_005).abs() < 1e-9, "rate was {rate}");
    }

    /// Cache-heavy sessions can report more cached than raw input tokens; the
    /// pure-input count must clamp at zero rather than going negative and
    /// flipping the derived rate.
    #[test]
    fn cost_totals_never_report_negative_pure_input() {
        let mut s = session_on_day(1, 0.0);
        s.input_tokens = 1_000;
        s.cache_write_tokens = 5_000;
        s.cache_read_tokens = 9_000;

        let totals = aggregate_cost_totals(30, &[s]);
        assert_eq!(totals.pure_input_tokens, 0);
    }

    /// Per-model and per-project breakdowns must reconcile with the headline.
    /// Summing only the visible page made the bars claim $7.72 on a window
    /// whose real spend was $7,371.
    #[test]
    fn cost_totals_breakdowns_reconcile_with_the_headline() {
        let mut a = session_on_day(1, 10.0);
        a.model = "Claude Opus 5".into();
        a.project = "pulse".into();
        let mut b = session_on_day(2, 4.0);
        b.model = "Claude Sonnet 5".into();
        b.project = "pulse".into();
        let mut c = session_on_day(3, 6.0);
        c.model = "Claude Opus 5".into();
        c.project = "other".into();

        let totals = aggregate_cost_totals(30, &[a, b, c]);

        let model_sum: f64 = totals.by_model.iter().map(|m| m.cost).sum();
        let project_sum: f64 = totals.by_project.iter().map(|p| p.cost).sum();
        assert!((model_sum - totals.total_cost).abs() < 0.000_001);
        assert!((project_sum - totals.total_cost).abs() < 0.000_001);

        // Highest spend first, with sessions grouped rather than listed.
        assert_eq!(totals.by_model[0].label, "Claude Opus 5");
        assert!((totals.by_model[0].cost - 16.0).abs() < 0.000_001);
        assert_eq!(totals.by_model[0].sessions, 2);
        assert_eq!(totals.by_project[0].label, "pulse");
        assert!((totals.by_project[0].cost - 14.0).abs() < 0.000_001);
    }

    /// Runs only as an isolated process because poisoning the global cache is
    /// intentionally irreversible for that test binary.
    #[test]
    #[ignore = "poisons process-global shared state; run this test alone"]
    fn poisoned_shared_state_fails_the_entire_snapshot_instead_of_fabricating_defaults() {
        let (_guard, _claude_home, _codex_home) = isolated_homes("snapshot-poisoned");
        let shared = super::shared();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = shared.lock().expect("lock shared state before poison");
            panic!("simulated poller panic while holding shared state");
        }));

        let Err(error) = super::build_app_snapshot("live") else {
            panic!("a poisoned core state must fail the whole snapshot");
        };
        assert!(error.contains("shared snapshot state is unavailable"));
    }
    #[test]
    fn every_plan_override_survives_reload_and_matches_the_discord_payload() {
        let (_guard, _, _) = isolated_homes("plan-override-matrix");
        super::set_active_provider("codex".into()).unwrap();
        let mut session = sample_codex_snapshot();
        session.model = Some("gpt-6-astra".into());
        let automatic = codex_presence_core::RateLimitEnvelope {
            plan_type: Some("prolite".into()),
            scope: RateLimitScope::GlobalAccount,
            observed_at: Some(chrono::Utc::now()),
            ..Default::default()
        };
        if let Ok(mut data) = super::shared().lock() {
            data.codex_plan_envelopes = vec![automatic.clone()];
        }
        for (key, label) in [
            ("free", "Free"),
            ("go", "Go"),
            ("plus", "Plus"),
            ("pro_5x", "Pro 5x"),
            ("pro_20x", "Pro 20x"),
            ("business", "Business"),
            ("enterprise", "Enterprise"),
            ("edu", "Edu"),
        ] {
            super::set_plan_override(key.into(), Some("codex".into())).unwrap();
            super::seed_discord_state_from_disk();
            let config = TestCodexPresenceConfig::load_or_init().unwrap();
            let plan = super::resolve_codex_plan(
                &[session.clone()],
                &config,
                std::slice::from_ref(&automatic),
            );
            let info = super::get_plan_info();
            assert_eq!(info.plan_key, key);
            assert!(!info.detected);
            assert!(info.plan_name.starts_with(label));
            let preview = super::build_codex_discord_preview_with_plan(
                &[session.clone()],
                &config,
                true,
                None,
                &plan,
            );
            assert!(preview.state.contains(label), "{key}: {}", preview.state);
            assert!(!preview.state.contains("Unknown"));
        }
        assert!(super::set_plan_override("invalid".into(), Some("codex".into())).is_err());
        assert_eq!(super::get_plan_info().plan_key, "edu");
        super::set_plan_override("auto".into(), Some("codex".into())).unwrap();
        let automatic_info = super::get_plan_info();
        assert!(automatic_info.detected);
        assert_eq!(automatic_info.plan_key, "pro_5x");
    }
}
