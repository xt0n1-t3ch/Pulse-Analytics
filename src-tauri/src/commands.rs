//! Tauri command layer: the background poller, IPC commands, snapshots and provider presentation
//! behind the GUI.

use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use cc_discord_presence::codex::account_usage::{
    AccountUsageReading, effective_limits_from_envelopes, fresh_envelopes,
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
use cc_discord_presence::http;
use cc_discord_presence::opencode::{self, Config as OpenCodeConfig};
use cc_discord_presence::orion::{self, Config as OrionConfig};
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
use crate::live_writes::LiveWrites;
use crate::notifications::{NotificationRecord, NotificationStore, QuotaResetObservation};

const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const STALE_THRESHOLD: Duration = Duration::from_secs(120);
const STICKY_WINDOW: Duration = Duration::from_secs(120);
const ACTIVE_CUTOFF: Duration = Duration::from_secs(600);
const IDLE_CUTOFF: Duration = Duration::from_secs(300);
const APP_SNAPSHOT_CACHE_SCHEMA: u32 = 1;
const APP_SNAPSHOT_CACHE_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// A saved snapshot older than this is not shown at startup; the first live
/// poll arrives within seconds anyway.
const APP_SNAPSHOT_CACHE_MAX_AGE_HOURS: i64 = 24;
const DISCORD_USER_CACHE_TTL: Duration = Duration::from_secs(60);
const REPORTS_BUNDLE_CACHE_TTL: Duration = Duration::from_secs(60);

static START_TIME: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
static DISCORD_USER_CACHE: Mutex<Option<(Instant, Option<DiscordUserInfo>)>> = Mutex::new(None);
type ReportsBundleCacheEntry = (Instant, (u32, Option<String>, String), ReportsBundle);
static REPORTS_BUNDLE_CACHE: Mutex<Option<ReportsBundleCacheEntry>> = Mutex::new(None);
static SNAPSHOT_POLLER_STARTED: AtomicBool = AtomicBool::new(false);
static INITIAL_SNAPSHOT_READY: AtomicBool = AtomicBool::new(false);
/// Claude and Codex rows held between their once-a-minute writes.
static LIVE_SESSION_WRITES: std::sync::OnceLock<Mutex<LiveWrites<SessionInfo>>> =
    std::sync::OnceLock::new();
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
    orion_sessions: Vec<orion::Session>,
    orion_idle_since: Option<i64>,
    orion_diagnostics: Vec<String>,
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
    /// One-shot flag: the poller invalidates its usage cache on the next tick
    /// and forces a fresh API call. The flag is cleared after handling.
    usage_refresh_requested: bool,
    /// One-shot flag set only by the user's Refresh: the next tick reads the
    /// Codex account quota, the one read that starts a process.
    codex_usage_requested: bool,
}

#[derive(Clone)]
struct CachedUsage {
    five_hour_pct: f64,
    seven_day_pct: f64,
    extra_enabled: bool,
    extra_limit: Option<f64>,
    extra_used: Option<f64>,
    extra_pct: Option<f64>,
    /// Provenance observed by `UsageManager` when these numbers were produced:
    /// the OAuth handshake or the on-disk cache. Carried through so the UI can
    /// state how it knows.
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
    let agent = http::agent(API_PROBE_TIMEOUT, http::DEFAULT_REDIRECTS);
    let mut request = match provider {
        "anthropic" => agent
            .get("https://api.anthropic.com/v1/models")
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        _ => agent
            .get("https://api.openai.com/v1/models")
            .header("Authorization", format!("Bearer {key}")),
    };
    request = request.header("User-Agent", "cc-discord-presence");

    match request.call() {
        Ok(response) if response.status().is_success() => {
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
            format!(
                "provider rejected API probe (HTTP {})",
                response.status().as_u16()
            ),
        ),
        Err(ureq::Error::StatusCode(status)) => AccessRouteSnapshot::unavailable(
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
                // authenticated: report authenticated rather than signed out.
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
    // Only a fully unauthenticated read (no OAuth origin and no unexpired token)
    // is dropped, which keeps a missing/expired credential at "sign in
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
        .unwrap_or_else(|| Err("refresh to read the Codex account quota".to_string()));
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
                cc_discord_presence::codex::account_usage::reading_max_age(),
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

fn modified_time(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

/// Reloads a config file only when its modification time changes.
#[derive(Default)]
struct ConfigReload {
    modified: Option<SystemTime>,
}

impl ConfigReload {
    fn observed(path: &std::path::Path) -> Self {
        Self {
            modified: modified_time(path),
        }
    }

    fn reload_if_changed<T>(
        &mut self,
        path: &std::path::Path,
        load: impl FnOnce() -> anyhow::Result<T>,
    ) -> Option<T> {
        let modified = modified_time(path);
        if modified.is_some() && modified == self.modified {
            return None;
        }
        let loaded = load().ok()?;
        // Loading may create the file with defaults.
        self.modified = modified_time(path);
        Some(loaded)
    }
}

/// Each Codex account read starts a Codex app-server, so Pulse reads the
/// quota only when the user asks and the CLI exists. The CLI lookup itself
/// may launch a process on Windows, so it waits for the request too.
fn codex_probe_requested(user_refresh: bool, cli_available: impl FnOnce() -> bool) -> bool {
    user_refresh && cli_available()
}

fn request_codex_usage(
    trigger: &std::sync::mpsc::SyncSender<()>,
    force: &AtomicBool,
    wanted: bool,
) {
    if !wanted {
        return;
    }
    force.store(true, Ordering::Release);
    let _ = trigger.try_send(());
}

#[expect(
    clippy::too_many_arguments,
    reason = "one probe pass needs every provider cache and worker handle"
)]
fn independently_probe_access(
    usage_mgr: &mut UsageManager,
    codex_usage_trigger: &std::sync::mpsc::SyncSender<()>,
    codex_usage_force: &AtomicBool,
    codex_usage_latest: &Arc<Mutex<Option<Result<AccountUsageReading, String>>>>,
    codex_probe_wanted: bool,
    api_probe_cache: &mut ApiProbeCache,
    force_refresh: bool,
    active_provider: Provider,
    codex_plan_detector: &mut PlanDetector,
    codex_plan_config: &cc_discord_presence::codex::config::OpenAiPlanDisplayConfig,
) -> Vec<AccessRouteSnapshot> {
    // The worker owns the app-server; this tick never waits on it.
    request_codex_usage(codex_usage_trigger, codex_usage_force, codex_probe_wanted);

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
        Provider::Orion => crate::orion::access_route(),
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
                    cc_discord_presence::codex::account_usage::reading_max_age(),
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
/// process: without it, pausing Rich Presence would not survive a restart.
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
        Provider::Orion => {
            if let Ok(config) = OrionConfig::load() {
                data.discord_enabled = config.enabled;
                data.discord_prefs = crate::orion::prefs(&config);
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
            while codex_usage_requests.recv().is_ok() {
                let result = cc_discord_presence::codex::account_usage::read_shared_usage(
                    None,
                    true,
                    force.swap(false, Ordering::AcqRel),
                )
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
        let mut usage_mgr = UsageManager::new();
        let mut claude_config = PresenceConfig::load_or_init().unwrap_or_default();
        let mut codex_config = CodexPresenceConfig::load_or_init().unwrap_or_default();
        let mut codex_config_reload =
            ConfigReload::observed(&cc_discord_presence::codex::config::config_path());
        let mut claude_discord = ClaudeDiscordPresence::new(claude_config.effective_client_id());
        let mut codex_discord = CodexDiscordPresence::new(codex_config.effective_client_id());
        let mut codex_plan_detector = PlanDetector::new();
        let mut claude_publisher = PublisherLease::new(cc_discord_presence::config::lock_path());
        let mut codex_publisher =
            PublisherLease::new(cc_discord_presence::codex::config::lock_path());
        let mut api_probe_cache = ApiProbeCache::default();
        let mut last_snapshot_hash = None;
        let mut last_snapshot_saved: Option<Instant> = None;

        let go_usage = crate::opencode_go::start();
        let mut opencode_collector = opencode::Collector::default();
        let mut commandcode_collector = commandcode::Collector::default();
        let mut commandcode_publisher = commandcode::presence::Publisher::default();
        let mut commandcode_lease =
            PublisherLease::new(cc_discord_presence::storage::state_file("commandcode.lock"));
        let mut commandcode_git = CodexGitBranchCache::new(Duration::from_secs(30));
        let mut orion_collector = orion::Collector::default();
        let mut orion_publisher = orion::presence::Publisher::default();
        let mut orion_lease =
            PublisherLease::new(cc_discord_presence::storage::state_file("orion.lock"));
        let mut orion_git = CodexGitBranchCache::new(Duration::from_secs(30));
        let mut orion_idle_since: Option<i64> = None;
        let mut orion_writes = LiveWrites::<(SessionInfo, i64)>::default();
        let mut commandcode_writes = LiveWrites::<(SessionInfo, i64)>::default();
        let mut opencode_writes = LiveWrites::<(SessionInfo, i64)>::default();
        let mut opencode_runtime = opencode::process::RuntimeDetector::default();
        let mut opencode_publisher = opencode::presence::Publisher::default();
        let mut opencode_lease =
            PublisherLease::new(cc_discord_presence::storage::state_file("opencode.lock"));
        let mut opencode_git = CodexGitBranchCache::new(Duration::from_secs(30));
        loop {
            let provider = current_provider();
            let (discord_enabled, prefs, force_refresh, codex_refresh) = data
                .lock()
                .ok()
                .map(|mut d| {
                    d.active_provider = provider;
                    (
                        d.discord_enabled,
                        d.discord_prefs.clone(),
                        std::mem::take(&mut d.usage_refresh_requested),
                        std::mem::take(&mut d.codex_usage_requested),
                    )
                })
                .unwrap_or((true, DiscordDisplayPrefs::default(), false, false));
            let discord_enabled =
                discord_enabled && std::env::var("PULSE_DISABLE_DISCORD").as_deref() != Ok("1");

            if let Some(fresh) = codex_config_reload.reload_if_changed(
                &cc_discord_presence::codex::config::config_path(),
                CodexPresenceConfig::load_or_init,
            ) {
                codex_config = fresh;
            }
            let codex_probe_wanted = codex_probe_requested(
                codex_refresh,
                cc_discord_presence::codex::account_usage::codex_cli_available,
            );
            let mut independently_probed_routes = independently_probe_access(
                &mut usage_mgr,
                &codex_usage_trigger,
                &codex_usage_force,
                &codex_usage_latest,
                codex_probe_wanted,
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
                    let live = session.is_active(chrono::Utc::now().timestamp_millis());
                    let row = (crate::commandcode::session_info(session), session.updated);
                    if let Some((info, updated)) =
                        commandcode_writes.offer(&session.id, row, live, Instant::now())
                    {
                        persisted &= crate::db::upsert_external_session(&info, updated);
                    }
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
                let live_ids: HashSet<String> = active_ids.iter().cloned().collect();
                for (info, updated) in commandcode_writes.due(&live_ids, Instant::now()) {
                    crate::db::upsert_external_session(&info, updated);
                }
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
            let orion_config_result = OrionConfig::load();
            let mut orion_live = Vec::new();
            let mut orion_diagnostics = Vec::new();
            if let Ok(config) = &orion_config_result {
                let mut batch = orion_collector.poll(config);
                let now_ms = chrono::Utc::now().timestamp_millis();
                let mut persisted = true;
                for session in &mut batch.changed {
                    if session.is_recent(now_ms) {
                        session.branch = orion_git.get(&session.directory);
                    }
                    let row = (crate::orion::session_info(session), session.updated);
                    if let Some((info, updated)) = orion_writes.offer(
                        &session.id,
                        row,
                        session.is_active(now_ms),
                        Instant::now(),
                    ) {
                        persisted &= crate::db::upsert_external_session(&info, updated);
                    }
                }
                if persisted {
                    orion_collector.acknowledge(&batch);
                } else {
                    batch
                        .diagnostics
                        .push("Orion App analytics write failed; retrying this batch".into());
                }
                for session in &mut batch.live {
                    session.branch = orion_git.get(&session.directory);
                }
                let active_ids: Vec<_> = batch
                    .live
                    .iter()
                    .filter(|session| session.is_active(now_ms))
                    .map(|session| session.id.clone())
                    .collect();
                let live_ids: HashSet<String> = active_ids.iter().cloned().collect();
                for (info, updated) in orion_writes.due(&live_ids, Instant::now()) {
                    crate::db::upsert_external_session(&info, updated);
                }
                crate::db::mark_inactive("orion", &active_ids);
                orion_idle_since = orion::last_activity(&batch.live)
                    .max(batch.last_activity)
                    .or(orion_idle_since);
                orion_live = batch.live;
                orion_diagnostics = batch.diagnostics;
            } else {
                orion_diagnostics.push("Orion App configuration is unavailable".into());
            }
            if let Ok(mut d) = data.lock() {
                d.orion_sessions = orion_live.clone();
                d.orion_idle_since = orion_idle_since;
                d.orion_diagnostics = orion_diagnostics;
            }
            if provider != Provider::Orion {
                orion_publisher.shutdown();
                orion_lease.release();
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
                    let live = !session.is_idle(chrono::Utc::now().timestamp_millis());
                    let row = (crate::opencode::session_info(session), session.updated);
                    if let Some((info, updated)) =
                        opencode_writes.offer(&session.id, row, live, Instant::now())
                    {
                        persisted &= crate::db::upsert_opencode_session(&info, updated);
                    }
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
                let live_ids: HashSet<String> = active_ids.iter().cloned().collect();
                for (info, updated) in opencode_writes.due(&live_ids, Instant::now()) {
                    crate::db::upsert_opencode_session(&info, updated);
                }
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
                Provider::Orion => {
                    claude_publisher.release();
                    codex_publisher.release();
                    opencode_lease.release();
                    claude_discord.shutdown();
                    codex_discord.shutdown();
                    opencode_publisher.shutdown();
                    let owned = orion_lease.try_acquire().unwrap_or(false);
                    match orion_config_result {
                        Ok(mut config) if owned => {
                            config.enabled &= discord_enabled;
                            orion_publisher.update(
                                orion::preferred_session(&orion_live),
                                orion_idle_since,
                                &config,
                            );
                        }
                        _ => orion_publisher.shutdown(),
                    }
                    if let Ok(mut d) = data.lock() {
                        d.sessions = ActiveSessions::None;
                        d.access = merge_access_routes(&independently_probed_routes, []);
                    }
                    if owned {
                        (orion_publisher.status().to_string(), "Pulse".into())
                    } else {
                        ("Controlled by external publisher".into(), "external".into())
                    }
                }
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
                                // account is still authenticated: not signed out.
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
                    let account_usage = codex_usage_latest
                        .lock()
                        .ok()
                        .and_then(|slot| slot.clone())
                        .unwrap_or_else(|| {
                            Err("refresh to read the Codex account quota".to_string())
                        });
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
                                        cc_discord_presence::codex::account_usage::reading_max_age()
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
                    let processes = cc_discord_presence::process_list::snapshot();
                    let opencode_running =
                        cc_discord_presence::codex::process::opencode_in(&processes);
                    let codex_desktop_running =
                        cc_discord_presence::codex::process::codex_app_in(&processes);
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
                    let now = Instant::now();
                    if snapshot_save_due(last_snapshot_saved, now) {
                        if let Err(error) = persist_app_snapshot(&snapshot) {
                            tracing::warn!(error = %error, "failed to persist Pulse app snapshot");
                        }
                        last_snapshot_saved = Some(now);
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
        Provider::Orion => shared()
            .lock()
            .map(|data| {
                data.orion_sessions
                    .iter()
                    .map(crate::orion::live_session_info)
                    .collect()
            })
            .unwrap_or_default(),
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
    pub orion_diagnostics: Vec<String>,
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
    /// Pulse build that wrote the snapshot. Snapshots from another build, or
    /// without this field, are not shown because their costs and plans may differ.
    #[serde(default)]
    app_version: String,
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
        orion_diagnostics: cached.orion_diagnostics.clone(),
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
        Provider::Orion => {
            let config = OrionConfig::load().map_err(|error| error.to_string())?;
            Ok((
                crate::orion::preview(&cached.orion_sessions, cached.orion_idle_since, &config),
                crate::orion::settings(&config, &cached.discord_status, &cached.discord_publisher),
            ))
        }
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
    cc_discord_presence::storage::cache_path("app-snapshot.json")
}

/// The saved snapshot only paints the first frame of the next launch, so a
/// minute-old copy is enough; live changes do not rewrite it every poll.
const APP_SNAPSHOT_SAVE_INTERVAL: Duration = Duration::from_secs(60);

fn snapshot_save_due(last_saved: Option<Instant>, now: Instant) -> bool {
    last_saved
        .is_none_or(|saved| now.saturating_duration_since(saved) >= APP_SNAPSHOT_SAVE_INTERVAL)
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
        app_version: env!("CARGO_PKG_VERSION").to_string(),
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
    let fresh = chrono::DateTime::parse_from_rfc3339(&persisted.saved_at).is_ok_and(|saved| {
        let age = chrono::Utc::now().signed_duration_since(saved);
        age >= -chrono::Duration::minutes(5)
            && age <= chrono::Duration::hours(APP_SNAPSHOT_CACHE_MAX_AGE_HOURS)
    });
    if persisted.schema_version != APP_SNAPSHOT_CACHE_SCHEMA
        || persisted.provider != provider.as_str()
        || persisted.app_version != env!("CARGO_PKG_VERSION")
        || !fresh
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

#[derive(Clone, Serialize)]
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

#[derive(Clone, Default, Serialize)]
pub struct SessionInfo {
    pub opencode: Option<opencode::Metadata>,
    /// Orion App telemetry that has no cross-provider field (absent for other providers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orion: Option<crate::orion::OrionTelemetry>,
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
                orion: None,
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
                orion: None,
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
    let writes = LIVE_SESSION_WRITES.get_or_init(|| Mutex::new(LiveWrites::default()));
    let mut writes = writes.lock().ok();
    let now = Instant::now();
    let mut changed_count = 0;
    for s in result {
        let key = format!("{}:{}", provider.as_str(), s.session_id);
        let fingerprint = persistent_session_fingerprint(s);
        let unchanged = fingerprints
            .as_ref()
            .and_then(|items| items.get(&key))
            .is_some_and(|previous| *previous == fingerprint);
        if unchanged {
            continue;
        }
        if let Some(items) = fingerprints.as_mut() {
            items.insert(key.clone(), fingerprint);
        }
        let row = match writes.as_mut() {
            Some(writes) => writes.offer(&key, s.clone(), true, now),
            None => Some(s.clone()),
        };
        if row.is_some_and(|row| crate::db::upsert_session(&row)) {
            changed_count += 1;
        }
    }
    if let Some(writes) = writes.as_mut() {
        let provider_prefix = format!("{}:", provider.as_str());
        let mut live = active_keys.clone();
        // Other providers' held rows are not this call's to flush.
        live.extend(
            writes
                .held_keys()
                .filter(|key| !key.starts_with(&provider_prefix))
                .map(str::to_string),
        );
        for row in writes.due(&live, now) {
            changed_count += usize::from(crate::db::upsert_session(&row));
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
    // Persist for whichever provider is active; a switch kept only in `shared()`
    // would silently turn Rich Presence back on at every restart.
    match current_provider() {
        Provider::Orion => {
            let mut config = OrionConfig::load().map_err(|error| error.to_string())?;
            config.enabled = enabled;
            config.save().map_err(|error| error.to_string())?;
        }
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
#[expect(
    clippy::too_many_arguments,
    reason = "Tauri passes each preference as its own invoke argument"
)]
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
        Provider::Orion => {
            let mut config = OrionConfig::load().map_err(|error| error.to_string())?;
            crate::orion::apply_prefs(&mut config, &prefs);
            config.save().map_err(|error| error.to_string())?;
        }
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
/// next tick. The UI's refresh button is wired to this; real data arrives within ~5s.
#[tauri::command]
pub fn refresh_usage() {
    if let Ok(mut d) = shared().lock() {
        d.usage_refresh_requested = true;
        d.codex_usage_requested = true;
    }
}

/// The startup refresh: provider HTTP reads only, never a Codex app-server.
pub fn request_usage_refresh() {
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
        sessions.extend(
            data.orion_sessions
                .iter()
                .map(crate::orion::live_session_info),
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
/// while Claude is active always carries `show_credits = false` because Claude
/// has no such field, so copying it here silently disabled Credits on the Codex side
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
    /// Composed while Codex was active: the value is the user's choice.
    Apply,
    /// Composed while a provider without Credits was active; keep what Codex has.
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
            Provider::OpenCode | Provider::CommandCode | Provider::Orion => (
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
    if current_provider() == Provider::Orion {
        let mut config = OrionConfig::load().map_err(|error| error.to_string())?;
        config.layout.fields.sort_by_key(|item| {
            parsed
                .iter()
                .position(|field| *field == item.field)
                .unwrap_or(usize::MAX)
        });
        config.save().map_err(|error| error.to_string())?;
        return get_discord_settings();
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
        Provider::Orion => {
            let config = OrionConfig::load().map_err(|error| error.to_string())?;
            Ok(crate::orion::settings(
                &config,
                &cached.discord_status,
                &cached.discord_publisher,
            ))
        }
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

#[cfg(test)]
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

#[cfg(test)]
fn build_codex_discord_preview(
    sessions: &[CodexSessionSnapshot],
    config: &CodexPresenceConfig,
    desktop_surface_running: bool,
) -> DiscordPresencePreview {
    build_codex_discord_preview_with_access(sessions, config, desktop_surface_running, None)
}

#[cfg(test)]
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
    crate::discord_dirs::local_storage_dirs()
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
    let needle = format!("/banners/{user_id}/");
    let mut files = Vec::new();
    for root in crate::discord_dirs::cache_dirs() {
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
        Provider::Orion => PlanInfo {
            provider: "orion".into(),
            plan_key: String::new(),
            plan_name: "No account quota reported".into(),
            detected: false,
        },
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
        Provider::OpenCode | Provider::CommandCode | Provider::Orion => {
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
    // session: re-read them only after persistence succeeds.
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
#[expect(
    clippy::too_many_arguments,
    reason = "Tauri exposes these filters as flat invoke arguments; nesting them would break the IPC shape"
)]
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
                Provider::OpenCode | Provider::CommandCode | Provider::Orion => {
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

/// Re-reads every Claude transcript under the configured projects roots, as
/// one-off input for the history repair. Writes no parse checkpoints.
fn claude_history_session_infos() -> Result<Vec<SessionInfo>, String> {
    const WHOLE_HISTORY: Duration = Duration::from_secs(60 * 60 * 24 * 365 * 50);
    let roots = cc_discord_presence::config::projects_paths();
    let snapshots = session::collect_active_sessions_multi(
        &roots,
        WHOLE_HISTORY,
        WHOLE_HISTORY,
        &mut GitBranchCache::new(Duration::from_secs(300)),
        &mut SessionParseCache::without_checkpoint_writes(),
        &cc_discord_presence::config::read_ide_workspace_folders(),
    )
    .map_err(|error| error.to_string())?;
    Ok(build_claude_session_infos(&snapshots))
}

/// Shows how many stored Claude sessions differ from their transcripts and by
/// how much, without changing anything.
pub fn preview_claude_history_repair_blocking()
-> Result<crate::db::ClaudeHistoryRepairSummary, String> {
    let infos = claude_history_session_infos()?;
    crate::db::preview_claude_history_repair(&infos).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn preview_claude_history_repair() -> Result<crate::db::ClaudeHistoryRepairSummary, String>
{
    offload(preview_claude_history_repair_blocking).await
}

/// Backs up the analytics database, then rewrites stored Claude sessions from
/// their transcripts. Sessions whose transcript is gone are left unchanged.
#[tauri::command]
pub async fn apply_claude_history_repair() -> Result<crate::db::ClaudeHistoryRepairSummary, String>
{
    offload(|| {
        let infos = claude_history_session_infos()?;
        crate::db::apply_claude_history_repair(&infos).map_err(|error| format!("{error:#}"))
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
/// did not fit, which is how the screen came to report $7.73 against an
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
        .filter(|provider| {
            !matches!(
                provider,
                Provider::OpenCode | Provider::CommandCode | Provider::Orion
            )
        })
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
mod codex_probe_gate_tests;

#[cfg(test)]
mod tests;
