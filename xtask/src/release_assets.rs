//! Assembles the complete public asset set from the per-platform artifacts and writes
//! the updater manifest and `SHA256SUMS.txt`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Serialize, Serializer};

use crate::clock;
use crate::fsx;
use crate::repo_config;
use crate::semver;

/// One required public file: `<prefix>*<suffix>`, optionally an updater signature.
pub struct Requirement {
    pub prefix: &'static str,
    pub suffix: &'static str,
    /// Tauri updater platform key when the file is a `.sig` payload signature.
    pub updater_target: Option<&'static str>,
}

const fn plain(prefix: &'static str, suffix: &'static str) -> Requirement {
    Requirement {
        prefix,
        suffix,
        updater_target: None,
    }
}

const fn signed(prefix: &'static str, suffix: &'static str, target: &'static str) -> Requirement {
    Requirement {
        prefix,
        suffix,
        updater_target: Some(target),
    }
}

/// Every entry is required. The `.sig` payloads are what the in-app updater verifies
/// against the pubkey in `tauri.conf.json`, so a release without them would publish
/// installers that Pulse can never offer as an update.
pub const REQUIREMENTS: [Requirement; 22] = [
    plain("pulse-windows-x64-", ".exe"),
    signed("pulse-windows-x64-", ".exe.sig", "windows-x86_64"),
    plain("pulse-windows-x64-", ".msi"),
    plain("pulse-windows-x64", ".spdx.json"),
    plain("pulse-windows-arm64-", ".exe"),
    signed("pulse-windows-arm64-", ".exe.sig", "windows-aarch64"),
    plain("pulse-windows-arm64-", ".msi"),
    plain("pulse-windows-arm64", ".spdx.json"),
    plain("pulse-macos-arm64-", ".app.tar.gz"),
    signed("pulse-macos-arm64-", ".app.tar.gz.sig", "darwin-aarch64"),
    plain("pulse-macos-arm64-", ".dmg"),
    plain("pulse-macos-x64-", ".app.tar.gz"),
    signed("pulse-macos-x64-", ".app.tar.gz.sig", "darwin-x86_64"),
    plain("pulse-macos-x64-", ".dmg"),
    plain("pulse-linux-x64-", ".deb"),
    plain("pulse-linux-x64-", ".rpm"),
    plain("pulse-linux-x64-", ".AppImage"),
    signed("pulse-linux-x64-", ".AppImage.sig", "linux-x86_64"),
    plain("pulse-linux-arm64-", ".deb"),
    plain("pulse-linux-arm64-", ".rpm"),
    plain("pulse-linux-arm64-", ".AppImage"),
    signed("pulse-linux-arm64-", ".AppImage.sig", "linux-aarch64"),
];

#[derive(Serialize)]
struct PlatformEntry {
    signature: String,
    url: String,
}

/// A map that serializes in insertion order; `serde_json` would sort keys alphabetically.
struct Ordered(Vec<(&'static str, PlatformEntry)>);

impl Serialize for Ordered {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.0.iter().map(|(key, entry)| (key, entry)))
    }
}

#[derive(Serialize)]
struct UpdaterManifest {
    version: String,
    pub_date: String,
    notes: String,
    platforms: Ordered,
}

fn matches_requirement(name: &str, requirement: &Requirement) -> bool {
    name.starts_with(requirement.prefix)
        && name
            .to_ascii_lowercase()
            .ends_with(&requirement.suffix.to_ascii_lowercase())
}

/// Collects the release set from `artifacts` into the empty (or new) `output` folder.
pub fn assemble(artifacts: &Path, output: &Path, tag: &str, repository: &str) -> Result<String> {
    let Some(version) = semver::release_tag_version(tag) else {
        bail!("Tag must be a semantic version tag such as v1.6.1");
    };
    if !repo_config::is_owner_name(repository) {
        bail!("Repository must be in owner/name form");
    }

    if !artifacts.is_dir() {
        bail!("Artifacts directory not found: {}", artifacts.display());
    }
    let mut assets = fsx::walk_files(artifacts)?;
    assets.sort_by_key(|path| (fsx::file_name(path), path.clone()));
    if assets.is_empty() {
        bail!("No release assets were produced");
    }

    let mut selected: Vec<&PathBuf> = Vec::new();
    let mut platforms = Vec::new();
    for requirement in &REQUIREMENTS {
        let found: Vec<&PathBuf> = assets
            .iter()
            .filter(|path| matches_requirement(&fsx::file_name(path), requirement))
            .collect();
        if found.len() != 1 {
            bail!(
                "Missing required release asset prefix '{}' suffix '{}': found {}",
                requirement.prefix,
                requirement.suffix,
                found.len()
            );
        }
        let asset = found[0];
        selected.push(asset);

        if let Some(target) = requirement.updater_target {
            let signature = fsx::read_text(asset)?.trim().to_owned();
            let name = fsx::file_name(asset);
            if signature.is_empty() {
                bail!("Updater signature is empty: {name}");
            }
            // The signed payload is the installer the .sig sits next to.
            let payload = name.strip_suffix(".sig").unwrap_or(&name);
            platforms.push((
                target,
                PlatformEntry {
                    signature,
                    url: format!(
                        "https://github.com/{repository}/releases/download/{tag}/{payload}"
                    ),
                },
            ));
        }
    }

    let selected_names: Vec<String> = selected.iter().map(|path| fsx::file_name(path)).collect();
    for (target, entry) in &platforms {
        let payload = entry.url.rsplit('/').next().unwrap_or_default();
        if !selected_names.iter().any(|name| name == payload) {
            bail!("Updater target {target} references an unpublished payload: {payload}");
        }
    }
    let mut sorted_names = selected_names.clone();
    sorted_names.sort();
    sorted_names.dedup();
    if sorted_names.len() != selected_names.len() {
        bail!("Release assets have duplicate names");
    }

    if output.exists() {
        let occupied = fs::read_dir(output)
            .with_context(|| format!("list {}", output.display()))?
            .next()
            .is_some();
        if occupied {
            bail!(
                "Release asset output directory must be empty: {}",
                output.display()
            );
        }
    } else {
        fs::create_dir_all(output).with_context(|| format!("create {}", output.display()))?;
    }

    for asset in &selected {
        let destination = output.join(fsx::file_name(asset));
        fs::copy(asset, &destination)
            .with_context(|| format!("copy {} to {}", asset.display(), destination.display()))?;
    }

    // Written before the checksum pass so the manifest the updater reads is covered
    // by SHA256SUMS.txt like every other published asset.
    let updater_targets = platforms.len();
    let manifest = UpdaterManifest {
        version: version.to_owned(),
        pub_date: clock::utc_timestamp(clock::now_unix()),
        notes: format!("Pulse {tag}. See the release notes for details."),
        platforms: Ordered(platforms),
    };
    let mut manifest_text = serde_json::to_string_pretty(&manifest)?;
    manifest_text.push('\n');
    fsx::write_text(&output.join("latest.json"), &manifest_text)?;
    fsx::write_sha256sums(output)?;

    Ok(format!(
        "Release assets prepared: files={} updater-targets={updater_targets} checksums=SHA256SUMS.txt",
        selected.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirements_cover_six_updater_targets_and_both_arm64_lanes() {
        let targets: Vec<_> = REQUIREMENTS
            .iter()
            .filter_map(|r| r.updater_target)
            .collect();
        assert_eq!(
            targets,
            [
                "windows-x86_64",
                "windows-aarch64",
                "darwin-aarch64",
                "darwin-x86_64",
                "linux-x86_64",
                "linux-aarch64"
            ]
        );
        assert!(
            REQUIREMENTS
                .iter()
                .any(|r| r.prefix == "pulse-windows-arm64-")
        );
        assert!(
            REQUIREMENTS
                .iter()
                .any(|r| r.prefix == "pulse-linux-arm64-")
        );
    }

    #[test]
    fn suffix_matching_ignores_case_but_prefix_matching_does_not() {
        let requirement = plain("pulse-linux-x64-", ".AppImage");
        assert!(matches_requirement(
            "pulse-linux-x64-Pulse.appimage",
            &requirement
        ));
        assert!(!matches_requirement(
            "Pulse-linux-x64-Pulse.AppImage",
            &requirement
        ));
        assert!(!matches_requirement(
            "pulse-linux-x64-Pulse.AppImage.sig",
            &requirement
        ));
    }
}
