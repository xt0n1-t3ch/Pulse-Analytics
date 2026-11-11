//! Release-workflow steps that must behave the same on Windows, macOS and Linux runners,
//! so they live here instead of in per-OS shell snippets.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::fsx;

const VERIFICATION_CONFIG: &str = "src-tauri/.build/platform-verification.json";

/// The `args` output of the "Prepare bundle arguments" step: the Tauri bundle arguments for
/// `target`. Verification builds (no publication) turn updater artifacts off through a scratch
/// config under `src-tauri/.build`, because they have no signing key.
pub fn bundle_args(
    root: &Path,
    target: &str,
    bundles: &str,
    publish_release: bool,
) -> Result<String> {
    let mut arguments = format!("--target {target} --bundles {bundles}");
    if !publish_release {
        fsx::write_text(
            &root.join(VERIFICATION_CONFIG),
            "{\"bundle\":{\"createUpdaterArtifacts\":false}}\n",
        )?;
        arguments.push_str(" --config .build/platform-verification.json");
    }
    Ok(format!("args={arguments} -- --locked"))
}

/// Appends `line` to the file named by `GITHUB_OUTPUT`; without it (a local run) prints it.
pub fn write_github_output(line: &str) -> Result<()> {
    let Some(path) = std::env::var_os("GITHUB_OUTPUT") else {
        println!("{line}");
        return Ok(());
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open GITHUB_OUTPUT {}", Path::new(&path).display()))?;
    writeln!(file, "{line}").context("write GITHUB_OUTPUT")
}

/// Host triple from `rustc -vV` output.
pub fn host_triple(version_output: &str) -> Option<&str> {
    version_output
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::trim)
}

fn run_cargo(root: &Path, arguments: &[&str], failure: &str) -> Result<()> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(arguments)
        .current_dir(root)
        .status()
        .with_context(|| format!("failed to start cargo {}", arguments.join(" ")))?;
    if !status.success() {
        bail!("{failure}");
    }
    Ok(())
}

/// Whether a native job lints before it tests.
///
/// The local gate (`cargo xtask check`) already lints the Linux host and the Windows
/// target, so the release lints only where no local run reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeLint {
    /// Run the tests only.
    Skip,
    /// Run warning-denying Clippy, then the tests.
    Clippy,
}

/// The cargo invocations of one native verification, in order.
///
/// The tests run in the release profile, the profile of the bundle that ships. The bundle
/// build that follows then reuses their dependency artifacts instead of compiling every
/// crate a second time (measured on Linux x64, 2026-11-10: 25 of 400 crates recompiled).
pub fn native_verification_commands(target: &str, lint: NativeLint) -> Vec<Vec<&str>> {
    let mut commands = Vec::new();
    if lint == NativeLint::Clippy {
        commands.push(vec![
            "clippy",
            "--locked",
            "--workspace",
            "--all-targets",
            "--target",
            target,
            "--",
            "-D",
            "warnings",
        ]);
    }
    commands.push(vec![
        "test",
        "--release",
        "--locked",
        "--workspace",
        "--target",
        target,
    ]);
    commands
}

/// Proves the runner is native for `target`, then runs the tests for it, after Clippy
/// when `lint` asks for it.
pub fn verify_native(root: &Path, target: &str, lint: NativeLint) -> Result<String> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(rustc)
        .arg("-vV")
        .output()
        .context("Cannot identify Rust host")?;
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(host) = host_triple(&text) else {
        bail!("Cannot identify Rust host");
    };
    if host != target {
        bail!("Expected native {target} runner, got {host}");
    }
    for arguments in native_verification_commands(target, lint) {
        let failure = format!("Native cargo {} failed", arguments[0]);
        run_cargo(root, &arguments, &failure)?;
    }
    Ok(format!("Native verification passed for {target}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_triple_reads_the_rustc_host_line() {
        let text = "rustc 1.99.1 (abc 2026-10-01)\nbinary: rustc\nhost: x86_64-unknown-linux-gnu\nrelease: 1.99.1\n";
        assert_eq!(host_triple(text), Some("x86_64-unknown-linux-gnu"));
        assert_eq!(host_triple("no host here"), None);
    }
}
