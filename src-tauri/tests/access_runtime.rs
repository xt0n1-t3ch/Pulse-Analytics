//! Integration tests for access routes: local history never changes provider authentication proof.

use cc_discord_presence::usage::{
    UsageData, UsageLimit, UsageLimitModel, UsageLimitScope, UsageWindow,
};
use chrono::{TimeZone, Utc};
use codex_presence_core::{
    IndividualSpendLimit, QuotaScope, QuotaWindow, RateLimitResetCredit,
    RateLimitResetCreditsSummary, RateLimitScope, UsageSignal, UsageSnapshot, UsageSource,
};
use pulse::access::{
    AccessAvailability, AccessFreshness, AccessProof, AccessRouteSnapshot, AccessSnapshot,
    AccessSourceKind, AuthMethod, access_route_from_usage,
    access_route_from_usage_with_account_details, access_route_from_usage_with_reset_credits,
    api_source, displayable_window_percent, subscription_source, visible_routes, window_label,
};
use std::collections::BTreeMap;

fn observed_usage() -> UsageSnapshot {
    UsageSnapshot {
        source: UsageSource::new(
            "codex-subscription:default",
            [UsageSignal::CodexSubscriptionUsage],
        ),
        scopes: vec![QuotaScope {
            id: Some("codex".to_string()),
            name: Some("Weekly".to_string()),
            kind: RateLimitScope::GlobalAccount,
            windows: vec![QuotaWindow {
                window_minutes: 10_080,
                used_percent: 63.0,
                remaining_percent: 37.0,
                resets_at: Utc.timestamp_opt(1_800_000_000, 0).single(),
            }],
        }],
        credits: None,
        observed_at: Utc.timestamp_opt(1_799_999_000, 0).single(),
        provenance_source: "app_server".to_string(),
    }
}

fn route() -> AccessRouteSnapshot {
    access_route_from_usage(
        subscription_source("codex", Some("Pro 20x".to_string())),
        observed_usage(),
        Utc.timestamp_opt(1_799_999_004, 0).single().unwrap(),
        chrono::Duration::seconds(30),
        Utc.timestamp_opt(1_799_999_010, 0).single().unwrap(),
    )
}

#[test]
fn unavailable_subscription_route_can_expose_local_history_without_provider_proof() {
    let route = AccessRouteSnapshot::unavailable(
        subscription_source("claude", Some("max_20x".to_string())),
        "token expired",
    )
    .with_local_history(300);

    assert_eq!(route.source.proof, AccessProof::None);
    assert_eq!(route.source.plan, None);
    assert!(route.local_history.available);
    assert_eq!(route.local_history.sessions, 300);
    assert_eq!(route.availability, AccessAvailability::Unavailable);
}

#[test]
fn access_snapshot_attaches_provider_history_without_changing_authentication() {
    let snapshot = AccessSnapshot::new(vec![AccessRouteSnapshot::unavailable(
        subscription_source("claude", None),
        "token expired",
    )])
    .with_local_history(&BTreeMap::from([("claude".to_string(), 300)]));

    let route = &snapshot.routes[0];
    assert_eq!(route.source.proof, AccessProof::None);
    assert_eq!(route.local_history.sessions, 300);
    assert!(route.local_history.available);
}

#[test]
fn sparse_weekly_usage_stays_dynamic_and_available() {
    let route = route();
    assert_eq!(route.source.kind, AccessSourceKind::CodexSubscription);
    assert_eq!(route.availability, AccessAvailability::Available);
    assert_eq!(route.freshness, AccessFreshness::Fresh);
    assert_eq!(route.windows.len(), 1);
    assert_eq!(route.windows[0].key, "weekly");
    assert_eq!(window_label(&route.windows[0]), "Weekly");
    assert_eq!(
        displayable_window_percent(&route, &route.windows[0]),
        Some(63.0)
    );
}

#[test]
fn reported_global_five_hour_and_weekly_windows_both_remain_available() {
    let now = Utc.timestamp_opt(1_799_999_010, 0).single().unwrap();
    let route = access_route_from_usage(
        subscription_source("codex", None),
        UsageSnapshot {
            scopes: vec![QuotaScope {
                id: Some("codex".to_string()),
                name: None,
                kind: RateLimitScope::GlobalAccount,
                windows: vec![
                    QuotaWindow {
                        window_minutes: 300,
                        used_percent: 25.0,
                        remaining_percent: 75.0,
                        resets_at: None,
                    },
                    QuotaWindow {
                        window_minutes: 10_080,
                        used_percent: 5.0,
                        remaining_percent: 95.0,
                        resets_at: None,
                    },
                ],
            }],
            observed_at: Some(now),
            ..observed_usage()
        },
        now,
        chrono::Duration::seconds(30),
        now,
    );

    assert_eq!(route.availability, AccessAvailability::Available);
    assert_eq!(route.windows.len(), 2);
    assert_eq!(route.windows[0].key, "five_hour");
    assert_eq!(route.windows[0].quota.remaining_percent, 75.0);
    assert_eq!(route.windows[1].key, "weekly");
    assert_eq!(route.windows[1].quota.remaining_percent, 95.0);
}

#[test]
fn missing_quota_windows_are_unavailable_instead_of_zero_percent_usage() {
    let now = Utc.timestamp_opt(1_799_999_010, 0).single().unwrap();
    let route = access_route_from_usage(
        subscription_source("codex", None),
        UsageSnapshot {
            scopes: Vec::new(),
            credits: None,
            observed_at: Some(now),
            ..observed_usage()
        },
        now,
        chrono::Duration::seconds(30),
        now,
    );

    assert_eq!(route.availability, AccessAvailability::Unavailable);
    assert!(route.windows.is_empty());
}

#[test]
fn reported_zero_usage_is_available_and_distinct_from_a_missing_window() {
    let now = Utc.timestamp_opt(1_799_999_010, 0).single().unwrap();
    let route = access_route_from_usage(
        subscription_source("codex", None),
        UsageSnapshot {
            scopes: vec![QuotaScope {
                id: Some("codex".to_string()),
                name: None,
                kind: RateLimitScope::GlobalAccount,
                windows: vec![QuotaWindow {
                    window_minutes: 300,
                    used_percent: 0.0,
                    remaining_percent: 100.0,
                    resets_at: None,
                }],
            }],
            observed_at: Some(now),
            ..observed_usage()
        },
        now,
        chrono::Duration::seconds(30),
        now,
    );

    assert_eq!(route.availability, AccessAvailability::Available);
    assert_eq!(route.windows.len(), 1);
    assert_eq!(route.windows[0].key, "five_hour");
    assert_eq!(
        displayable_window_percent(&route, &route.windows[0]),
        Some(0.0)
    );
    assert_eq!(route.windows[0].quota.remaining_percent, 100.0);
}

#[test]
fn codex_route_keeps_reset_credits_separate_from_spend_credits() {
    let summary = RateLimitResetCreditsSummary {
        available_count: 1,
        credits: Some(vec![RateLimitResetCredit {
            id: "reset-1".to_string(),
            reset_type: "codexRateLimits".to_string(),
            status: "available".to_string(),
            granted_at: 1_800_000_000,
            expires_at: Some(1_800_100_000),
            title: Some("Full reset".to_string()),
            description: None,
        }]),
    };
    let route = access_route_from_usage_with_reset_credits(
        subscription_source("codex", None),
        UsageSnapshot {
            scopes: Vec::new(),
            credits: None,
            ..observed_usage()
        },
        Some(summary.clone()),
        Utc.timestamp_opt(1_799_999_004, 0).single().unwrap(),
        chrono::Duration::seconds(30),
        Utc.timestamp_opt(1_799_999_010, 0).single().unwrap(),
    );

    assert_eq!(route.availability, AccessAvailability::Available);
    assert_eq!(route.credits, None);
    assert_eq!(route.rate_limit_reset_credits, Some(summary));
}

#[test]
fn codex_route_preserves_individual_spend_limits_without_fake_quota_windows() {
    let limit = IndividualSpendLimit {
        limit_id: "workspace".to_string(),
        limit: Some("100.00".to_string()),
        used: Some("25.00".to_string()),
        remaining_percent: 75.0,
        resets_at: Utc.timestamp_opt(1_800_100_000, 0).single(),
    };
    let route = access_route_from_usage_with_account_details(
        subscription_source("codex", None),
        UsageSnapshot {
            scopes: Vec::new(),
            credits: None,
            ..observed_usage()
        },
        None,
        vec![limit.clone()],
        Utc.timestamp_opt(1_799_999_004, 0).single().unwrap(),
        chrono::Duration::seconds(30),
        Utc.timestamp_opt(1_799_999_010, 0).single().unwrap(),
    );

    assert_eq!(route.availability, AccessAvailability::Available);
    assert!(route.windows.is_empty());
    assert_eq!(route.individual_limits, vec![limit]);
}

#[test]
fn generic_scope_name_uses_duration_labels_for_each_window() {
    let route = access_route_from_usage(
        subscription_source("codex", None),
        UsageSnapshot {
            source: UsageSource::new(
                "codex-subscription:default",
                [UsageSignal::CodexSubscriptionUsage],
            ),
            scopes: vec![QuotaScope {
                id: Some("codex".to_string()),
                name: Some("Account quota".to_string()),
                kind: RateLimitScope::GlobalAccount,
                windows: vec![
                    QuotaWindow {
                        window_minutes: 300,
                        used_percent: 12.0,
                        remaining_percent: 88.0,
                        resets_at: None,
                    },
                    QuotaWindow {
                        window_minutes: 10_080,
                        used_percent: 31.0,
                        remaining_percent: 69.0,
                        resets_at: None,
                    },
                ],
            }],
            credits: None,
            observed_at: Utc.timestamp_opt(1_799_999_000, 0).single(),
            provenance_source: "app_server".to_string(),
        },
        Utc.timestamp_opt(1_799_999_004, 0).single().unwrap(),
        chrono::Duration::seconds(30),
        Utc.timestamp_opt(1_799_999_010, 0).single().unwrap(),
    );

    assert_eq!(route.windows.len(), 2);
    assert!(route.windows.iter().all(|window| window.label.is_none()));
    assert_eq!(window_label(&route.windows[0]), "5h");
    assert_eq!(window_label(&route.windows[1]), "7d");
}

#[test]
fn claude_structured_limits_keep_session_weekly_and_fable_windows() {
    let now = Utc.timestamp_opt(1_799_999_010, 0).single().unwrap();
    let usage = UsageData {
        five_hour: UsageWindow {
            utilization: 99.0,
            resets_at: None,
        },
        seven_day: UsageWindow {
            utilization: 99.0,
            resets_at: None,
        },
        sonnet_free: None,
        limits: vec![
            UsageLimit {
                kind: Some("session".to_string()),
                percent: Some(12.0),
                resets_at: None,
                scope: None,
            },
            UsageLimit {
                kind: Some("weekly_all".to_string()),
                percent: Some(34.0),
                resets_at: None,
                scope: None,
            },
            UsageLimit {
                kind: Some("weekly_scoped".to_string()),
                percent: Some(68.0),
                resets_at: None,
                scope: Some(UsageLimitScope {
                    model: Some(UsageLimitModel {
                        display_name: Some("Fable".to_string()),
                        id: Some("fable".to_string()),
                    }),
                }),
            },
        ],
        extra_usage: None,
    };
    let mut source = subscription_source("claude", Some("max_20x".to_string()));
    source.proof = AccessProof::QuotaResponse;
    let route = pulse::access::claude_route_from_usage(
        source,
        &usage,
        now,
        now,
        chrono::Duration::minutes(5),
        now,
        "OAuth usage API",
    );

    assert_eq!(route.windows.len(), 3);
    assert_eq!(window_label(&route.windows[0]), "5h");
    assert_eq!(route.windows[0].quota.used_percent, 12.0);
    assert_eq!(route.windows[0].quota.remaining_percent, 88.0);
    assert_eq!(window_label(&route.windows[1]), "7d");
    assert_eq!(route.windows[1].quota.used_percent, 34.0);
    assert_eq!(route.windows[1].quota.remaining_percent, 66.0);
    assert_eq!(window_label(&route.windows[2]), "Fable");
    assert_eq!(route.windows[2].quota.used_percent, 68.0);
    assert_eq!(route.source.plan.as_deref(), Some("max_20x"));
}

#[test]
fn claude_partial_structured_limits_merge_missing_legacy_kinds() {
    let now = Utc.timestamp_opt(1_799_999_010, 0).single().unwrap();
    let usage = UsageData {
        five_hour: UsageWindow {
            utilization: 11.0,
            resets_at: None,
        },
        seven_day: UsageWindow {
            utilization: 22.0,
            resets_at: None,
        },
        sonnet_free: Some(UsageWindow {
            utilization: 33.0,
            resets_at: None,
        }),
        limits: vec![UsageLimit {
            kind: Some("weekly_scoped".to_string()),
            percent: Some(68.0),
            resets_at: None,
            scope: Some(UsageLimitScope {
                model: Some(UsageLimitModel {
                    display_name: Some("Fable".to_string()),
                    id: Some("fable".to_string()),
                }),
            }),
        }],
        extra_usage: None,
    };
    let mut source = subscription_source("claude", Some("max_20x".to_string()));
    source.proof = AccessProof::QuotaResponse;
    let route = pulse::access::claude_route_from_usage(
        source,
        &usage,
        now,
        now,
        chrono::Duration::minutes(5),
        now,
        "OAuth usage API",
    );

    assert_eq!(route.windows.len(), 4);
    let by_key = route
        .windows
        .iter()
        .map(|window| (window.key.as_str(), window.quota.used_percent))
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        by_key.len(),
        route.windows.len(),
        "window keys must be unique"
    );
    assert_eq!(by_key.get("five_hour"), Some(&11.0));
    assert_eq!(by_key.get("weekly"), Some(&22.0));
    assert_eq!(by_key.get("sonnet_free"), Some(&33.0));
    assert_eq!(by_key.get("fable"), Some(&68.0));
}

#[test]
fn codex_access_route_keeps_global_and_model_scoped_weekly_windows() {
    let now = Utc.timestamp_opt(1_799_999_010, 0).single().unwrap();
    let mut source = subscription_source("codex", None);
    source.proof = AccessProof::QuotaResponse;
    let route = access_route_from_usage(
        source,
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
                        used_percent: 83.0,
                        remaining_percent: 17.0,
                        resets_at: None,
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
                        resets_at: None,
                    }],
                },
            ],
            credits: None,
            observed_at: Some(now),
            provenance_source: "Codex account API".to_string(),
        },
        now,
        chrono::Duration::seconds(30),
        now,
    );

    assert_eq!(route.windows.len(), 2);
    assert_eq!(window_label(&route.windows[0]), "7d");
    assert_eq!(window_label(&route.windows[1]), "GPT-5.3-Codex-Spark");
    assert_eq!(route.windows[0].quota.used_percent, 83.0);
    assert_eq!(route.windows[1].quota.used_percent, 0.0);
    assert_eq!(route.source.plan, None);
}

#[test]
fn codex_model_scope_windows_have_unique_stable_keys_and_labels() {
    let now = Utc.timestamp_opt(1_799_999_010, 0).single().unwrap();
    let mut source = subscription_source("codex", None);
    source.proof = AccessProof::QuotaResponse;
    let route = access_route_from_usage(
        source,
        UsageSnapshot {
            source: UsageSource::new(
                "codex-subscription:default",
                [UsageSignal::CodexSubscriptionUsage],
            ),
            scopes: vec![QuotaScope {
                id: Some("codex_bengalfox".to_string()),
                name: Some("GPT-5.3-Codex-Spark".to_string()),
                kind: RateLimitScope::ModelScoped,
                windows: vec![
                    QuotaWindow {
                        window_minutes: 300,
                        used_percent: 0.0,
                        remaining_percent: 100.0,
                        resets_at: None,
                    },
                    QuotaWindow {
                        window_minutes: 10_080,
                        used_percent: 13.0,
                        remaining_percent: 87.0,
                        resets_at: None,
                    },
                ],
            }],
            credits: None,
            observed_at: Some(now),
            provenance_source: "Codex account API".to_string(),
        },
        now,
        chrono::Duration::seconds(30),
        now,
    );

    let identities = route
        .windows
        .iter()
        .map(|window| (window.key.as_str(), window_label(window)))
        .collect::<Vec<_>>();
    assert_eq!(
        identities,
        vec![
            (
                "gpt_5.3_codex_spark_five_hour",
                "GPT-5.3-Codex-Spark · 5h".to_string(),
            ),
            (
                "gpt_5.3_codex_spark_weekly",
                "GPT-5.3-Codex-Spark · 7d".to_string(),
            ),
        ]
    );
}

#[test]
fn unproved_subscription_plan_is_not_exposed_as_authenticated_metadata() {
    let route = access_route_from_usage(
        subscription_source("codex", Some("Pro".to_string())),
        observed_usage(),
        Utc.timestamp_opt(1_799_999_004, 0).single().unwrap(),
        chrono::Duration::seconds(30),
        Utc.timestamp_opt(1_799_999_010, 0).single().unwrap(),
    );

    assert_eq!(route.source.proof, AccessProof::None);
    assert_eq!(route.source.plan, None);
}

#[test]
fn stale_usage_is_diagnostic_only_and_never_numeric() {
    let route = access_route_from_usage(
        subscription_source("codex", Some("Pro".to_string())),
        observed_usage(),
        Utc.timestamp_opt(1_799_999_004, 0).single().unwrap(),
        chrono::Duration::seconds(5),
        Utc.timestamp_opt(1_800_000_010, 0).single().unwrap(),
    );
    assert_eq!(route.freshness, AccessFreshness::Stale);
    assert_eq!(displayable_window_percent(&route, &route.windows[0]), None);
}

#[test]
fn api_route_requires_authenticated_proof_and_hybrid_keeps_routes_separate() {
    let subscription = route();
    let unproved_api = AccessRouteSnapshot::unavailable(
        api_source("openai", AuthMethod::ApiKey, AccessProof::None),
        "API key not authenticated",
    );
    let proven_api = AccessRouteSnapshot::unavailable(
        api_source(
            "openai",
            AuthMethod::ApiKey,
            AccessProof::AuthenticatedProbe,
        ),
        "no quota response",
    );

    let visible = visible_routes(vec![subscription.clone(), unproved_api, proven_api.clone()]);
    assert_eq!(visible, vec![subscription, proven_api]);
}

#[test]
fn no_data_route_has_no_zero_window_or_fake_proof() {
    let route =
        AccessRouteSnapshot::unavailable(subscription_source("claude", None), "no usage response");
    assert_eq!(route.availability, AccessAvailability::Unavailable);
    assert_eq!(route.freshness, AccessFreshness::Unknown);
    assert_eq!(route.windows.len(), 0);
    assert_eq!(route.source.proof, AccessProof::None);
    assert_eq!(route.source.kind, AccessSourceKind::ClaudeSubscription);
}

#[test]
fn jsonl_usage_does_not_upgrade_subscription_to_quota_proof() {
    let route = access_route_from_usage(
        subscription_source("codex", None),
        UsageSnapshot {
            source: UsageSource::new("codex-session-jsonl", [UsageSignal::CodexSessionJsonl]),
            scopes: vec![QuotaScope {
                id: Some("codex".to_string()),
                name: Some("Weekly".to_string()),
                kind: RateLimitScope::GlobalAccount,
                windows: vec![QuotaWindow {
                    window_minutes: 10_080,
                    used_percent: 25.0,
                    remaining_percent: 75.0,
                    resets_at: None,
                }],
            }],
            credits: None,
            observed_at: Utc.timestamp_opt(1_800_000_000, 0).single(),
            provenance_source: "Codex JSONL rate_limits fallback".to_string(),
        },
        Utc.timestamp_opt(1_800_000_001, 0).single().unwrap(),
        chrono::Duration::minutes(15),
        Utc.timestamp_opt(1_800_000_002, 0).single().unwrap(),
    );

    assert_eq!(route.source.proof, AccessProof::None);
    assert_eq!(route.availability, AccessAvailability::Unavailable);
    assert!(route.windows.is_empty());
}

#[test]
fn mismatched_usage_lane_fails_closed_without_quota_windows() {
    let route = access_route_from_usage(
        subscription_source("codex", None),
        UsageSnapshot {
            source: UsageSource::new("anthropic-api:configured", [UsageSignal::AnthropicApiUsage]),
            scopes: vec![QuotaScope {
                id: Some("anthropic".to_string()),
                name: Some("Weekly".to_string()),
                kind: RateLimitScope::GlobalAccount,
                windows: vec![QuotaWindow {
                    window_minutes: 10_080,
                    used_percent: 10.0,
                    remaining_percent: 90.0,
                    resets_at: None,
                }],
            }],
            credits: None,
            observed_at: Utc.timestamp_opt(1_800_000_000, 0).single(),
            provenance_source: "provider API".to_string(),
        },
        Utc.timestamp_opt(1_800_000_001, 0).single().unwrap(),
        chrono::Duration::minutes(15),
        Utc.timestamp_opt(1_800_000_002, 0).single().unwrap(),
    );

    assert_eq!(route.availability, AccessAvailability::Unavailable);
    assert!(route.windows.is_empty());
    assert!(
        route
            .error
            .as_deref()
            .is_some_and(|error| error.contains("not selectable"))
    );
}

#[test]
fn unknown_usage_source_fails_closed_without_quota_windows() {
    let route = access_route_from_usage(
        subscription_source("codex", None),
        UsageSnapshot {
            source: UsageSource::default(),
            scopes: vec![QuotaScope {
                id: Some("codex".to_string()),
                name: Some("Weekly".to_string()),
                kind: RateLimitScope::GlobalAccount,
                windows: vec![QuotaWindow {
                    window_minutes: 10_080,
                    used_percent: 15.0,
                    remaining_percent: 85.0,
                    resets_at: None,
                }],
            }],
            credits: None,
            observed_at: Utc.timestamp_opt(1_800_000_000, 0).single(),
            provenance_source: "unknown".to_string(),
        },
        Utc.timestamp_opt(1_800_000_001, 0).single().unwrap(),
        chrono::Duration::minutes(15),
        Utc.timestamp_opt(1_800_000_002, 0).single().unwrap(),
    );

    assert_eq!(route.availability, AccessAvailability::Unavailable);
    assert!(route.windows.is_empty());
}
