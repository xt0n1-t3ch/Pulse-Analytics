//! Codex model catalog parity and the astra, sol and terra contracts.

mod common;

use common::{TempDir, assert_failure_contains, assert_success, write};
use serde_json::json;
use xtask::catalog;

fn entry(id: &str, name: &str, raw: u64, percent: u64, rates: [f64; 4]) -> serde_json::Value {
    json!({
        "id": id,
        "display_name": name,
        "reasoning_efforts": ["low", "medium", "high", "xhigh", "max", "ultra"],
        "supports_fast": true,
        "fast_usage_multiplier": 2.0,
        "long_context_pricing": {"input_and_cache": 2.0, "output": 1.5},
        "context": {
            "raw_tokens": raw, "effective_percent": percent, "api_tokens": 1_050_000,
            "max_input_tokens": 922_000, "max_output_tokens": 128_000,
            "long_context_input_threshold": 272_000
        },
        "api_pricing": {
            "input_per_million": rates[0], "cache_write_per_million": rates[1],
            "cached_input_per_million": rates[2], "output_per_million": rates[3]
        },
        "credit_rates": null
    })
}

fn catalog_json() -> String {
    json!({"models": [
        entry("gpt-6-astra", "GPT-6 Astra", 1_050_000, 100, [10.0, 12.5, 1.0, 50.0]),
        entry("gpt-6.1-sol", "GPT-6.1 Sol", 500_000, 95, [2.0, 2.5, 0.1, 10.0]),
        entry("gpt-6.1-terra", "GPT-6.1 Terra", 500_000, 95, [1.0, 1.25, 0.05, 5.0]),
    ]})
    .to_string()
}

/// A Pulse checkout and a canonical checkout side by side, like the real folders.
fn checkouts(
    local: &str,
    upstream: Option<&str>,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let parent = TempDir::new();
    let pulse = parent.path().join("pulse");
    let canonical = parent.path().join("Codex-Discord-Rich-Presence");
    write(pulse.join("src/codex/model_catalog.json"), local);
    if let Some(upstream) = upstream {
        write(canonical.join("src/model_catalog.json"), upstream);
    }
    (parent, pulse, canonical)
}

#[test]
fn identical_catalogs_with_the_published_rates_pass() {
    let json = catalog_json();
    let (_parent, pulse, canonical) = checkouts(&json, Some(&json));

    let message = assert_success(catalog::parity(&pulse, Some(&canonical)));

    assert!(message.starts_with("MODEL CATALOG PARITY OK"), "{message}");
    assert!(
        message.contains("byte parity with the canonical catalog"),
        "{message}"
    );
    // The sibling folder is found without being named.
    assert_success(catalog::parity(&pulse, None));
}

#[test]
fn a_one_byte_difference_fails_parity() {
    let json = catalog_json();
    let (_parent, pulse, canonical) = checkouts(&json, Some(&format!("{json}\n")));

    assert_failure_contains(
        catalog::parity(&pulse, Some(&canonical)),
        "Model catalog parity failed: Pulse differs from the canonical runtime catalog.",
    );
}

#[test]
fn a_missing_default_checkout_skips_only_the_byte_comparison() {
    let json = catalog_json();
    let (_parent, pulse, _canonical) = checkouts(&json, None);

    let message = assert_success(catalog::parity(&pulse, None));

    assert!(message.contains("byte parity skipped"), "{message}");

    let drifted = json.replace("\"input_per_million\":1.0", "\"input_per_million\":2.0");
    write(pulse.join("src/codex/model_catalog.json"), &drifted);
    assert_failure_contains(
        catalog::parity(&pulse, None),
        "GPT-6.1 Terra catalog contract failed",
    );
}

#[test]
fn a_named_checkout_that_does_not_exist_is_an_error() {
    let json = catalog_json();
    let (parent, pulse, _canonical) = checkouts(&json, None);

    assert_failure_contains(
        catalog::parity(&pulse, Some(&parent.path().join("elsewhere"))),
        "Canonical model catalog not found",
    );
}
