//! Re-vendors the Codex runtime from an immutable annotated upstream tag.
//!
//! Every file is rebuilt in a scratch tree and verified there; the live tree is touched
//! only after the whole inventory passes, so a rejected sync leaves `src/codex` as it was.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;

use super::{CONTRACT_RELATIVE, check, text_of};
use crate::fsx::{self, RemoveOnDrop};
use crate::git;
use crate::semver::{self, is_full_sha};

const OFFICIAL_REPOSITORY: &str = "https://github.com/xt0n1-t3ch/Codex-Discord-Rich-Presence";

pub struct UpdateOptions<'a> {
    pub root: &'a Path,
    pub contract_path: &'a Path,
    pub tag: &'a str,
    pub commit: &'a str,
    /// A local clone to sync from instead of GitHub; enables test provenance.
    pub test_repository: Option<&'a Path>,
    pub test_mode: bool,
}

#[derive(Serialize)]
struct FileEntry {
    source: String,
    target: String,
    mode: String,
    source_sha256: String,
    target_sha256: String,
}

/// One vendored file between the write pass and the manifest pass.
struct Staged {
    source_rel: String,
    target_rel: String,
    mode: String,
    source_path: PathBuf,
    target_path: PathBuf,
}

#[derive(Serialize)]
struct Manifest {
    schema_version: u32,
    sync_version: i64,
    repository: String,
    #[serde(rename = "ref")]
    reference: String,
    commit: String,
    upstream_committed_at: String,
    strategy: String,
    inventory: String,
    provenance: &'static str,
    contract_sha256: String,
    files: Vec<FileEntry>,
    local_adapters: Vec<String>,
}

/// Rewrites `crate::<module>` to `crate::codex::<module>` for the namespace modules,
/// matching only whole words, so `crate::config_x` and `my_crate::config` are untouched.
pub fn rebase_namespace(source: &str, modules: &[String]) -> String {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut output = String::with_capacity(source.len() + 64);
    let mut cursor = 0;
    for (index, matched) in source.match_indices("crate::") {
        if index < cursor {
            continue;
        }
        let before_ok = source[..index]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word(c));
        let after = &source[index + matched.len()..];
        let word_len = after.find(|c: char| !is_word(c)).unwrap_or(after.len());
        if before_ok && modules.iter().any(|module| module == &after[..word_len]) {
            output.push_str(&source[cursor..index]);
            output.push_str("crate::codex::");
            cursor = index + matched.len();
        }
    }
    output.push_str(&source[cursor..]);
    output
}

fn format_rust_file(path: &Path) -> Result<()> {
    let output = Command::new("rustfmt")
        .args(["--edition", "2024", "--config", "skip_children=true"])
        .arg(path)
        .output()
        .context("failed to start rustfmt")?;
    if !output.status.success() {
        bail!(
            "rustfmt failed for {}: {}{}",
            path.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn assert_pulse_adapters(codex_dir: &Path) -> Result<()> {
    let module_path = codex_dir.join("mod.rs");
    let process_path = codex_dir.join("process.rs");
    if !module_path.is_file() {
        bail!("Pulse-owned adapter is missing: src/codex/mod.rs");
    }
    if !process_path.is_file() {
        bail!("Pulse-owned adapter is missing: src/codex/process.rs");
    }
    if !fsx::read_text(&module_path)?.contains("pub mod process;") {
        bail!("Pulse-owned src/codex/mod.rs must expose the process adapter");
    }
    let process = fsx::read_text(&process_path)?;
    for symbol in [
        "Codex.exe",
        "is_opencode_running",
        "is_codex_app_running",
        "is_desktop_surface_running",
    ] {
        if !process.contains(symbol) {
            bail!("Pulse-owned process adapter is missing required symbol: {symbol}");
        }
    }
    Ok(())
}

fn inventory_available(inventory: &Value, checkout: &Path) -> bool {
    inventory
        .get("files")
        .and_then(Value::as_array)
        .is_some_and(|files| {
            files
                .iter()
                .all(|file| checkout.join(text_of(file.get("source"))).is_file())
        })
}

/// Picks the contract's latest inventory when the checkout has all its sources, else the
/// first inventory (in contract order) that the checkout satisfies.
fn choose_inventory<'a>(contract: &'a Value, checkout: &Path) -> Result<(String, &'a Value)> {
    let inventories = contract
        .get("inventories")
        .and_then(Value::as_object)
        .context("Vendoring contract latest_inventory is missing")?;
    let latest = text_of(contract.get("latest_inventory"));
    let Some(latest_inventory) = inventories.get(&latest) else {
        bail!("Vendoring contract latest_inventory is missing");
    };
    if inventory_available(latest_inventory, checkout) {
        return Ok((latest, latest_inventory));
    }
    inventories
        .iter()
        .find(|(_, inventory)| inventory_available(inventory, checkout))
        .map(|(name, inventory)| (name.clone(), inventory))
        .context("Canonical tag does not satisfy any vendoring inventory")
}

/// Verifies the tag and commit on the remote, vendors the inventory and installs it.
pub fn run(options: &UpdateOptions<'_>) -> Result<String> {
    let UpdateOptions {
        root,
        contract_path,
        tag,
        commit,
        test_repository,
        test_mode,
    } = *options;
    if semver::release_tag_version(tag).is_none() {
        bail!("Tag must be an immutable semantic version tag such as v1.7.2");
    }
    if !is_full_sha(commit) {
        bail!("Commit must be a full 40-character Git SHA");
    }
    let commit = commit.to_ascii_lowercase();

    let root = fsx::absolute(root)?;
    let codex_dir = root.join("src/codex");
    assert_pulse_adapters(&codex_dir)?;

    let contract_text = fsx::read_text(contract_path)?;
    let contract: Value = serde_json::from_str(&contract_text)
        .map_err(|error| anyhow::anyhow!("{CONTRACT_RELATIVE} is not valid JSON: {error}"))?;
    if text_of(contract.get("official_repository")) != OFFICIAL_REPOSITORY {
        bail!("Vendoring contract names an unexpected official repository");
    }

    let (source, provenance) = if test_mode {
        let Some(repository) = test_repository.filter(|path| path.is_dir()) else {
            bail!("--test-mode requires an existing local --test-repository");
        };
        (
            fsx::absolute(repository)?.to_string_lossy().into_owned(),
            "test",
        )
    } else {
        if test_repository.is_some() {
            bail!("--test-repository is accepted only with --test-mode");
        }
        (format!("{OFFICIAL_REPOSITORY}.git"), "official")
    };

    let tag_ref = format!("refs/tags/{tag}");
    let peeled_ref = format!("{tag_ref}^{{}}");
    let remote_refs = git::run_checked(
        None,
        &["ls-remote", "--tags", &source, &tag_ref, &peeled_ref],
    )?;
    let ref_sha = |name: &str| {
        remote_refs.iter().find_map(|line| {
            let mut parts = line.split_whitespace();
            let (sha, reference) = (parts.next()?, parts.next()?);
            (is_full_sha(sha) && reference == name).then(|| sha.to_ascii_lowercase())
        })
    };
    if ref_sha(&tag_ref).is_none() {
        bail!("Repository has no tag named {tag}");
    }
    let Some(resolved) = ref_sha(&peeled_ref) else {
        bail!("Tag {tag} must be an annotated tag");
    };
    if resolved != commit {
        bail!("Tag {tag} does not resolve to commit {commit} (resolved {resolved})");
    }

    let work = std::env::temp_dir().join(format!("pulse-codex-rp-sync-{}", fsx::unique_suffix()));
    let _cleanup = RemoveOnDrop(work.clone());
    let checkout = work.join("canonical");
    let verification_root = work.join("verification");
    fs::create_dir_all(&work).with_context(|| format!("create {}", work.display()))?;

    let checkout_arg = checkout.to_string_lossy().into_owned();
    git::run_checked(
        None,
        &[
            "-c",
            "core.autocrlf=false",
            "clone",
            "--quiet",
            "--depth",
            "1",
            "--branch",
            tag,
            "--single-branch",
            &source,
            &checkout_arg,
        ],
    )?;
    let checked_out = git::first_line_lower(Some(&checkout), &["rev-parse", "HEAD"])?;
    if checked_out != commit {
        bail!("Checked out commit {checked_out} does not match requested commit {commit}");
    }

    let (inventory_name, inventory) = choose_inventory(&contract, &checkout)?;
    let modules: Vec<String> = contract
        .get("namespace_modules")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| text_of(Some(item))).collect())
        .unwrap_or_default();
    let no_files = Vec::new();
    let mappings = inventory
        .get("files")
        .and_then(Value::as_array)
        .unwrap_or(&no_files);

    // Pass 1: write every target. Pass 2: format Rust files, so a module file formats
    // after its children exist whatever the inventory order.
    let mut written: Vec<Staged> = Vec::new();
    for mapping in mappings {
        let (source_rel, target_rel, mode) = (
            text_of(mapping.get("source")),
            text_of(mapping.get("target")),
            text_of(mapping.get("mode")),
        );
        let source_path = checkout.join(&source_rel);
        let target_path = verification_root.join(&target_rel);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        match mode.as_str() {
            "byte-copy" => {
                fs::copy(&source_path, &target_path)
                    .with_context(|| format!("copy {}", source_path.display()))?;
            }
            "namespace-rebase" | "legacy-overlay" => {
                let text = String::from_utf8_lossy(&fs::read(&source_path)?).into_owned();
                let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
                fsx::write_text(&target_path, &rebase_namespace(text, &modules))?;
            }
            _ => bail!("Unsupported vendoring mode '{mode}' for {source_rel}"),
        }
        written.push(Staged {
            source_rel,
            target_rel,
            mode,
            source_path,
            target_path,
        });
    }
    for staged in &written {
        if staged
            .target_path
            .extension()
            .is_some_and(|extension| extension == "rs")
        {
            format_rust_file(&staged.target_path)?;
        }
    }
    let mut entries = Vec::new();
    for staged in written {
        entries.push(FileEntry {
            source_sha256: fsx::sha256_file(&staged.source_path)?,
            target_sha256: super::target_sha256(&staged.target_path, &staged.mode)?,
            source: staged.source_rel,
            target: staged.target_rel,
            mode: staged.mode,
        });
    }

    let adapters: Vec<String> = contract
        .get("local_adapters")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| text_of(Some(item))).collect())
        .unwrap_or_default();
    for adapter in &adapters {
        let destination = verification_root.join(adapter);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(root.join(adapter), &destination)
            .with_context(|| format!("copy adapter {adapter}"))?;
    }

    let committed_at = git::first_line(Some(&checkout), &["show", "-s", "--format=%cI", "HEAD"])?;
    let manifest = Manifest {
        schema_version: 2,
        sync_version: contract
            .get("sync_version")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        repository: OFFICIAL_REPOSITORY.to_owned(),
        reference: tag.to_owned(),
        commit: commit.clone(),
        upstream_committed_at: committed_at,
        strategy: text_of(inventory.get("strategy")),
        inventory: inventory_name.clone(),
        provenance,
        contract_sha256: fsx::sha256_hex(contract_text.as_bytes()),
        files: entries,
        local_adapters: adapters,
    };
    let mut manifest_json = serde_json::to_string_pretty(&manifest)?;
    manifest_json.push('\n');
    fsx::write_text(
        &verification_root.join("src/codex/UPSTREAM.json"),
        &manifest_json,
    )?;

    check::run(&verification_root, contract_path, test_mode)
        .context("Vendored source integrity verification failed")?;

    for mapping in mappings {
        let target = text_of(mapping.get("target"));
        let destination = root.join(&target);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(verification_root.join(&target), &destination)
            .with_context(|| format!("install {target}"))?;
    }
    let manifest_path = codex_dir.join("UPSTREAM.json");
    let temporary = codex_dir.join("UPSTREAM.json.tmp");
    fsx::write_text(&temporary, &manifest_json)?;
    fs::rename(&temporary, &manifest_path).context("replace src/codex/UPSTREAM.json")?;

    check::run(&root, contract_path, test_mode)
        .context("Vendored source integrity verification failed after installation")?;
    Ok(format!(
        "Codex Rich Presence synced from {tag} at {commit} inventory={inventory_name}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modules() -> Vec<String> {
        ["config", "cost", "session"]
            .iter()
            .map(|m| (*m).to_owned())
            .collect()
    }

    #[test]
    fn rebase_rewrites_only_whole_namespace_words() {
        let source = "use crate::config::A;\nuse crate::session::{B, C};\nlet _ = crate::config_x::D;\nuse my_crate::config::E;\nuse crate::other::F;\ncrate::cost\n";
        let expected = "use crate::codex::config::A;\nuse crate::codex::session::{B, C};\nlet _ = crate::config_x::D;\nuse my_crate::config::E;\nuse crate::other::F;\ncrate::codex::cost\n";
        assert_eq!(rebase_namespace(source, &modules()), expected);
    }

    #[test]
    fn rebase_is_idempotent_on_already_rebased_text() {
        let once = rebase_namespace("use crate::cost::x;", &modules());
        assert_eq!(rebase_namespace(&once, &modules()), once);
    }
}
