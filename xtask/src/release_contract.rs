//! Proves a release tag, every version surface and the repository release contract
//! agree before any platform build starts.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::changelog;
use crate::fsx;
use crate::git;
use crate::semver::{self, is_full_sha};

const CONTRACT_PATH: &str = "scripts/release-contract.json";
const MIN_CHANGELOG_CHARS: usize = 80;

fn parse_toml(path: &Path) -> Result<toml::Table> {
    let text = fsx::read_text(path)?;
    toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn read_json(path: &Path) -> Result<Value> {
    let text = fsx::read_text(path)?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn cargo_manifest_version(path: &Path) -> Result<String> {
    parse_toml(path)?
        .get("package")
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("Unable to find package version in {}", path.display()))
}

fn lock_package_version(path: &Path, name: &str) -> Result<String> {
    let lock = parse_toml(path)?;
    lock.get("package")
        .and_then(toml::Value::as_array)
        .and_then(|packages| {
            packages
                .iter()
                .find(|package| package.get("name").and_then(toml::Value::as_str) == Some(name))
        })
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("Unable to find {name} in {}", path.display()))
}

fn json_string(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// True when `source` has a `[CONFIG_]SCHEMA_VERSION: u32|i64 = <version>;` declaration.
pub fn declares_schema(source: &str, version: u64) -> bool {
    let wanted = version.to_string();
    source.match_indices("SCHEMA_VERSION").any(|(index, name)| {
        let rest = source[index + name.len()..].trim_start();
        let Some(rest) = rest.strip_prefix(':') else {
            return false;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest
            .strip_prefix("u32")
            .or_else(|| rest.strip_prefix("i64"))
        else {
            return false;
        };
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            return false;
        };
        rest.trim_start()
            .strip_prefix(wanted.as_str())
            .is_some_and(|tail| tail.trim_start().starts_with(';'))
    })
}

/// The shared-core pin of the root manifest, followed into
/// `[workspace.dependencies]` when `[dependencies]` inherits it.
struct CorePin {
    has_git: bool,
    has_path: bool,
    rev: Option<String>,
    version: Option<String>,
}

fn core_pin(root: &Path) -> Result<CorePin> {
    let manifest = parse_toml(&root.join("Cargo.toml"))?;
    let dependency = manifest
        .get("dependencies")
        .and_then(|dependencies| dependencies.get("codex-presence-core"))
        .and_then(toml::Value::as_table)
        .context("Cargo.toml does not declare codex-presence-core")?;
    let dependency = if dependency.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
        manifest
            .get("workspace")
            .and_then(|workspace| workspace.get("dependencies"))
            .and_then(|dependencies| dependencies.get("codex-presence-core"))
            .and_then(toml::Value::as_table)
            .context("Cargo.toml inherits codex-presence-core but [workspace.dependencies] does not declare it")?
    } else {
        dependency
    };
    let text = |key: &str| {
        dependency
            .get(key)
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
    };
    Ok(CorePin {
        has_git: dependency.contains_key("git"),
        has_path: dependency.contains_key("path"),
        rev: text("rev"),
        version: text("version"),
    })
}

fn assert_repository_contract(root: &Path, version: &str) -> Result<()> {
    let contract_path = root.join(CONTRACT_PATH);
    if !contract_path.is_file() {
        return Ok(());
    }
    let contract = read_json(&contract_path)
        .map_err(|error| anyhow::anyhow!("{CONTRACT_PATH} is not valid JSON: {error:#}"))?;
    if contract.pointer("/schema_version").and_then(Value::as_i64) != Some(1) {
        bail!("{CONTRACT_PATH} schema_version must be 1");
    }
    let product_version = json_string(&contract, "/product/version");
    if product_version != version {
        bail!("Release contract product version '{product_version}' does not match '{version}'");
    }

    let core_version = json_string(&contract, "/core/version");
    let pin = core_pin(root)?;
    if pin.has_path || !pin.has_git {
        bail!(
            "Pulse releases require codex-presence-core as an immutable Git dependency, not a path dependency"
        );
    }
    let Some(rev) = pin.rev.filter(|rev| is_full_sha(rev)) else {
        bail!("codex-presence-core must use a full 40-character Git rev");
    };
    if pin.version.as_deref() != Some(core_version.as_str()) {
        bail!("codex-presence-core dependency version does not match {core_version}");
    }

    let manifest = read_json(&root.join(json_string(&contract, "/core/manifest")))?;
    let manifest_commit = manifest
        .get("canonical_commit")
        .and_then(Value::as_str)
        .or_else(|| manifest.pointer("/core/commit").and_then(Value::as_str))
        .or_else(|| manifest.get("commit").and_then(Value::as_str))
        .unwrap_or_default();
    let manifest_version = manifest
        .pointer("/integration/version")
        .and_then(Value::as_str)
        .or_else(|| manifest.pointer("/core/version").and_then(Value::as_str))
        .or_else(|| manifest.get("core_version").and_then(Value::as_str))
        .unwrap_or_default();
    if !manifest_commit.eq_ignore_ascii_case(&rev) {
        bail!("Canonical core manifest SHA does not match the Cargo Git rev");
    }
    if manifest_version.trim().is_empty() || manifest_version != core_version {
        bail!("Canonical core manifest version does not match {core_version}");
    }

    for key in ["configuration", "database"] {
        let source_path = json_string(&contract, &format!("/{key}/source"));
        let schema = contract
            .pointer(&format!("/{key}/schema_version"))
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let source = fsx::read_text(&root.join(&source_path))?;
        if !declares_schema(&source, schema) {
            bail!("{source_path} does not declare schema {schema}");
        }
    }

    let sbom = json_string(&contract, "/release/windows_sbom");
    if !sbom.ends_with(".spdx.json")
        || json_string(&contract, "/release/checksum_manifest") != "SHA256SUMS.txt"
    {
        bail!("Release integrity contract must require an SPDX JSON SBOM and SHA256SUMS.txt");
    }
    Ok(())
}

/// Version surfaces that must equal the tag: `(label, version)`.
fn version_surfaces(root: &Path) -> Result<Vec<(&'static str, String)>> {
    let lock = root.join("Cargo.lock");
    let package = read_json(&root.join("package.json"))?;
    let frontend = read_json(&root.join("frontend/package.json"))?;
    let tauri = read_json(&root.join("src-tauri/tauri.conf.json"))?;
    Ok(vec![
        (
            "Cargo.toml",
            cargo_manifest_version(&root.join("Cargo.toml"))?,
        ),
        (
            "src-tauri/Cargo.toml",
            cargo_manifest_version(&root.join("src-tauri/Cargo.toml"))?,
        ),
        (
            "Cargo.lock:cc-discord-presence",
            lock_package_version(&lock, "cc-discord-presence")?,
        ),
        ("Cargo.lock:pulse", lock_package_version(&lock, "pulse")?),
        ("package.json", json_string(&package, "/version")),
        ("frontend/package.json", json_string(&frontend, "/version")),
        ("src-tauri/tauri.conf.json", json_string(&tauri, "/version")),
    ])
}

/// Verifies `tag` against the checkout at `root`. `test_mode` skips the check that
/// `HEAD` is the tagged commit, because fixtures are built on branches.
pub fn check(root: &Path, tag: &str, expected_commit: &str, test_mode: bool) -> Result<String> {
    let Some(version) = semver::release_tag_version(tag) else {
        bail!("Tag must be a semantic version tag such as v1.5.2 or v1.5.3-rc.1");
    };
    if !is_full_sha(expected_commit) {
        bail!("ExpectedCommit must be a full 40-character Git SHA");
    }
    let expected_commit = expected_commit.to_ascii_lowercase();
    let root = fsx::absolute(root)?;

    let surfaces = version_surfaces(&root)?;
    for (label, value) in &surfaces {
        if value != version {
            bail!("Release version mismatch: tag={version} {label}={value}");
        }
    }
    assert_repository_contract(&root, version)?;

    for surface in ["README.md", "docs/index.md"] {
        let content = fsx::read_text(&root.join(surface))?;
        if !content.contains(&format!("v{version}")) {
            bail!("{surface} does not identify release v{version}");
        }
    }

    let changelog_text = fsx::read_text(&root.join("CHANGELOG.md"))?;
    let Some(section) = changelog::section_body(&changelog_text, version) else {
        bail!("CHANGELOG.md has no section for {version}");
    };
    if section.trim().chars().count() < MIN_CHANGELOG_CHARS {
        bail!("CHANGELOG.md section for {version} is too short to publish");
    }

    let tag_ref = format!("refs/tags/{tag}");
    let tag_type = git::run(Some(&root), &["cat-file", "-t", &tag_ref])?;
    if !tag_type.success || tag_type.lines.first().map(String::as_str) != Some("tag") {
        bail!("Release tag {tag} must be an annotated tag");
    }
    let tagged_commit =
        git::first_line_lower(Some(&root), &["rev-parse", &format!("{tag}^{{commit}}")])?;
    if tagged_commit != expected_commit {
        bail!(
            "Release tag {tag} resolves to {tagged_commit} instead of expected commit {expected_commit}"
        );
    }
    let reachable = git::run(
        Some(&root),
        &[
            "merge-base",
            "--is-ancestor",
            &expected_commit,
            "origin/main",
        ],
    )?;
    if !reachable.success {
        bail!("Release commit {expected_commit} is not reachable from origin/main");
    }
    if !test_mode {
        let head = git::first_line_lower(Some(&root), &["rev-parse", "HEAD"])?;
        if head != expected_commit {
            bail!("Release checkout HEAD {head} does not match expected commit {expected_commit}");
        }
    }

    Ok(format!(
        "Release contract verified: tag={tag} version={version} commit={expected_commit} surfaces={}",
        surfaces.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_declaration_matches_the_real_constants_only() {
        assert!(declares_schema(
            "const CONFIG_SCHEMA_VERSION: u32 = 13;",
            13
        ));
        assert!(declares_schema("const SCHEMA_VERSION: i64 = 7;\n", 7));
        assert!(declares_schema(
            "pub const DB_SCHEMA_VERSION : i64 =  7 ;",
            7
        ));
        assert!(!declares_schema("const SCHEMA_VERSION: i64 = 17;", 7));
        assert!(!declares_schema("const SCHEMA_VERSION: u64 = 7;", 7));
        assert!(!declares_schema("const SCHEMA_VERSION: i64 = 7", 7));
        assert!(!declares_schema("let x = SCHEMA_VERSION;", 7));
    }
}
