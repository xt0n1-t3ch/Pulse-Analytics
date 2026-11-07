//! Integration tests for decoding Codex account usage responses into quota windows.

use chrono::{TimeZone, Utc};

use cc_discord_presence::codex::account_usage::parse_rate_limits_response;
use cc_discord_presence::codex::telemetry::limits::RateLimitScope;

const ACCOUNT_RESPONSE: &str = r#"{
  "id": 2,
  "result": {
    "rateLimits": {
      "limitId": "codex",
      "primary": { "usedPercent": 83, "windowDurationMins": 10080, "resetsAt": 1785369546 },
      "credits": { "hasCredits": true, "unlimited": false, "balance": "2014.0899875000" },
      "planType": "pro_20x"
    },
    "rateLimitsByLimitId": {
      "codex": {
        "limitId": "codex",
        "primary": { "usedPercent": 83, "windowDurationMins": 10080, "resetsAt": 1785369546 },
        "credits": { "hasCredits": true, "unlimited": false, "balance": "2014.0899875000" },
        "planType": "pro_20x"
      },
      "codex_bengalfox": {
        "limitId": "codex_bengalfox",
        "limitName": "GPT-5.3-Codex-Spark",
        "primary": { "usedPercent": 0, "windowDurationMins": 10080, "resetsAt": 1785596138 },
        "credits": null,
        "planType": "pro_20x"
      }
    }
  }
}"#;

#[test]
fn account_response_keeps_global_quota_and_credits_coherent() {
    let observed_at = Utc.timestamp_opt(1_785_000_000, 0).single().unwrap();
    let reading = parse_rate_limits_response(ACCOUNT_RESPONSE, observed_at).unwrap();

    assert_eq!(reading.envelopes.len(), 2);
    let global = reading
        .envelopes
        .iter()
        .find(|item| item.scope == RateLimitScope::GlobalAccount)
        .expect("global account quota");
    let weekly = global.limits.primary().expect("weekly window");
    assert_eq!(weekly.used_percent, 83.0);
    assert_eq!(weekly.remaining_percent, 17.0);
    assert_eq!(weekly.window_minutes, 10_080);
    assert_eq!(
        global
            .credits
            .as_ref()
            .and_then(|item| item.balance.as_deref()),
        Some("2014.0899875000")
    );
    assert_eq!(global.observed_at, Some(observed_at));
    assert_eq!(global.plan_type.as_deref(), Some("pro_20x"));
}

#[test]
fn account_response_keeps_model_scoped_spark_weekly_window_separate() {
    let observed_at = Utc.timestamp_opt(1_785_000_000, 0).single().unwrap();
    let reading = parse_rate_limits_response(ACCOUNT_RESPONSE, observed_at).unwrap();

    let spark = reading
        .envelopes
        .iter()
        .find(|item| item.scope == RateLimitScope::ModelScoped)
        .expect("model-scoped Codex Spark quota");
    assert_eq!(spark.limit_id.as_deref(), Some("codex_bengalfox"));
    assert_eq!(spark.limit_name.as_deref(), Some("GPT-5.3-Codex-Spark"));
    let weekly = spark.limits.primary().expect("Spark weekly window");
    assert_eq!(weekly.window_minutes, 10_080);
    assert_eq!(weekly.used_percent, 0.0);
    assert_eq!(weekly.remaining_percent, 100.0);
    assert_eq!(spark.credits, None);
}

#[test]
fn account_response_does_not_invent_unreported_model_scopes() {
    let observed_at = Utc.timestamp_opt(1_785_000_000, 0).single().unwrap();
    let reading = parse_rate_limits_response(
        r#"{"id":2,"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":13,"windowDurationMins":10080,"resetsAt":null},"credits":null},"rateLimitsByLimitId":{"codex":{"limitId":"codex","primary":{"usedPercent":13,"windowDurationMins":10080,"resetsAt":null},"credits":null}}}}"#,
        observed_at,
    )
    .expect("global-only account snapshot should remain available");

    assert_eq!(reading.envelopes.len(), 1);
    assert_eq!(reading.envelopes[0].scope, RateLimitScope::GlobalAccount);
    assert!(
        reading
            .envelopes
            .iter()
            .all(|envelope| envelope.scope != RateLimitScope::ModelScoped),
        "Spark/model scopes must only exist when rateLimitsByLimitId reports them"
    );
    let weekly = reading.envelopes[0]
        .limits
        .primary()
        .expect("global weekly quota");
    assert_eq!(weekly.used_percent, 13.0);
    assert_eq!(weekly.remaining_percent, 87.0);
}

#[test]
fn sparse_or_null_response_is_not_presented_as_live_usage() {
    let observed_at = Utc.timestamp_opt(1_785_000_000, 0).single().unwrap();
    let err = parse_rate_limits_response(
        r#"{"id":2,"result":{"rateLimits":{"primary":null},"rateLimitsByLimitId":null}}"#,
        observed_at,
    )
    .expect_err("empty quota response must stay unavailable");

    assert!(err.to_string().contains("no quota windows"));
}

#[test]
fn credits_only_account_response_remains_renderable() {
    let observed_at = Utc.timestamp_opt(1_785_000_000, 0).single().unwrap();
    let reading = parse_rate_limits_response(
        r#"{"id":2,"result":{"rateLimits":{"limitId":"codex","primary":null,"secondary":null,"credits":{"hasCredits":true,"unlimited":false,"balance":"42.50"}},"rateLimitsByLimitId":null}}"#,
        observed_at,
    )
    .expect("credits-only account snapshot should remain available");

    assert_eq!(reading.envelopes.len(), 1);
    assert_eq!(
        reading.envelopes[0]
            .credits
            .as_ref()
            .and_then(|credits| credits.balance.as_deref()),
        Some("42.50")
    );
    assert!(reading.envelopes[0].limits.primary().is_none());
}

#[test]
fn quota_window_without_duration_is_rejected() {
    let observed_at = Utc.timestamp_opt(1_785_000_000, 0).single().unwrap();
    let error = parse_rate_limits_response(
        r#"{"id":2,"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":85,"windowDurationMins":null,"resetsAt":null},"credits":null},"rateLimitsByLimitId":null}}"#,
        observed_at,
    )
    .expect_err("durationless quota must not be represented as a 0-minute window");

    assert!(error.to_string().contains("no quota windows or credits"));
}

#[test]
fn map_key_supplies_a_missing_limit_id() {
    let observed_at = Utc.timestamp_opt(1_785_000_000, 0).single().unwrap();
    let reading = parse_rate_limits_response(
        r#"{"id":2,"result":{"rateLimits":{"primary":null},"rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":10,"windowDurationMins":10080,"resetsAt":null}}}}}"#,
        observed_at,
    )
    .expect("map key is the canonical limit ID");

    assert_eq!(reading.envelopes[0].limit_id.as_deref(), Some("codex"));
    assert_eq!(reading.envelopes[0].scope, RateLimitScope::GlobalAccount);
}

#[test]
fn individual_spend_limit_is_preserved_separately_from_quota_windows() {
    let observed_at = Utc.timestamp_opt(1_785_000_000, 0).single().unwrap();
    let reading = parse_rate_limits_response(
        r#"{"id":2,"result":{"rateLimits":{"limitId":"workspace","primary":null,"secondary":null,"credits":null,"individualLimit":{"limit":"100.00","used":"25.00","remainingPercent":75,"resetsAt":1785000100}},"rateLimitsByLimitId":null}}"#,
        observed_at,
    )
    .expect("individual spend control is current account usage");

    assert!(reading.envelopes.is_empty());
    let limit = &reading.individual_limits[0];
    assert_eq!(limit.limit_id, "workspace:individual");
    assert_eq!(limit.limit.as_deref(), Some("100.00"));
    assert_eq!(limit.used.as_deref(), Some("25.00"));
    assert_eq!(limit.remaining_percent, 75.0);
}

#[test]
fn account_response_preserves_canonical_reset_credit_summary() {
    let observed_at = Utc.timestamp_opt(1_785_000_000, 0).single().unwrap();
    let reading = parse_rate_limits_response(
        r#"{
          "id": 2,
          "result": {
            "rateLimits": {
              "limitId": "codex",
              "primary": {"usedPercent": 83, "windowDurationMins": 10080},
              "credits": null
            },
            "rateLimitResetCredits": {
              "availableCount": 2,
              "credits": [{
                "id": "reset-1",
                "resetType": "codexRateLimits",
                "status": "available",
                "grantedAt": 1784488000,
                "expiresAt": 1784677005,
                "title": "Full reset",
                "description": "Weekly and session windows"
              }]
            }
          }
        }"#,
        observed_at,
    )
    .expect("reset-credit response");

    let summary = reading
        .rate_limit_reset_credits
        .expect("canonical reset-credit summary");
    assert_eq!(summary.available_count, 2);
    let credit = &summary.credits.expect("reset-credit details")[0];
    assert_eq!(credit.id, "reset-1");
    assert_eq!(credit.reset_type, "codexRateLimits");
    assert_eq!(credit.status, "available");
    assert_eq!(credit.expires_at, Some(1784677005));
}
