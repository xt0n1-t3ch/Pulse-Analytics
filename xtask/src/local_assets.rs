//! Selects the exact-version Windows x64 installers for the recovery release and writes
//! their checksums atomically.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::fsx::{self, RemoveOnDrop};
use crate::semver;

/// Builds `output` from the bundle folder. `output` must not exist; it appears only
/// once complete, so a failed run never leaves a partial release folder behind.
pub fn prepare(bundle: &Path, output: &Path, tag: &str) -> Result<String> {
    let Some(version) = semver::release_tag_version(tag) else {
        bail!("Tag must be a semantic version tag such as v1.7.2 or v1.7.3-rc.1");
    };
    if !bundle.is_dir() {
        bail!("Bundle directory not found: {}", bundle.display());
    }
    let bundle = fsx::absolute(bundle)?;
    let output = fsx::absolute(output)?;
    if output.exists() {
        bail!(
            "Local release output must not already exist: {}",
            output.display()
        );
    }

    let required: Vec<PathBuf> = vec![
        bundle.join(format!("nsis/Pulse_{version}_x64-setup.exe")),
        bundle.join(format!("msi/Pulse_{version}_x64_en-US.msi")),
        bundle.join("pulse-windows-x64.spdx.json"),
    ];
    for path in &required {
        if !path.is_file() {
            bail!(
                "Required local release asset is missing: {}",
                path.display()
            );
        }
    }

    let parent = output
        .parent()
        .context("Temporary release path must remain beside the explicit output directory")?;
    let temporary = parent.join(format!(
        "{}.tmp-{}",
        fsx::file_name(&output),
        fsx::unique_suffix()
    ));
    fs::create_dir_all(&temporary).with_context(|| format!("create {}", temporary.display()))?;
    let _cleanup = RemoveOnDrop(temporary.clone());

    for path in &required {
        let destination = temporary.join(fsx::file_name(path));
        fs::copy(path, &destination)
            .with_context(|| format!("copy {} to {}", path.display(), destination.display()))?;
    }
    fsx::write_sha256sums(&temporary)?;
    fs::rename(&temporary, &output)
        .with_context(|| format!("move {} to {}", temporary.display(), output.display()))?;

    Ok(format!(
        "Local release assets prepared: version={version} files=4 output={}",
        output.display()
    ))
}
