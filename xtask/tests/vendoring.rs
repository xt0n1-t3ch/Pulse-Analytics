//! Vendoring integrity (schema 2 mirror and schema 3 core pin) and the updater.

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use common::{
    TempDir, assert_failure_contains, assert_success, contract_path, git, git_identity, git_output,
    read, repository_root, write,
};
use xtask::upstream::check as integrity;
use xtask::upstream::update::{self, UpdateOptions};

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

fn check(root: &Path, test_mode: bool) -> anyhow::Result<String> {
    integrity::run(root, &contract_path(), test_mode)
}

fn sync(canonical: &Path, root: &Path, tag: &str, commit: &str) -> anyhow::Result<String> {
    update::run(&UpdateOptions {
        root,
        contract_path: &contract_path(),
        tag,
        commit,
        test_repository: Some(canonical),
        test_mode: true,
    })
}

#[test]
fn vendoring_integrity_is_offline_and_accepts_matching_hashes() {
    let fixture = manifest_fixture(EMPTY_SHA256);

    let message = assert_success(check(fixture.path(), false));

    assert!(message.contains("integrity verified"), "{message}");
}

#[test]
fn vendoring_integrity_rejects_malformed_manifest() {
    let fixture = TempDir::new();
    write(fixture.path().join("src/codex/UPSTREAM.json"), "{oops");

    assert_failure_contains(check(fixture.path(), false), "not valid JSON");
}

#[test]
fn vendoring_integrity_rejects_legacy_manifest_without_hash_contract() {
    let fixture = TempDir::new();
    write(
        fixture.path().join("src/codex/UPSTREAM.json"),
        r#"{"repository":"https://github.com/xt0n1-t3ch/Codex-Discord-Rich-Presence","branch":"main","commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
    );

    assert_failure_contains(
        check(fixture.path(), false),
        "schema_version 2 and sync_version 2",
    );
}

#[test]
fn vendoring_integrity_rejects_mismatched_file_hash() {
    let fixture = manifest_fixture(&"0".repeat(64));

    assert_failure_contains(check(fixture.path(), false), "hash mismatch");
}

#[test]
fn vendoring_integrity_rejects_self_declared_one_file_inventory() {
    let fixture = one_file_manifest_fixture();

    assert_failure_contains(check(fixture.path(), false), "expected inventory");
}

#[test]
fn vendoring_integrity_accepts_test_provenance_only_in_test_mode() {
    let fixture = manifest_fixture(EMPTY_SHA256);
    let manifest = fixture.path().join("src/codex/UPSTREAM.json");
    write(
        &manifest,
        &read(&manifest).replace("\"provenance\": \"official\"", "\"provenance\": \"test\""),
    );

    assert_failure_contains(check(fixture.path(), false), "--test-mode");
    assert_success(check(fixture.path(), true));
}

#[test]
fn core_pin_manifest_is_verified_against_the_cargo_dependency() {
    let rev = "a".repeat(40);
    let fixture = TempDir::new();
    let root = fixture.path();
    write(root.join("src/codex/config.rs"), "");
    write(
        root.join("Cargo.toml"),
        &format!(
            "[package]\nname = \"x\"\n\n[dependencies]\ncodex-presence-core = {{ git = \"https://example.invalid/core\", rev = \"{rev}\", version = \"2.0.1\" }}\n"
        ),
    );
    write(
        root.join("src/codex/UPSTREAM.json"),
        &format!(
            r#"{{
  "schema_version": 3,
  "repository": "https://github.com/xt0n1-t3ch/Codex-Discord-Rich-Presence",
  "canonical_release": "v1.11.3",
  "canonical_commit": "{rev}",
  "integration": {{
    "mode": "git-rev", "package": "codex-presence-core", "version": "2.0.1",
    "rev": "{rev}", "promotion_mode": "git-rev", "promotion_status": "promoted"
  }},
  "compatibility": {{
    "pulse": "1.9.7", "presence_config_schema": 13,
    "usage_snapshot_revision": 1, "app_snapshot_revision": 1
  }},
  "shared_contracts": [
    "presence-composer", "presence-field-layout", "semantic-quota-windows",
    "usage-snapshot", "credit-balance"
  ],
  "local_adapters": ["src/codex/config.rs"]
}}
"#
        ),
    );

    let message = assert_success(check(root, false));
    assert_eq!(
        message,
        "Codex presence core contract verified: mode=git-rev release=v1.11.3 version=2.0.1 adapters=1"
    );

    let cargo = read(root.join("Cargo.toml"));
    write(
        root.join("Cargo.toml"),
        &cargo.replace(&rev, &"b".repeat(40)),
    );
    assert_failure_contains(check(root, false), "Cargo core Git rev does not match");
    write(root.join("Cargo.toml"), &cargo.replace("2.0.1", "2.0.2"));
    assert_failure_contains(check(root, false), "Cargo core version does not match");
    write(root.join("Cargo.toml"), &cargo);

    // The real manifest keeps the pin in [workspace.dependencies] and inherits it.
    let pin = format!(
        "{{ git = \"https://example.invalid/core\", rev = \"{rev}\", version = \"2.0.1\" }}"
    );
    let inherited = format!(
        "[workspace]\nmembers = [\".\"]\n\n[workspace.dependencies]\ncodex-presence-core = {pin}\n\n[package]\nname = \"x\"\n\n[dependencies]\ncodex-presence-core.workspace = true\n"
    );
    write(root.join("Cargo.toml"), &inherited);
    assert_success(check(root, false));
    write(
        root.join("Cargo.toml"),
        &inherited.replace("2.0.1", "2.0.2"),
    );
    assert_failure_contains(check(root, false), "Cargo core version does not match");
    write(root.join("Cargo.toml"), &cargo);

    fs::remove_file(root.join("src/codex/config.rs")).expect("remove adapter");
    assert_failure_contains(
        check(root, false),
        "Pulse adapter is missing: src/codex/config.rs",
    );
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
fn updater_syncs_explicit_tag_and_commit_without_touching_pulse_adapters() {
    let canonical = canonical_fixture();
    let commit = git_output(canonical.path(), &["rev-parse", "HEAD"]);
    let pulse = pulse_fixture(true);
    let process_before = fs::read(pulse.path().join("src/codex/process.rs")).expect("process");
    let module_before = fs::read(pulse.path().join("src/codex/mod.rs")).expect("module");

    assert_success(sync(canonical.path(), pulse.path(), "v1.7.2", &commit));

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
    assert!(
        formatted.status.success(),
        "{}",
        String::from_utf8_lossy(&formatted.stdout)
    );
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

    assert_success(check(pulse.path(), true));

    let compiled = Command::new("rustc")
        .arg("--edition=2024")
        .arg("--crate-name=pulse_vendor_fixture")
        .arg("--crate-type=lib")
        .arg(pulse.path().join("src/lib.rs"))
        .arg("-o")
        .arg(pulse.path().join("pulse_vendor_fixture.rlib"))
        .output()
        .expect("compile synced fixture");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
}

#[test]
fn updater_rejects_missing_process_compatibility_before_writing() {
    let canonical = canonical_fixture();
    let commit = git_output(canonical.path(), &["rev-parse", "HEAD"]);
    let pulse = pulse_fixture(false);
    let sentinel = pulse.path().join("src/codex/config.rs");
    write(&sentinel, "keep me\n");

    assert_failure_contains(
        sync(canonical.path(), pulse.path(), "v1.7.2", &commit),
        "is_desktop_surface_running",
    );
    assert_eq!(read(sentinel), "keep me\n");
}

#[test]
fn updater_rejects_mutable_or_mismatched_refs() {
    let canonical = canonical_fixture();
    let commit = git_output(canonical.path(), &["rev-parse", "HEAD"]);
    let pulse = pulse_fixture(true);

    assert_failure_contains(
        sync(canonical.path(), pulse.path(), "main", &commit),
        "semantic version tag",
    );
    assert_failure_contains(
        sync(canonical.path(), pulse.path(), "v1.7.2", &"0".repeat(40)),
        "does not resolve to commit",
    );
}

#[test]
fn updater_rejects_lightweight_tags() {
    let canonical = canonical_fixture_with_tag(false);
    let commit = git_output(canonical.path(), &["rev-parse", "HEAD"]);
    let pulse = pulse_fixture(true);

    assert_failure_contains(
        sync(canonical.path(), pulse.path(), "v1.7.2", &commit),
        "annotated tag",
    );
}

#[test]
fn updater_refuses_a_test_repository_outside_test_mode() {
    let canonical = canonical_fixture();
    let commit = git_output(canonical.path(), &["rev-parse", "HEAD"]);
    let pulse = pulse_fixture(true);

    let result = update::run(&UpdateOptions {
        root: pulse.path(),
        contract_path: &contract_path(),
        tag: "v1.7.2",
        commit: &commit,
        test_repository: Some(canonical.path()),
        test_mode: false,
    });

    assert_failure_contains(result, "only with --test-mode");
}

fn manifest_fixture(hash: &str) -> TempDir {
    let fixture = TempDir::new();
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
    let fixture = TempDir::new();
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
    let fixture = TempDir::new();
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
    git(fixture.path(), &["init", "-q"]);
    git_identity(fixture.path());
    git(fixture.path(), &["add", "src"]);
    git(fixture.path(), &["commit", "-qm", "fixture"]);
    if annotated {
        git(fixture.path(), &["tag", "-a", "v1.7.2", "-m", "fixture"]);
    } else {
        git(fixture.path(), &["tag", "v1.7.2"]);
    }
    fixture
}

fn pulse_fixture(complete_process_adapter: bool) -> TempDir {
    let fixture = TempDir::new();
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
