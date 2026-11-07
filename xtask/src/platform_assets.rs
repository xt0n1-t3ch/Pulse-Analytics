//! Collects one platform's installers under platform-prefixed names so same-basename
//! macOS files cannot collide.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;

use crate::fsx;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Platform {
    #[value(name = "windows-x64")]
    WindowsX64,
    #[value(name = "windows-arm64")]
    WindowsArm64,
    #[value(name = "macos-arm64")]
    MacosArm64,
    #[value(name = "macos-x64")]
    MacosX64,
    #[value(name = "linux-x64")]
    LinuxX64,
    #[value(name = "linux-arm64")]
    LinuxArm64,
}

impl Platform {
    pub const ALL: [Platform; 6] = [
        Platform::WindowsX64,
        Platform::WindowsArm64,
        Platform::MacosArm64,
        Platform::MacosX64,
        Platform::LinuxX64,
        Platform::LinuxArm64,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::WindowsX64 => "windows-x64",
            Platform::WindowsArm64 => "windows-arm64",
            Platform::MacosArm64 => "macos-arm64",
            Platform::MacosX64 => "macos-x64",
            Platform::LinuxX64 => "linux-x64",
            Platform::LinuxArm64 => "linux-arm64",
        }
    }

    /// The `.sig` entries are the updater payload signatures Tauri emits when
    /// `createUpdaterArtifacts` is on. They are required, not optional: a release
    /// without them cannot be offered through the in-app updater.
    pub fn required_suffixes(self) -> &'static [&'static str] {
        match self {
            Platform::WindowsX64 | Platform::WindowsArm64 => &[".exe", ".exe.sig", ".msi"],
            Platform::MacosArm64 | Platform::MacosX64 => {
                &[".app.tar.gz", ".app.tar.gz.sig", ".dmg"]
            }
            Platform::LinuxX64 | Platform::LinuxArm64 => {
                &[".deb", ".rpm", ".AppImage", ".AppImage.sig"]
            }
        }
    }
}

/// First existing folder among `candidates`, the bundle output Tauri used on this runner.
pub fn first_existing_directory(candidates: &[PathBuf]) -> Option<&PathBuf> {
    candidates.iter().find(|candidate| candidate.is_dir())
}

/// [`collect`] from the first existing folder among `candidates`: Tauri puts bundles in a
/// different folder depending on the target and workspace layout.
pub fn collect_first_existing(
    candidates: &[PathBuf],
    output: &Path,
    platform: Platform,
    verification_only: bool,
) -> Result<String> {
    let Some(input) = first_existing_directory(candidates) else {
        bail!(
            "No release assets bundle directory found for {}",
            platform.as_str()
        );
    };
    collect(input, output, platform, verification_only)
}

/// Copies the platform's required files from `input` to `output` as `pulse-<platform>-<name>`.
/// `verification_only` drops the signature and updater tarball requirements: unsigned
/// native verification is not a public release.
pub fn collect(
    input: &Path,
    output: &Path,
    platform: Platform,
    verification_only: bool,
) -> Result<String> {
    let suffixes: Vec<&str> = platform
        .required_suffixes()
        .iter()
        .copied()
        .filter(|suffix| {
            !verification_only || !(suffix.ends_with(".sig") || *suffix == ".app.tar.gz")
        })
        .collect();

    if !input.is_dir() {
        bail!(
            "Release asset input directory not found: {}",
            input.display()
        );
    }
    let mut files = fsx::walk_files(input)?;
    files.sort();
    if files.is_empty() {
        bail!("No release assets were produced for {}", platform.as_str());
    }
    fs::create_dir_all(output).with_context(|| format!("create {}", output.display()))?;

    let mut collected = 0usize;
    for suffix in suffixes {
        let found: Vec<&PathBuf> = files
            .iter()
            .filter(|path| {
                fsx::file_name(path)
                    .to_ascii_lowercase()
                    .ends_with(&suffix.to_ascii_lowercase())
            })
            .collect();
        if found.len() != 1 {
            bail!(
                "Missing required release asset for {} suffix '{suffix}': found {}",
                platform.as_str(),
                found.len()
            );
        }
        let destination_name = format!("pulse-{}-{}", platform.as_str(), fsx::file_name(found[0]));
        let destination = output.join(&destination_name);
        if destination.exists() {
            bail!("Release asset collision: {destination_name}");
        }
        fs::copy(found[0], &destination)
            .with_context(|| format!("copy {} to {}", found[0].display(), destination.display()))?;
        collected += 1;
    }

    Ok(format!(
        "Platform release assets collected: platform={} files={collected}",
        platform.as_str()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_platform_requires_an_updater_signature() {
        for platform in Platform::ALL {
            let suffixes = platform.required_suffixes();
            assert!(suffixes.iter().any(|s| s.ends_with(".sig")), "{platform:?}");
        }
        assert_eq!(Platform::LinuxArm64.as_str(), "linux-arm64");
    }

    #[test]
    fn first_existing_directory_skips_missing_candidates() {
        let here = std::env::temp_dir();
        let candidates = vec![here.join("pulse-xtask-does-not-exist-0"), here.clone()];
        assert_eq!(first_existing_directory(&candidates), Some(&here));
        assert_eq!(first_existing_directory(&candidates[..1]), None);
    }
}
