use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::config;

/// Cache usage data for 5 minutes — it changes slowly and the endpoint
/// shares rate limits with Claude Code itself.
const USAGE_CACHE_TTL: Duration = Duration::from_secs(300);
/// Fallback backoff when Retry-After header is missing (reduced from 300s).
const USAGE_RATE_LIMIT_FALLBACK: Duration = Duration::from_secs(30);
const USAGE_API_TIMEOUT: Duration = Duration::from_secs(10);
const USAGE_API_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// Honest User-Agent identifying this tool and its version.
const USER_AGENT: &str = concat!("cc-discord-presence/", env!("CARGO_PKG_VERSION"));

/// Claude Code CLI's registered OAuth client ID (used for token refresh).
const OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const TOKEN_REFRESH_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const TOKEN_REFRESH_COOLDOWN: Duration = Duration::from_secs(120);

/// Endpoint for enabling/disabling pay-per-use extra usage.
/// NOTE: This is inferred from the usage URL pattern (internal Anthropic API).
/// If the toggle is not working, open Chrome DevTools on claude.ai, click the
/// Extra Usage toggle, and update this constant with the correct URL + method.
const EXTRA_USAGE_TOGGLE_URL: &str = "https://api.anthropic.com/api/oauth/extra-usage";

/// Build a shared HTTP agent with the standard timeout.
fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(USAGE_API_TIMEOUT).build()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageData {
    pub five_hour: UsageWindow,
    pub seven_day: UsageWindow,
    // API returns "seven_day_sonnet"; keep alias for forward-compat
    #[serde(rename = "seven_day_sonnet", alias = "sonnet_free", default)]
    pub sonnet_free: Option<UsageWindow>,
    /// Current Anthropic responses also expose a structured `limits` array.
    /// Keep every provider-reported bucket here so new model-scoped windows
    /// (for example Fable) are not silently discarded by a fixed schema.
    #[serde(default, deserialize_with = "deserialize_usage_limits")]
    pub limits: Vec<UsageLimit>,
    #[serde(default)]
    pub extra_usage: Option<ExtraUsage>,
}

impl UsageData {
    fn normalize_cached_units(mut self) -> Self {
        if let Some(extra) = self.extra_usage.as_mut()
            && extra.monthly_limit.unwrap_or_default() >= 1000.0
        {
            extra.monthly_limit = extra.monthly_limit.map(|v| v / 100.0);
            extra.used_credits = extra.used_credits.map(|v| v / 100.0);
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageWindow {
    pub utilization: f64,
    /// Reset timestamp — API returns `null` for windows that haven't started
    /// (e.g. `seven_day_sonnet` when the user hasn't used Sonnet yet).
    /// Must be Optional to avoid breaking the whole UsageData parse.
    #[serde(default)]
    pub resets_at: Option<DateTime<Utc>>,
}

/// One provider-reported quota bucket from the structured Claude usage API.
/// Unknown kinds and missing percentages are retained during decoding but are
/// ignored by the access adapter rather than turned into fabricated windows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageLimit {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "deserialize_usage_percent")]
    pub percent: Option<f64>,
    #[serde(default)]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scope: Option<UsageLimitScope>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageLimitScope {
    #[serde(default)]
    pub model: Option<UsageLimitModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageLimitModel {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
}

fn deserialize_usage_limits<'de, D>(deserializer: D) -> Result<Vec<UsageLimit>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Vec<UsageLimit>>::deserialize(deserializer)?.unwrap_or_default())
}

fn deserialize_usage_percent<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<f64>::deserialize(deserializer)?
        .filter(|value| value.is_finite() && (0.0..=100.0).contains(value)))
}

/// Pay-per-use (extra) usage beyond the plan's included quota.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExtraUsage {
    #[serde(default)]
    pub is_enabled: bool,
    /// Monthly spending limit in USD (e.g. 75.0)
    pub monthly_limit: Option<f64>,
    /// Credits consumed this month in USD (e.g. 48.89)
    pub used_credits: Option<f64>,
    /// Percent of monthly limit consumed (0–100)
    pub utilization: Option<f64>,
}

#[derive(Serialize, Deserialize)]
struct UsageCacheFile {
    fetched_at_unix: u64,
    data: UsageData,
    /// Subscription tier of the account these figures were fetched for.
    ///
    /// Stored with the numbers rather than read from the current credentials:
    /// a cache written under Pro and later read after signing in as Max would
    /// otherwise be labelled `Cached · Max` while the figures still describe
    /// Pro. Absent in caches written before this field existed, in which case
    /// the tier is simply not claimed.
    #[serde(default)]
    subscription: Option<String>,
    /// Exact Claude Max multiplier observed with the cached subscription.
    /// Without this field, a legacy `subscriptionType=max` cache is unclaimed.
    #[serde(default)]
    rate_limit_tier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: OAuthCredentials,
}

#[derive(Debug, Deserialize)]
struct OAuthCredentials {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "expiresAt")]
    expires_at: i64,
    #[serde(rename = "refreshToken", default)]
    refresh_token: Option<String>,
    #[serde(rename = "subscriptionType", default)]
    subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier", default)]
    rate_limit_tier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAuthRefreshResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
}

#[derive(Debug)]
struct PlanFields {
    subscription_type: Option<String>,
    rate_limit_tier: Option<String>,
}

fn read_plan_fields_from_path(path: &Path) -> Option<PlanFields> {
    let data = std::fs::read_to_string(path).ok()?;
    let creds = serde_json::from_str::<CredentialsFile>(&data).ok()?;
    let credentials = PlanFields {
        subscription_type: creds.claude_ai_oauth.subscription_type,
        rate_limit_tier: creds.claude_ai_oauth.rate_limit_tier,
    };
    Some(merge_account_plan_fields(
        credentials,
        read_account_plan_fields(path),
    ))
}

/// Claude Code's account config sits next to the credentials directory
/// (`~/.claude.json` beside `~/.claude/`), or inside a custom Claude home.
fn account_config_candidates(credentials_path: &Path) -> Vec<PathBuf> {
    let Some(claude_dir) = credentials_path.parent() else {
        return Vec::new();
    };
    vec![
        claude_dir.join(".claude.json"),
        claude_dir.with_extension("json"),
    ]
}

/// Reads the plan fields Claude Code keeps in `oauthAccount`. Claude Code
/// refreshes this block from the account profile, while the credentials file
/// keeps the `rateLimitTier` recorded at login, so an upgrade from Max 5x to
/// Max 20x shows up here first.
fn read_account_plan_fields(credentials_path: &Path) -> Option<PlanFields> {
    let text = account_config_candidates(credentials_path)
        .into_iter()
        .find_map(|path| std::fs::read_to_string(path).ok())?;
    let config = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let account = config.get("oauthAccount")?;
    let field = |name: &str| {
        account
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    Some(PlanFields {
        subscription_type: field("organizationType"),
        rate_limit_tier: field("userRateLimitTier").or_else(|| field("organizationRateLimitTier")),
    })
}

/// Prefers the account profile's plan when it names a recognized Claude plan,
/// and otherwise keeps the credentials values.
fn merge_account_plan_fields(credentials: PlanFields, account: Option<PlanFields>) -> PlanFields {
    let Some(account) = account else {
        return credentials;
    };
    let recognized = |value: &Option<String>| {
        value
            .as_deref()
            .is_some_and(|v| classify_claude_plan_signal(&v.trim().to_ascii_lowercase()).is_some())
    };
    if !recognized(&account.rate_limit_tier) {
        return credentials;
    }
    PlanFields {
        subscription_type: if recognized(&account.subscription_type) {
            account.subscription_type
        } else {
            credentials.subscription_type
        },
        rate_limit_tier: account.rate_limit_tier,
    }
}

#[derive(Debug, Deserialize)]
struct ApiUsageData {
    five_hour: UsageWindow,
    seven_day: UsageWindow,
    #[serde(rename = "seven_day_sonnet", alias = "sonnet_free", default)]
    sonnet_free: Option<UsageWindow>,
    #[serde(default, deserialize_with = "deserialize_usage_limits")]
    limits: Vec<UsageLimit>,
    #[serde(default)]
    extra_usage: Option<ApiExtraUsage>,
}

#[derive(Debug, Deserialize)]
struct ApiExtraUsage {
    #[serde(default)]
    is_enabled: bool,
    monthly_limit: Option<f64>,
    used_credits: Option<f64>,
    utilization: Option<f64>,
}

impl From<ApiUsageData> for UsageData {
    fn from(value: ApiUsageData) -> Self {
        Self {
            five_hour: value.five_hour,
            seven_day: value.seven_day,
            sonnet_free: value.sonnet_free,
            limits: value.limits,
            extra_usage: value.extra_usage.map(|extra| ExtraUsage {
                is_enabled: extra.is_enabled,
                monthly_limit: extra.monthly_limit.map(|v| v / 100.0),
                used_credits: extra.used_credits.map(|v| v / 100.0),
                utilization: extra.utilization,
            }),
        }
    }
}

pub fn detect_plan_key(
    subscription_type: Option<&str>,
    rate_limit_tier: Option<&str>,
) -> Option<&'static str> {
    let sub = subscription_type.unwrap_or("").trim().to_ascii_lowercase();
    let tier = rate_limit_tier.unwrap_or("").trim().to_ascii_lowercase();
    let sub_signal = classify_claude_plan_signal(&sub);
    let tier_signal = classify_claude_plan_signal(&tier);

    // `subscriptionType=max` is only a family label. Do not turn it into a
    // plan claim until the credentials also carry the exact 5x/20x tier. A
    // conflicting pair is equally unknown; neither provider field wins by
    // precedence because that would make an accidental cross-tier claim.
    match (sub_signal, tier_signal) {
        (Some(ClaudePlanSignal::Max), Some(ClaudePlanSignal::Max5x)) => Some("max_5x"),
        (Some(ClaudePlanSignal::Max), Some(ClaudePlanSignal::Max20x)) => Some("max_20x"),
        (Some(ClaudePlanSignal::Max), _) => None,
        (Some(subscription), None) => subscription.key(),
        (Some(subscription), Some(tier)) if subscription == tier => subscription.key(),
        (None, Some(tier)) => tier.key(),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudePlanSignal {
    Free,
    Pro,
    Teams,
    Enterprise,
    Max,
    Max5x,
    Max20x,
}

impl ClaudePlanSignal {
    fn key(self) -> Option<&'static str> {
        Some(match self {
            Self::Free => "free",
            Self::Pro => "pro",
            Self::Teams => "team",
            Self::Enterprise => "enterprise",
            // A bare Max family signal is intentionally not a claim.
            Self::Max => return None,
            Self::Max5x => "max_5x",
            Self::Max20x => "max_20x",
        })
    }
}

fn classify_claude_plan_signal(raw: &str) -> Option<ClaudePlanSignal> {
    if raw.is_empty() {
        return None;
    }
    // Match complete provider identifiers rather than substrings. This keeps
    // `professional`, `freedom`, and `myteam-preview` from becoming false
    // Pro/Free/Team claims while accepting canonical underscore/hyphen IDs.
    let tokens = raw
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let has = |token: &str| tokens.contains(&token);
    let families = [
        has("max"),
        has("enterprise"),
        has("team") || has("teams"),
        has("pro"),
        has("free"),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if families != 1 || (has("20x") && has("5x")) {
        return None;
    }

    // Multiplier syntax belongs to Claude Max only. A Codex-style `pro_20x`
    // must not collapse into Claude's ordinary Pro plan.
    if (has("20x") || has("5x")) && !has("max") {
        return None;
    }
    if has("20x") && has("max") {
        return Some(ClaudePlanSignal::Max20x);
    }
    if has("5x") && has("max") {
        return Some(ClaudePlanSignal::Max5x);
    }
    if has("max") {
        return Some(ClaudePlanSignal::Max);
    }
    if has("enterprise") {
        return Some(ClaudePlanSignal::Enterprise);
    }
    if has("team") || has("teams") {
        return Some(ClaudePlanSignal::Teams);
    }
    if has("pro") {
        return Some(ClaudePlanSignal::Pro);
    }
    if has("free") {
        return Some(ClaudePlanSignal::Free);
    }
    None
}

/// How the usage figures on screen were actually obtained.
///
/// Recorded at the moment a request succeeds, never inferred from configuration
/// or defaults. Pulse used to label every quota reading `api`, which was wrong on
/// two counts: it is not an API-key call, and the numbers may not have come from
/// the network at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageOrigin {
    /// Auth scheme actually presented on the wire.
    pub auth: UsageAuth,
    /// Host actually queried.
    pub endpoint: String,
    /// Subscription tier reported by the credentials Pulse authenticated with.
    pub subscription: Option<String>,
    /// Rate-limit tier reported alongside the subscription family.
    pub rate_limit_tier: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageAuth {
    /// `Authorization: Bearer <access_token>` from `~/.claude/.credentials.json`,
    /// with the `anthropic-beta: oauth-2025-04-20` header. The only scheme the
    /// Claude usage endpoint accepts today.
    OAuth,
    /// Served from Pulse's own on-disk cache, so no request was made this cycle.
    Cache,
}

impl UsageOrigin {
    /// Short label for the UI. Names the scheme, then the plan when known.
    pub fn label(&self) -> String {
        let scheme = match self.auth {
            UsageAuth::OAuth => "OAuth",
            UsageAuth::Cache => "Cached",
        };
        match crate::usage::detect_plan_key(
            self.subscription.as_deref(),
            self.rate_limit_tier.as_deref(),
        )
        .map(crate::plan::name_from_key)
        {
            Some(plan) => format!("{scheme} · {plan}"),
            None => scheme.to_string(),
        }
    }
}

/// Host of the usage endpoint, derived from `USAGE_API_URL` so the reported
/// provenance cannot drift away from the URL actually called.
fn usage_endpoint_host() -> &'static str {
    USAGE_API_URL
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(USAGE_API_URL)
        .split('/')
        .next()
        .unwrap_or(USAGE_API_URL)
}

type FileUsageCache = (UsageData, Option<String>, Option<String>, DateTime<Utc>);

pub struct UsageManager {
    read_only: bool,
    cached_usage: Option<UsageData>,
    last_fetch: Option<Instant>,
    /// Set only when a request or cache read actually produced the figures.
    last_usage_origin: Option<UsageOrigin>,
    /// Timestamp attached to the response/cache that produced the figures.
    /// Keeping it separate from `last_fetch` prevents a cache read from
    /// masquerading as a fresh provider observation.
    last_usage_observed_at: Option<DateTime<Utc>>,
    credentials: Option<CredentialsFile>,
    subscription_type_cache: Option<String>,
    last_refresh_attempt: Option<Instant>,
    /// Backoff until this instant after a 429 rate-limit response.
    rate_limit_until: Option<Instant>,
    /// Human-readable status for TUI display.
    last_error_hint: Option<String>,
    /// Shared HTTP agent for connection reuse across API calls.
    agent: ureq::Agent,
    /// Number of fetch attempts — use shorter cache TTL for first 3 fetches.
    fetch_count: u32,
    /// Test-only override for the credentials file path.
    credentials_path_override: Option<PathBuf>,
}

impl UsageManager {
    pub fn for_credentials(path: PathBuf, allow_refresh: bool) -> Self {
        Self {
            read_only: !allow_refresh,
            credentials_path_override: Some(path),
            ..Self::new()
        }
    }

    pub fn read_only() -> Self {
        Self {
            read_only: true,
            ..Self::new()
        }
    }

    pub fn new() -> Self {
        Self {
            read_only: false,
            cached_usage: None,
            last_fetch: None,
            last_usage_origin: None,
            last_usage_observed_at: None,
            credentials: None,
            subscription_type_cache: None,
            last_refresh_attempt: None,
            rate_limit_until: None,
            last_error_hint: None,
            agent: http_agent(),
            fetch_count: 0,
            credentials_path_override: None,
        }
    }

    /// Returns the cached figures together with the subscription they were
    /// fetched for, so the caller can report provenance without borrowing the
    /// tier from whatever credentials happen to be loaded now.
    fn try_read_file_cache() -> Option<FileUsageCache> {
        let path = crate::config::usage_cache_path();
        let raw = std::fs::read_to_string(path).ok()?;
        let cache: UsageCacheFile = serde_json::from_str(&raw).ok()?;
        let now_unix = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        if cache.fetched_at_unix > now_unix || cache.fetched_at_unix > i64::MAX as u64 {
            return None;
        }
        if now_unix - cache.fetched_at_unix < USAGE_CACHE_TTL.as_secs() {
            let subscription = cache.subscription.filter(|plan| !plan.trim().is_empty());
            let observed_at = Utc
                .timestamp_opt(cache.fetched_at_unix as i64, 0)
                .single()?;
            Some((
                cache.data.normalize_cached_units(),
                subscription,
                cache.rate_limit_tier.filter(|tier| !tier.trim().is_empty()),
                observed_at,
            ))
        } else {
            None
        }
    }

    fn write_file_cache(
        data: &UsageData,
        subscription: Option<String>,
        rate_limit_tier: Option<String>,
    ) {
        let path = crate::config::usage_cache_path();
        let fetched_at_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let Ok(json) = serde_json::to_string(&UsageCacheFile {
            fetched_at_unix,
            data: data.clone(),
            subscription,
            rate_limit_tier,
        }) else {
            return;
        };
        if let Err(err) = std::fs::write(&path, json) {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "failed to write usage cache"
            );
        }
    }

    pub fn get_usage(&mut self) -> Option<UsageData> {
        // Shorter cache TTL for first 3 fetches to get initial data faster
        let ttl = if self.fetch_count < 3 {
            Duration::from_secs(30)
        } else {
            USAGE_CACHE_TTL
        };
        if let Some(ref usage) = self.cached_usage
            && let Some(last) = self.last_fetch
            && last.elapsed() < ttl
        {
            return Some(usage.clone());
        }

        if self.credentials_path_override.is_none()
            && let Some((cached, cached_subscription, cached_rate_limit_tier, observed_at)) =
                Self::try_read_file_cache()
        {
            // No request was made this cycle; say so instead of implying a live
            // read, and name only the tier stored alongside these very figures.
            self.last_usage_origin = Some(UsageOrigin {
                auth: UsageAuth::Cache,
                endpoint: crate::config::usage_cache_path().display().to_string(),
                subscription: cached_subscription,
                rate_limit_tier: cached_rate_limit_tier,
            });
            self.cached_usage = Some(cached.clone());
            self.last_fetch = Some(Instant::now());
            self.last_usage_observed_at = Some(observed_at);
            self.rate_limit_until = None;
            return Some(cached);
        }

        // Respect rate-limit backoff
        if let Some(until) = self.rate_limit_until
            && Instant::now() < until
        {
            return self.cached_usage.clone();
        }

        self.fetch_usage()
    }

    pub fn invalidate_cache(&mut self) {
        self.last_fetch = None;
        self.rate_limit_until = None;
    }

    /// Returns a hint about why usage data is unavailable (for TUI display).
    /// If rate-limited, shows a live countdown.
    pub fn retry_at(&self) -> Option<DateTime<Utc>> {
        let remaining = self
            .rate_limit_until?
            .checked_duration_since(Instant::now())?;
        Some(Utc::now() + chrono::Duration::from_std(remaining).ok()?)
    }

    pub fn error_hint_with_countdown(&self) -> Option<String> {
        if let Some(until) = self.rate_limit_until {
            let now = Instant::now();
            if until > now {
                let remaining = (until - now).as_secs();
                if remaining > 0 {
                    return Some(format!("refreshing in {}s", remaining));
                }
            }
            return None;
        }
        self.last_error_hint.clone()
    }

    /// How the figures currently held by this manager were obtained.
    ///
    /// `None` until something actually succeeds — a failed or rate-limited
    /// attempt must never produce a provenance claim.
    pub fn last_usage_origin(&self) -> Option<&UsageOrigin> {
        self.last_usage_origin.as_ref()
    }

    /// Timestamp observed on the provider response or stored alongside the
    /// file cache. Consumers use this to classify cache data as fresh/stale.
    pub fn last_usage_observed_at(&self) -> Option<DateTime<Utc>> {
        self.last_usage_observed_at
    }

    /// Describes the request `call_usage_api` just made. The scheme is not a
    /// guess: that function has exactly one code path, an OAuth bearer token
    /// plus the `anthropic-beta: oauth-2025-04-20` header. The subscription is
    /// read back from the very credentials that signed the request.
    fn observed_oauth_origin(&self) -> UsageOrigin {
        let credentials = PlanFields {
            subscription_type: self
                .credentials
                .as_ref()
                .and_then(|creds| creds.claude_ai_oauth.subscription_type.clone()),
            rate_limit_tier: self
                .credentials
                .as_ref()
                .and_then(|creds| creds.claude_ai_oauth.rate_limit_tier.clone()),
        };
        let fields = merge_account_plan_fields(
            credentials,
            read_account_plan_fields(&self.credentials_path()),
        );
        UsageOrigin {
            auth: UsageAuth::OAuth,
            endpoint: usage_endpoint_host().to_string(),
            subscription: fields
                .subscription_type
                .filter(|plan| !plan.trim().is_empty()),
            rate_limit_tier: fields
                .rate_limit_tier
                .filter(|tier| !tier.trim().is_empty()),
        }
    }

    /// Returns a clone of the current OAuth access token, if credentials are loaded.
    pub fn get_access_token(&mut self) -> Option<String> {
        self.ensure_credentials();
        self.credentials
            .as_ref()
            .map(|c| c.claude_ai_oauth.access_token.clone())
    }

    /// True when on-disk OAuth credentials are present and not expired.
    ///
    /// This is proof of authentication that is independent of whether the most
    /// recent usage figures came from a live API read or from the short-lived
    /// local cache. Consumers use it so a validly signed-in account that is
    /// momentarily served cached numbers is not mislabelled "sign in required".
    pub fn has_valid_oauth_credentials(&mut self) -> bool {
        self.ensure_credentials();
        self.credentials
            .as_ref()
            .is_some_and(|c| c.claude_ai_oauth.expires_at > Utc::now().timestamp_millis())
    }

    pub fn subscription_type(&mut self) -> Option<String> {
        if self.subscription_type_cache.is_some() {
            return self.subscription_type_cache.clone();
        }
        self.ensure_credentials();
        if let Some(ref creds) = self.credentials {
            self.subscription_type_cache = creds.claude_ai_oauth.subscription_type.clone();
        }
        self.subscription_type_cache.clone()
    }

    pub fn detected_plan_key(&mut self) -> Option<String> {
        if let Some(fields) = self.read_plan_fields_from_disk()
            && let Some(key) = detect_plan_key(
                fields.subscription_type.as_deref(),
                fields.rate_limit_tier.as_deref(),
            )
        {
            return Some(key.to_string());
        }

        self.ensure_credentials();
        let creds = self.credentials.as_ref()?;
        detect_plan_key(
            creds.claude_ai_oauth.subscription_type.as_deref(),
            creds.claude_ai_oauth.rate_limit_tier.as_deref(),
        )
        .map(str::to_string)
    }

    // ── Core API call + response handling (shared by all fetch paths) ──────

    /// Send an authenticated GET to the usage API using the current credentials.
    fn call_usage_api(&self) -> Option<Result<ureq::Response, ureq::Error>> {
        let creds = self.credentials.as_ref()?;
        Some(
            self.agent
                .get(USAGE_API_URL)
                .set(
                    "Authorization",
                    &format!("Bearer {}", creds.claude_ai_oauth.access_token),
                )
                .set("anthropic-beta", "oauth-2025-04-20")
                .set("User-Agent", USER_AGENT)
                .call(),
        )
    }

    /// Parse a usage API response, updating cache and error state.
    fn handle_usage_response(
        &mut self,
        response: Result<ureq::Response, ureq::Error>,
    ) -> Option<UsageData> {
        match response {
            Ok(resp) => {
                let body = match resp.into_string() {
                    Ok(b) => b,
                    Err(e) => {
                        warn!("Failed to read usage API response: {e}");
                        self.last_error_hint = Some("bad response".to_string());
                        return self.cached_usage.clone();
                    }
                };

                match serde_json::from_str::<ApiUsageData>(&body) {
                    Ok(parsed) => {
                        let usage: UsageData = parsed.into();
                        // Recorded here, on an observed 200, so the footer states
                        // the handshake that actually produced these numbers.
                        self.last_usage_origin = Some(self.observed_oauth_origin());
                        self.last_usage_observed_at = Some(Utc::now());
                        self.cached_usage = Some(usage.clone());
                        self.last_fetch = Some(Instant::now());
                        self.last_error_hint = None;
                        self.rate_limit_until = None;
                        if self.credentials_path_override.is_none() {
                            Self::write_file_cache(
                                &usage,
                                self.last_usage_origin
                                    .as_ref()
                                    .and_then(|origin| origin.subscription.clone()),
                                self.last_usage_origin
                                    .as_ref()
                                    .and_then(|origin| origin.rate_limit_tier.clone()),
                            );
                        }
                        Some(usage)
                    }
                    Err(e) => {
                        warn!("Failed to parse usage API response: {e}");
                        self.last_error_hint = Some("parse error".to_string());
                        self.cached_usage.clone()
                    }
                }
            }
            Err(ureq::Error::Status(429, resp)) => {
                let retry_after = Self::parse_retry_after(&resp);
                debug!(
                    retry_after_secs = retry_after.as_secs(),
                    "Usage API rate limited"
                );
                self.rate_limit_until = Some(Instant::now() + retry_after);
                let secs = retry_after.as_secs();
                self.last_error_hint = Some(format!("refreshing in {secs}s"));
                self.cached_usage.clone()
            }
            Err(e) => {
                debug!("Usage API request failed: {e}");
                self.last_error_hint = Some("API unreachable".to_string());
                self.cached_usage.clone()
            }
        }
    }

    // ── Fetch orchestration ───────────────────────────────────────────────

    fn fetch_usage(&mut self) -> Option<UsageData> {
        self.fetch_count = self.fetch_count.saturating_add(1);
        self.ensure_credentials();
        if self.credentials.is_none() {
            self.last_error_hint = Some("no credentials — check .credentials.json".to_string());
            return None;
        }

        // Refresh expired tokens before calling the API
        if let Some(ref creds) = self.credentials
            && creds.claude_ai_oauth.expires_at < Utc::now().timestamp_millis()
        {
            debug!("OAuth token expired, attempting refresh");
            if !self.try_refresh_token() {
                self.credentials = None;
                self.ensure_credentials();
                if let Some(ref creds) = self.credentials {
                    if creds.claude_ai_oauth.expires_at < Utc::now().timestamp_millis() {
                        debug!("Token still expired after reload, skipping API call");
                        self.last_error_hint = Some("token expired".to_string());
                        return self.cached_usage.clone();
                    }
                } else {
                    self.last_error_hint = Some("token expired, refresh failed".to_string());
                    return self.cached_usage.clone();
                }
            }
        }

        let response = match self.call_usage_api() {
            Some(r) => r,
            None => return self.cached_usage.clone(),
        };

        // On 401, try refreshing the token once and retry
        if let Err(ureq::Error::Status(401, _)) = &response {
            debug!("Usage API returned 401, attempting token refresh");
            self.last_error_hint = Some("re-authenticating...".to_string());
            if self.try_refresh_token()
                && let Some(retry_response) = self.call_usage_api()
            {
                return self.handle_usage_response(retry_response);
            }
            self.credentials = None;
            self.last_error_hint = Some("auth failed — re-login to claude.ai".to_string());
            return self.cached_usage.clone();
        }

        self.handle_usage_response(response)
    }

    /// Parse the Retry-After header from a 429 response.
    fn parse_retry_after(resp: &ureq::Response) -> Duration {
        resp.header("retry-after")
            .and_then(|v| v.parse::<u64>().ok())
            .map(|secs| Duration::from_secs(secs + 5)) // reduced safety margin (was 15s)
            .unwrap_or(USAGE_RATE_LIMIT_FALLBACK)
    }

    /// Attempt to refresh the OAuth token using the refresh_token.
    /// Returns true if the token was successfully refreshed and credentials updated.
    fn try_refresh_token(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        // Respect cooldown to avoid hammering the endpoint
        if let Some(last) = self.last_refresh_attempt
            && last.elapsed() < TOKEN_REFRESH_COOLDOWN
        {
            return false;
        }
        self.last_refresh_attempt = Some(Instant::now());

        let refresh_token = self
            .credentials
            .as_ref()
            .and_then(|c| c.claude_ai_oauth.refresh_token.clone());

        let Some(refresh_token) = refresh_token else {
            debug!("No refresh token available");
            return false;
        };

        debug!("Refreshing OAuth token");
        let body = serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": OAUTH_CLIENT_ID,
        });

        let result = self
            .agent
            .post(TOKEN_REFRESH_URL)
            .set("Content-Type", "application/json")
            .send_string(&body.to_string());

        match result {
            Ok(resp) => {
                let body_str = match resp.into_string() {
                    Ok(b) => b,
                    Err(e) => {
                        warn!("Failed to read token refresh response: {e}");
                        return false;
                    }
                };

                let refresh_resp = match serde_json::from_str::<OAuthRefreshResponse>(&body_str) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("Failed to parse token refresh response: {e}");
                        return false;
                    }
                };

                let expires_at =
                    Utc::now().timestamp_millis() + (refresh_resp.expires_in as i64 * 1000);

                // Write updated credentials back to file atomically
                if self.save_refreshed_credentials(&refresh_resp, expires_at) {
                    // Reload from the file we just wrote
                    self.credentials = None;
                    self.ensure_credentials();
                    debug!(
                        expires_in = refresh_resp.expires_in,
                        "OAuth token refreshed successfully"
                    );
                    true
                } else {
                    false
                }
            }
            Err(ureq::Error::Status(status, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                debug!(status, body = %body, "Token refresh HTTP error");
                false
            }
            Err(e) => {
                debug!(error = %e, "Token refresh request failed");
                false
            }
        }
    }

    /// Read the credentials file as raw JSON, update the token fields, and write back.
    /// This preserves all other fields (scopes, subscriptionType, etc.).
    fn save_refreshed_credentials(
        &self,
        refresh_resp: &OAuthRefreshResponse,
        expires_at: i64,
    ) -> bool {
        let cred_path = config::credentials_path();
        let Ok(data) = std::fs::read_to_string(&cred_path) else {
            return false;
        };
        let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&data) else {
            return false;
        };

        if let Some(oauth) = json
            .get_mut("claudeAiOauth")
            .and_then(|v| v.as_object_mut())
        {
            oauth.insert(
                "accessToken".to_string(),
                serde_json::Value::String(refresh_resp.access_token.clone()),
            );
            oauth.insert(
                "expiresAt".to_string(),
                serde_json::Value::Number(serde_json::Number::from(expires_at)),
            );
            if let Some(ref new_refresh) = refresh_resp.refresh_token {
                oauth.insert(
                    "refreshToken".to_string(),
                    serde_json::Value::String(new_refresh.clone()),
                );
            }
        } else {
            return false;
        }

        let Ok(updated) = serde_json::to_string_pretty(&json) else {
            return false;
        };
        if std::fs::write(&cred_path, updated).is_err() {
            warn!(
                "Failed to write refreshed credentials to {}",
                cred_path.display()
            );
            return false;
        }
        true
    }

    fn read_plan_fields_from_disk(&self) -> Option<PlanFields> {
        let cred_path = self
            .credentials_path_override
            .clone()
            .unwrap_or_else(config::credentials_path);
        read_plan_fields_from_path(&cred_path)
    }

    fn credentials_path(&self) -> PathBuf {
        self.credentials_path_override
            .clone()
            .unwrap_or_else(config::credentials_path)
    }

    #[cfg(test)]
    fn with_credentials_path(path: PathBuf) -> Self {
        Self {
            credentials_path_override: Some(path),
            ..Self::new()
        }
    }

    fn ensure_credentials(&mut self) {
        if self.credentials.is_some() {
            return;
        }

        let cred_path = self.credentials_path();
        let Ok(data) = std::fs::read_to_string(&cred_path) else {
            debug!("Cannot read credentials file: {}", cred_path.display());
            return;
        };

        match serde_json::from_str::<CredentialsFile>(&data) {
            Ok(creds) => {
                self.subscription_type_cache = creds.claude_ai_oauth.subscription_type.clone();
                self.credentials = Some(creds);
            }
            Err(e) => {
                warn!("Cannot parse credentials: {e}");
            }
        }
    }
}

impl Default for UsageManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Claude.ai session-cookie toggle helpers ───────────────────────────────────

/// Fetches the first organization UUID accessible with the given claude.ai session cookie.
fn get_org_uuid(session_key: &str) -> Option<String> {
    let resp = http_agent()
        .get("https://claude.ai/api/organizations")
        .set("Cookie", &format!("sessionKey={session_key}"))
        .set("User-Agent", "Mozilla/5.0")
        .call()
        .ok()?;
    let text = resp.into_string().ok()?;
    let orgs: Vec<serde_json::Value> = serde_json::from_str(&text).ok()?;
    orgs.into_iter()
        .next()?
        .get("uuid")?
        .as_str()
        .map(str::to_string)
}

/// Sends a PATCH to the claude.ai overage_spend_limit endpoint.
/// Returns `true` on HTTP success, `false` otherwise (caller falls back to Bearer path).
fn toggle_extra_usage_via_session(session_key: &str, org_uuid: &str, enabled: bool) -> bool {
    let url = format!("https://claude.ai/api/organizations/{org_uuid}/overage_spend_limit");
    let body = format!("{{\"is_enabled\":{enabled}}}");
    let result = http_agent()
        .request("PATCH", &url)
        .set("Cookie", &format!("sessionKey={session_key}"))
        .set("Content-Type", "application/json")
        .set("User-Agent", "Mozilla/5.0")
        .send_string(&body);
    match result {
        Ok(resp) => {
            debug!(
                enabled,
                status = resp.status(),
                "extra usage toggle via session cookie"
            );
            true
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            debug!(enabled, status, body = %body, "extra usage session toggle HTTP error");
            false
        }
        Err(e) => {
            debug!(enabled, error = %e, "extra usage session toggle failed");
            false
        }
    }
}

// ── Public toggle entry point ─────────────────────────────────────────────────

/// Spawns a background thread that disables Extra Usage, waits 3 seconds, then re-enables it.
///
/// Prefers the `claude.ai` session-cookie path (real endpoint).
/// Falls back to the Bearer-token path as a secondary attempt; any HTTP errors there
/// are logged at `debug` level only (the endpoint may not exist).
pub fn spawn_extra_usage_toggle_cycle(access_token: String, session_key: Option<String>) {
    std::thread::spawn(move || {
        // Resolve org UUID once — only needed for the session-cookie path.
        let org_uuid = session_key.as_deref().and_then(get_org_uuid);

        let agent = http_agent();

        let do_toggle = |enabled: bool| {
            // Session-cookie path (real claude.ai endpoint).
            if let (Some(sk), Some(uuid)) = (&session_key, &org_uuid)
                && toggle_extra_usage_via_session(sk, uuid, enabled)
            {
                return;
            }

            // Bearer-token fallback (endpoint may 404 — debug only, no WARN).
            let body = format!("{{\"enabled\":{enabled}}}");
            let result = agent
                .put(EXTRA_USAGE_TOGGLE_URL)
                .set("Authorization", &format!("Bearer {access_token}"))
                .set("anthropic-beta", "oauth-2025-04-20")
                .set("Content-Type", "application/json")
                .set("User-Agent", USER_AGENT)
                .send_string(&body);
            match result {
                Ok(resp) => {
                    debug!(
                        enabled,
                        status = resp.status(),
                        "extra usage bearer toggle response"
                    );
                }
                Err(ureq::Error::Status(status, resp)) => {
                    let body = resp.into_string().unwrap_or_default();
                    debug!(enabled, status, body = %body, "extra usage bearer toggle HTTP error");
                }
                Err(e) => {
                    debug!(enabled, error = %e, "extra usage bearer toggle failed");
                }
            }
        };

        do_toggle(false);
        std::thread::sleep(Duration::from_secs(3));
        do_toggle(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Redirects `PULSE_HOME` at a throwaway directory for the caller's scope.
    ///
    /// A successful `handle_usage_response` writes the usage file cache. Without
    /// this, running the suite on a developer machine would stamp fixture quota
    /// figures into the real `~/.pulse-analytics/claude/discord-presence-usage-cache.json`, and
    /// Pulse would display those invented numbers for the next five minutes.
    struct IsolatedPulseHome {
        _guard: std::sync::MutexGuard<'static, ()>,
        _dir: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
    }

    impl IsolatedPulseHome {
        fn new() -> Self {
            let guard = crate::config::home_env_lock()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let dir = tempfile::tempdir().expect("temp claude home");
            std::fs::create_dir(dir.path().join("claude")).expect("isolated usage directory");
            let previous = std::env::var_os("PULSE_HOME");
            unsafe { std::env::set_var("PULSE_HOME", dir.path()) };
            Self {
                _guard: guard,
                _dir: dir,
                previous,
            }
        }
    }

    impl Drop for IsolatedPulseHome {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => unsafe { std::env::set_var("PULSE_HOME", value) },
                None => unsafe { std::env::remove_var("PULSE_HOME") },
            }
        }
    }

    fn credentials_fixture(subscription: &str) -> CredentialsFile {
        serde_json::from_str(&format!(
            r#"{{"claudeAiOauth":{{"accessToken":"token","expiresAt":9999999999999,
                "refreshToken":"refresh","subscriptionType":"{subscription}",
                "rateLimitTier":"default_claude_max_20x"}}}}"#
        ))
        .expect("credentials fixture")
    }

    fn usage_response_fixture() -> ureq::Response {
        ureq::Response::new(
            200,
            "OK",
            r#"{"five_hour":{"utilization":10.0,"resets_at":null},
                "seven_day":{"utilization":14.0,"resets_at":null}}"#,
        )
        .expect("usage response fixture")
    }

    #[test]
    fn successful_usage_fetch_records_the_handshake_it_actually_used() {
        let _home = IsolatedPulseHome::new();
        // A temporary credentials path keeps the account-profile lookup away
        // from the real `~/.claude.json`.
        let claude_dir = tempfile::tempdir().expect("tempdir");
        let mut manager =
            UsageManager::with_credentials_path(claude_dir.path().join(".credentials.json"));
        manager.credentials = Some(credentials_fixture("max"));

        let usage = manager.handle_usage_response(Ok(usage_response_fixture()));
        assert!(usage.is_some(), "fixture response must parse");

        let origin = manager
            .last_usage_origin()
            .expect("a successful fetch must record how it authenticated");
        assert_eq!(origin.auth, UsageAuth::OAuth);
        assert_eq!(origin.endpoint, "api.anthropic.com");
        assert_eq!(origin.subscription.as_deref(), Some("max"));
        assert_eq!(
            origin.rate_limit_tier.as_deref(),
            Some("default_claude_max_20x")
        );
        assert_eq!(origin.label(), "OAuth · Max 20x");
        assert_ne!(
            origin.label().to_ascii_lowercase(),
            "api",
            "the footer must name the real handshake, not a generic api label"
        );
    }

    #[test]
    fn usage_origin_label_degrades_without_a_subscription_field() {
        let _home = IsolatedPulseHome::new();
        // A temporary credentials path keeps the account-profile lookup away
        // from the real `~/.claude.json`.
        let claude_dir = tempfile::tempdir().expect("tempdir");
        let mut manager =
            UsageManager::with_credentials_path(claude_dir.path().join(".credentials.json"));
        manager.credentials = Some(
            serde_json::from_str(
                r#"{"claudeAiOauth":{"accessToken":"token","expiresAt":9999999999999}}"#,
            )
            .expect("credentials without plan fields"),
        );

        manager.handle_usage_response(Ok(usage_response_fixture()));
        let origin = manager.last_usage_origin().expect("origin recorded");

        assert_eq!(origin.subscription, None);
        assert_eq!(origin.label(), "OAuth");
    }

    #[test]
    fn usage_origin_does_not_claim_bare_claude_max() {
        let _home = IsolatedPulseHome::new();
        // A temporary credentials path keeps the account-profile lookup away
        // from the real `~/.claude.json`.
        let claude_dir = tempfile::tempdir().expect("tempdir");
        let mut manager =
            UsageManager::with_credentials_path(claude_dir.path().join(".credentials.json"));
        manager.credentials = Some(
            serde_json::from_str(
                r#"{"claudeAiOauth":{"accessToken":"token","expiresAt":9999999999999,
                    "subscriptionType":"max"}}"#,
            )
            .expect("credentials without Max multiplier"),
        );

        manager.handle_usage_response(Ok(usage_response_fixture()));
        let origin = manager.last_usage_origin().expect("origin recorded");

        assert_eq!(origin.subscription.as_deref(), Some("max"));
        assert_eq!(origin.rate_limit_tier, None);
        assert_eq!(origin.label(), "OAuth");
    }

    #[test]
    fn a_failed_fetch_does_not_invent_an_origin() {
        let _home = IsolatedPulseHome::new();
        // A temporary credentials path keeps the account-profile lookup away
        // from the real `~/.claude.json`.
        let claude_dir = tempfile::tempdir().expect("tempdir");
        let mut manager =
            UsageManager::with_credentials_path(claude_dir.path().join(".credentials.json"));
        manager.credentials = Some(credentials_fixture("max"));

        manager.handle_usage_response(Err(ureq::Error::Status(
            500,
            ureq::Response::new(500, "Server Error", "boom").expect("error response"),
        )));

        assert!(
            manager.last_usage_origin().is_none(),
            "an origin must describe an observed successful fetch, never an attempt"
        );
    }

    #[test]
    fn api_usage_extra_usage_is_normalized_to_usd() {
        let parsed: ApiUsageData = serde_json::from_str(
            r#"{
                "five_hour": { "utilization": 1.0, "resets_at": null },
                "seven_day": { "utilization": 2.0, "resets_at": null },
                "extra_usage": {
                    "is_enabled": true,
                    "monthly_limit": 20000,
                    "used_credits": 20035.0,
                    "utilization": 100.0
                }
            }"#,
        )
        .unwrap();

        let usage: UsageData = parsed.into();
        let extra = usage.extra_usage.unwrap();
        assert_eq!(extra.monthly_limit, Some(200.0));
        assert_eq!(extra.used_credits, Some(200.35));
    }

    #[test]
    fn structured_usage_limits_preserve_model_scoped_fable_bucket() {
        let parsed: ApiUsageData = serde_json::from_str(
            r#"{
                "five_hour": { "utilization": 1.0, "resets_at": null },
                "seven_day": { "utilization": 2.0, "resets_at": null },
                "limits": [
                    { "kind": "session", "percent": 1.0, "resets_at": null },
                    { "kind": "weekly_all", "percent": 2.0, "resets_at": null },
                    { "kind": "weekly_scoped", "percent": 68.0, "resets_at": null,
                      "scope": { "model": { "display_name": "Fable", "id": "fable" } } }
                ]
            }"#,
        )
        .expect("structured limits fixture");

        let usage: UsageData = parsed.into();
        assert_eq!(usage.limits.len(), 3);
        assert_eq!(
            usage.limits[2]
                .scope
                .as_ref()
                .and_then(|scope| scope.model.as_ref())
                .and_then(|model| model.display_name.as_deref()),
            Some("Fable")
        );
        assert_eq!(usage.limits[2].percent, Some(68.0));
    }

    #[test]
    fn structured_usage_limits_drop_out_of_range_percentages() {
        let parsed: ApiUsageData = serde_json::from_str(
            r#"{
                "five_hour": { "utilization": 1.0, "resets_at": null },
                "seven_day": { "utilization": 2.0, "resets_at": null },
                "limits": [
                    { "kind": "session", "percent": -1.0 },
                    { "kind": "weekly_all", "percent": 150.0 }
                ]
            }"#,
        )
        .expect("structured limits fixture");

        let usage: UsageData = parsed.into();
        assert_eq!(usage.limits[0].percent, None);
        assert_eq!(usage.limits[1].percent, None);
    }

    #[test]
    fn conflicting_claude_plan_signals_fail_closed() {
        assert_eq!(
            classify_claude_plan_signal("default_claude_max_5x_20x"),
            None
        );
        assert_eq!(classify_claude_plan_signal("claude_team_pro"), None);
    }

    #[test]
    fn cached_usage_cents_are_normalized_once() {
        let usage = UsageData {
            five_hour: UsageWindow {
                utilization: 0.0,
                resets_at: None,
            },
            seven_day: UsageWindow {
                utilization: 0.0,
                resets_at: None,
            },
            sonnet_free: None,
            limits: Vec::new(),
            extra_usage: Some(ExtraUsage {
                is_enabled: true,
                monthly_limit: Some(20000.0),
                used_credits: Some(20035.0),
                utilization: Some(100.0),
            }),
        }
        .normalize_cached_units();

        let extra = usage.extra_usage.unwrap();
        assert_eq!(extra.monthly_limit, Some(200.0));
        assert_eq!(extra.used_credits, Some(200.35));
    }

    #[test]
    fn detected_plan_key_reads_fresh_plan_fields_from_credentials_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        std::fs::write(
            &path,
            r#"{
                "claudeAiOauth": {
                    "accessToken": "token",
                    "expiresAt": 4102444800000,
                    "subscriptionType": "max",
                    "rateLimitTier": "default_claude_max_5x"
                }
            }"#,
        )
        .expect("write credentials");

        let mut manager = UsageManager::with_credentials_path(path.clone());
        assert_eq!(manager.detected_plan_key().as_deref(), Some("max_5x"));

        std::fs::write(
            &path,
            r#"{
                "claudeAiOauth": {
                    "accessToken": "token",
                    "expiresAt": 4102444800000,
                    "subscriptionType": "max",
                    "rateLimitTier": "default_claude_max_20x"
                }
            }"#,
        )
        .expect("rewrite credentials");

        assert_eq!(manager.detected_plan_key().as_deref(), Some("max_20x"));
    }

    fn write_max_credentials(dir: &Path, tier: &str) -> PathBuf {
        let path = dir.join(".credentials.json");
        std::fs::write(
            &path,
            format!(
                r#"{{"claudeAiOauth": {{"accessToken": "token", "expiresAt": 4102444800000,
                    "subscriptionType": "max", "rateLimitTier": "{tier}"}}}}"#
            ),
        )
        .expect("write credentials");
        path
    }

    /// Claude Code keeps the login-time `rateLimitTier` in the credentials
    /// file, while `oauthAccount` in `.claude.json` follows the account
    /// profile. After an upgrade from Max 5x to Max 20x only the latter moves.
    #[test]
    fn account_profile_tier_overrides_stale_login_tier() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_max_credentials(dir.path(), "default_claude_max_5x");
        std::fs::write(
            dir.path().join(".claude.json"),
            r#"{"oauthAccount": {"organizationType": "claude_max",
                "organizationRateLimitTier": "default_claude_max_20x",
                "userRateLimitTier": null}}"#,
        )
        .expect("write account config");

        let mut manager = UsageManager::with_credentials_path(path);
        assert_eq!(manager.detected_plan_key().as_deref(), Some("max_20x"));
    }

    #[test]
    fn user_tier_takes_precedence_over_organization_tier() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_max_credentials(dir.path(), "default_claude_max_5x");
        std::fs::write(
            dir.path().join(".claude.json"),
            r#"{"oauthAccount": {"organizationType": "claude_max",
                "organizationRateLimitTier": "default_claude_max_5x",
                "userRateLimitTier": "default_claude_max_20x"}}"#,
        )
        .expect("write account config");

        let mut manager = UsageManager::with_credentials_path(path);
        assert_eq!(manager.detected_plan_key().as_deref(), Some("max_20x"));
    }

    #[test]
    fn credentials_tier_is_kept_without_a_recognized_account_tier() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_max_credentials(dir.path(), "default_claude_max_20x");

        let mut manager = UsageManager::with_credentials_path(path.clone());
        assert_eq!(
            manager.detected_plan_key().as_deref(),
            Some("max_20x"),
            "no account config"
        );

        std::fs::write(
            dir.path().join(".claude.json"),
            r#"{"oauthAccount": {"organizationType": "claude_max",
                "organizationRateLimitTier": "some_future_tier", "userRateLimitTier": ""}}"#,
        )
        .expect("write account config");
        assert_eq!(
            manager.detected_plan_key().as_deref(),
            Some("max_20x"),
            "unrecognized account tier"
        );

        std::fs::write(dir.path().join(".claude.json"), "not json").expect("corrupt config");
        assert_eq!(
            manager.detected_plan_key().as_deref(),
            Some("max_20x"),
            "unreadable account config"
        );
    }

    #[test]
    fn account_config_is_found_beside_the_claude_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        let claude_dir = root.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).expect("claude dir");
        let path = write_max_credentials(&claude_dir, "default_claude_max_5x");
        std::fs::write(
            root.path().join(".claude.json"),
            r#"{"oauthAccount": {"organizationType": "claude_max",
                "organizationRateLimitTier": "default_claude_max_20x"}}"#,
        )
        .expect("write sibling account config");

        let mut manager = UsageManager::with_credentials_path(path);
        assert_eq!(manager.detected_plan_key().as_deref(), Some("max_20x"));
    }

    #[test]
    fn detect_plan_key_prefers_tier_detail_for_max() {
        assert_eq!(
            detect_plan_key(Some("max"), Some("default_claude_max_20x")),
            Some("max_20x")
        );
        assert_eq!(detect_plan_key(Some("claude_pro_2025"), None), Some("pro"));
        assert_eq!(detect_plan_key(Some("team"), None), Some("team"));
    }

    #[test]
    fn ambiguous_or_conflicting_claude_max_signals_are_unknown() {
        assert_eq!(detect_plan_key(Some("max"), None), None);
        assert_eq!(
            detect_plan_key(Some("max"), Some("default_claude_max")),
            None
        );
        assert_eq!(
            detect_plan_key(Some("pro"), Some("default_claude_max_20x")),
            None
        );
        assert_eq!(detect_plan_key(Some("pro_20x"), None), None);
    }

    #[test]
    fn plan_detection_requires_delimited_known_ids() {
        assert_eq!(detect_plan_key(Some("professional"), None), None);
        assert_eq!(detect_plan_key(Some("freedom"), None), None);
        assert_eq!(detect_plan_key(Some("myteam-preview"), None), None);
        assert_eq!(detect_plan_key(Some("claude-pro"), None), Some("pro"));
        assert_eq!(
            detect_plan_key(None, Some("default_claude_max_20x")),
            Some("max_20x")
        );
    }

    #[test]
    fn file_cache_rejects_future_and_out_of_range_timestamps() {
        let _home = IsolatedPulseHome::new();
        let data = UsageData {
            five_hour: UsageWindow {
                utilization: 1.0,
                resets_at: None,
            },
            seven_day: UsageWindow {
                utilization: 2.0,
                resets_at: None,
            },
            sonnet_free: None,
            limits: Vec::new(),
            extra_usage: None,
        };
        let write_cache = |fetched_at_unix| {
            let cache = UsageCacheFile {
                fetched_at_unix,
                data: data.clone(),
                subscription: None,
                rate_limit_tier: None,
            };
            std::fs::write(
                crate::config::usage_cache_path(),
                serde_json::to_vec(&cache).expect("cache JSON"),
            )
            .expect("cache write");
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_secs();
        write_cache(now.saturating_add(3600));
        assert!(
            UsageManager::try_read_file_cache().is_none(),
            "future cache observations must not become fresh quota"
        );

        write_cache(i64::MAX as u64 + 1);
        assert!(
            UsageManager::try_read_file_cache().is_none(),
            "timestamps outside DateTime's Unix range must be rejected"
        );
    }
}
