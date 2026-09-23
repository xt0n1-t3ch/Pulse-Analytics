//! Debug-only HTTP bridge for browser development.
//!
//! Tauri's `invoke()` transport only exists inside the native webview. The
//! Vite browser therefore talks to this loopback endpoint while development
//! work is in progress. The bridge is intentionally small and boring:
//! loopback-only binding, one authenticated POST endpoint, an explicit command
//! allowlist, and no fixtures or fallback payloads. Safe settings and
//! notification controls remain available only through that authenticated
//! path; shell/open-url actions are intentionally excluded.
//!
//! The endpoint is compiled only for debug builds. A token must be present in
//! `PULSE_DEV_BRIDGE_TOKEN`; an unset or empty token means that no listener is
//! started. CORS is restricted to the local Vite origins rather than `*`.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Loopback port shared by the Vite development proxy and this bridge.
pub const DEV_BRIDGE_PORT: u16 = 1421;
/// Environment variable carrying the shared secret for browser requests.
pub const DEV_BRIDGE_TOKEN_ENV: &str = "PULSE_DEV_BRIDGE_TOKEN";
const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_HEADER_LINE_BYTES: usize = 4 * 1024;
const MAX_HEADER_COUNT: usize = 64;
const MAX_CHUNK_COUNT: usize = 1024;
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const MAX_DAYS: i64 = 3_660;
const MAX_LIMIT: i64 = 500;
fn configured_port(name: &str, default: u16) -> io::Result<u16> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u16>()
            .ok()
            .filter(|port| *port >= 1024)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid {name}"))),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid {name}"),
        )),
    }
}

fn bind_address() -> io::Result<SocketAddr> {
    let bridge_port = configured_port("PULSE_DEV_BRIDGE_PORT", DEV_BRIDGE_PORT)?;
    if bridge_port == configured_port("PULSE_DEV_PORT", 1420)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "UI and bridge ports must differ",
        ));
    }
    Ok(SocketAddr::from((Ipv4Addr::LOCALHOST, bridge_port)))
}

fn allowed_origin(origin: Option<&str>) -> Option<&str> {
    allowed_origin_for_port(origin, configured_port("PULSE_DEV_PORT", 1420).ok()?)
}

fn allowed_origin_for_port(origin: Option<&str>, port: u16) -> Option<&str> {
    match origin {
        None => Some(""),
        Some(value)
            if value == format!("http://localhost:{port}")
                || value == format!("http://127.0.0.1:{port}") =>
        {
            Some(value)
        }
        Some(_) => None,
    }
}

fn configured_token() -> Option<String> {
    std::env::var(DEV_BRIDGE_TOKEN_ENV)
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

#[derive(Debug, PartialEq, Eq)]
enum AuthError {
    Missing,
    Invalid,
}

fn authorize(header: Option<&str>, expected_token: &str) -> Result<(), AuthError> {
    if expected_token.is_empty() {
        return Err(AuthError::Invalid);
    }
    let Some(value) = header else {
        return Err(AuthError::Missing);
    };
    let Some(provided) = value.strip_prefix("Bearer ") else {
        return Err(AuthError::Invalid);
    };
    (!provided.is_empty() && provided == expected_token)
        .then_some(())
        .ok_or(AuthError::Invalid)
}

#[derive(Debug, Deserialize)]
struct InvokeRequest {
    command: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Default)]
struct BridgeTarget {
    account_id: Option<String>,
    account_request: Option<crate::accounts::ConnectAccountRequest>,
    command: String,
    days: Option<i64>,
    session_id: Option<String>,
    session_ids: Option<Vec<String>>,
    provider: Option<String>,
    plan: Option<String>,
    enabled: Option<bool>,
    show_project: Option<bool>,
    show_branch: Option<bool>,
    show_model: Option<bool>,
    show_activity: Option<bool>,
    show_tokens: Option<bool>,
    show_cost: Option<bool>,
    show_limits: Option<bool>,
    show_credits: Option<bool>,
    show_context: Option<bool>,
    show_systems: Option<bool>,
    field_order: Option<Vec<String>>,
    design: Option<String>,
    monthly_budget: Option<f64>,
    alert_threshold_pct: Option<f64>,
    project: Option<String>,
    from_iso: Option<String>,
    to_iso: Option<String>,
    model: Option<String>,
    min_cost: Option<f64>,
    max_cost: Option<f64>,
    limit: Option<i64>,
    query: Option<String>,
    start_hour: i64,
    end_hour: i64,
    notification_limit: Option<usize>,
    notification_id: Option<i64>,
    notification_token: Option<String>,
    recommendation_id: Option<String>,
}

impl BridgeTarget {
    #[cfg(test)]
    fn for_test(command: &str) -> Self {
        Self {
            command: command.to_string(),
            start_hour: 0,
            end_hour: 23,
            ..Self::default()
        }
    }

    fn from_request(request: InvokeRequest) -> Result<Self, String> {
        let command = request.command.trim().to_string();
        if command.is_empty() {
            return Err("command is required".to_string());
        }
        let args = match request.args {
            Value::Null => Map::new(),
            Value::Object(args) => args,
            _ => return Err("args must be a JSON object".to_string()),
        };

        let optional_string = |name: &str| -> Result<Option<String>, String> {
            match args.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(value)) => Ok((!value.is_empty()).then(|| value.clone())),
                Some(_) => Err(format!("{name} must be a string")),
            }
        };
        let optional_i64 = |name: &str| -> Result<Option<i64>, String> {
            match args.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Number(value)) => value
                    .as_i64()
                    .map(Some)
                    .ok_or_else(|| format!("{name} must be an integer")),
                Some(_) => Err(format!("{name} must be an integer")),
            }
        };
        let optional_f64 = |name: &str| -> Result<Option<f64>, String> {
            match args.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Number(value)) => value
                    .as_f64()
                    .filter(|number| number.is_finite())
                    .map(Some)
                    .ok_or_else(|| format!("{name} must be a finite number")),
                Some(_) => Err(format!("{name} must be a number")),
            }
        };
        let optional_bool = |name: &str| -> Result<Option<bool>, String> {
            match args.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Bool(value)) => Ok(Some(*value)),
                Some(_) => Err(format!("{name} must be a boolean")),
            }
        };
        let optional_string_array = |name: &str| -> Result<Option<Vec<String>>, String> {
            match args.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Array(values)) => values
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_string)
                            .ok_or_else(|| format!("{name} must contain only strings"))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Some),
                Some(_) => Err(format!("{name} must be an array")),
            }
        };
        let days = optional_i64("days")?.filter(|value| *value > 0);
        if args
            .get("days")
            .is_some_and(|value| !value.is_null() && days.is_none())
        {
            return Err("days must be a positive integer".to_string());
        }
        if days.is_some_and(|value| value > MAX_DAYS) {
            return Err(format!("days must be at most {MAX_DAYS}"));
        }
        let start_hour = optional_i64("startHour")?.unwrap_or(0);
        let end_hour = optional_i64("endHour")?.unwrap_or(23);
        if !(0..=23).contains(&start_hour) || !(0..=23).contains(&end_hour) {
            return Err("hour range must be between 0 and 23".to_string());
        }
        let limit = optional_i64("limit")?;
        if limit.is_some_and(|value| value <= 0) {
            return Err("limit must be positive".to_string());
        }
        if limit.is_some_and(|value| value > MAX_LIMIT) {
            return Err(format!("limit must be at most {MAX_LIMIT}"));
        }
        let notification_limit = limit.map(|value| value as usize);
        Ok(Self {
            account_id: optional_string("accountId")?,
            account_request: args
                .get("request")
                .map(|request| {
                    serde_json::from_value(request.clone())
                        .map_err(|_| "Invalid account connection request".to_string())
                })
                .transpose()?,
            command,
            days,
            session_id: optional_string("sessionId")?,
            session_ids: optional_string_array("sessionIds")?,
            provider: optional_string("provider")?,
            plan: optional_string("plan")?,
            enabled: optional_bool("enabled")?,
            show_project: optional_bool("showProject")?,
            show_branch: optional_bool("showBranch")?,
            show_model: optional_bool("showModel")?,
            show_activity: optional_bool("showActivity")?,
            show_tokens: optional_bool("showTokens")?,
            show_cost: optional_bool("showCost")?,
            show_limits: optional_bool("showLimits")?,
            show_credits: optional_bool("showCredits")?,
            show_context: optional_bool("showContext")?,
            show_systems: optional_bool("showSystems")?,
            field_order: optional_string_array("order")?,
            design: optional_string("design")?,
            monthly_budget: optional_f64("monthlyBudget")?,
            alert_threshold_pct: optional_f64("alertThresholdPct")?,
            project: optional_string("project")?,
            from_iso: optional_string("fromIso")?,
            to_iso: optional_string("toIso")?,
            model: optional_string("model")?,
            min_cost: optional_f64("minCost")?,
            max_cost: optional_f64("maxCost")?,
            query: optional_string("query")?,
            start_hour,
            end_hour,
            limit,
            notification_limit,
            notification_id: optional_i64("id")?,
            notification_token: optional_string("token")?,
            recommendation_id: optional_string("recId")?,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum DispatchError {
    UnknownCommand,
    InvalidArgs(String),
    Unavailable(String),
    Serialization(String),
}

fn required<T>(value: Option<T>, name: &str) -> Result<T, DispatchError> {
    value.ok_or_else(|| DispatchError::InvalidArgs(format!("{name} is required")))
}

fn serialize<T: Serialize>(value: T) -> Result<String, DispatchError> {
    serde_json::to_string(&value).map_err(|error| DispatchError::Serialization(error.to_string()))
}

fn dispatch_error_response(error: &DispatchError) -> (u16, &'static str) {
    match error {
        DispatchError::UnknownCommand => (404, "unknown or unavailable command"),
        DispatchError::InvalidArgs(_) => (400, "invalid invoke arguments"),
        DispatchError::Unavailable(_) => (503, "backend unavailable"),
        DispatchError::Serialization(_) => (500, "backend response unavailable"),
    }
}

fn dispatch(target: &BridgeTarget) -> Result<String, DispatchError> {
    let days = target.days.unwrap_or(30);
    let session_id = target.session_id.clone();
    let project = target.project.clone();
    let command = target.command.as_str();
    match command {
        "list_accounts" => crate::accounts::list_accounts()
            .map_err(DispatchError::Unavailable)
            .and_then(serialize),
        "discover_accounts" => crate::accounts::discover_accounts()
            .map_err(DispatchError::Unavailable)
            .and_then(serialize),
        "connect_account" => {
            let request = required(target.account_request.as_ref(), "request")?;
            crate::accounts::connect_account(crate::accounts::ConnectAccountRequest {
                account_id: request.account_id.clone(),
                provider: request.provider.clone(),
                method: request.method.clone(),
                label: request.label.clone(),
                path: request.path.clone(),
                api_key: request.api_key.clone(),
            })
            .map_err(DispatchError::Unavailable)
            .and_then(serialize)
        }
        "remove_account" | "refresh_account" | "cancel_account_connection" => {
            let id = required(target.account_id.clone(), "accountId")?;
            let result = match command {
                "remove_account" => crate::accounts::remove_account(id),
                "refresh_account" => crate::accounts::refresh_account(id),
                _ => crate::accounts::cancel_account_connection(id),
            };
            result
                .map_err(DispatchError::Unavailable)
                .and_then(serialize)
        }
        // Live state and settings are read from the same shared command layer
        // the native Tauri window uses. No fixture branch exists here.
        "get_health" => serialize(crate::commands::get_health()),
        "get_app_snapshot" => crate::commands::get_app_snapshot()
            .map_err(DispatchError::Unavailable)
            .and_then(serialize),
        "get_access_snapshot" => serialize(crate::commands::get_access_snapshot()),
        "get_metrics" => serialize(crate::commands::get_metrics()),
        "get_live_sessions" => serialize(crate::commands::get_live_sessions()),
        "get_discord_preview" => crate::commands::get_discord_preview()
            .map_err(DispatchError::Unavailable)
            .and_then(serialize),
        "get_discord_settings" => crate::commands::get_discord_settings()
            .map_err(DispatchError::Unavailable)
            .and_then(serialize),
        "get_rate_limits" => serialize(crate::commands::get_rate_limits()),
        "get_discord_user" => serialize(crate::commands::get_discord_user()),
        "get_plan_info" => serialize(crate::commands::get_plan_info()),
        // Read-only: the repair itself stays native-only.
        "preview_claude_history_repair" => {
            crate::commands::preview_claude_history_repair_blocking()
                .map_err(DispatchError::Unavailable)
                .and_then(serialize)
        }
        "get_active_provider" => serialize(crate::commands::get_active_provider()),
        "get_app_settings" => serialize(crate::commands::get_app_settings()),
        "get_provider_copy" => serialize(crate::commands::get_provider_copy()),
        "get_notifications" => crate::commands::get_notifications(target.notification_limit)
            .map_err(DispatchError::Unavailable)
            .and_then(serialize),
        "get_unread_notification_count" => {
            serialize(crate::commands::get_unread_notification_count())
        }
        "get_dashboard_bundle" => serialize(crate::commands::dashboard_bundle_blocking(
            target.provider.clone(),
        )),
        "get_session_history" => serialize(crate::commands::get_session_history(
            Some(days),
            project,
            Some(target.limit.unwrap_or(200)),
            target.provider.clone(),
        )),
        "get_session_history_filtered" => serialize(crate::commands::get_session_history_filtered(
            target.from_iso.clone(),
            target.to_iso.clone(),
            project,
            target.model.clone(),
            target.min_cost,
            target.max_cost,
            Some(target.limit.unwrap_or(500)),
            target.provider.clone(),
        )),
        "get_sessions_by_hour_range" => serialize(crate::commands::get_sessions_by_hour_range(
            target.start_hour,
            target.end_hour,
            Some(days),
            target.provider.clone(),
        )),
        "search_sessions" => target
            .query
            .clone()
            .filter(|query| !query.is_empty())
            .ok_or_else(|| DispatchError::InvalidArgs("query is required".to_string()))
            .map(|query| {
                crate::commands::search_sessions(
                    query,
                    Some(target.limit.unwrap_or(100)),
                    target.provider.clone(),
                )
            })
            .and_then(serialize),
        "get_daily_stats" => serialize(crate::commands::get_daily_stats(
            Some(days),
            target.provider.clone(),
        )),
        "get_analytics_summary" => serialize(crate::commands::get_analytics_summary(
            target.provider.clone(),
        )),
        "get_context_breakdown" => serialize(crate::commands::get_context_breakdown(
            session_id,
            target.provider.clone(),
        )),
        "get_context_breakdowns" => serialize(crate::commands::get_context_breakdowns(
            target.session_ids.clone(),
            target.provider.clone(),
        )),
        "get_sessions_context_usage" => serialize(crate::commands::get_sessions_context_usage(
            Some(days),
            target.provider.clone(),
        )),
        "get_project_stats" => serialize(crate::commands::get_project_stats(
            Some(days),
            target.provider.clone(),
        )),
        "get_hourly_activity" => serialize(crate::commands::get_hourly_activity(
            Some(days),
            target.provider.clone(),
        )),
        "get_top_sessions" => serialize(crate::commands::get_top_sessions(
            Some(target.limit.unwrap_or(10)),
            Some(days),
            target.provider.clone(),
        )),
        "get_cost_forecast" => {
            serialize(crate::commands::get_cost_forecast(target.provider.clone()))
        }
        "get_cost_totals" => serialize(crate::commands::cost_totals_blocking(
            days,
            project,
            target.provider.clone(),
        )),
        "get_costs_bundle" => serialize(crate::commands::costs_bundle_blocking(
            project,
            target.provider.clone(),
        )),
        "get_budget_status" => {
            serialize(crate::commands::get_budget_status(target.provider.clone()))
        }
        "get_model_distribution" => serialize(crate::commands::get_model_distribution(Some(days))),
        "get_model_distribution_v2" => serialize(crate::commands::get_model_distribution_v2(
            Some(days),
            target.provider.clone(),
        )),
        "get_db_size" => serialize(crate::commands::get_db_size()),
        "get_reports_bundle" => serialize(crate::commands::reports_bundle_blocking(
            days,
            project,
            target.provider.clone(),
        )),
        "generate_html_report" => serialize(crate::report::generate_html_report_scoped(
            target.provider.as_deref(),
            target.days,
            project.as_deref(),
        )),
        "generate_markdown_report" => serialize(crate::report::generate_markdown_report_scoped(
            target.provider.as_deref(),
            target.days,
            project.as_deref(),
        )),
        "export_all_data" => serialize(crate::commands::export_all_data(target.provider.clone())),
        "get_cache_health" => serialize(
            &crate::commands::reports_bundle_blocking(
                days,
                project.clone(),
                target.provider.clone(),
            )
            .cache_health,
        ),
        "get_inflection_points" => serialize(
            &crate::commands::reports_bundle_blocking(
                days,
                project.clone(),
                target.provider.clone(),
            )
            .inflection_points,
        ),
        "get_model_routing" => serialize(
            &crate::commands::reports_bundle_blocking(
                days,
                project.clone(),
                target.provider.clone(),
            )
            .model_routing,
        ),
        "get_recommendations" => serialize(
            &crate::commands::reports_bundle_blocking(days, project, target.provider.clone())
                .recommendations,
        ),
        "get_trace_overview" => serialize(
            &crate::commands::reports_bundle_blocking(days, project, target.provider.clone())
                .trace_overview,
        ),
        "get_tool_frequency" => serialize(
            &crate::commands::reports_bundle_blocking(days, project, target.provider.clone())
                .tool_frequency,
        ),
        "get_prompt_complexity" => serialize(
            &crate::commands::reports_bundle_blocking(days, project, target.provider.clone())
                .prompt_complexity,
        ),
        "get_session_health" => serialize(
            &crate::commands::reports_bundle_blocking(days, project, target.provider.clone())
                .session_health,
        ),
        "copy_fix_prompt" => target
            .recommendation_id
            .clone()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| DispatchError::InvalidArgs("recId is required".to_string()))
            .map(|id| {
                crate::commands::reports_bundle_blocking(days, project, target.provider.clone())
                    .recommendations
                    .into_iter()
                    .find(|recommendation| recommendation.id == id)
                    .map(|recommendation| recommendation.fix_prompt)
                    .unwrap_or_default()
            })
            .and_then(serialize),
        // Authenticated browser review can exercise the same safe controls as
        // the native UI. Each arm calls the canonical command directly so the
        // response reflects the real persisted state or a real backend error.
        "refresh_usage" => {
            crate::commands::refresh_usage();
            serialize(())
        }
        "mark_notification_read" => required(target.notification_id, "id")
            .and_then(|id| {
                crate::commands::mark_notification_read(id).map_err(DispatchError::Unavailable)
            })
            .and_then(serialize),
        "mark_all_notifications_read" => crate::commands::mark_all_notifications_read()
            .map_err(DispatchError::Unavailable)
            .and_then(serialize),
        "mark_notification_unread" => required(target.notification_id, "id")
            .and_then(|id| {
                crate::commands::mark_notification_unread(id).map_err(DispatchError::Unavailable)
            })
            .and_then(serialize),
        "mark_all_notifications_unread" => crate::commands::mark_all_notifications_unread()
            .map_err(DispatchError::Unavailable)
            .and_then(serialize),
        "dismiss_all_notifications" => crate::commands::dismiss_all_notifications()
            .map_err(DispatchError::Unavailable)
            .and_then(serialize),
        "restore_notifications" => required(target.notification_token.clone(), "token")
            .and_then(|token| {
                crate::commands::restore_notifications(token).map_err(DispatchError::Unavailable)
            })
            .and_then(serialize),
        "dismiss_notification" => required(target.notification_id, "id")
            .and_then(|id| {
                crate::commands::dismiss_notification(id).map_err(DispatchError::Unavailable)
            })
            .and_then(serialize),
        "set_discord_enabled" => required(target.enabled, "enabled")
            .and_then(|enabled| {
                crate::commands::set_discord_enabled(enabled).map_err(DispatchError::Unavailable)
            })
            .and_then(serialize),
        "set_discord_display_prefs" => {
            let prefs = crate::commands::DiscordDisplayPrefs {
                show_project: required(target.show_project, "showProject")?,
                show_branch: required(target.show_branch, "showBranch")?,
                show_model: required(target.show_model, "showModel")?,
                show_activity: required(target.show_activity, "showActivity")?,
                show_tokens: required(target.show_tokens, "showTokens")?,
                show_cost: required(target.show_cost, "showCost")?,
                show_limits: required(target.show_limits, "showLimits")?,
                show_credits: required(target.show_credits, "showCredits")?,
                show_context: required(target.show_context, "showContext")?,
                show_systems: required(target.show_systems, "showSystems")?,
            };
            crate::commands::set_discord_display_prefs(
                prefs.show_project,
                prefs.show_branch,
                prefs.show_model,
                prefs.show_activity,
                prefs.show_tokens,
                prefs.show_cost,
                prefs.show_limits,
                prefs.show_credits,
                prefs.show_context,
                prefs.show_systems,
            )
            .map_err(DispatchError::Unavailable)
            .and_then(serialize)
        }
        "set_discord_field_order" => required(target.field_order.clone(), "order")
            .and_then(|order| {
                crate::commands::set_discord_field_order(order).map_err(DispatchError::Unavailable)
            })
            .and_then(serialize),
        "set_codex_desktop_design" => required(target.design.clone(), "design")
            .and_then(|design| {
                crate::commands::set_codex_desktop_design(design)
                    .map_err(DispatchError::Unavailable)
            })
            .and_then(serialize),
        "set_active_provider" => {
            crate::commands::set_active_provider(required(target.provider.clone(), "provider")?)
                .map_err(DispatchError::Unavailable)
                .and_then(serialize)
        }
        "set_close_to_tray" => {
            let enabled = required(target.enabled, "enabled")?;
            crate::commands::set_close_to_tray(enabled)
                .map_err(DispatchError::Unavailable)
                .and_then(serialize)
        }
        "set_plan_override" => crate::commands::set_plan_override(
            required(target.plan.clone(), "plan")?,
            target.provider.clone(),
        )
        .map_err(DispatchError::Unavailable)
        .and_then(serialize),
        "set_budget" => {
            let monthly_budget = required(target.monthly_budget, "monthlyBudget")?;
            crate::commands::set_budget(monthly_budget, target.alert_threshold_pct)
                .map_err(DispatchError::InvalidArgs)
                .and_then(serialize)
        }
        // Shell/browser actions deliberately stay Tauri-only; the bridge has
        // no authority to open arbitrary URLs or invoke signed installers.
        _ => Err(DispatchError::UnknownCommand),
    }
}

fn spawn_with_token(token: String) {
    std::thread::spawn(move || {
        if let Err(error) = run_with_token(&token) {
            tracing::warn!(error = %error, "dev bridge stopped");
        }
    });
}

/// Starts the bridge from the Tauri debug binary. Missing credentials keep the
/// app usable while ensuring an accidental unauthenticated listener cannot
/// appear on developer machines.
pub fn spawn() {
    let Some(token) = configured_token() else {
        tracing::warn!("{DEV_BRIDGE_TOKEN_ENV} is unset; dev bridge is disabled");
        return;
    };
    spawn_with_token(token);
}

/// Runs the bridge as a standalone debug process. The process exits before
/// binding if the shared token is missing, so Vite cannot silently fall back to
/// mocks or an unauthenticated endpoint.
pub fn run() -> io::Result<()> {
    let token = configured_token().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{DEV_BRIDGE_TOKEN_ENV} must be set to run the dev bridge"),
        )
    })?;
    // Validate credentials before starting the poller so a mistyped token
    // cannot trigger provider reads in a process that will immediately exit.
    cc_discord_presence::storage::initialize().map_err(io::Error::other)?;
    crate::commands::start_background_poller_without_app();
    run_with_token(&token)
}

fn run_with_token(token: &str) -> io::Result<()> {
    let address = bind_address()?;
    let listener = TcpListener::bind(address)?;
    tracing::info!(%address, "dev bridge listening");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = handle(stream, token) {
                    tracing::debug!(error = %error, "dev bridge request failed");
                }
            }
            Err(error) => tracing::debug!(error = %error, "dev bridge accept failed"),
        }
    }
    Ok(())
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_bounded_line<R: Read>(reader: &mut BufReader<R>, max_bytes: usize) -> io::Result<String> {
    let mut bytes = Vec::with_capacity(max_bytes.min(1024));
    let read = reader
        .take((max_bytes + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if read > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request line too large",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "request is not UTF-8"))
}

fn read_chunked_body<R: Read>(reader: &mut BufReader<R>) -> io::Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut chunk_count = 0usize;
    let mut framing_bytes = 0usize;
    loop {
        let size_line = read_bounded_line(reader, MAX_HEADER_LINE_BYTES)?;
        chunk_count = chunk_count.saturating_add(1);
        framing_bytes = framing_bytes.saturating_add(size_line.len());
        if chunk_count > MAX_CHUNK_COUNT || framing_bytes > MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request chunk framing too large",
            ));
        }
        let size_token = size_line
            .trim()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        if size_token.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "missing chunk size",
            ));
        }
        let chunk_size = usize::from_str_radix(size_token, 16)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid chunk size"))?;
        if chunk_size == 0 {
            let mut trailer_bytes = 0usize;
            let mut trailer_count = 0usize;
            loop {
                let trailer = read_bounded_line(reader, MAX_HEADER_LINE_BYTES)?;
                if trailer.is_empty() || trailer.trim().is_empty() {
                    return Ok(body);
                }
                trailer_count = trailer_count.saturating_add(1);
                trailer_bytes = trailer_bytes.saturating_add(trailer.len());
                if trailer_count > MAX_HEADER_COUNT || trailer_bytes > MAX_HEADER_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "request trailers too large",
                    ));
                }
            }
        }
        if body.len().saturating_add(chunk_size) > MAX_BODY_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request body too large",
            ));
        }
        let offset = body.len();
        body.resize(offset + chunk_size, 0);
        reader.read_exact(&mut body[offset..])?;
        let mut ending = [0u8; 2];
        reader.read_exact(&mut ending)?;
        if ending != *b"\r\n" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid chunk terminator",
            ));
        }
        framing_bytes = framing_bytes.saturating_add(ending.len());
        if framing_bytes > MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request chunk framing too large",
            ));
        }
    }
}

fn read_request<R: Read>(reader: &mut BufReader<R>) -> io::Result<HttpRequest> {
    let request_line = read_bounded_line(reader, MAX_HEADER_LINE_BYTES)?;
    let request_line_bytes = request_line.len();
    if request_line_bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request line too large or missing",
        ));
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut headers = HashMap::new();
    let mut header_bytes = request_line_bytes;
    let mut header_count = 0;
    loop {
        let line = read_bounded_line(reader, MAX_HEADER_LINE_BYTES)?;
        let line_bytes = line.len();
        if line_bytes == 0 || line.trim().is_empty() {
            break;
        }
        header_count += 1;
        header_bytes = header_bytes.saturating_add(line_bytes);
        if header_count > MAX_HEADER_COUNT || header_bytes > MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request headers too large",
            ));
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let length = headers
        .get("content-length")
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid content length"))?
        .unwrap_or(0);
    let transfer_encoding = headers
        .get("transfer-encoding")
        .map(|value| value.trim().to_ascii_lowercase());
    if transfer_encoding.is_some() && headers.contains_key("content-length") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "conflicting request body framing",
        ));
    }
    let body = match transfer_encoding.as_deref() {
        Some("chunked") => read_chunked_body(reader)?,
        Some(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported transfer encoding",
            ));
        }
        None => {
            if length > MAX_BODY_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "request body too large",
                ));
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body)?;
            body
        }
    };
    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn handle(mut stream: TcpStream, expected_token: &str) -> io::Result<()> {
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    stream.set_write_timeout(Some(REQUEST_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let request = match read_request(&mut reader) {
        Ok(request) => request,
        Err(error) if error.to_string() == "request body too large" => {
            return respond(&mut stream, 413, "request body too large", "", None);
        }
        Err(error) => {
            let message = format!("malformed request: {error}");
            return respond(&mut stream, 400, &message, "", None);
        }
    };
    let origin = request.headers.get("origin").map(String::as_str);
    let Some(allow_origin) = allowed_origin(origin) else {
        return respond(&mut stream, 403, "origin not allowed", "", None);
    };
    if request.method == "OPTIONS" {
        return respond(&mut stream, 204, "", "", Some(allow_origin));
    }
    if request.method != "POST" {
        return respond(
            &mut stream,
            405,
            "method not allowed",
            "",
            Some(allow_origin),
        );
    }
    if request.path != "/invoke" {
        return respond(&mut stream, 404, "not found", "", Some(allow_origin));
    }
    let auth = request.headers.get("authorization").map(String::as_str);
    if authorize(auth, expected_token).is_err() {
        return respond(&mut stream, 401, "unauthorized", "", Some(allow_origin));
    }
    if !request
        .headers
        .get("content-type")
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("application/json"))
    {
        return respond(
            &mut stream,
            415,
            "content type must be application/json",
            "",
            Some(allow_origin),
        );
    }
    let invoke: InvokeRequest = match serde_json::from_slice(&request.body) {
        Ok(invoke) => invoke,
        Err(_) => {
            return respond(
                &mut stream,
                400,
                "invalid JSON request",
                "",
                Some(allow_origin),
            );
        }
    };
    let target = match BridgeTarget::from_request(invoke) {
        Ok(target) => target,
        Err(_) => {
            return respond(
                &mut stream,
                400,
                "invalid invoke arguments",
                "",
                Some(allow_origin),
            );
        }
    };
    match dispatch(&target) {
        Ok(body) => respond(&mut stream, 200, "", &body, Some(allow_origin)),
        Err(error) => {
            let (status, message) = dispatch_error_response(&error);
            respond(&mut stream, status, message, "", Some(allow_origin))
        }
    }
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    message: &str,
    body: &str,
    allow_origin: Option<&str>,
) -> io::Result<()> {
    let mut response_status = status;
    let mut payload = if body.is_empty() {
        if status == 204 {
            String::new()
        } else {
            serde_json::json!({ "error": message }).to_string()
        }
    } else {
        body.to_string()
    };
    if payload.len() > MAX_RESPONSE_BYTES {
        response_status = 500;
        payload = serde_json::json!({
            "error": format!(
                "response body exceeds the development bridge limit of {MAX_RESPONSE_BYTES} bytes"
            )
        })
        .to_string();
    }
    let reason = match response_status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let cors = allow_origin
        .filter(|origin| !origin.is_empty())
        .map(|origin| format!("Access-Control-Allow-Origin: {origin}\r\n"))
        .unwrap_or_default();
    write!(
        stream,
        "HTTP/1.1 {response_status} {reason}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         {cors}\
         Vary: Origin\r\n\
         Access-Control-Allow-Methods: POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Authorization, Content-Type\r\n\
         Connection: close\r\n\
         \r\n\
         {payload}",
        len = payload.len()
    )?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_binds_only_to_ipv4_loopback() {
        assert_eq!(
            bind_address().expect("valid address").ip(),
            std::net::IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert_eq!(
            bind_address().expect("valid address").port(),
            DEV_BRIDGE_PORT
        );
    }

    #[test]
    fn custom_port_only_accepts_its_exact_loopback_origins() {
        assert_eq!(
            allowed_origin_for_port(Some("http://localhost:1430"), 1430),
            Some("http://localhost:1430")
        );
        for origin in [
            "http://localhost:1420",
            "http://localhost:1430.evil.example",
            "http://127.0.0.1:1431",
            "https://localhost:1430",
        ] {
            assert_eq!(allowed_origin_for_port(Some(origin), 1430), None);
        }
    }

    #[test]
    fn bridge_requires_the_configured_bearer_token() {
        assert_eq!(authorize(None, "secret"), Err(AuthError::Missing));
        assert_eq!(
            authorize(Some("Bearer wrong"), "secret"),
            Err(AuthError::Invalid)
        );
        assert_eq!(
            authorize(Some("Basic secret"), "secret"),
            Err(AuthError::Invalid)
        );
        assert_eq!(authorize(Some("Bearer secret"), "secret"), Ok(()));
    }

    #[test]
    fn chunked_browser_request_reassembles_the_authenticated_invoke_body() {
        let payload = r#"{"command":"get_dashboard_bundle","args":{"provider":"claude"}}"#;
        let split = 19;
        let raw = format!(
            "POST /invoke HTTP/1.1\r\nHost: 127.0.0.1:1421\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\nAuthorization: Bearer secret\r\n\r\n{:x}\r\n{}\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            split,
            &payload[..split],
            payload.len() - split,
            &payload[split..],
        );
        let mut reader = BufReader::new(std::io::Cursor::new(raw.into_bytes()));

        let request = read_request(&mut reader).expect("chunked request");
        let invoke: InvokeRequest = serde_json::from_slice(&request.body).expect("invoke JSON");
        let target = BridgeTarget::from_request(invoke).expect("bridge target");

        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/invoke");
        assert_eq!(target.command, "get_dashboard_bundle");
        assert_eq!(target.provider.as_deref(), Some("claude"));
    }

    #[test]
    fn request_parser_rejects_ambiguous_body_framing() {
        let raw = b"POST /invoke HTTP/1.1\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        let mut reader = BufReader::new(std::io::Cursor::new(raw));

        let error = read_request(&mut reader).expect_err("ambiguous framing must fail closed");

        assert_eq!(error.to_string(), "conflicting request body framing");
    }

    #[test]
    fn chunked_request_rejects_excessive_framing_before_body_limit() {
        let mut raw = String::from("POST /invoke HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n");
        for _ in 0..=MAX_CHUNK_COUNT {
            raw.push_str("1\r\nx\r\n");
        }
        raw.push_str("0\r\n\r\n");
        let mut reader = BufReader::new(std::io::Cursor::new(raw.into_bytes()));

        let error = read_request(&mut reader).expect_err("chunk framing must be bounded");

        assert_eq!(error.to_string(), "request chunk framing too large");
    }

    #[test]
    fn request_args_preserve_real_command_inputs() {
        let target = BridgeTarget::from_request(InvokeRequest {
            command: "get_cost_totals".to_string(),
            args: serde_json::json!({
                "days": 90,
                "project": "pulse",
                "provider": "claude"
            }),
        })
        .expect("valid request");
        assert_eq!(target.days, Some(90));
        assert_eq!(target.project.as_deref(), Some("pulse"));
        assert_eq!(target.provider.as_deref(), Some("claude"));
    }

    #[test]
    fn bridge_dispatches_real_health_and_reports_unavailable_backend() {
        let health = dispatch(&BridgeTarget::for_test("get_health")).expect("health dispatch");
        assert!(health.contains("\"version\""));

        assert!(matches!(
            dispatch(&BridgeTarget::for_test("not_a_command")),
            Err(DispatchError::UnknownCommand)
        ));
        assert_eq!(
            dispatch_error_response(&DispatchError::Unavailable("database".to_string())),
            (503, "backend unavailable")
        );
        // A command that can fail in the real backend must remain an explicit
        // unavailable/error path; it may never be replaced by a fixture.
        let settings = dispatch(&BridgeTarget::for_test("get_discord_settings"));
        assert!(settings.is_ok() || matches!(settings, Err(DispatchError::Unavailable(_))));
    }

    #[test]
    fn only_the_vite_dev_origin_is_accepted() {
        assert_eq!(allowed_origin(None), Some(""));
        assert_eq!(
            allowed_origin(Some("http://localhost:1420")),
            Some("http://localhost:1420")
        );
        assert_eq!(
            allowed_origin(Some("http://127.0.0.1:1420")),
            Some("http://127.0.0.1:1420")
        );
        assert_eq!(allowed_origin(Some("https://evil.example")), None);
    }

    #[test]
    fn safe_controls_require_their_real_arguments() {
        for command in [
            "set_discord_enabled",
            "set_active_provider",
            "set_plan_override",
            "set_budget",
            "set_discord_field_order",
            "set_codex_desktop_design",
            "mark_notification_read",
            "mark_notification_unread",
            "restore_notifications",
            "dismiss_notification",
        ] {
            let target = BridgeTarget::for_test(command);
            assert!(matches!(
                dispatch(&target),
                Err(DispatchError::InvalidArgs(_))
            ));
        }
        assert!(matches!(
            dispatch(&BridgeTarget::for_test("open_app_release_page")),
            Err(DispatchError::UnknownCommand)
        ));
        assert!(matches!(
            dispatch(&BridgeTarget::for_test("not_a_command")),
            Err(DispatchError::UnknownCommand)
        ));
    }

    #[test]
    fn browser_bridge_rejects_out_of_range_budget_mutations() {
        for args in [
            serde_json::json!({"monthlyBudget": -1, "alertThresholdPct": 80}),
            serde_json::json!({"monthlyBudget": 10, "alertThresholdPct": 101}),
        ] {
            let target = BridgeTarget::from_request(InvokeRequest {
                command: "set_budget".to_string(),
                args,
            })
            .expect("syntactically valid budget request");
            assert!(matches!(
                dispatch(&target),
                Err(DispatchError::InvalidArgs(_))
            ));
        }
    }

    #[test]
    fn browser_bridge_never_exposes_history_deletion() {
        assert!(matches!(
            dispatch(&BridgeTarget::for_test("clear_history")),
            Err(DispatchError::UnknownCommand)
        ));
    }

    #[test]
    fn browser_bridge_bounds_expensive_query_arguments() {
        for (name, value) in [("days", MAX_DAYS + 1), ("limit", MAX_LIMIT + 1)] {
            let request = InvokeRequest {
                command: "get_metrics".to_string(),
                args: serde_json::json!({name: value}),
            };
            let error = BridgeTarget::from_request(request).expect_err("bound must reject");
            assert!(error.contains(name));
        }
    }
}
