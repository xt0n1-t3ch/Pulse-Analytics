use super::store::AccountRecord;
use super::{AccountBalance, AccountIdentity, AccountSnapshot, AccountWindow};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use std::io::Read;

#[derive(Debug)]
struct ProviderFailure {
    status: u16,
    retry_after: u64,
    entitlement_required: bool,
}
impl std::fmt::Display for ProviderFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.entitlement_required {
            formatter.write_str("OpenCode Go subscription required.")
        } else {
            write!(formatter, "Provider returned HTTP {}", self.status)
        }
    }
}
impl std::error::Error for ProviderFailure {}

pub(super) fn read(record: &AccountRecord) -> AccountSnapshot {
    let mut snapshot = AccountSnapshot::pending(record);
    let result = match record.provider.as_str() {
        "claude" => read_claude(record, &mut snapshot),
        "codex" => read_codex(record, &mut snapshot),
        "opencode" => read_opencode(record, &mut snapshot),
        "commandcode" => read_commandcode(record, &mut snapshot),
        _ => Err(anyhow::anyhow!("Unsupported provider")),
    };
    match result {
        Ok(()) => {
            snapshot.status = "connected".into();
            snapshot.observed_at = Some(Utc::now().to_rfc3339());
            snapshot.expires_at = Some(
                (Utc::now()
                    + Duration::seconds(if record.provider == "claude" {
                        360
                    } else {
                        120
                    }))
                .to_rfc3339(),
            );
        }
        Err(error) => {
            let message = error.to_string();
            if let Some(failure) = error.downcast_ref::<ProviderFailure>() {
                snapshot.retry_at =
                    Some((Utc::now() + Duration::seconds(failure.retry_after as i64)).to_rfc3339());
            }
            snapshot.status = if message.contains("subscription required") {
                "unavailable"
            } else if message.contains("401") || message.contains("expired") {
                "expired"
            } else {
                "error"
            }
            .into();
            snapshot.error = Some(message);
        }
    }
    snapshot
}

pub(super) fn read_json(path: &std::path::Path) -> Result<Value> {
    let file = std::fs::File::open(path).context("Provider profile is unavailable")?;
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        bail!("Provider profile exceeds the supported size");
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Provider profile is not valid JSON"))
}

pub(super) fn http_json(url: &str, key: &str, query: &[(&str, &str)]) -> Result<Value> {
    if key.is_empty() || key.len() > 16384 || key.contains(['\r', '\n']) {
        bail!("Provider credentials are invalid");
    }
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(12))
        .redirects(0)
        .build();
    let mut request = agent
        .get(url)
        .set("Authorization", &format!("Bearer {key}"))
        .set("User-Agent", concat!("Pulse/", env!("CARGO_PKG_VERSION")))
        .set("Accept", "application/json")
        .set("Content-Type", "application/json");
    for (name, value) in query {
        request = request.query(name, value);
    }
    let response = match request.call() {
        Ok(response) => response,
        Err(ureq::Error::Status(status, response)) => {
            let retry_after = response
                .header("retry-after")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(if status == 429 { 300 } else { 120 })
                .clamp(30, 86_400);
            let entitlement_required = if status == 403 && url.starts_with("https://opencode.ai/") {
                let mut body = String::new();
                let _ = response.into_reader().take(16384).read_to_string(&mut body);
                serde_json::from_str::<Value>(&body)
                    .ok()
                    .is_some_and(|value| {
                        value.pointer("/error/type").and_then(Value::as_str)
                            == Some("EntitlementError")
                    })
            } else {
                false
            };
            return Err(ProviderFailure {
                status,
                retry_after,
                entitlement_required,
            }
            .into());
        }
        Err(_) => bail!("Provider could not be reached"),
    };
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        bail!("Provider response exceeds the supported size");
    }
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("Provider returned invalid JSON"))
}

fn string(value: &Value, name: &str) -> Option<String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}
fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
}
fn timestamp(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|value| value.to_rfc3339());
    }
    let number = value.as_i64()?;
    if number <= 0 {
        return None;
    }
    if number > 100_000_000_000 {
        DateTime::from_timestamp_millis(number).map(|value| value.to_rfc3339())
    } else {
        DateTime::from_timestamp(number, 0).map(|value| value.to_rfc3339())
    }
}

fn windows_from_route(route: &crate::access::AccessRouteSnapshot) -> Vec<AccountWindow> {
    route
        .windows
        .iter()
        .map(|window| AccountWindow {
            id: window.key.clone(),
            label: crate::access::window_label(window),
            scope: window.label.clone().unwrap_or_else(|| "Account".into()),
            duration_minutes: Some(window.quota.window_minutes),
            used: None,
            limit: None,
            unit: None,
            used_percent: Some(window.quota.used_percent),
            resets_at: window.quota.resets_at.map(|value| value.to_rfc3339()),
        })
        .collect()
}

fn read_claude(record: &AccountRecord, snapshot: &mut AccountSnapshot) -> Result<()> {
    let mut manager = cc_discord_presence::usage::UsageManager::for_credentials(
        record.source_path.join(".credentials.json"),
        record.origin == "managed",
    );
    let reading = manager.get_usage();
    snapshot.retry_at = manager.retry_at().map(|time| time.to_rfc3339());
    let usage = reading.ok_or_else(|| {
        anyhow::anyhow!(
            manager
                .error_hint_with_countdown()
                .unwrap_or_else(|| "Claude usage is unavailable".into())
        )
    })?;
    snapshot.plan = manager.detected_plan_key();
    let observed = manager.last_usage_observed_at().unwrap_or_else(Utc::now);
    let route = crate::access::claude_route_from_usage(
        crate::access::subscription_source("claude", snapshot.plan.clone()),
        &usage,
        observed,
        Utc::now(),
        Duration::seconds(120),
        Utc::now(),
        "Claude OAuth usage API",
    );
    snapshot.windows = windows_from_route(&route);
    if let Some(extra) = route.extra_usage {
        snapshot.windows.push(AccountWindow {
            id: "extra_usage".into(),
            label: if extra.enabled {
                "Extra usage"
            } else {
                "Extra usage (disabled)"
            }
            .into(),
            scope: "Account".into(),
            duration_minutes: None,
            used: extra.used,
            limit: extra.limit,
            unit: Some("USD".into()),
            used_percent: extra.utilization,
            resets_at: None,
        });
    }
    let config = read_json(&record.source_path.join(".claude.json"))
        .ok()
        .or_else(|| {
            (record.origin == "local"
                && record
                    .source_path
                    .file_name()
                    .is_some_and(|name| name == ".claude"))
            .then(|| read_json(&record.source_path.with_extension("json")).ok())
            .flatten()
        });
    if let Some(identity) = config.as_ref().and_then(|value| value.get("oauthAccount")) {
        snapshot.identity = AccountIdentity {
            subject: string(identity, "accountUuid"),
            name: string(identity, "displayName"),
            email: string(identity, "emailAddress"),
            organization: string(identity, "organizationName")
                .or_else(|| string(identity, "organizationUuid")),
            verified: true,
        };
    }
    Ok(())
}

fn read_codex(record: &AccountRecord, snapshot: &mut AccountSnapshot) -> Result<()> {
    use codex_presence_core::{
        UsageSignal, UsageSource, UsageStream, snapshot_from_stream_with_provenance,
    };
    let mut manager = cc_discord_presence::codex::account_usage::AccountUsageManager::for_home(
        record.source_path.clone(),
        record.origin == "local",
    );
    let reading = manager.get_usage(false)?;
    let tier = cc_discord_presence::codex::telemetry::plan::parse_plan_type(
        reading.account_plan_type.as_deref(),
    );
    snapshot.plan =
        if tier == cc_discord_presence::codex::telemetry::plan::DetectedPlanTier::Unknown {
            reading.account_plan_type.clone()
        } else {
            Some(tier.title().to_owned())
        };
    snapshot.identity.email = reading.account_email.clone();
    snapshot.identity.verified = reading.account_email.is_some();
    let stream = UsageStream::new(
        UsageSource::new(
            format!("account:{}", record.id),
            [UsageSignal::CodexSubscriptionUsage],
        ),
        reading.envelopes.clone(),
    );
    {
        let usage = snapshot_from_stream_with_provenance(&stream, "Codex account API");
        let mut source = crate::access::subscription_source("codex", snapshot.plan.clone());
        source.id = format!("account:{}", record.id);
        let route = crate::access::access_route_from_usage_with_account_details(
            source,
            usage,
            reading.rate_limit_reset_credits.clone(),
            reading.individual_limits.clone(),
            Utc::now(),
            Duration::seconds(120),
            Utc::now(),
        );
        snapshot.windows = windows_from_route(&route);
        if let Some(credits) = route.credits
            && let Some(value) = credits.display_value()
        {
            snapshot.balances.push(AccountBalance {
                id: "credits".into(),
                label: "Credits".into(),
                value: value.into(),
                unit: "credits".into(),
                expires_at: None,
                cycle_ends_at: None,
            });
        }
    }
    snapshot.reset_credits = reading.rate_limit_reset_credits;
    snapshot.individual_limits = reading.individual_limits;
    Ok(())
}

fn api_key(record: &AccountRecord) -> Result<String> {
    let value = read_json(&record.source_path.join("auth.json"))?;
    let key = if record.provider == "opencode" && record.origin == "local" {
        value.pointer("/opencode-go/key")
    } else {
        value.get("apiKey")
    };
    key.and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("Provider key is not configured"))
}

fn native_window(
    id: &str,
    label: &str,
    scope: &str,
    value: &Value,
    minutes: Option<u64>,
    unit: Option<&str>,
) -> Option<AccountWindow> {
    let used = number(&value["used"]);
    let limit = number(&value["cap"]).or_else(|| number(&value["limit"]));
    let percent = number(&value["percent"])
        .or_else(|| number(&value["usedPercent"]))
        .or_else(|| {
            used.zip(limit)
                .filter(|(_, limit)| *limit > 0.0)
                .map(|(used, limit)| used / limit * 100.0)
        });
    if used.is_none() && limit.is_none() && percent.is_none() {
        return None;
    }
    Some(AccountWindow {
        id: id.into(),
        label: label.into(),
        scope: scope.into(),
        duration_minutes: minutes,
        used,
        limit,
        unit: unit.map(str::to_owned),
        used_percent: percent,
        resets_at: timestamp(&value["resetAt"]).or_else(|| timestamp(&value["resetsAt"])),
    })
}

fn optional_allowances(value: &Value, snapshot: &mut AccountSnapshot) {
    let Some(models) = value
        .get("modelAllowances")
        .or_else(|| value.get("allowances"))
        .and_then(Value::as_array)
    else {
        return;
    };
    for model in models {
        let Some(id) = string(model, "modelId").or_else(|| string(model, "model")) else {
            continue;
        };
        if let Some(window) = native_window(
            &format!("model:{id}"),
            &id,
            &id,
            model,
            None,
            model.get("unit").and_then(Value::as_str),
        ) {
            snapshot.windows.push(window);
        }
    }
}

fn read_opencode(record: &AccountRecord, snapshot: &mut AccountSnapshot) -> Result<()> {
    let key = api_key(record)?;
    let value = http_json("https://opencode.ai/zen/go/v1/usage", &key, &[])?;
    snapshot.plan = Some("OpenCode Go".into());
    for (id, label, minutes) in [
        ("rolling", "5-hour limit", Some(300)),
        ("weekly", "Weekly limit", Some(10080)),
        ("monthly", "Monthly limit", None),
    ] {
        if let Some(window) =
            native_window(id, label, "Account", &value["usage"][id], minutes, None)
        {
            snapshot.windows.push(window);
        }
    }
    optional_allowances(&value, snapshot);
    if let Some(balance) = value.get("balance").and_then(number) {
        snapshot.balances.push(AccountBalance {
            id: "zen".into(),
            label: "Zen balance".into(),
            value: balance.to_string(),
            unit: "USD".into(),
            expires_at: None,
            cycle_ends_at: None,
        });
    }
    if let Some(account) = value.get("account") {
        snapshot.identity = AccountIdentity {
            subject: string(account, "id"),
            name: string(account, "name"),
            email: string(account, "email"),
            organization: None,
            verified: true,
        };
    }
    if snapshot.windows.is_empty() {
        bail!("OpenCode Go returned no supported usage windows");
    }
    if snapshot.balances.is_empty() {
        snapshot.notice = Some(
            "The Go usage API does not report a Zen credit balance for this connection.".into(),
        );
    }
    Ok(())
}

fn read_commandcode(record: &AccountRecord, snapshot: &mut AccountSnapshot) -> Result<()> {
    let key = api_key(record)?;
    let who = http_json("https://api.commandcode.ai/alpha/whoami", &key, &[])?;
    if who.get("success").and_then(Value::as_bool) == Some(false) {
        bail!("Command Code could not verify this connection");
    }
    let org = string(&who["org"], "id");
    let query: Vec<(&str, &str)> = org
        .as_deref()
        .map(|org| vec![("orgId", org)])
        .unwrap_or_default();
    snapshot.identity = AccountIdentity {
        subject: string(&who["user"], "id"),
        name: string(&who["user"], "name"),
        email: string(&who["user"], "email"),
        organization: string(&who["org"], "name"),
        verified: true,
    };
    let credits = http_json(
        "https://api.commandcode.ai/alpha/billing/credits",
        &key,
        &query,
    )?;
    let subscription = http_json(
        "https://api.commandcode.ai/alpha/billing/subscriptions",
        &key,
        &query,
    )?;
    snapshot.plan = string(&subscription["data"], "planId");
    for (id, label) in [
        ("monthlyCredits", "Monthly credits"),
        ("purchasedCredits", "Purchased credits"),
        ("freeCredits", "Free credits"),
    ] {
        if let Some(value) = number(&credits["credits"][id]) {
            snapshot.balances.push(AccountBalance {
                id: id.into(),
                label: label.into(),
                value: value.to_string(),
                unit: "credits".into(),
                expires_at: None,
                cycle_ends_at: if id == "monthlyCredits" {
                    timestamp(&subscription["data"]["currentPeriodEnd"])
                } else {
                    None
                },
            });
        }
    }
    for (id, label, minutes) in [
        ("fiveHour", "5-hour limit", Some(300)),
        ("weekly", "Weekly limit", Some(10080)),
        ("monthly", "Monthly limit", None),
    ] {
        if let Some(window) = native_window(
            id,
            label,
            "Account",
            &credits["windowLimits"][id],
            minutes,
            Some("credits"),
        ) {
            snapshot.windows.push(window);
        }
    }
    if !snapshot.windows.iter().any(|window| window.id == "monthly")
        && let Some(window) = command_monthly_window(&credits, &subscription)
    {
        snapshot.windows.push(window);
    }
    optional_allowances(&credits, snapshot);
    if snapshot.plan.is_none() && snapshot.balances.is_empty() {
        snapshot.notice = Some("Authenticated account. Billing data is not available.".into());
    }
    Ok(())
}

fn command_monthly_window(credits: &Value, subscription: &Value) -> Option<AccountWindow> {
    let data = &subscription["data"];
    if data["status"] != "active" {
        return None;
    }
    let plan = data["planId"].as_str()?;
    let allocation = number(&data["monthlyCredits"])
        .or_else(|| number(&data["monthlyAllocation"]))
        .or_else(|| cc_discord_presence::commandcode::monthly_allocation(plan))?;
    let remaining = number(&credits["credits"]["monthlyCredits"])?;
    let used = (allocation - remaining).max(0.0);
    Some(AccountWindow {
        id: "monthly".into(),
        label: "Monthly limit".into(),
        scope: "Account".into(),
        duration_minutes: None,
        used: Some(used),
        limit: Some(allocation),
        unit: Some("credits".into()),
        used_percent: (allocation > 0.0).then(|| used / allocation * 100.0),
        resets_at: timestamp(&data["currentPeriodEnd"]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_zero_and_unstarted_windows_are_preserved_without_epoch_dates() {
        let window = native_window(
            "fiveHour",
            "5-hour",
            "Account",
            &serde_json::json!({"used":0,"cap":14,"resetAt":0}),
            Some(300),
            Some("credits"),
        )
        .unwrap();
        assert_eq!(window.used_percent, Some(0.0));
        assert_eq!(window.resets_at, None);
        assert!(
            native_window(
                "weekly",
                "Weekly",
                "Account",
                &Value::Null,
                Some(10080),
                None
            )
            .is_none()
        );
    }
    #[test]
    fn monetary_units_are_not_inferred_from_percentages() {
        let window = native_window(
            "rolling",
            "5-hour",
            "Account",
            &serde_json::json!({"percent":32,"resetsAt":"2026-09-20T00:00:00Z"}),
            Some(300),
            None,
        )
        .unwrap();
        assert_eq!(window.unit, None);
        assert_eq!(window.limit, None);
        assert_eq!(window.used, None);
    }
    #[test]
    fn monthly_allocation_uses_the_authenticated_plan_and_not_other_credit_buckets() {
        let credits = serde_json::json!({"credits":{"monthlyCredits":64.0,"purchasedCredits":500,"freeCredits":20}});
        let mut subscription = serde_json::json!({"data":{"status":"active","planId":"individual-goat","currentPeriodEnd":"2026-10-16T04:43:10Z"}});
        let window = command_monthly_window(&credits, &subscription).unwrap();
        assert_eq!(window.limit, Some(70.0));
        assert_eq!(window.used, Some(6.0));
        assert!(window.resets_at.is_some());
        subscription["data"]["planId"] = "unknown".into();
        assert!(command_monthly_window(&credits, &subscription).is_none());
        subscription["data"]["monthlyAllocation"] = 90.into();
        assert_eq!(
            command_monthly_window(&credits, &subscription)
                .unwrap()
                .limit,
            Some(90.0)
        );
        subscription["data"]["status"] = "canceled".into();
        assert!(command_monthly_window(&credits, &subscription).is_none());
    }
}
