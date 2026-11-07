//! The Codex model catalog must be a byte-for-byte copy of the canonical one, and three
//! models must match their published rates and limits.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::fsx;

const LOCAL_CATALOG: &str = "src/codex/model_catalog.json";
const UPSTREAM_CATALOG: &str = "src/model_catalog.json";
const UPSTREAM_DIRECTORY: &str = "Codex-Discord-Rich-Presence";

/// Expected facts for one catalog entry. Rates are USD per million tokens
/// (OpenAI API pricing page and Codex `models.json`, both read 2026-11-07).
/// `None` means the contract makes no claim about that field.
struct ModelContract {
    id: &'static str,
    display_name: &'static str,
    raw_tokens: Option<u64>,
    effective_percent: Option<u64>,
    api_tokens: Option<u64>,
    max_input_tokens: u64,
    max_output_tokens: Option<u64>,
    long_context_threshold: u64,
    /// input, cache write, cached input, output
    rates: [f64; 4],
    efforts: Option<&'static str>,
}

const MODELS: [ModelContract; 3] = [
    ModelContract {
        id: "gpt-6-astra",
        display_name: "GPT-6 Astra",
        raw_tokens: None,
        effective_percent: None,
        api_tokens: None,
        max_input_tokens: 922_000,
        max_output_tokens: None,
        long_context_threshold: 272_000,
        rates: [10.0, 12.5, 1.0, 50.0],
        efforts: None,
    },
    ModelContract {
        id: "gpt-6.1-sol",
        display_name: "GPT-6.1 Sol",
        raw_tokens: Some(500_000),
        effective_percent: Some(95),
        api_tokens: None,
        max_input_tokens: 922_000,
        max_output_tokens: None,
        long_context_threshold: 272_000,
        rates: [2.0, 2.5, 0.1, 10.0],
        efforts: Some("low,medium,high,xhigh,max,ultra"),
    },
    ModelContract {
        id: "gpt-6.1-terra",
        display_name: "GPT-6.1 Terra",
        raw_tokens: Some(500_000),
        effective_percent: Some(95),
        api_tokens: Some(1_050_000),
        max_input_tokens: 922_000,
        max_output_tokens: Some(128_000),
        long_context_threshold: 272_000,
        rates: [1.0, 1.25, 0.05, 5.0],
        efforts: Some("low,medium,high,xhigh,max,ultra"),
    },
];

fn number(value: &Value, pointer: &str) -> Option<f64> {
    value.pointer(pointer).and_then(Value::as_f64)
}

/// Why `model` breaks `contract`, or `None` when it matches.
fn violation(model: &Value, contract: &ModelContract) -> Option<String> {
    let wrong = |what: &str| Some(format!("{what} differs"));
    if model.get("display_name").and_then(Value::as_str) != Some(contract.display_name) {
        return wrong("display_name");
    }
    let integer_checks = [
        (
            "context.raw_tokens",
            "/context/raw_tokens",
            contract.raw_tokens,
        ),
        (
            "context.effective_percent",
            "/context/effective_percent",
            contract.effective_percent,
        ),
        (
            "context.api_tokens",
            "/context/api_tokens",
            contract.api_tokens,
        ),
        (
            "context.max_input_tokens",
            "/context/max_input_tokens",
            Some(contract.max_input_tokens),
        ),
        (
            "context.max_output_tokens",
            "/context/max_output_tokens",
            contract.max_output_tokens,
        ),
        (
            "context.long_context_input_threshold",
            "/context/long_context_input_threshold",
            Some(contract.long_context_threshold),
        ),
    ];
    for (label, pointer, expected) in integer_checks {
        if let Some(expected) = expected
            && model.pointer(pointer).and_then(Value::as_u64) != Some(expected)
        {
            return wrong(label);
        }
    }
    let [input, cache_write, cached, output] = contract.rates;
    let rate_checks = [
        (
            "api_pricing.input_per_million",
            "/api_pricing/input_per_million",
            input,
        ),
        (
            "api_pricing.cache_write_per_million",
            "/api_pricing/cache_write_per_million",
            cache_write,
        ),
        (
            "api_pricing.cached_input_per_million",
            "/api_pricing/cached_input_per_million",
            cached,
        ),
        (
            "api_pricing.output_per_million",
            "/api_pricing/output_per_million",
            output,
        ),
        // Long requests cost 2x input and cache, 1.5x output.
        (
            "long_context_pricing.input_and_cache",
            "/long_context_pricing/input_and_cache",
            2.0,
        ),
        (
            "long_context_pricing.output",
            "/long_context_pricing/output",
            1.5,
        ),
        // Fast mode doubles every standard rate.
        ("fast_usage_multiplier", "/fast_usage_multiplier", 2.0),
    ];
    for (label, pointer, expected) in rate_checks {
        if number(model, pointer) != Some(expected) {
            return wrong(label);
        }
    }
    if model.get("supports_fast").and_then(Value::as_bool) != Some(true) {
        return wrong("supports_fast");
    }
    if let Some(expected) = contract.efforts {
        let efforts: Vec<&str> = model
            .get("reasoning_efforts")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if efforts.join(",") != expected {
            return wrong("reasoning_efforts");
        }
    }
    None
}

/// Checks the contract of every pinned model in the catalog text.
pub fn check_contracts(catalog_text: &str) -> Result<()> {
    let catalog: Value = serde_json::from_str(catalog_text).context("parse model catalog")?;
    let models = catalog
        .get("models")
        .and_then(Value::as_array)
        .context("model catalog has no models")?;
    for contract in &MODELS {
        let matching: Vec<&Value> = models
            .iter()
            .filter(|model| model.get("id").and_then(Value::as_str) == Some(contract.id))
            .collect();
        if matching.len() != 1 {
            bail!(
                "{} catalog contract failed: expected one entry, found {}",
                contract.display_name,
                matching.len()
            );
        }
        if let Some(reason) = violation(matching[0], contract) {
            bail!(
                "{} catalog contract failed: {reason}",
                contract.display_name
            );
        }
    }
    Ok(())
}

/// Where the canonical catalog lives when the caller does not say: a sibling checkout.
pub fn default_upstream_root(root: &Path) -> PathBuf {
    root.parent().map_or_else(
        || PathBuf::from(UPSTREAM_DIRECTORY),
        |parent| parent.join(UPSTREAM_DIRECTORY),
    )
}

/// Runs the parity check. `upstream_root` is explicit (`--upstream-root`) or `None` for the
/// default sibling checkout; a missing default checkout skips the byte comparison and says
/// so, while a missing explicit one fails.
pub fn parity(root: &Path, upstream_root: Option<&Path>) -> Result<String> {
    let local_path = root.join(LOCAL_CATALOG);
    let local =
        std::fs::read(&local_path).with_context(|| format!("read {}", local_path.display()))?;

    let explicit = upstream_root.is_some();
    let upstream_dir = upstream_root.map_or_else(|| default_upstream_root(root), Path::to_path_buf);
    let upstream_path = upstream_dir.join(UPSTREAM_CATALOG);
    let parity_note = if upstream_path.is_file() {
        let upstream = std::fs::read(&upstream_path)
            .with_context(|| format!("read {}", upstream_path.display()))?;
        if fsx::sha256_hex(&local) != fsx::sha256_hex(&upstream) {
            bail!("Model catalog parity failed: Pulse differs from the canonical runtime catalog.");
        }
        "byte parity with the canonical catalog".to_owned()
    } else if explicit {
        bail!(
            "Canonical model catalog not found: {}",
            upstream_path.display()
        );
    } else {
        format!(
            "byte parity skipped, no canonical checkout at {}",
            upstream_dir.display()
        )
    };

    check_contracts(&String::from_utf8_lossy(&local))?;
    Ok(format!(
        "MODEL CATALOG PARITY OK ({parity_note}; astra, sol and terra contracts)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(id: &str, name: &str, raw: u64, percent: u64, rates: [f64; 4]) -> Value {
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
            }
        })
    }

    fn catalog(terra_input: f64) -> String {
        json!({"models": [
            entry("gpt-6-astra", "GPT-6 Astra", 1_050_000, 100, [10.0, 12.5, 1.0, 50.0]),
            entry("gpt-6.1-sol", "GPT-6.1 Sol", 500_000, 95, [2.0, 2.5, 0.1, 10.0]),
            entry("gpt-6.1-terra", "GPT-6.1 Terra", 500_000, 95, [terra_input, 1.25, 0.05, 5.0]),
        ]})
        .to_string()
    }

    #[test]
    fn published_rates_and_limits_pass() {
        check_contracts(&catalog(1.0)).expect("terra at $1 input");
    }

    #[test]
    fn a_drifted_rate_names_the_model_and_the_field() {
        let error = check_contracts(&catalog(2.0)).unwrap_err().to_string();
        assert!(
            error.contains("GPT-6.1 Terra catalog contract failed"),
            "{error}"
        );
        assert!(error.contains("api_pricing.input_per_million"), "{error}");
    }

    #[test]
    fn a_missing_model_fails_with_its_display_name() {
        let text = json!({"models": []}).to_string();
        let error = check_contracts(&text).unwrap_err().to_string();
        assert!(
            error.contains("GPT-6 Astra catalog contract failed"),
            "{error}"
        );
    }
}
