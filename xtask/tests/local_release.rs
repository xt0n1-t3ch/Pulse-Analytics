//! The Windows-only recovery script stays PowerShell; its structure is checked everywhere
//! and its refusal to run without `-WindowsOnlyRecovery` is exercised on Windows.

mod common;

use common::{read, repository_root};

fn script() -> String {
    read(repository_root().join("scripts/release-local.ps1"))
}

#[test]
fn local_release_delegates_checks_and_assets_to_xtask() {
    let source = script();

    for command in [
        "cargo xtask release-contract",
        "cargo xtask upstream check",
        "cargo xtask sbom new",
        "cargo xtask sbom check",
        "cargo xtask local-release-assets",
        "cargo xtask changelog notes",
    ] {
        assert!(source.contains(command), "{command}");
    }
    for removed in [
        "npm run",
        "python",
        "check-release-contract.ps1",
        "new-windows-sbom.ps1",
        ".py",
    ] {
        assert!(!source.contains(removed), "{removed}");
    }
    assert!(source.contains("bun run build"));
}

#[test]
fn local_release_requires_explicit_windows_recovery_before_external_commands() {
    let source = script();
    let gate = source
        .find("if (-not $WindowsOnlyRecovery)")
        .expect("recovery gate");
    let remote = source
        .find("$repository = Get-RequiredRepository")
        .expect("remote preflight");
    assert!(gate < remote);
    assert!(source.contains(r#""--draft", "--verify-tag", "--latest=false""#));
    assert!(source.contains("gh release edit $Tag --draft=false --latest=false"));
    assert!(source.contains("Windows x64 recovery"));
}

#[test]
fn local_release_uploads_a_draft_and_publishes_only_after_the_bytes_verify() {
    let source = script();
    assert!(source.contains("Get-ReleaseHttpStatus"));
    assert!(source.contains("Assert-RemoteAnnotatedTag"));
    assert!(source.contains("Remove-FailedRelease"));
    assert!(source.contains("--draft"));
    assert!(source.contains("--verify-tag"));
    assert!(source.contains("gh release download"));
    assert!(source.contains("Published asset hash mismatch"));
    assert!(source.contains("GitHub release $Tag is public but not immutable"));
    assert!(!source.contains("Pulse_*_x64"));

    let sbom_gate = source
        .find("cargo xtask sbom check")
        .expect("local release must validate SPDX");
    let draft_create = source
        .find("gh @arguments")
        .expect("local release must create a draft");
    let download = source
        .find("gh release download")
        .expect("local release must download its assets");
    let publish = source
        .find("gh release edit $Tag --draft=false")
        .expect("local release must finalize the draft");
    let immutable = source
        .find("$immutable = ($immutableOutput")
        .expect("local release must verify immutable state");
    let finalized = source
        .find("$releaseFinalized = $true")
        .expect("local release must record finalization");
    assert!(sbom_gate < draft_create);
    assert!(draft_create < download && download < publish);
    assert!(publish < immutable && immutable < finalized);
}

#[cfg(windows)]
fn powershell_on_path() -> std::path::PathBuf {
    let path = std::env::var_os("PATH").expect("PATH must be available to locate PowerShell");
    for directory in std::env::split_paths(&path) {
        for name in ["pwsh.exe", "powershell.exe"] {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    panic!("neither pwsh nor powershell was found on PATH");
}

#[cfg(windows)]
#[test]
fn local_release_refuses_to_run_without_the_recovery_flag() {
    let output = std::process::Command::new(powershell_on_path())
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(repository_root().join("scripts/release-local.ps1"))
        .env("PATH", "")
        .output()
        .expect("run the local release gate without network tools");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success());
    assert!(text.contains("-WindowsOnlyRecovery"), "{text}");
}
