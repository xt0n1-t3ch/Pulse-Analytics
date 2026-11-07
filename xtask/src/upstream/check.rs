//! Verifies `src/codex/UPSTREAM.json` against the repository: schema 3 (immutable core
//! pin) or schema 2 (vendored mirror inventory with per-file hashes).

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::{
    CONTRACT_RELATIVE, assert_unique, contained_path, exact_string_set, pattern, required_string,
    target_sha256, text_of,
};
use crate::clock;
use crate::fsx;

fn read_json(path: &Path, label: &str) -> Result<Value> {
    let text = fsx::read_text(path)?;
    serde_json::from_str(&text)
        .map_err(|error| anyhow::anyhow!("{label} is not valid JSON: {error}"))
}

fn positive(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_i64)
        .is_some_and(|number| number >= 1)
}

/// Checks the manifest under `root` against the contract at `contract_path`.
/// `test_mode` accepts `provenance: "test"` fixtures.
pub fn run(root: &Path, contract_path: &Path, test_mode: bool) -> Result<String> {
    let root = fsx::absolute(root)?;
    let manifest_path = root.join("src/codex/UPSTREAM.json");
    if !contract_path.is_file() {
        bail!("{CONTRACT_RELATIVE} not found");
    }
    if !manifest_path.is_file() {
        bail!("src/codex/UPSTREAM.json not found. Run `cargo xtask upstream update`");
    }
    let contract = read_json(contract_path, CONTRACT_RELATIVE)?;
    let manifest = read_json(&manifest_path, "src/codex/UPSTREAM.json")?;

    let official = required_string(&contract, "official_repository", pattern::github_url)?;
    let repository = required_string(&manifest, "repository", pattern::github_url)?;
    if repository != official {
        bail!("src/codex/UPSTREAM.json repository must be {official}");
    }

    if manifest.get("schema_version").and_then(Value::as_i64) == Some(3) {
        return check_core_pin(&root, &manifest);
    }
    check_mirror(&root, &contract, &manifest, test_mode)
}

fn check_core_pin(root: &Path, manifest: &Value) -> Result<String> {
    let release = required_string(manifest, "canonical_release", pattern::release_tag)?;
    let (Some(integration), Some(compatibility)) =
        (manifest.get("integration"), manifest.get("compatibility"))
    else {
        bail!("src/codex/UPSTREAM.json schema 3 requires integration and compatibility objects");
    };
    let mode = required_string(integration, "mode", pattern::integration_mode)?;
    required_string(integration, "package", pattern::is_codex_core_package)?;
    let version = required_string(integration, "version", pattern::core_version)?;
    required_string(integration, "promotion_mode", pattern::git_rev_mode)?;
    if !pattern::core_version(&text_of(compatibility.get("pulse"))) {
        bail!("src/codex/UPSTREAM.json has an invalid compatibility pulse version");
    }
    if ![
        "presence_config_schema",
        "usage_snapshot_revision",
        "app_snapshot_revision",
    ]
    .iter()
    .all(|key| positive(compatibility.get(key)))
    {
        bail!("src/codex/UPSTREAM.json compatibility revisions must be positive");
    }

    let contracts = exact_string_set(manifest.get("shared_contracts"));
    assert_unique(&contracts, "Shared core contracts")?;
    for required in [
        "presence-composer",
        "presence-field-layout",
        "semantic-quota-windows",
        "usage-snapshot",
        "credit-balance",
    ] {
        if !contracts.iter().any(|contract| contract == required) {
            bail!("Shared core contracts are missing {required}");
        }
    }
    let adapters = exact_string_set(manifest.get("local_adapters"));
    assert_unique(&adapters, "Local adapters")?;
    for adapter in &adapters {
        if !contained_path(root, adapter, "src/codex/")?.is_file() {
            bail!("Pulse adapter is missing: {adapter}");
        }
    }

    let cargo_text = fsx::read_text(&root.join("Cargo.toml"))?;
    let cargo: toml::Table = toml::from_str(&cargo_text).context("parse Cargo.toml")?;
    let dependency = crate::core_pin::core_dependency(&cargo)?;
    let cargo_text_of = |key: &str| dependency.get(key).and_then(toml::Value::as_str);
    if cargo_text_of("version") != Some(version.as_str()) {
        bail!("Cargo core version does not match src/codex/UPSTREAM.json");
    }
    if mode == "local-path" {
        if manifest
            .get("canonical_commit")
            .is_some_and(|value| !value.is_null())
        {
            bail!("Local-path integration must not claim a canonical commit");
        }
        let path = required_string(integration, "path", pattern::plain_path)?;
        if cargo_text_of("path") != Some(path.as_str()) {
            bail!("Cargo core path does not match src/codex/UPSTREAM.json");
        }
        if text_of(integration.get("promotion_status")) != "pending-local-validation" {
            bail!("Local-path integration must remain pending-local-validation");
        }
    } else {
        let commit = required_string(manifest, "canonical_commit", pattern::sha1_hex)?;
        let rev = required_string(integration, "rev", pattern::sha1_hex)?;
        if !commit.eq_ignore_ascii_case(&rev) {
            bail!("Canonical commit does not match integration rev");
        }
        let cargo_rev = cargo_text_of("rev").unwrap_or_default();
        if !cargo_rev.eq_ignore_ascii_case(&rev) || !dependency.contains_key("git") {
            bail!("Cargo core Git rev does not match src/codex/UPSTREAM.json");
        }
    }
    Ok(format!(
        "Codex presence core contract verified: mode={mode} release={release} version={version} adapters={}",
        adapters.len()
    ))
}

fn check_mirror(
    root: &Path,
    contract: &Value,
    manifest: &Value,
    test_mode: bool,
) -> Result<String> {
    let sync_version = contract.get("sync_version").and_then(Value::as_i64);
    if manifest.get("schema_version").and_then(Value::as_i64) != Some(2)
        || manifest.get("sync_version").and_then(Value::as_i64) != sync_version
    {
        bail!(
            "src/codex/UPSTREAM.json must use schema_version 2 and sync_version {}",
            sync_version.unwrap_or_default()
        );
    }

    required_string(manifest, "ref", pattern::release_tag)?;
    let commit = required_string(manifest, "commit", pattern::sha1_hex)?;
    let inventory_name = required_string(manifest, "inventory", pattern::slug)?;
    let strategy = required_string(manifest, "strategy", pattern::slug)?;
    let provenance = required_string(manifest, "provenance", pattern::provenance)?;
    if provenance == "test" && !test_mode {
        bail!("Test provenance is accepted only with --test-mode");
    }
    let committed_at = text_of(manifest.get("upstream_committed_at"));
    if clock::parse_iso_seconds(&committed_at).is_none() {
        bail!("src/codex/UPSTREAM.json has an invalid upstream_committed_at");
    }

    let Some(inventory) = contract
        .get("inventories")
        .and_then(|all| all.get(&inventory_name))
    else {
        bail!("src/codex/UPSTREAM.json names an unknown expected inventory: {inventory_name}");
    };
    if strategy != text_of(inventory.get("strategy")) {
        bail!(
            "src/codex/UPSTREAM.json strategy does not match expected inventory {inventory_name}"
        );
    }

    let expected_adapters = exact_string_set(contract.get("local_adapters"));
    assert_unique(&expected_adapters, "Vendoring contract adapters")?;
    let actual_adapters = exact_string_set(manifest.get("local_adapters"));
    assert_unique(&actual_adapters, "Manifest adapters")?;
    if expected_adapters != actual_adapters {
        bail!("src/codex/UPSTREAM.json adapters do not match the expected inventory contract");
    }

    let no_files = Vec::new();
    let expected_files = inventory
        .get("files")
        .and_then(Value::as_array)
        .unwrap_or(&no_files);
    let actual_files = manifest
        .get("files")
        .and_then(Value::as_array)
        .unwrap_or(&no_files);
    if actual_files.len() != expected_files.len() {
        bail!(
            "src/codex/UPSTREAM.json expected inventory '{inventory_name}' requires {} files, found {}",
            expected_files.len(),
            actual_files.len()
        );
    }

    let mut by_target: Vec<(String, &Value)> = Vec::new();
    for file in actual_files {
        let target = required_string(file, "target", pattern::rust_or_json_under_codex)?;
        if by_target.iter().any(|(existing, _)| *existing == target) {
            bail!("Duplicate vendored target in src/codex/UPSTREAM.json: {target}");
        }
        by_target.push((target, file));
    }

    for expected in expected_files {
        let source = text_of(expected.get("source"));
        let target = text_of(expected.get("target"));
        let mode = text_of(expected.get("mode"));
        let Some((_, actual)) = by_target.iter().find(|(name, _)| *name == target) else {
            bail!(
                "src/codex/UPSTREAM.json expected inventory '{inventory_name}' is missing {target}"
            );
        };
        if required_string(actual, "source", pattern::rust_or_json_under_src)? != source
            || required_string(actual, "mode", pattern::mode)? != mode
        {
            bail!("Vendored mapping differs from expected inventory '{inventory_name}': {target}");
        }
        required_string(actual, "source_sha256", pattern::sha256_hex)?;
        let expected_hash = required_string(actual, "target_sha256", pattern::sha256_hex)?;

        contained_path(root, &source, "src/")?;
        let target_path = contained_path(root, &target, "src/codex/")?;
        if actual_adapters.contains(&target) {
            bail!("Pulse-owned adapter cannot be listed as a mirrored file: {target}");
        }
        if !target_path.is_file() {
            bail!("Vendored file is missing: {target}");
        }
        let actual_hash = target_sha256(&target_path, &mode)?;
        if actual_hash != expected_hash.to_ascii_lowercase() {
            bail!(
                "Vendored file hash mismatch: {target} expected={expected_hash} actual={actual_hash}"
            );
        }
    }

    let module_path = root.join("src/codex/mod.rs");
    if !module_path.is_file() {
        bail!("Pulse-owned adapter is missing: src/codex/mod.rs");
    }
    let module_source = fsx::read_text(&module_path)?;
    for declaration in inventory
        .get("required_module_declarations")
        .and_then(Value::as_array)
        .unwrap_or(&no_files)
    {
        let declaration = text_of(Some(declaration));
        if !module_source.contains(&declaration) {
            bail!("src/codex/mod.rs is missing required declaration: {declaration}");
        }
    }

    Ok(format!(
        "Codex Rich Presence integrity verified: inventory={inventory_name} commit={commit} files={}",
        actual_files.len()
    ))
}
