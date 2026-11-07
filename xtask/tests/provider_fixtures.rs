//! Provider and metrics fixture contract: plain data checks over `tests/fixtures`, so they
//! run on every OS without a WebView.

mod common;

use std::collections::BTreeSet;
use std::path::Path;

use common::{read, repository_root};
use serde_json::Value;

const REQUIRED_FIELDS: [&str; 6] = [
    "fixture_version",
    "raw_payload",
    "status",
    "source_contract",
    "captured_at",
    "expected_dto",
];
const REQUIRED_PROVIDER_FIXTURES: [&str; 11] = [
    "codex_subscription",
    "openai_api_negative_auth",
    "openai_api_negative_status",
    "claude_subscription",
    "anthropic_api_negative_auth",
    "anthropic_api_negative_status",
    "hybrid",
    "no_data",
    "stale_cache",
    "window_removal",
    "discord",
];
const SOURCE_KINDS: [&str; 4] = [
    "codex_subscription",
    "open_ai_api",
    "claude_subscription",
    "anthropic_api",
];
const ROUTE_FIELDS: [&str; 11] = [
    "source",
    "availability",
    "freshness",
    "provenance",
    "observed_at",
    "fetched_at",
    "expires_at",
    "windows",
    "credits",
    "extra_usage",
    "error",
];

fn json(path: &Path) -> Value {
    serde_json::from_str(&read(path)).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn require_fields(value: &Value, fields: &[&str], what: &str) {
    for field in fields {
        assert!(value.get(*field).is_some(), "{what} is missing '{field}'");
    }
}

fn entries(manifest: &Value) -> Vec<Value> {
    manifest["fixtures"]
        .as_array()
        .expect("manifest fixtures")
        .clone()
}

fn allowed_kinds(contract: &str) -> &'static [&'static str] {
    match contract {
        "codex_subscription.v1" | "window_delta.v1" => &["codex_subscription"],
        "open_ai_api.v1" => &["open_ai_api"],
        "claude_subscription.v1" | "cache.v1" => &["claude_subscription"],
        "anthropic_api.v1" => &["anthropic_api"],
        "hybrid.v1" => &["claude_subscription", "codex_subscription"],
        "aggregate.v1" => &[],
        other => panic!("unsupported source_contract '{other}'"),
    }
}

fn check_route(name: &str, route: &Value) {
    require_fields(route, &ROUTE_FIELDS, &format!("fixture '{name}' route"));
    let kind = text(&route["source"], "kind");
    assert!(
        SOURCE_KINDS.contains(&kind),
        "fixture '{name}' has an unknown access source kind"
    );
    assert!(
        ["available", "unavailable"].contains(&text(route, "availability")),
        "fixture '{name}' availability"
    );
    assert!(
        ["fresh", "stale", "unknown"].contains(&text(route, "freshness")),
        "fixture '{name}' freshness"
    );
    assert!(
        [
            "app_server",
            "provider_api",
            "memory_cache",
            "session_jsonl",
            "none"
        ]
        .contains(&text(route, "provenance")),
        "fixture '{name}' provenance"
    );
    assert!(
        route["error"].is_null() || route["error"].is_string(),
        "fixture '{name}' route error"
    );
    if !route["credits"].is_null() {
        require_fields(
            &route["credits"],
            &["balance", "has_credits", "unlimited"],
            &format!("fixture '{name}' credits"),
        );
    }
    if !route["extra_usage"].is_null() {
        require_fields(
            &route["extra_usage"],
            &["enabled", "limit", "used", "utilization"],
            &format!("fixture '{name}' extra_usage"),
        );
    }
    for window in route["windows"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        require_fields(
            window,
            &[
                "key",
                "label",
                "window_minutes",
                "used_percent",
                "remaining_percent",
                "resets_at",
            ],
            &format!("fixture '{name}' window"),
        );
    }
}

fn check_provider_fixture(name: &str, payload: &Value, entry: &Value, manifest: &Value) {
    require_fields(payload, &REQUIRED_FIELDS, &format!("fixture '{name}'"));
    assert_eq!(
        payload["fixture_version"].as_i64(),
        manifest["fixture_version"].as_i64(),
        "fixture '{name}' version does not match the manifest"
    );
    assert_eq!(
        text(payload, "source_contract"),
        text(entry, "source_contract"),
        "fixture '{name}' source_contract does not match the manifest"
    );

    let dto = &payload["expected_dto"];
    if text(payload, "source_contract") == "discord_ipc.v1" {
        let events = dto["events"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        assert!(
            events.len() >= 2
                && dto["proof_requirements"]["set_activity"] == true
                && dto["proof_requirements"]["clear_activity"] == true,
            "Discord fixture must require SET_ACTIVITY and clear_activity proof."
        );
        assert!(
            events
                .iter()
                .all(|event| !text(event, "nonce").trim().is_empty()),
            "Discord fixture events must carry non-empty correlation nonces."
        );
        return;
    }

    let routes: Vec<&Value> = match dto.get("routes").and_then(Value::as_array) {
        Some(routes) => routes.iter().collect(),
        None => vec![dto],
    };
    let actual: BTreeSet<&str> = routes
        .iter()
        .map(|route| text(&route["source"], "kind"))
        .collect();
    let allowed: BTreeSet<&str> = allowed_kinds(text(payload, "source_contract"))
        .iter()
        .copied()
        .collect();
    assert_eq!(
        actual, allowed,
        "fixture '{name}' source kinds do not match its contract"
    );
    for route in routes {
        check_route(name, route);
    }
}

#[test]
fn provider_fixtures_keep_the_access_source_contract() {
    let root = repository_root().join("tests/fixtures/providers");
    let manifest = json(&root.join("manifest.json"));
    let fixtures = entries(&manifest);
    assert_eq!(
        fixtures.len(),
        11,
        "provider fixture manifest must list 11 fixtures"
    );

    let names: Vec<&str> = fixtures.iter().map(|entry| text(entry, "name")).collect();
    assert_eq!(
        names.iter().collect::<BTreeSet<_>>().len(),
        fixtures.len(),
        "duplicate fixture names"
    );
    for required in REQUIRED_PROVIDER_FIXTURES {
        assert!(
            names.contains(&required),
            "provider fixture manifest is missing '{required}'"
        );
    }
    for entry in &fixtures {
        let path = root.join(text(entry, "path"));
        assert!(
            path.is_file(),
            "missing provider fixture asset: {}",
            path.display()
        );
        check_provider_fixture(text(entry, "name"), &json(&path), entry, &manifest);
    }
}

#[test]
fn metrics_fixtures_keep_true_zero_distinct_from_unavailable() {
    let root = repository_root().join("tests/fixtures/metrics");
    let manifest = json(&root.join("manifest.json"));
    let fixtures = entries(&manifest);
    let names: BTreeSet<&str> = fixtures.iter().map(|entry| text(entry, "name")).collect();
    assert!(
        fixtures.len() == 2 && names == BTreeSet::from(["true_zero", "unavailable"]),
        "metrics manifest must contain unique true_zero and unavailable entries"
    );

    for entry in &fixtures {
        let name = text(entry, "name");
        let path = root.join(text(entry, "path"));
        assert!(
            path.is_file(),
            "missing metrics fixture asset: {}",
            path.display()
        );
        let metric = json(&path);
        require_fields(
            &metric,
            &REQUIRED_FIELDS,
            &format!("metrics fixture '{name}'"),
        );
        assert_eq!(
            metric["fixture_version"].as_i64(),
            manifest["fixture_version"].as_i64(),
            "metrics fixture '{name}' is not aligned with its manifest"
        );
        assert_eq!(
            text(&metric, "source_contract"),
            text(entry, "source_contract")
        );
        let dto = &metric["expected_dto"];
        require_fields(
            dto,
            &["cost", "cost_available", "cost_basis"],
            &format!("metrics fixture '{name}' expected_dto"),
        );

        let (available, basis) = match name {
            "true_zero" => (true, "exact"),
            _ => (false, "unavailable"),
        };
        assert!(
            dto["cost"].as_f64() == Some(0.0)
                && dto["cost_available"] == available
                && text(dto, "cost_basis") == basis,
            "{name}: a zero transport value must keep cost_available={available} and cost_basis={basis}"
        );
    }
}
