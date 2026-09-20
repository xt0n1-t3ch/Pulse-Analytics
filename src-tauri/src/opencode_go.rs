use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use codex_presence_core::QuotaWindow;
use serde_json::Value;

use crate::access::{
    AccessAvailability, AccessFreshness, AccessProof, AccessProvenance, AccessRouteSnapshot,
    AccessSource, AccessSourceKind, AccessWindow, AuthMethod,
};

const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";

pub fn start() -> Arc<Mutex<Option<AccessRouteSnapshot>>> {
    let latest = Arc::new(Mutex::new(None));
    let output = Arc::clone(&latest);
    std::thread::spawn(move || {
        loop {
            let route = fetch_usage();
            if let Ok(mut latest) = output.lock() {
                *latest = Some(route);
            }
            std::thread::sleep(Duration::from_secs(60));
        }
    });
    latest
}

pub fn presence_text(routes: &[AccessRouteSnapshot], now: DateTime<Utc>) -> Option<String> {
    let route = routes.iter().find(|route| {
        route.source.kind == AccessSourceKind::OpenCodeGo
            && route.source.provider == "opencode"
            && route.source.proof == AccessProof::QuotaResponse
            && route.availability == AccessAvailability::Available
            && route.freshness == AccessFreshness::Fresh
            && route.expires_at.is_some_and(|expires| expires > now)
    })?;
    let mut parts = Vec::new();
    for (key, label) in [("rolling", "5h"), ("weekly", "7d"), ("monthly", "month")] {
        let window = route
            .windows
            .iter()
            .find(|window| window.key == format!("opencode-go:{key}"))?;
        let percent = window.quota.used_percent;
        if !percent.is_finite()
            || !(0.0..=100.0).contains(&percent)
            || window.quota.resets_at.is_none_or(|reset| reset <= now)
        {
            return None;
        }
        parts.push(format!("{label} {percent:.0}%"));
    }
    Some(format!("Go {} used", parts.join(" · ")))
}

fn source() -> AccessSource {
    AccessSource {
        id: "opencode-go:local".into(),
        kind: AccessSourceKind::OpenCodeGo,
        provider: "opencode".into(),
        auth_method: AuthMethod::ApiKey,
        proof: AccessProof::None,
        plan: None,
    }
}

fn fetch_usage() -> AccessRouteSnapshot {
    let path = cc_discord_presence::opencode::data_dir().join("auth.json");
    let key = std::fs::read(path)
        .ok()
        .filter(|bytes| bytes.len() <= 1024 * 1024)
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| {
            value
                .pointer("/opencode-go/key")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let Some(key) = key.filter(|key| !key.is_empty() && !key.contains(['\r', '\n'])) else {
        return AccessRouteSnapshot::unavailable(source(), "OpenCode Go API key is not configured");
    };
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(4))
        .redirects(0)
        .build();
    let response = agent
        .get(USAGE_URL)
        .set("User-Agent", concat!("Pulse/", env!("CARGO_PKG_VERSION")))
        .set("Accept", "application/json")
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {key}"))
        .call();
    match response {
        Ok(response) => {
            let mut bytes = Vec::new();
            if response
                .into_reader()
                .take(65_537)
                .read_to_end(&mut bytes)
                .is_err()
                || bytes.len() > 65_536
            {
                return AccessRouteSnapshot::unavailable(
                    source(),
                    "OpenCode Go returned an invalid usage response",
                );
            }
            match serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|value| parse_usage(&value, Utc::now()))
            {
                Some(route) => route,
                None => AccessRouteSnapshot::unavailable(
                    source(),
                    "OpenCode Go usage fields are incomplete",
                ),
            }
        }
        Err(ureq::Error::Status(status, _)) => AccessRouteSnapshot::unavailable(
            source(),
            format!("OpenCode Go usage returned HTTP {status}"),
        ),
        Err(_) => {
            AccessRouteSnapshot::unavailable(source(), "OpenCode Go usage could not be reached")
        }
    }
}

fn parse_usage(value: &Value, observed: DateTime<Utc>) -> Option<AccessRouteSnapshot> {
    let mut windows = Vec::new();
    for (key, label, minutes) in [
        ("rolling", "5-hour usage", 300),
        ("weekly", "Weekly usage", 10_080),
        ("monthly", "Monthly usage", 0),
    ] {
        let window = value.get("usage")?.get(key)?;
        let percent = window.get("percent")?.as_f64()?;
        if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
            return None;
        }
        let resets_at = DateTime::parse_from_rfc3339(window.get("resetsAt")?.as_str()?)
            .ok()?
            .with_timezone(&Utc);
        windows.push(AccessWindow {
            key: format!("opencode-go:{key}"),
            label: Some(label.into()),
            quota: QuotaWindow {
                window_minutes: minutes,
                used_percent: percent,
                remaining_percent: 100.0 - percent,
                resets_at: Some(resets_at),
            },
        });
    }
    let mut route = AccessRouteSnapshot::unavailable(source(), "");
    route.source.proof = AccessProof::QuotaResponse;
    route.availability = AccessAvailability::Available;
    route.freshness = AccessFreshness::Fresh;
    route.provenance = AccessProvenance::ProviderApi;
    route.observed_at = Some(observed);
    route.fetched_at = Some(observed);
    route.expires_at = Some(observed + chrono::Duration::seconds(120));
    route.windows = windows;
    route.error = None;
    route.unavailable_reason = None;
    Some(route)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presence_requires_fresh_go_proof_and_all_unexpired_windows() {
        let now = DateTime::parse_from_rfc3339("2026-09-05T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let value = serde_json::json!({"usage":{"rolling":{"percent":0,"resetsAt":"2026-09-05T06:00:00Z"},"weekly":{"percent":73,"resetsAt":"2026-09-07T00:00:00Z"},"monthly":{"percent":79,"resetsAt":"2026-09-07T07:55:16Z"}}});
        let route = parse_usage(&value, now).unwrap();
        assert_eq!(
            presence_text(std::slice::from_ref(&route), now).as_deref(),
            Some("Go 5h 0% · 7d 73% · month 79% used")
        );
        assert!(
            presence_text(
                std::slice::from_ref(&route),
                now + chrono::Duration::seconds(120)
            )
            .is_none()
        );
        for mutation in 0..5 {
            let mut invalid = route.clone();
            match mutation {
                0 => invalid.source.kind = AccessSourceKind::CodexSubscription,
                1 => invalid.source.proof = AccessProof::None,
                2 => invalid.freshness = AccessFreshness::Stale,
                3 => {
                    invalid.windows.pop();
                }
                _ => invalid.windows[0].quota.resets_at = Some(now),
            }
            assert!(presence_text(&[invalid], now).is_none());
        }
    }

    #[test]
    fn all_three_go_windows_require_provider_values_and_keep_calendar_reset() {
        let mut value = serde_json::json!({"usage":{"rolling":{"percent":0,"resetsAt":"2026-09-05T06:00:00Z"},"weekly":{"percent":73,"resetsAt":"2026-09-07T00:00:00Z"},"monthly":{"percent":79,"resetsAt":"2026-09-07T07:55:16Z"}}});
        let route = parse_usage(&value, Utc::now()).unwrap();
        assert_eq!(route.windows.len(), 3);
        assert_eq!(route.windows[1].quota.used_percent, 73.0);
        assert_eq!(route.windows[2].quota.window_minutes, 0);
        assert_eq!(route.windows[2].label.as_deref(), Some("Monthly usage"));
        value["usage"]["monthly"]["percent"] = Value::Null;
        assert!(parse_usage(&value, Utc::now()).is_none());
    }
}
