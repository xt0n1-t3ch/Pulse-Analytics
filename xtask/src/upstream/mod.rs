//! The Codex vendoring commands: `check`, `update` and `freshness`. The contract file is
//! `scripts/codex-vendor-contract.json`; the manifest is `src/codex/UPSTREAM.json`.

pub mod check;
pub mod freshness;
pub mod update;

use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};
use serde_json::Value;

use crate::fsx;

pub const CONTRACT_RELATIVE: &str = "scripts/codex-vendor-contract.json";

/// Value as text: strings as-is, null as empty, anything else as its JSON spelling.
pub(crate) fn text_of(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

/// A required, non-blank string property that must satisfy `valid`.
pub(crate) fn required_string(
    object: &Value,
    name: &str,
    valid: fn(&str) -> bool,
) -> Result<String> {
    let value = text_of(object.get(name));
    if value.trim().is_empty() || !valid(&value) {
        bail!("src/codex/UPSTREAM.json has an invalid {name}");
    }
    Ok(value)
}

pub(crate) mod pattern {
    pub fn github_url(text: &str) -> bool {
        text.starts_with("https://github.com/")
    }

    fn numeric(part: &str) -> bool {
        !part.is_empty()
            && part.bytes().all(|b| b.is_ascii_digit())
            && (part == "0" || !part.starts_with('0'))
    }

    pub fn core_version(text: &str) -> bool {
        let parts: Vec<&str> = text.split('.').collect();
        parts.len() == 3 && parts.iter().all(|part| numeric(part))
    }

    /// `vX.Y.Z` with an optional loose `-pre.release` suffix, the shape of upstream refs.
    pub fn release_tag(text: &str) -> bool {
        let Some(rest) = text.strip_prefix('v') else {
            return false;
        };
        let (core, pre) = match rest.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (rest, None),
        };
        core_version(core)
            && pre.is_none_or(|pre| {
                !pre.is_empty()
                    && pre
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
            })
    }

    pub fn sha1_hex(text: &str) -> bool {
        text.len() == 40 && text.bytes().all(|b| b.is_ascii_hexdigit())
    }

    pub fn sha256_hex(text: &str) -> bool {
        text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
    }

    /// `[a-z0-9][a-z0-9-]*`
    pub fn slug(text: &str) -> bool {
        let mut bytes = text.bytes();
        bytes
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }

    /// `[a-z][a-z-]*`
    pub fn mode(text: &str) -> bool {
        let mut bytes = text.bytes();
        bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            && bytes.all(|b| b.is_ascii_lowercase() || b == b'-')
    }

    pub fn is_codex_core_package(text: &str) -> bool {
        text == "codex-presence-core"
    }

    pub fn git_rev_mode(text: &str) -> bool {
        text == "git-rev"
    }

    pub fn integration_mode(text: &str) -> bool {
        matches!(text, "local-path" | "git-rev")
    }

    pub fn provenance(text: &str) -> bool {
        matches!(text, "official" | "test")
    }

    /// Any text without line breaks or double quotes.
    pub fn plain_path(text: &str) -> bool {
        !text.is_empty() && !text.contains(['\r', '\n', '"'])
    }

    pub fn rust_or_json_under_codex(text: &str) -> bool {
        text.starts_with("src/codex/")
            && text.len() > "src/codex/".len()
            && (text.ends_with(".rs") || text.ends_with(".json"))
    }

    pub fn rust_or_json_under_src(text: &str) -> bool {
        text.starts_with("src/")
            && text.len() > "src/".len()
            && (text.ends_with(".rs") || text.ends_with(".json"))
    }
}

/// Resolves a vendored path under `root`, refusing absolute paths, `..` segments and
/// anything outside `required_prefix`.
pub(crate) fn contained_path(
    root: &Path,
    relative: &str,
    required_prefix: &str,
) -> Result<PathBuf> {
    let normalized = relative.replace('\\', "/");
    let escapes = Path::new(&normalized).components().any(|component| {
        matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    });
    if escapes || !normalized.starts_with(required_prefix) {
        bail!("Vendored path escapes its allowed prefix: {relative}");
    }
    let joined = root.join(&normalized);
    if !joined.starts_with(root) {
        bail!("Vendored path escapes the repository root: {relative}");
    }
    Ok(joined)
}

/// Sorted string items of a JSON array (empty when absent); see [`assert_unique`].
pub(crate) fn exact_string_set(values: Option<&Value>) -> Vec<String> {
    let mut strings: Vec<String> = values
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| text_of(Some(item))).collect())
        .unwrap_or_default();
    strings.sort();
    strings
}

/// Fails when a sorted list holds the same entry twice.
pub(crate) fn assert_unique(values: &[String], label: &str) -> Result<()> {
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        bail!("{label} contains duplicate entries");
    }
    Ok(())
}

/// Hash recorded in the manifest: raw bytes for `byte-copy`, otherwise the UTF-8 text with
/// every line ending normalized to `\n` (a leading BOM is dropped, as the PowerShell
/// reader did), so a CRLF checkout of a vendored file still verifies.
pub(crate) fn target_sha256(path: &Path, mode: &str) -> Result<String> {
    if mode == "byte-copy" {
        return fsx::sha256_file(path);
    }
    let bytes =
        std::fs::read(path).map_err(|error| anyhow::anyhow!("read {}: {error}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    Ok(fsx::sha256_hex(normalized.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_ref_pattern_is_loose_about_prerelease_but_strict_about_core() {
        assert!(pattern::release_tag("v1.11.3"));
        assert!(pattern::release_tag("v1.11.3-rc.1"));
        assert!(!pattern::release_tag("1.11.3"));
        assert!(!pattern::release_tag("v1.11"));
        assert!(!pattern::release_tag("v1.011.3"));
        assert!(!pattern::release_tag("v1.11.3-"));
        assert!(!pattern::release_tag("main"));
    }

    #[test]
    fn slug_and_mode_patterns_match_the_script_regexes() {
        assert!(pattern::slug("model-catalog-v3"));
        assert!(!pattern::slug("-bad"));
        assert!(!pattern::slug("Bad"));
        assert!(pattern::mode("namespace-rebase"));
        assert!(!pattern::mode("byte_copy"));
        assert!(pattern::rust_or_json_under_codex(
            "src/codex/model_catalog.json"
        ));
        assert!(!pattern::rust_or_json_under_codex("src/codex/readme.md"));
        assert!(!pattern::rust_or_json_under_codex("src/codex/"));
    }

    #[test]
    fn contained_paths_reject_escapes_and_wrong_prefixes() {
        let root = Path::new("/repo");
        assert!(contained_path(root, "src/codex/cost.rs", "src/codex/").is_ok());
        assert!(contained_path(root, "src\\codex\\cost.rs", "src/codex/").is_ok());
        for bad in [
            "src/codex/../../etc/passwd",
            "/etc/passwd",
            "src/other.rs",
            "src/codex/../x",
        ] {
            let message = format!("{:#}", contained_path(root, bad, "src/codex/").unwrap_err());
            assert!(message.contains("escapes"), "{bad}: {message}");
        }
    }

    #[test]
    fn duplicates_are_detected_after_sorting() {
        let values = exact_string_set(Some(&serde_json::json!(["b", "a", "b"])));
        assert_eq!(values, ["a", "b", "b"]);
        let error = assert_unique(&values, "Manifest adapters").unwrap_err();
        assert!(format!("{error}").contains("Manifest adapters contains duplicate entries"));
    }
}
