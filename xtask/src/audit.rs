//! `cargo audit --deny warnings` with the reviewed exceptions in
//! `scripts/rustsec-accepted-warnings.json`, which expire on `review_after`.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::clock;
use crate::fsx;

const CONTRACT_PATH: &str = "scripts/rustsec-accepted-warnings.json";

/// `RUSTSEC-YYYY-NNNN`
fn is_advisory_id(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("RUSTSEC-") else {
        return false;
    };
    rest.split_once('-').is_some_and(|(year, number)| {
        year.len() == 4
            && number.len() == 4
            && year
                .bytes()
                .chain(number.bytes())
                .all(|b| b.is_ascii_digit())
    })
}

/// Validates the contract and returns the `cargo audit` arguments it implies.
/// `now_unix` is passed in so expiry is testable.
pub fn arguments(contract_text: &str, now_unix: i64) -> Result<Vec<String>> {
    let contract: Value = serde_json::from_str(contract_text)
        .map_err(|error| anyhow::anyhow!("RustSec exception contract is invalid JSON: {error}"))?;
    if contract.get("schema_version").and_then(Value::as_i64) != Some(1) {
        bail!("RustSec exception contract must use schema_version 1");
    }
    let review_after = contract
        .get("review_after")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(review_seconds) = clock::parse_iso_seconds(review_after) else {
        bail!("RustSec exception contract has an invalid review_after");
    };
    if review_seconds < now_unix {
        let date = clock::utc_timestamp(review_seconds.max(0) as u64);
        bail!("RustSec exception contract expired on {}", &date[..10]);
    }

    let mut arguments: Vec<String> = ["audit", "--deny", "warnings"].map(str::to_owned).into();
    let mut seen: Vec<&str> = Vec::new();
    let advisories = contract.get("advisories").and_then(Value::as_array);
    for advisory in advisories.into_iter().flatten() {
        let id = advisory
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let reason = advisory
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !is_advisory_id(id) || reason.trim().is_empty() {
            bail!("RustSec exception contract contains an invalid advisory");
        }
        if seen.contains(&id) {
            bail!("RustSec exception contract contains duplicate advisory {id}");
        }
        seen.push(id);
        arguments.push("--ignore".to_owned());
        arguments.push(id.to_owned());
    }
    Ok(arguments)
}

pub fn run(root: &Path) -> Result<String> {
    let contract = fsx::read_text(&root.join(CONTRACT_PATH))?;
    let arguments = arguments(&contract, clock::now_unix() as i64)?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(&arguments)
        .current_dir(root)
        .status()
        .context(
            "failed to start cargo audit (install it with `cargo install cargo-audit --locked`)",
        )?;
    if !status.success() {
        bail!("cargo {} failed with {status}", arguments.join(" "));
    }
    Ok("RustSec audit passed".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_793_000_000; // late October 2026

    fn contract(review_after: &str, advisories: &str) -> String {
        format!(
            r#"{{"schema_version":1,"review_after":"{review_after}","advisories":{advisories}}}"#
        )
    }

    #[test]
    fn valid_contract_becomes_ignore_arguments_in_file_order() {
        let text = contract(
            "2026-11-17",
            r#"[{"id":"RUSTSEC-2024-0429","reason":"a"},{"id":"RUSTSEC-2025-0057","reason":"b"}]"#,
        );
        assert_eq!(
            arguments(&text, NOW).expect("valid"),
            [
                "audit",
                "--deny",
                "warnings",
                "--ignore",
                "RUSTSEC-2024-0429",
                "--ignore",
                "RUSTSEC-2025-0057"
            ]
        );
    }

    #[test]
    fn expired_malformed_and_duplicate_contracts_fail_with_the_script_messages() {
        let expired = contract("2026-10-01", "[]");
        assert!(
            format!("{}", arguments(&expired, NOW).unwrap_err()).contains("expired on 2026-10-01")
        );

        let bad_date = contract("someday", "[]");
        assert!(
            format!("{}", arguments(&bad_date, NOW).unwrap_err()).contains("invalid review_after")
        );

        let bad_id = contract("2026-11-17", r#"[{"id":"RUSTSEC-24-1","reason":"a"}]"#);
        assert!(format!("{}", arguments(&bad_id, NOW).unwrap_err()).contains("invalid advisory"));

        let no_reason = contract("2026-11-17", r#"[{"id":"RUSTSEC-2024-0429","reason":" "}]"#);
        assert!(
            format!("{}", arguments(&no_reason, NOW).unwrap_err()).contains("invalid advisory")
        );

        let duplicate = contract(
            "2026-11-17",
            r#"[{"id":"RUSTSEC-2024-0429","reason":"a"},{"id":"RUSTSEC-2024-0429","reason":"b"}]"#,
        );
        assert!(
            format!("{}", arguments(&duplicate, NOW).unwrap_err()).contains("duplicate advisory")
        );

        assert!(format!("{}", arguments("{oops", NOW).unwrap_err()).contains("invalid JSON"));
        assert!(
            format!("{}", arguments(r#"{"schema_version":2}"#, NOW).unwrap_err())
                .contains("schema_version 1")
        );
    }
}
