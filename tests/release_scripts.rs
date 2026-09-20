use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const LEGACY_VENDORED_FILES: [&str; 10] = [
    "config.rs",
    "cost.rs",
    "discord.rs",
    "session.rs",
    "util.rs",
    "session/activity.rs",
    "session/parser.rs",
    "telemetry/limits.rs",
    "telemetry/plan.rs",
    "telemetry/service_tier.rs",
];
const VENDORED_FILES: [&str; 14] = [
    "app.rs",
    "config.rs",
    "cost.rs",
    "discord.rs",
    "model.rs",
    "model_catalog.json",
    "process_guard.rs",
    "session.rs",
    "util.rs",
    "session/activity.rs",
    "session/parser.rs",
    "telemetry/limits.rs",
    "telemetry/plan.rs",
    "telemetry/service_tier.rs",
];

#[test]
fn vendoring_integrity_is_offline_and_accepts_matching_hashes() {
    let fixture = manifest_fixture(EMPTY_SHA256);

    let mut command = script_command("check-codex-rich-presence-upstream.ps1");
    command.arg("-Root").arg(fixture.path()).env("PATH", "");
    let output = command.output().expect("run integrity checker");

    assert_success(&output);
    assert!(stdout(&output).contains("integrity verified"));
}

#[test]
fn vendoring_integrity_rejects_malformed_manifest() {
    let fixture = TempDir::new().expect("fixture");
    write(fixture.path().join("src/codex/UPSTREAM.json"), "{oops");

    let output = script_command("check-codex-rich-presence-upstream.ps1")
        .arg("-Root")
        .arg(fixture.path())
        .output()
        .expect("run integrity checker");

    assert_failure_contains(&output, "not valid JSON");
}

#[test]
fn vendoring_integrity_rejects_legacy_manifest_without_hash_contract() {
    let fixture = TempDir::new().expect("fixture");
    write(
        fixture.path().join("src/codex/UPSTREAM.json"),
        r#"{"repository":"https://github.com/xt0n1-t3ch/Codex-Discord-Rich-Presence","branch":"main","commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
    );

    let output = script_command("check-codex-rich-presence-upstream.ps1")
        .arg("-Root")
        .arg(fixture.path())
        .output()
        .expect("run integrity checker");

    assert_failure_contains(&output, "schema_version 2 and sync_version 2");
}

#[test]
fn vendoring_integrity_rejects_mismatched_file_hash() {
    let fixture = manifest_fixture(&"0".repeat(64));

    let output = script_command("check-codex-rich-presence-upstream.ps1")
        .arg("-Root")
        .arg(fixture.path())
        .output()
        .expect("run integrity checker");

    assert_failure_contains(&output, "hash mismatch");
}

#[test]
fn vendoring_integrity_rejects_self_declared_one_file_inventory() {
    let fixture = one_file_manifest_fixture();

    let output = script_command("check-codex-rich-presence-upstream.ps1")
        .arg("-Root")
        .arg(fixture.path())
        .output()
        .expect("run integrity checker");

    assert_failure_contains(&output, "expected inventory");
}

#[test]
fn vendoring_contract_versions_inventory_and_owns_exact_adapters() {
    let contract = read(repository_root().join("scripts/codex-vendor-contract.json"));

    assert!(contract.contains("\"legacy-v1\""));
    assert!(contract.contains("\"model-catalog-v2\""));
    assert!(contract.contains("\"model-catalog-v3\""));
    assert!(contract.contains("\"src/app.rs\""));
    assert!(contract.contains("\"src/process_guard.rs\""));
    assert!(contract.contains("\"src/model_catalog.json\""));
    assert!(contract.contains("\"mode\": \"byte-copy\""));
    assert_eq!(contract.matches("src/codex/process.rs").count(), 1);
    assert_eq!(contract.matches("src/codex/mod.rs").count(), 1);
}

#[test]
fn vendored_windows_polling_commands_use_silent_launcher() {
    let root = repository_root();
    let manifest = read(root.join("src/codex/UPSTREAM.json"));
    let parser = read(root.join("src/codex/session/parser.rs"));

    assert!(
        manifest.contains(r#""schema_version": 3"#)
            && manifest.contains(r#""canonical_release": "v1.11.2""#)
            && manifest
                .contains(r#""canonical_commit": "0caece60a71eb657d7b829e3b6c7c4896a0a536c""#)
            && manifest.contains(r#""mode": "git-rev""#)
            && manifest.contains(r#""rev": "0caece60a71eb657d7b829e3b6c7c4896a0a536c""#)
            && manifest.contains(r#""version": "2.0.1""#)
            && manifest.contains(r#""package": "codex-presence-core""#),
        "Pulse must declare the promoted immutable core contract"
    );
    assert!(
        !parser.contains(r#"Command::new("git")"#),
        "the five-second Git branch probe must not create a visible Windows console"
    );
    assert_eq!(
        parser
            .matches(r#"crate::codex::util::silent_command("git")"#)
            .count(),
        2,
        "both branch and detached-HEAD probes must use the shared silent launcher"
    );
}

#[test]
fn posix_vendoring_entrypoints_delegate_to_the_powershell_contract() {
    for script in [
        "scripts/check-codex-rich-presence-upstream.sh",
        "scripts/update-codex-rich-presence.sh",
    ] {
        let source = read(repository_root().join(script));
        assert!(source.contains("exec pwsh"), "{script}");
        assert!(!source.contains("git ls-remote"), "{script}");
        assert!(!source.contains("python3"), "{script}");
    }
}

#[test]
fn updater_syncs_explicit_tag_and_commit_without_touching_pulse_adapters() {
    let canonical = canonical_fixture();
    let commit = git_output(canonical.path(), ["rev-parse", "HEAD"]);
    let pulse = pulse_fixture(true);
    let process_before = fs::read(pulse.path().join("src/codex/process.rs")).expect("process");
    let module_before = fs::read(pulse.path().join("src/codex/mod.rs")).expect("module");

    let output = update_command(canonical.path(), pulse.path(), "v1.7.2", &commit)
        .output()
        .expect("run updater");

    assert_success(&output);
    assert_eq!(
        fs::read(pulse.path().join("src/codex/process.rs")).expect("process"),
        process_before
    );
    assert_eq!(
        fs::read(pulse.path().join("src/codex/mod.rs")).expect("module"),
        module_before
    );

    let cost = read(pulse.path().join("src/codex/cost.rs"));
    assert_eq!(cost.matches("pub fn speed_multiplier").count(), 1);
    assert!(cost.contains("crate::codex::config"));
    let formatted = Command::new("rustfmt")
        .arg("--edition=2024")
        .arg("--check")
        .arg(pulse.path().join("src/codex/cost.rs"))
        .output()
        .expect("check synced Rust formatting");
    assert_success(&formatted);
    assert_eq!(
        fs::read(canonical.path().join("src/model_catalog.json")).expect("canonical catalog"),
        fs::read(pulse.path().join("src/codex/model_catalog.json")).expect("vendored catalog")
    );

    let manifest = read(pulse.path().join("src/codex/UPSTREAM.json"));
    assert!(manifest.contains("\"schema_version\": 2"));
    assert!(manifest.contains("\"ref\": \"v1.7.2\""));
    assert!(manifest.contains(&commit));
    assert_eq!(manifest.matches("\"source_sha256\"").count(), 14);
    assert_eq!(manifest.matches("\"target_sha256\"").count(), 14);
    assert!(manifest.contains("\"local_adapters\""));
    assert!(manifest.contains("\"provenance\": \"test\""));

    let integrity = script_command("check-codex-rich-presence-upstream.ps1")
        .arg("-Root")
        .arg(pulse.path())
        .arg("-TestMode")
        .output()
        .expect("run integrity checker");
    assert_success(&integrity);

    let compiled = Command::new("rustc")
        .arg("--edition=2024")
        .arg("--crate-name=pulse_vendor_fixture")
        .arg("--crate-type=lib")
        .arg(pulse.path().join("src/lib.rs"))
        .arg("-o")
        .arg(pulse.path().join("pulse_vendor_fixture.rlib"))
        .output()
        .expect("compile synced fixture");
    assert_success(&compiled);
}

#[test]
fn updater_rejects_missing_process_compatibility_before_writing() {
    let canonical = canonical_fixture();
    let commit = git_output(canonical.path(), ["rev-parse", "HEAD"]);
    let pulse = pulse_fixture(false);
    let sentinel = pulse.path().join("src/codex/config.rs");
    write(&sentinel, "keep me\n");

    let output = update_command(canonical.path(), pulse.path(), "v1.7.2", &commit)
        .output()
        .expect("run updater");

    assert_failure_contains(&output, "is_desktop_surface_running");
    assert_eq!(read(sentinel), "keep me\n");
}

#[test]
fn updater_rejects_mutable_or_mismatched_refs() {
    let canonical = canonical_fixture();
    let commit = git_output(canonical.path(), ["rev-parse", "HEAD"]);
    let pulse = pulse_fixture(true);

    let malformed = update_command(canonical.path(), pulse.path(), "main", &commit)
        .output()
        .expect("run updater");
    assert_failure_contains(&malformed, "semantic version tag");

    let mismatch = update_command(canonical.path(), pulse.path(), "v1.7.2", &"0".repeat(40))
        .output()
        .expect("run updater");
    assert_failure_contains(&mismatch, "does not resolve to commit");
}

#[test]
fn updater_rejects_lightweight_tags() {
    let canonical = canonical_fixture_with_tag(false);
    let commit = git_output(canonical.path(), ["rev-parse", "HEAD"]);
    let pulse = pulse_fixture(true);

    let output = update_command(canonical.path(), pulse.path(), "v1.7.2", &commit)
        .output()
        .expect("run updater");

    assert_failure_contains(&output, "annotated tag");
}

#[test]
fn release_contract_accepts_stable_and_prerelease_tags() {
    let stable = release_fixture("1.5.2");
    let stable_commit = git_output(stable.path(), ["rev-parse", "HEAD"]);
    let stable_output = release_contract_command(stable.path(), "v1.5.2", &stable_commit)
        .output()
        .expect("run release check");
    assert_success(&stable_output);

    let prerelease = release_fixture("1.5.3-rc.1");
    let prerelease_commit = git_output(prerelease.path(), ["rev-parse", "HEAD"]);
    let prerelease_output =
        release_contract_command(prerelease.path(), "v1.5.3-rc.1", &prerelease_commit)
            .output()
            .expect("run release check");
    assert_success(&prerelease_output);
}

#[test]
fn release_contract_rejects_malformed_or_mismatched_tags() {
    let fixture = release_fixture("1.5.2");
    let commit = git_output(fixture.path(), ["rev-parse", "HEAD"]);

    let malformed = release_contract_command(fixture.path(), "release-1.5.2", &commit)
        .output()
        .expect("run release check");
    assert_failure_contains(&malformed, "semantic version tag");

    for tag in ["v01.5.2", "v1.5.2-rc..1"] {
        let malformed = release_contract_command(fixture.path(), tag, &commit)
            .output()
            .expect("run release check");
        assert_failure_contains(&malformed, "semantic version tag");
    }

    let mismatch = release_contract_command(fixture.path(), "v1.5.3", &commit)
        .output()
        .expect("run release check");
    assert_failure_contains(&mismatch, "version mismatch");
}

#[test]
fn release_contract_requires_annotated_main_reachable_tag_and_expected_commit() {
    let lightweight = release_fixture_with_tag("1.5.2", false, true);
    let lightweight_commit = git_output(lightweight.path(), ["rev-parse", "HEAD"]);
    let lightweight_output =
        release_contract_command(lightweight.path(), "v1.5.2", &lightweight_commit)
            .output()
            .expect("run release check");
    assert_failure_contains(&lightweight_output, "annotated tag");

    let unreachable = release_fixture_with_tag("1.5.2", true, false);
    let tagged_commit = git_output(unreachable.path(), ["rev-list", "-n", "1", "v1.5.2"]);
    let unreachable_output = release_contract_command(unreachable.path(), "v1.5.2", &tagged_commit)
        .output()
        .expect("run release check");
    assert_failure_contains(&unreachable_output, "not reachable");
}

#[test]
fn release_contract_validates_readme_and_docs_version_surfaces() {
    let fixture = release_fixture("1.5.2");
    let commit = git_output(fixture.path(), ["rev-parse", "HEAD"]);
    write(fixture.path().join("README.md"), "Download v1.5.1\n");

    let output = release_contract_command(fixture.path(), "v1.5.2", &commit)
        .output()
        .expect("run release check");

    assert_failure_contains(&output, "README.md");
}

#[test]
fn release_assets_reject_empty_input_and_checksum_nonempty_input() {
    let fixture = TempDir::new().expect("fixture");
    let artifacts = fixture.path().join("artifacts");
    let output = fixture.path().join("release-assets");
    fs::create_dir_all(&artifacts).expect("artifacts");

    let empty = release_assets_command(&artifacts, &output)
        .output()
        .expect("run asset collector");
    assert_failure_contains(&empty, "No release assets");

    write(
        artifacts.join("pulse-windows-x64/pulse-windows-x64-Pulse.exe"),
        "exe",
    );
    let collected = release_assets_command(&artifacts, &output)
        .output()
        .expect("run asset collector");
    assert_failure_contains(&collected, "Missing required release asset");

    write_complete_platform_artifacts(&artifacts);
    let collected = release_assets_command(&artifacts, &output)
        .output()
        .expect("run asset collector");
    assert_success(&collected);
    assert!(output.join("pulse-windows-x64-Pulse.exe").is_file());
    assert!(output.join("pulse-windows-x64.spdx.json").is_file());
    let sums = read(output.join("SHA256SUMS.txt"));
    assert!(sums.contains("  pulse-windows-x64-Pulse.exe\n"));
    assert!(sums.contains("  pulse-windows-x64.spdx.json\n"));
}

#[test]
fn release_assets_publish_a_signed_updater_manifest_for_every_platform() {
    let fixture = TempDir::new().expect("fixture");
    let artifacts = fixture.path().join("artifacts");
    let output = fixture.path().join("release-assets");
    write_complete_platform_artifacts(&artifacts);

    let collected = release_assets_command(&artifacts, &output)
        .output()
        .expect("run asset collector");

    assert_success(&collected);
    let manifest = read(output.join("latest.json"));
    assert!(manifest.contains(r#""version": "1.6.1""#), "{manifest}");
    for (target, payload) in [
        ("windows-x86_64", "pulse-windows-x64-Pulse.exe"),
        ("windows-aarch64", "pulse-windows-arm64-Pulse.exe"),
        ("darwin-aarch64", "pulse-macos-arm64-Pulse.app.tar.gz"),
        ("darwin-x86_64", "pulse-macos-x64-Pulse.app.tar.gz"),
        ("linux-x86_64", "pulse-linux-x64-Pulse.AppImage"),
        ("linux-aarch64", "pulse-linux-arm64-Pulse.AppImage"),
    ] {
        assert!(manifest.contains(target), "{target} missing:\n{manifest}");
        assert!(
            manifest.contains(&format!(
                "https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/releases/download/v1.6.1/{payload}"
            )),
            "{payload} download URL missing:\n{manifest}"
        );
        assert!(output.join(payload).is_file(), "{payload} not published");
    }
    assert!(manifest.contains("signature-for-windows"), "{manifest}");

    // The manifest itself must be covered by the checksum file it ships with.
    let sums = read(output.join("SHA256SUMS.txt"));
    assert!(sums.contains("  latest.json\n"), "{sums}");
    assert_eq!(sums.lines().count(), 23, "{sums}");
}

#[test]
fn local_release_assets_select_only_the_exact_tag_version() {
    let fixture = TempDir::new().expect("fixture");
    let bundle = fixture.path().join("bundle");
    let output = fixture.path().join("publication/v1.7.2");
    write(bundle.join("nsis/Pulse_1.7.1_x64-setup.exe"), "stale exe");
    write(bundle.join("msi/Pulse_1.7.1_x64_en-US.msi"), "stale msi");
    write(bundle.join("nsis/Pulse_1.7.2_x64-setup.exe"), "current exe");
    write(bundle.join("msi/Pulse_1.7.2_x64_en-US.msi"), "current msi");
    write(
        bundle.join("pulse-windows-x64.spdx.json"),
        "{\"spdxVersion\":\"SPDX-2.3\"}",
    );

    let collected = local_release_assets_command(&bundle, &output, "v1.7.2")
        .output()
        .expect("run local asset collector");

    assert_success(&collected);
    let names = fs::read_dir(&output)
        .expect("release output")
        .map(|entry| entry.expect("release asset").file_name())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 4, "unexpected release assets: {names:?}");
    assert!(output.join("Pulse_1.7.2_x64-setup.exe").is_file());
    assert!(output.join("Pulse_1.7.2_x64_en-US.msi").is_file());
    assert!(output.join("pulse-windows-x64.spdx.json").is_file());
    assert!(!output.join("Pulse_1.7.1_x64-setup.exe").exists());
    let sums = read(output.join("SHA256SUMS.txt"));
    assert_eq!(sums.lines().count(), 3, "{sums}");
    assert!(sums.contains("  Pulse_1.7.2_x64-setup.exe\n"), "{sums}");
    assert!(sums.contains("  Pulse_1.7.2_x64_en-US.msi\n"), "{sums}");
    assert!(sums.contains("  pulse-windows-x64.spdx.json\n"), "{sums}");
}

#[test]
fn local_release_assets_reject_stale_installers_when_exact_version_is_missing() {
    let fixture = TempDir::new().expect("fixture");
    let bundle = fixture.path().join("bundle");
    let output = fixture.path().join("release-assets");
    write(bundle.join("nsis/Pulse_1.7.1_x64-setup.exe"), "stale exe");
    write(bundle.join("msi/Pulse_1.7.1_x64_en-US.msi"), "stale msi");
    write(
        bundle.join("pulse-windows-x64.spdx.json"),
        "{\"spdxVersion\":\"SPDX-2.3\"}",
    );

    let collected = local_release_assets_command(&bundle, &output, "v1.7.2")
        .output()
        .expect("run local asset collector");

    assert_failure_contains(&collected, "Pulse_1.7.2_x64-setup.exe");
    assert!(
        !output.exists(),
        "a failed collection must not promote output"
    );
}

#[test]
fn local_release_assets_preserve_preexisting_output_byte_for_byte() {
    let fixture = TempDir::new().expect("fixture");
    let bundle = fixture.path().join("bundle");
    let output = fixture.path().join("release-assets");
    write(bundle.join("nsis/Pulse_1.7.2_x64-setup.exe"), "current exe");
    write(bundle.join("msi/Pulse_1.7.2_x64_en-US.msi"), "current msi");
    write(
        bundle.join("pulse-windows-x64.spdx.json"),
        "{\"spdxVersion\":\"SPDX-2.3\"}",
    );
    let sentinel = output.join("test-results/sentinel.bin");
    write(&sentinel, "preserve me exactly");

    let collected = local_release_assets_command(&bundle, &output, "v1.7.2")
        .output()
        .expect("run local asset collector");

    assert_failure_contains(&collected, "must not already exist");
    assert_eq!(read(sentinel), "preserve me exactly");
    assert_eq!(fs::read_dir(&output).expect("output").count(), 1);
}

#[test]
fn local_release_assets_use_the_strict_release_semver_contract() {
    let fixture = TempDir::new().expect("fixture");
    let bundle = fixture.path().join("bundle");
    fs::create_dir_all(&bundle).expect("bundle");

    for tag in ["v1.7.2-01", "v1.7.2-rc..1", "v1.7.2-.beta"] {
        let output = fixture.path().join(tag.replace('.', "_"));
        let collected = local_release_assets_command(&bundle, &output, tag)
            .output()
            .expect("run local asset collector");
        assert_failure_contains(&collected, "semantic version tag");
        assert!(!output.exists(), "invalid tag promoted output: {tag}");
    }
}

#[test]
fn windows_sbom_validator_accepts_complete_spdx_and_rejects_bad_integrity() {
    let fixture = TempDir::new().expect("fixture");
    let artifact = fixture.path().join("Pulse_1.7.2_x64-setup.exe");
    let sbom = fixture.path().join("pulse-windows-x64.spdx.json");
    write(&artifact, "current exe");
    write(
        &sbom,
        r#"{
  "spdxVersion": "SPDX-2.3",
  "dataLicense": "CC0-1.0",
  "packages": [
    {"name": "pulse", "versionInfo": "1.7.2"},
    {"name": "dependency", "versionInfo": "1.0.0"}
  ],
  "files": [{
    "fileName": "Pulse_1.7.2_x64-setup.exe",
    "checksums": [{
      "algorithm": "SHA256",
      "checksumValue": "b04c26dc8979dba91381554488a4c8fdc93bf8e0c01a4ad009f301402b758610"
    }]
  }]
}"#,
    );

    let valid = windows_sbom_command(&artifact, &sbom)
        .output()
        .expect("validate SPDX");
    assert_success(&valid);

    let malformed = read(&sbom).replace("CC0-1.0", "NOASSERTION");
    write(&sbom, &malformed);
    let invalid = windows_sbom_command(&artifact, &sbom)
        .output()
        .expect("reject SPDX");
    assert_failure_contains(&invalid, "SPDX 2.3 JSON document under CC0-1.0");
}

#[test]
fn release_assets_reject_a_platform_without_its_updater_signature() {
    let fixture = TempDir::new().expect("fixture");
    let artifacts = fixture.path().join("artifacts");
    let output = fixture.path().join("release-assets");
    write_complete_platform_artifacts(&artifacts);
    fs::remove_file(artifacts.join("pulse-linux-x64/pulse-linux-x64-Pulse.AppImage.sig"))
        .expect("drop the Linux updater signature");

    let collected = release_assets_command(&artifacts, &output)
        .output()
        .expect("run asset collector");

    assert_failure_contains(&collected, ".AppImage.sig");
}

/// Writes one complete, signed artifact set so updater assertions start from a
/// releasable state instead of rebuilding the fixture in each test.
fn write_complete_platform_artifacts(artifacts: &Path) {
    for platform in ["windows-x64", "windows-arm64"] {
        write(
            artifacts.join(format!("pulse-{platform}/pulse-{platform}-Pulse.exe")),
            "exe",
        );
        write(
            artifacts.join(format!("pulse-{platform}/pulse-{platform}-Pulse.exe.sig")),
            &format!("signature-for-{platform}"),
        );
        write(
            artifacts.join(format!("pulse-{platform}/pulse-{platform}-Pulse.msi")),
            "msi",
        );
        write(
            artifacts.join(format!("pulse-{platform}/pulse-{platform}.spdx.json")),
            r#"{"spdxVersion":"SPDX-2.3"}"#,
        );
    }
    for platform in ["macos-arm64", "macos-x64"] {
        write(
            artifacts.join(format!("pulse-{platform}/pulse-{platform}-Pulse.dmg")),
            "dmg",
        );
        write(
            artifacts.join(format!(
                "pulse-{platform}/pulse-{platform}-Pulse.app.tar.gz"
            )),
            "app",
        );
        write(
            artifacts.join(format!(
                "pulse-{platform}/pulse-{platform}-Pulse.app.tar.gz.sig"
            )),
            &format!("signature-for-{platform}"),
        );
    }
    for platform in ["linux-x64", "linux-arm64"] {
        for extension in ["deb", "rpm", "AppImage"] {
            write(
                artifacts.join(format!(
                    "pulse-{platform}/pulse-{platform}-Pulse.{extension}"
                )),
                extension,
            );
        }
        write(
            artifacts.join(format!(
                "pulse-{platform}/pulse-{platform}-Pulse.AppImage.sig"
            )),
            &format!("signature-for-{platform}"),
        );
    }
}

#[test]
fn platform_asset_collection_prevents_same_basename_mac_collisions() {
    let fixture = TempDir::new().expect("fixture");
    let artifacts = fixture.path().join("artifacts");
    for platform in ["macos-arm64", "macos-x64"] {
        let input = fixture.path().join(platform);
        write(input.join("Pulse.app.tar.gz"), platform);
        write(input.join("Pulse.app.tar.gz.sig"), platform);
        write(input.join("Pulse.dmg"), platform);

        let output = platform_assets_command(&input, &artifacts, platform)
            .output()
            .expect("collect platform assets");
        assert_success(&output);
    }

    assert!(
        artifacts
            .join("pulse-macos-arm64-Pulse.app.tar.gz")
            .is_file()
    );
    assert!(artifacts.join("pulse-macos-x64-Pulse.app.tar.gz").is_file());
    assert!(
        artifacts
            .join("pulse-macos-arm64-Pulse.app.tar.gz.sig")
            .is_file(),
        "the updater signature must travel with its payload"
    );
}

#[test]
fn manual_release_workflow_pins_actions_and_gates_publication_on_preflight() {
    let root = repository_root();
    let release = read(root.join(".github/workflows/release.yml"));
    let local_release = read(root.join("scripts/release-local.ps1"));
    let freshness = read(root.join(".github/workflows/upstream-freshness.yml"));
    let audit = read(root.join("scripts/audit-rust.ps1"));
    let combined = format!("{release}\n{freshness}");

    assert!(release.contains("workflow_dispatch:"));
    assert!(release.contains("Existing annotated release tag"));
    assert_eq!(release.matches("ref: ${{ inputs.tag }}").count(), 1);
    assert!(
        release
            .matches("ref: ${{ needs.preflight.outputs.tag_commit }}")
            .count()
            >= 2
    );
    assert!(!release.contains("tags: [\"v*.*.*\"]"));
    assert!(release.contains("needs: [preflight, build]"));
    assert!(!release.contains("--no-run"));
    assert!(release.contains("cargo test --locked --workspace"));
    assert!(release.contains("cargo clippy --locked --workspace --all-targets -- -D warnings"));
    assert!(release.contains("contents: write"));
    assert!(release.contains("cancel-in-progress: false"));
    assert!(release.contains("immutable-releases"));
    assert!(!release.contains("gh run list --workflow ci.yml"));
    assert!(release.contains("trap cleanup_draft EXIT INT TERM"));
    assert!(release.contains("scripts/audit-rust.ps1"));
    assert!(audit.contains(r#"@("audit", "--deny", "warnings")"#));
    assert!(audit.contains("rustsec-accepted-warnings.json"));
    assert!(release.contains("check-release-contract.ps1"));
    assert!(release.contains("tag_commit: ${{ steps.release_target.outputs.tag_commit }}"));
    assert!(release.contains("tag_commit=$tagCommit"));
    assert!(release.contains("EXPECTED_TAG_COMMIT: ${{ needs.preflight.outputs.tag_commit }}"));
    assert!(release.contains("Release tag moved after preflight"));
    let fetch_tag = release
        .find("Fetch annotated release tag")
        .expect("release workflow must fetch the remote tag object");
    let validate_tag = release
        .find("Validate annotated release target")
        .expect("release workflow must validate the fetched tag");
    assert!(fetch_tag < validate_tag);
    assert!(release.contains(
        "git fetch --force origin \"refs/tags/${env:RELEASE_TAG}:refs/tags/${env:RELEASE_TAG}\""
    ));
    assert!(release.contains("gh release view \"$TAG\" --json databaseId --jq .databaseId"));
    assert!(
        !release.contains("gh api \"repos/${GITHUB_REPOSITORY}/releases/tags/${TAG}\" --jq .id")
    );
    assert!(release.contains("check-codex-rich-presence-upstream.ps1"));
    assert!(release.contains("NPM_VERSION: \"11.10.1\""));
    assert!(release.contains("No release assets"));
    assert!(release.contains("new-windows-sbom.ps1"));
    assert!(release.contains("check-windows-sbom.ps1"));
    assert!(release.contains(
        "- name: Generate and verify Windows SPDX SBOM\n        if: runner.os == 'Windows'\n        shell: pwsh\n        env:\n          PLATFORM: ${{ matrix.label }}"
    ));
    assert!(local_release.contains("prepare-local-release-assets.ps1"));
    assert!(local_release.contains("new-windows-sbom.ps1"));
    assert!(local_release.contains("check-windows-sbom.ps1"));
    assert!(local_release.contains("check-codex-rich-presence-upstream.ps1"));
    assert!(local_release.contains("Get-ReleaseHttpStatus"));
    assert!(local_release.contains("Assert-RemoteAnnotatedTag"));
    assert!(local_release.contains("Remove-FailedRelease"));
    assert!(local_release.contains("extract-changelog-section.py"));
    assert!(local_release.contains("--draft"));
    assert!(local_release.contains("--verify-tag"));
    assert!(local_release.contains("gh release download"));
    assert!(local_release.contains("Published asset hash mismatch"));
    assert!(local_release.contains("GitHub release $Tag is public but not immutable"));
    assert!(!local_release.contains("Pulse_*_x64"));
    let sbom_gate = local_release
        .find("check-windows-sbom.ps1")
        .expect("local release must validate SPDX");
    let draft_create = local_release
        .find("gh @arguments")
        .expect("local release must create a draft");
    let download = local_release
        .find("gh release download")
        .expect("local release must download its assets");
    let publish = local_release
        .find("gh release edit $Tag --draft=false")
        .expect("local release must finalize the draft");
    let immutable = local_release
        .find("$immutable = ($immutableOutput")
        .expect("local release must verify immutable state");
    let finalized = local_release
        .find("$releaseFinalized = $true")
        .expect("local release must record finalization");
    assert!(sbom_gate < draft_create);
    assert!(draft_create < download && download < publish);
    assert!(publish < immutable && immutable < finalized);
    assert!(
        release.contains("Verify updater manifest targets every platform"),
        "the release must prove latest.json covers every updater target"
    );
    assert!(release.contains("release-assets/latest.json"));
    assert!(freshness.contains("workflow_dispatch:"));
    assert!(freshness.contains("issues: write"));

    for line in combined.lines().map(str::trim) {
        if let Some(action) = line.strip_prefix("- uses: ") {
            let revision = action.rsplit_once('@').expect("action revision").1;
            assert_eq!(revision.len(), 40, "action is not pinned: {action}");
            assert!(
                revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "action is not pinned: {action}"
            );
        }
    }
}

#[test]
fn release_matrix_requires_native_arm64_artifacts_for_windows_and_linux() {
    let root = repository_root();
    let workflow = read(root.join(".github/workflows/release.yml"));
    let platform_assets = read(root.join("scripts/release-platform-assets.ps1"));
    let release_assets = read(root.join("scripts/release-assets.ps1"));
    let installer = read(root.join("scripts/install.sh"));
    let windows_installer = read(root.join("scripts/install.ps1"));

    for value in [
        "windows-arm64",
        "windows-11-arm",
        "aarch64-pc-windows-msvc",
        "linux-arm64",
        "ubuntu-22.04-arm",
        "aarch64-unknown-linux-gnu",
    ] {
        assert!(
            workflow.contains(value),
            "release matrix is missing {value}"
        );
    }
    for platform in ["windows-arm64", "linux-arm64"] {
        assert!(
            platform_assets.contains(platform),
            "platform collector is missing {platform}"
        );
        assert!(
            release_assets.contains(platform),
            "release assembler is missing {platform}"
        );
    }
    for updater_target in ["windows-aarch64", "linux-aarch64"] {
        assert!(
            release_assets.contains(updater_target),
            "updater manifest is missing {updater_target}"
        );
    }
    assert!(installer.contains("aarch64"));
    assert!(installer.contains("arm64"));
    assert!(windows_installer.contains("OSArchitecture"));
    assert!(windows_installer.contains("Arm64"));
}

#[test]
fn release_checks_are_native_for_all_six_platforms_and_publish_is_opt_in() {
    let root = repository_root();
    let workflow = read(root.join(".github/workflows/release.yml"));
    let build = workflow
        .split("\n  build:")
        .nth(1)
        .expect("build job")
        .split("\n  publish:")
        .next()
        .expect("build body");
    for (label, runner, target) in [
        ("windows-x64", "windows-2022", "x86_64-pc-windows-msvc"),
        ("windows-arm64", "windows-11-arm", "aarch64-pc-windows-msvc"),
        ("macos-arm64", "macos-latest", "aarch64-apple-darwin"),
        ("macos-x64", "macos-15-intel", "x86_64-apple-darwin"),
        ("linux-x64", "ubuntu-22.04", "x86_64-unknown-linux-gnu"),
        (
            "linux-arm64",
            "ubuntu-22.04-arm",
            "aarch64-unknown-linux-gnu",
        ),
    ] {
        let row =
            format!("- label: {label}\n            os: {runner}\n            target: {target}");
        assert!(build.contains(&row), "missing native matrix entry: {label}");
    }
    assert!(build.contains("$hostTarget -ne $env:TARGET"));
    assert!(build.contains(r#"cargo test --locked --workspace --target "$env:TARGET""#));
    assert!(build.contains(
        r#"cargo clippy --locked --workspace --all-targets --target "$env:TARGET" -- -D warnings"#
    ));
    assert!(workflow.contains("publish_release:"));
    assert!(workflow.contains("type: boolean\n        default: false"));
    let publish = workflow.split("\n  publish:").nth(1).expect("publish job");
    assert!(publish.contains("if: inputs.publish_release"));
    assert!(publish.contains("needs: [preflight, build]"));
    assert!(build.contains("name: frontend-dist"));
    assert!(
        !build.contains("npm run build"),
        "native jobs must consume the shared frontend artifact"
    );
    assert_eq!(workflow.matches("npm run build").count(), 1);
}

#[test]
fn github_macos_packages_require_updater_signatures_not_apple_credentials() {
    let workflow = read(repository_root().join(".github/workflows/release.yml"));
    let gate = workflow
        .find("Require updater signing credentials")
        .expect("updater gate");
    let native_build = workflow.find("Build Tauri bundle").expect("native build");
    assert!(gate < native_build);
    assert!(workflow.contains("TAURI_SIGNING_PRIVATE_KEY"));
    assert!(!workflow.contains("secrets.APPLE_"));
    assert!(workflow.contains("Verify macOS package architecture"));
    assert!(workflow.contains(r#"lipo "$app/Contents/MacOS/$executable" -verify_arch "$arch""#));
    assert!(workflow.contains("no Apple Developer ID signature or notarization"));
    assert!(workflow.contains("createUpdaterArtifacts\":false"));
}

#[test]
fn release_artifact_actions_use_node24_pins_and_repository_retention() {
    let workflow = read(repository_root().join(".github/workflows/release.yml"));
    assert!(workflow.contains("actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a"));
    assert!(
        workflow.contains("actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c")
    );
    assert_eq!(workflow.matches("retention-days: 0").count(), 2);
    assert!(!workflow.contains("retention-days: 7"));
}

#[test]
fn local_release_requires_explicit_windows_recovery_before_external_commands() {
    let output = script_command("release-local.ps1")
        .env("PATH", "")
        .output()
        .expect("run local release gate without network tools");
    assert_failure_contains(&output, "-WindowsOnlyRecovery");
    let source = read(repository_root().join("scripts/release-local.ps1"));
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
fn unsigned_verification_assets_do_not_relax_the_public_release_contract() {
    let fixture = TempDir::new().expect("fixture");
    for (platform, names) in [
        ("windows-x64", vec!["Pulse.exe", "Pulse.msi"]),
        ("windows-arm64", vec!["Pulse.exe", "Pulse.msi"]),
        ("macos-x64", vec!["Pulse.dmg"]),
        ("macos-arm64", vec!["Pulse.dmg"]),
        (
            "linux-x64",
            vec!["Pulse.deb", "Pulse.rpm", "Pulse.AppImage"],
        ),
        (
            "linux-arm64",
            vec!["Pulse.deb", "Pulse.rpm", "Pulse.AppImage"],
        ),
    ] {
        let input = fixture.path().join(format!("input-{platform}"));
        for name in &names {
            write(input.join(name), "unsigned test fixture");
        }
        let strict = platform_assets_command(
            &input,
            &fixture.path().join(format!("strict-{platform}")),
            platform,
        )
        .output()
        .expect("strict collector");
        assert_failure_contains(&strict, "Missing required release asset");
        let dest = fixture.path().join(format!("verify-{platform}"));
        let verified = platform_assets_command(&input, &dest, platform)
            .arg("-VerificationOnly")
            .output()
            .expect("verification collector");
        assert_success(&verified);
        assert_eq!(
            fs::read_dir(dest).expect("collected files").count(),
            names.len()
        );
    }
    let publish = release_assets_command(fixture.path(), &fixture.path().join("public"))
        .output()
        .expect("reject unsigned publication");
    assert_failure_contains(&publish, "Missing required release asset");
}

#[test]
fn tauri_e2e_runner_starts_owned_frontend_before_pulse() {
    let source = read(repository_root().join("scripts/e2e/run-pulse.ps1"));
    let tauri = source
        .split("function Invoke-TauriMode")
        .nth(1)
        .expect("Tauri mode function");

    let frontend_start = tauri
        .find("Start-OwnedProcess $bun")
        .expect("Tauri mode must launch the frontend dev process");
    let pulse_start = tauri
        .find("Start-OwnedProcess $binary")
        .expect("Tauri mode must launch the repo-owned Pulse binary");

    assert!(frontend_start < pulse_start);
    assert!(tauri.contains("Assert-PortOwned $browserPort"));
    assert!(tauri.contains("@(\"run\", \"dev:ui\")"));
    assert!(!tauri.contains("Assert-PortOwned $bridgePort"));
    assert!(tauri.contains("Wait-Http \"http://localhost:$browserPort/\""));
}

fn manifest_fixture(hash: &str) -> TempDir {
    let fixture = TempDir::new().expect("fixture");
    for relative in LEGACY_VENDORED_FILES {
        write(fixture.path().join("src/codex").join(relative), "");
    }
    write(
        fixture.path().join("src/codex/mod.rs"),
        "pub mod config;\npub mod cost;\npub mod discord;\npub mod process;\npub mod session;\npub mod util;\n",
    );
    write(fixture.path().join("src/codex/process.rs"), "");

    let files = LEGACY_VENDORED_FILES
        .iter()
        .map(|relative| {
            let target_hash = if *relative == "cost.rs" {
                hash
            } else {
                EMPTY_SHA256
            };
            format!(
                r#"    {{
      "source": "src/{relative}",
      "target": "src/codex/{relative}",
      "mode": "legacy-overlay",
      "source_sha256": "{EMPTY_SHA256}",
      "target_sha256": "{target_hash}"
    }}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    write(
        fixture.path().join("src/codex/UPSTREAM.json"),
        &format!(
            r#"{{
  "schema_version": 2,
  "sync_version": 2,
  "repository": "https://github.com/xt0n1-t3ch/Codex-Discord-Rich-Presence",
  "ref": "v1.7.1",
  "commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "upstream_committed_at": "2026-07-06T00:00:00Z",
  "strategy": "legacy-overlay-v1",
  "inventory": "legacy-v1",
  "provenance": "official",
  "local_adapters": ["src/codex/mod.rs", "src/codex/process.rs"],
  "files": [
{files}
  ]
}}
"#
        ),
    );
    fixture
}

fn one_file_manifest_fixture() -> TempDir {
    let fixture = TempDir::new().expect("fixture");
    write(fixture.path().join("src/codex/cost.rs"), "");
    write(
        fixture.path().join("src/codex/UPSTREAM.json"),
        &format!(
            r#"{{
  "schema_version": 2,
  "sync_version": 2,
  "repository": "https://github.com/xt0n1-t3ch/Codex-Discord-Rich-Presence",
  "ref": "v1.7.1",
  "commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "upstream_committed_at": "2026-07-06T00:00:00Z",
  "strategy": "legacy-overlay-v1",
  "inventory": "legacy-v1",
  "provenance": "official",
  "local_adapters": ["src/codex/mod.rs", "src/codex/process.rs"],
  "files": [{{
    "source": "src/cost.rs",
    "target": "src/codex/cost.rs",
    "mode": "legacy-overlay",
    "source_sha256": "{EMPTY_SHA256}",
    "target_sha256": "{EMPTY_SHA256}"
  }}]
}}
"#
        ),
    );
    fixture
}

fn canonical_fixture() -> TempDir {
    canonical_fixture_with_tag(true)
}

fn canonical_fixture_with_tag(annotated: bool) -> TempDir {
    let fixture = TempDir::new().expect("canonical fixture");
    for relative in VENDORED_FILES {
        let body = match relative {
            "config.rs" => "pub struct PricingConfig;\n",
            "cost.rs" => {
                "use crate::config::PricingConfig;\npub fn is_fast_capable(_: &str) -> bool { true }\npub fn speed_multiplier(_: &str, _: bool) -> f64 { 1.0 }\n"
            }
            "discord.rs" => {
                "use crate::session::CodexSessionSnapshot;\npub fn presence_lines() {}\n"
            }
            "model.rs" => "pub const MODEL_CATALOG: &str = include_str!(\"model_catalog.json\");\n",
            "model_catalog.json" => "{\"models\":[]}\n",
            "process_guard.rs" => "pub struct WindowsLineEndings;\r\n",
            "session.rs" => {
                "pub mod activity;\npub mod parser;\npub struct CodexSessionSnapshot;\n"
            }
            "session/activity.rs" | "session/parser.rs" => {
                "use crate::config::PricingConfig;\npub fn accepts(_: &PricingConfig) {}\n"
            }
            _ => "pub struct Fixture;\n",
        };
        write(fixture.path().join("src").join(relative), body);
    }
    git(fixture.path(), ["init", "-q"]);
    git(fixture.path(), ["config", "user.name", "Pulse Tests"]);
    git(
        fixture.path(),
        ["config", "user.email", "pulse-tests@example.invalid"],
    );
    git(fixture.path(), ["add", "src"]);
    git(fixture.path(), ["commit", "-qm", "fixture"]);
    if annotated {
        git(fixture.path(), ["tag", "-a", "v1.7.2", "-m", "fixture"]);
    } else {
        git(fixture.path(), ["tag", "v1.7.2"]);
    }
    fixture
}

fn pulse_fixture(complete_process_adapter: bool) -> TempDir {
    let fixture = TempDir::new().expect("pulse fixture");
    let desktop_probe = if complete_process_adapter {
        "pub fn is_desktop_surface_running() -> bool { false }\n"
    } else {
        ""
    };
    write(
        fixture.path().join("src/codex/process.rs"),
        &format!(
            "const CODEX_APP_PROCESS_NAME: &str = \"Codex.exe\";\npub fn is_opencode_running() -> bool {{ false }}\npub fn is_codex_app_running() -> bool {{ false }}\n{desktop_probe}"
        ),
    );
    write(
        fixture.path().join("src/codex/mod.rs"),
        "pub mod config;\npub mod cost;\npub mod discord;\npub mod model;\npub mod process;\npub mod session;\npub mod telemetry {\n    pub mod limits;\n    pub mod plan;\n    pub mod service_tier;\n}\npub mod util;\n",
    );
    write(fixture.path().join("src/lib.rs"), "pub mod codex;\n");
    fixture
}

fn release_fixture(version: &str) -> TempDir {
    release_fixture_with_tag(version, true, true)
}

fn release_fixture_with_tag(version: &str, annotated: bool, reachable: bool) -> TempDir {
    let fixture = TempDir::new().expect("release fixture");
    write(
        fixture.path().join("Cargo.toml"),
        &format!("[package]\nname = \"cc-discord-presence\"\nversion = \"{version}\"\n"),
    );
    write(
        fixture.path().join("src-tauri/Cargo.toml"),
        &format!("[package]\nname = \"pulse\"\nversion = \"{version}\"\n"),
    );
    write(
        fixture.path().join("src-tauri/tauri.conf.json"),
        &format!(r#"{{"version":"{version}"}}"#),
    );
    write(
        fixture.path().join("package.json"),
        &format!(r#"{{"name":"pulse","version":"{version}"}}"#),
    );
    write(
        fixture.path().join("frontend/package.json"),
        &format!(r#"{{"name":"pulse-frontend","version":"{version}"}}"#),
    );
    write(
        fixture.path().join("frontend/package-lock.json"),
        &format!(
            r#"{{"name":"pulse-frontend","version":"{version}","packages":{{"":{{"name":"pulse-frontend","version":"{version}"}}}}}}"#
        ),
    );
    write(
        fixture.path().join("Cargo.lock"),
        &format!(
            "[[package]]\nname = \"cc-discord-presence\"\nversion = \"{version}\"\n\n[[package]]\nname = \"pulse\"\nversion = \"{version}\"\n"
        ),
    );
    write(
        fixture.path().join("CHANGELOG.md"),
        &format!(
            "# Changelog\n\n## [{version}] - 2026-07-09\n\n### Fixed\n\n- Validate immutable release inputs before any platform build starts.\n- Publish only artifacts produced after every required quality check succeeds.\n"
        ),
    );
    write(
        fixture.path().join("README.md"),
        &format!("Download v{version}\n"),
    );
    write(
        fixture.path().join("docs/index.md"),
        &format!("App release: **v{version}**\n"),
    );
    git(fixture.path(), ["init", "-q", "-b", "main"]);
    git(fixture.path(), ["config", "user.name", "Pulse Tests"]);
    git(
        fixture.path(),
        ["config", "user.email", "pulse-tests@example.invalid"],
    );
    git(fixture.path(), ["add", "."]);
    git(fixture.path(), ["commit", "-qm", "release fixture"]);
    if !reachable {
        git(
            fixture.path(),
            ["checkout", "-q", "--orphan", "detached-release"],
        );
        git(
            fixture.path(),
            ["commit", "--allow-empty", "-qm", "detached"],
        );
    }
    let tag = format!("v{version}");
    if annotated {
        git_dynamic(fixture.path(), &["tag", "-a", &tag, "-m", "fixture"]);
    } else {
        git_dynamic(fixture.path(), &["tag", &tag]);
    }
    if !reachable {
        git(fixture.path(), ["checkout", "-q", "main"]);
    }
    let remote = fixture.path().join("origin.git");
    git_dynamic(
        fixture.path(),
        &["init", "--bare", remote.to_str().expect("remote")],
    );
    git_dynamic(
        fixture.path(),
        &["remote", "add", "origin", remote.to_str().expect("remote")],
    );
    git(fixture.path(), ["push", "-q", "origin", "main"]);
    git_dynamic(fixture.path(), &["push", "-q", "origin", &tag]);
    git(fixture.path(), ["fetch", "-q", "origin", "main"]);
    fixture
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn script_command(name: &str) -> Command {
    let mut command = Command::new(executable_on_path("pwsh"));
    command
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-File")
        .arg(repository_root().join("scripts").join(name));
    command
}

fn executable_on_path(name: &str) -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH must be available to locate pwsh");
    let candidates = if cfg!(windows) {
        vec![format!("{name}.exe"), name.to_string()]
    } else {
        vec![name.to_string()]
    };

    for directory in std::env::split_paths(&path) {
        for candidate in &candidates {
            let executable = directory.join(candidate);
            if executable.is_file() {
                return executable;
            }
        }
    }

    panic!("{name} was not found on PATH");
}

fn update_command(repository: &Path, root: &Path, tag: &str, commit: &str) -> Command {
    let mut command = script_command("update-codex-rich-presence.ps1");
    command
        .arg("-TestMode")
        .arg("-TestRepository")
        .arg(repository)
        .arg("-Tag")
        .arg(tag)
        .arg("-Commit")
        .arg(commit)
        .arg("-Root")
        .arg(root);
    command
}

fn release_contract_command(root: &Path, tag: &str, commit: &str) -> Command {
    let mut command = script_command("check-release-contract.ps1");
    command
        .arg("-Root")
        .arg(root)
        .arg("-Tag")
        .arg(tag)
        .arg("-ExpectedCommit")
        .arg(commit)
        .arg("-TestMode");
    command
}

fn platform_assets_command(input: &Path, output: &Path, platform: &str) -> Command {
    let mut command = script_command("release-platform-assets.ps1");
    command
        .arg("-InputDirectory")
        .arg(input)
        .arg("-OutputDirectory")
        .arg(output)
        .arg("-Platform")
        .arg(platform);
    command
}

fn release_assets_command(artifacts: &Path, output: &Path) -> Command {
    let mut command = script_command("release-assets.ps1");
    command
        .arg("-ArtifactsDirectory")
        .arg(artifacts)
        .arg("-OutputDirectory")
        .arg(output)
        .arg("-Tag")
        .arg("v1.6.1")
        .arg("-Repository")
        .arg("xt0n1-t3ch/Pulse-Claude-Code-Analytics");
    command
}

fn local_release_assets_command(bundle: &Path, output: &Path, tag: &str) -> Command {
    let mut command = script_command("prepare-local-release-assets.ps1");
    command
        .arg("-BundleDirectory")
        .arg(bundle)
        .arg("-OutputDirectory")
        .arg(output)
        .arg("-Tag")
        .arg(tag);
    command
}

fn windows_sbom_command(artifact: &Path, sbom: &Path) -> Command {
    let mut command = script_command("check-windows-sbom.ps1");
    command
        .arg("-ArtifactPath")
        .arg(artifact)
        .arg("-SbomPath")
        .arg(sbom)
        .arg("-PackageName")
        .arg("pulse")
        .arg("-PackageVersion")
        .arg("1.7.2");
    command
}

fn git<const N: usize>(directory: &Path, arguments: [&str; N]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .output()
        .expect("run git");
    assert_success(&output);
}

fn git_output<const N: usize>(directory: &Path, arguments: [&str; N]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .output()
        .expect("run git");
    assert_success(&output);
    stdout(&output).trim().to_string()
}

fn git_dynamic(directory: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .output()
        .expect("run git");
    assert_success(&output);
}

fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(path, contents).expect("write fixture");
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).expect("read file")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", output_text(output));
}

fn assert_failure_contains(output: &Output, expected: &str) {
    assert!(!output.status.success(), "command unexpectedly passed");
    assert!(
        output_text(output).contains(expected),
        "expected {expected:?}\n{}",
        output_text(output)
    );
}
