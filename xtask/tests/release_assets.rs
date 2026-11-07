//! Public asset assembly, platform collection, the recovery-release folder and SBOM validation.

mod common;

use std::fs;
use std::path::Path;

use common::{TempDir, assert_failure_contains, assert_success, read, write};
use xtask::local_assets;
use xtask::platform_assets::{self, Platform};
use xtask::release_assets;
use xtask::sbom;

fn assemble(artifacts: &Path, output: &Path) -> anyhow::Result<String> {
    release_assets::assemble(artifacts, output, "v1.6.1", "xt0n1-t3ch/Pulse-Analytics")
}

fn collect(
    input: &Path,
    output: &Path,
    platform: &str,
    verification_only: bool,
) -> anyhow::Result<String> {
    let platform = Platform::ALL
        .into_iter()
        .find(|p| p.as_str() == platform)
        .expect("platform");
    platform_assets::collect(input, output, platform, verification_only)
}

#[test]
fn release_assets_reject_empty_input_and_checksum_nonempty_input() {
    let fixture = TempDir::new();
    let artifacts = fixture.path().join("artifacts");
    let output = fixture.path().join("release-assets");
    fs::create_dir_all(&artifacts).expect("artifacts");

    assert_failure_contains(assemble(&artifacts, &output), "No release assets");

    write(
        artifacts.join("pulse-windows-x64/pulse-windows-x64-Pulse.exe"),
        "exe",
    );
    assert_failure_contains(
        assemble(&artifacts, &output),
        "Missing required release asset",
    );

    write_complete_platform_artifacts(&artifacts);
    let message = assert_success(assemble(&artifacts, &output));
    assert!(message.contains("files=22 updater-targets=6"), "{message}");
    assert!(output.join("pulse-windows-x64-Pulse.exe").is_file());
    assert!(output.join("pulse-windows-x64.spdx.json").is_file());
    let sums = read(output.join("SHA256SUMS.txt"));
    assert!(sums.contains("  pulse-windows-x64-Pulse.exe\n"));
    assert!(sums.contains("  pulse-windows-x64.spdx.json\n"));
}

#[test]
fn release_assets_publish_a_signed_updater_manifest_for_every_platform() {
    let fixture = TempDir::new();
    let artifacts = fixture.path().join("artifacts");
    let output = fixture.path().join("release-assets");
    write_complete_platform_artifacts(&artifacts);

    assert_success(assemble(&artifacts, &output));

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
                "https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/download/v1.6.1/{payload}"
            )),
            "{payload} download URL missing:\n{manifest}"
        );
        assert!(output.join(payload).is_file(), "{payload} not published");
    }
    assert!(manifest.contains("signature-for-windows"), "{manifest}");

    // The updater reads these keys in this order; a sorted map would bury `version`.
    let order = ["\"version\"", "\"pub_date\"", "\"notes\"", "\"platforms\""].map(|key| {
        manifest
            .find(key)
            .unwrap_or_else(|| panic!("{key} missing:\n{manifest}"))
    });
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{manifest}");
    let parsed: serde_json::Value = serde_json::from_str(&manifest).expect("latest.json is JSON");
    let published = parsed["pub_date"].as_str().expect("pub_date");
    assert_eq!(published.len(), 20, "{published}");
    assert!(
        published.ends_with('Z') && published.as_bytes()[10] == b'T',
        "{published}"
    );

    // The manifest itself must be covered by the checksum file it ships with.
    let sums = read(output.join("SHA256SUMS.txt"));
    assert!(sums.contains("  latest.json\n"), "{sums}");
    assert_eq!(sums.lines().count(), 23, "{sums}");
}

#[test]
fn release_assets_reject_a_platform_without_its_updater_signature() {
    let fixture = TempDir::new();
    let artifacts = fixture.path().join("artifacts");
    let output = fixture.path().join("release-assets");
    write_complete_platform_artifacts(&artifacts);
    fs::remove_file(artifacts.join("pulse-linux-x64/pulse-linux-x64-Pulse.AppImage.sig"))
        .expect("drop the Linux updater signature");

    assert_failure_contains(assemble(&artifacts, &output), ".AppImage.sig");
}

#[test]
fn release_assets_reject_an_empty_signature_and_a_dirty_output_folder() {
    let fixture = TempDir::new();
    let artifacts = fixture.path().join("artifacts");
    write_complete_platform_artifacts(&artifacts);
    write(
        artifacts.join("pulse-macos-x64/pulse-macos-x64-Pulse.app.tar.gz.sig"),
        "  \n",
    );
    assert_failure_contains(
        assemble(&artifacts, &fixture.path().join("empty-signature")),
        "Updater signature is empty",
    );

    write(
        artifacts.join("pulse-macos-x64/pulse-macos-x64-Pulse.app.tar.gz.sig"),
        "ok",
    );
    let occupied = fixture.path().join("occupied");
    write(occupied.join("stale.txt"), "stale");
    assert_failure_contains(assemble(&artifacts, &occupied), "must be empty");
    assert_failure_contains(
        release_assets::assemble(
            &artifacts,
            &fixture.path().join("x"),
            "v1.6",
            "xt0n1-t3ch/Pulse-Analytics",
        ),
        "semantic version tag",
    );
    assert_failure_contains(
        release_assets::assemble(&artifacts, &fixture.path().join("y"), "v1.6.1", "no-slash"),
        "owner/name",
    );
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
    let fixture = TempDir::new();
    let artifacts = fixture.path().join("artifacts");
    for platform in ["macos-arm64", "macos-x64"] {
        let input = fixture.path().join(platform);
        write(input.join("Pulse.app.tar.gz"), platform);
        write(input.join("Pulse.app.tar.gz.sig"), platform);
        write(input.join("Pulse.dmg"), platform);

        assert_success(collect(&input, &artifacts, platform, false));
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
fn platform_collection_uses_the_first_existing_bundle_folder() {
    let fixture = TempDir::new();
    let missing = fixture.path().join("target/release/bundle");
    let present = fixture.path().join("src-tauri/target/release/bundle");
    write(present.join("Pulse.dmg"), "dmg");
    write(present.join("Pulse.app.tar.gz"), "app");
    write(present.join("Pulse.app.tar.gz.sig"), "sig");
    let output = fixture.path().join("dist");

    let candidates = [missing.clone(), present];
    assert_success(platform_assets::collect_first_existing(
        &candidates,
        &output,
        Platform::MacosX64,
        false,
    ));
    assert!(output.join("pulse-macos-x64-Pulse.dmg").is_file());

    assert_failure_contains(
        platform_assets::collect_first_existing(&[missing], &output, Platform::MacosX64, false),
        "No release assets bundle directory found for macos-x64",
    );
}

#[test]
fn unsigned_verification_assets_do_not_relax_the_public_release_contract() {
    let fixture = TempDir::new();
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
        let strict = collect(
            &input,
            &fixture.path().join(format!("strict-{platform}")),
            platform,
            false,
        );
        assert_failure_contains(strict, "Missing required release asset");
        let dest = fixture.path().join(format!("verify-{platform}"));
        assert_success(collect(&input, &dest, platform, true));
        assert_eq!(
            fs::read_dir(dest).expect("collected files").count(),
            names.len()
        );
    }
    assert_failure_contains(
        assemble(fixture.path(), &fixture.path().join("public")),
        "Missing required release asset",
    );
}

#[test]
fn local_release_assets_select_only_the_exact_tag_version() {
    let fixture = TempDir::new();
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

    assert_success(local_assets::prepare(&bundle, &output, "v1.7.2"));

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
    let fixture = TempDir::new();
    let bundle = fixture.path().join("bundle");
    let output = fixture.path().join("release-assets");
    write(bundle.join("nsis/Pulse_1.7.1_x64-setup.exe"), "stale exe");
    write(bundle.join("msi/Pulse_1.7.1_x64_en-US.msi"), "stale msi");
    write(
        bundle.join("pulse-windows-x64.spdx.json"),
        "{\"spdxVersion\":\"SPDX-2.3\"}",
    );

    assert_failure_contains(
        local_assets::prepare(&bundle, &output, "v1.7.2"),
        "Pulse_1.7.2_x64-setup.exe",
    );
    assert!(
        !output.exists(),
        "a failed collection must not promote output"
    );
}

#[test]
fn local_release_assets_preserve_preexisting_output_byte_for_byte() {
    let fixture = TempDir::new();
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

    assert_failure_contains(
        local_assets::prepare(&bundle, &output, "v1.7.2"),
        "must not already exist",
    );
    assert_eq!(read(sentinel), "preserve me exactly");
    assert_eq!(fs::read_dir(&output).expect("output").count(), 1);
}

#[test]
fn local_release_assets_use_the_strict_release_semver_contract() {
    let fixture = TempDir::new();
    let bundle = fixture.path().join("bundle");
    fs::create_dir_all(&bundle).expect("bundle");

    for tag in ["v1.7.2-01", "v1.7.2-rc..1", "v1.7.2-.beta"] {
        let output = fixture.path().join(tag.replace('.', "_"));
        assert_failure_contains(
            local_assets::prepare(&bundle, &output, tag),
            "semantic version tag",
        );
        assert!(!output.exists(), "invalid tag promoted output: {tag}");
    }
}

#[test]
fn windows_sbom_validator_accepts_complete_spdx_and_rejects_bad_integrity() {
    let fixture = TempDir::new();
    let artifact = fixture.path().join("Pulse_1.7.2_x64-setup.exe");
    let sbom_path = fixture.path().join("pulse-windows-x64.spdx.json");
    write(&artifact, "current exe");
    write(
        &sbom_path,
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

    let message = assert_success(sbom::verify(&artifact, &sbom_path, "pulse", "1.7.2"));
    assert!(message.contains("sha256=b04c26dc"), "{message}");

    let healthy = read(&sbom_path);
    write(&sbom_path, &healthy.replace("CC0-1.0", "NOASSERTION"));
    assert_failure_contains(
        sbom::verify(&artifact, &sbom_path, "pulse", "1.7.2"),
        "SPDX 2.3 JSON document under CC0-1.0",
    );

    write(&sbom_path, &healthy.replace("b04c26dc", "00000000"));
    assert_failure_contains(
        sbom::verify(&artifact, &sbom_path, "pulse", "1.7.2"),
        "checksum mismatch",
    );

    write(
        &sbom_path,
        &healthy.replace(
            r#",
    {"name": "dependency", "versionInfo": "1.0.0"}"#,
            "",
        ),
    );
    assert_failure_contains(
        sbom::verify(&artifact, &sbom_path, "pulse", "1.7.2"),
        "dependency inventory is empty",
    );

    write(&sbom_path, &healthy);
    assert_failure_contains(
        sbom::verify(&artifact, &sbom_path, "pulse", "9.9.9"),
        "exactly one root package pulse@9.9.9",
    );
}
