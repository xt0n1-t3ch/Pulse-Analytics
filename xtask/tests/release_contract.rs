//! Release contract checks (tag, version surfaces, core pin, annotated main-reachable tag).

mod common;

use std::path::Path;

use common::{
    TempDir, assert_failure_contains, assert_success, git, git_identity, git_output, read, write,
};
use xtask::release_contract;

fn check(root: &Path, tag: &str, commit: &str) -> anyhow::Result<String> {
    release_contract::check(root, tag, commit, true)
}

#[test]
fn release_contract_accepts_stable_and_prerelease_tags() {
    let stable = release_fixture("1.5.2");
    let stable_commit = git_output(stable.path(), &["rev-parse", "HEAD"]);
    let message = assert_success(check(stable.path(), "v1.5.2", &stable_commit));
    assert!(message.contains("surfaces=7"), "{message}");
    assert!(
        message.contains(&format!("commit={stable_commit}")),
        "{message}"
    );

    let prerelease = release_fixture("1.5.3-rc.1");
    let prerelease_commit = git_output(prerelease.path(), &["rev-parse", "HEAD"]);
    assert_success(check(prerelease.path(), "v1.5.3-rc.1", &prerelease_commit));
}

#[test]
fn release_contract_rejects_malformed_or_mismatched_tags() {
    let fixture = release_fixture("1.5.2");
    let commit = git_output(fixture.path(), &["rev-parse", "HEAD"]);

    for tag in ["release-1.5.2", "v01.5.2", "v1.5.2-rc..1"] {
        assert_failure_contains(check(fixture.path(), tag, &commit), "semantic version tag");
    }
    assert_failure_contains(check(fixture.path(), "v1.5.3", &commit), "version mismatch");
    assert_failure_contains(check(fixture.path(), "v1.5.2", "abc"), "40-character");
}

#[test]
fn release_contract_requires_annotated_main_reachable_tag_and_expected_commit() {
    let lightweight = release_fixture_with_tag("1.5.2", false, true);
    let lightweight_commit = git_output(lightweight.path(), &["rev-parse", "HEAD"]);
    assert_failure_contains(
        check(lightweight.path(), "v1.5.2", &lightweight_commit),
        "annotated tag",
    );

    let unreachable = release_fixture_with_tag("1.5.2", true, false);
    let tagged_commit = git_output(unreachable.path(), &["rev-list", "-n", "1", "v1.5.2"]);
    assert_failure_contains(
        check(unreachable.path(), "v1.5.2", &tagged_commit),
        "not reachable",
    );

    let fixture = release_fixture("1.5.2");
    let other = "0".repeat(40);
    assert_failure_contains(
        check(fixture.path(), "v1.5.2", &other),
        "instead of expected commit",
    );
}

#[test]
fn release_contract_requires_head_to_be_the_tagged_commit_outside_test_mode() {
    let fixture = release_fixture("1.5.2");
    let commit = git_output(fixture.path(), &["rev-parse", "HEAD"]);
    assert_success(release_contract::check(
        fixture.path(),
        "v1.5.2",
        &commit,
        false,
    ));

    write(fixture.path().join("extra.txt"), "later work\n");
    git(fixture.path(), &["add", "."]);
    git(fixture.path(), &["commit", "-qm", "later"]);
    assert_failure_contains(
        release_contract::check(fixture.path(), "v1.5.2", &commit, false),
        "does not match expected commit",
    );
}

#[test]
fn release_contract_validates_readme_and_docs_version_surfaces() {
    let fixture = release_fixture("1.5.2");
    let commit = git_output(fixture.path(), &["rev-parse", "HEAD"]);
    write(fixture.path().join("README.md"), "Download v1.5.1\n");

    assert_failure_contains(check(fixture.path(), "v1.5.2", &commit), "README.md");
}

#[test]
fn release_contract_rejects_a_changelog_section_too_short_to_publish() {
    let fixture = release_fixture("1.5.2");
    let commit = git_output(fixture.path(), &["rev-parse", "HEAD"]);
    write(
        fixture.path().join("CHANGELOG.md"),
        "# Changelog\n\n## [1.5.2] - 2026-07-09\n\nToo short.\n\n[1.5.2]: https://example.invalid\n",
    );

    assert_failure_contains(
        check(fixture.path(), "v1.5.2", &commit),
        "too short to publish",
    );
}

#[test]
fn release_contract_enforces_the_core_pin_and_schema_contract() {
    let fixture = release_fixture("1.5.2");
    let commit = git_output(fixture.path(), &["rev-parse", "HEAD"]);
    write_repository_contract(fixture.path(), "1.5.2");
    assert_success(check(fixture.path(), "v1.5.2", &commit));

    let cargo = read(fixture.path().join("Cargo.toml"));
    write(
        fixture.path().join("Cargo.toml"),
        &cargo.replace(
            &format!("git = \"https://example.invalid/core\", rev = \"{CORE_REV}\""),
            "path = \"../core\"",
        ),
    );
    assert_failure_contains(
        check(fixture.path(), "v1.5.2", &commit),
        "immutable Git dependency",
    );
    write(fixture.path().join("Cargo.toml"), &cargo);

    write(
        fixture.path().join("src/config.rs"),
        "const CONFIG_SCHEMA_VERSION: u32 = 12;\n",
    );
    assert_failure_contains(
        check(fixture.path(), "v1.5.2", &commit),
        "does not declare schema 13",
    );
    write(
        fixture.path().join("src/config.rs"),
        "const CONFIG_SCHEMA_VERSION: u32 = 13;\n",
    );

    let upstream = read(fixture.path().join("src/codex/UPSTREAM.json"));
    write(
        fixture.path().join("src/codex/UPSTREAM.json"),
        &upstream.replace(CORE_REV, &"b".repeat(40)),
    );
    assert_failure_contains(
        check(fixture.path(), "v1.5.2", &commit),
        "manifest SHA does not match",
    );
}

#[test]
fn release_contract_reads_a_core_pin_inherited_from_the_workspace() {
    let fixture = release_fixture("1.5.2");
    let commit = git_output(fixture.path(), &["rev-parse", "HEAD"]);
    write_repository_contract(fixture.path(), "1.5.2");
    let pin = format!(
        "{{ git = \"https://example.invalid/core\", rev = \"{CORE_REV}\", version = \"2.0.1\" }}"
    );
    let inherited = format!(
        "[workspace]\nmembers = [\".\"]\n\n[workspace.dependencies]\ncodex-presence-core = {pin}\n\n[package]\nname = \"cc-discord-presence\"\nversion = \"1.5.2\"\n\n[dependencies]\ncodex-presence-core.workspace = true\n"
    );
    write(fixture.path().join("Cargo.toml"), &inherited);
    assert_success(check(fixture.path(), "v1.5.2", &commit));

    write(
        fixture.path().join("Cargo.toml"),
        &inherited.replace(&pin, "{ path = \"../core\", version = \"2.0.1\" }"),
    );
    assert_failure_contains(
        check(fixture.path(), "v1.5.2", &commit),
        "immutable Git dependency",
    );
}

const CORE_REV: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn write_repository_contract(root: &Path, version: &str) {
    write(
        root.join("Cargo.toml"),
        &format!(
            "[package]\nname = \"cc-discord-presence\"\nversion = \"{version}\"\n\n[dependencies]\ncodex-presence-core = {{ git = \"https://example.invalid/core\", rev = \"{CORE_REV}\", version = \"2.0.1\" }}\n"
        ),
    );
    write(
        root.join("src/codex/UPSTREAM.json"),
        &format!(
            r#"{{"schema_version":3,"canonical_commit":"{CORE_REV}","integration":{{"version":"2.0.1"}}}}"#
        ),
    );
    write(
        root.join("src/config.rs"),
        "const CONFIG_SCHEMA_VERSION: u32 = 13;\n",
    );
    write(
        root.join("src-tauri/src/db.rs"),
        "const SCHEMA_VERSION: i64 = 7;\n",
    );
    write(
        root.join("scripts/release-contract.json"),
        &format!(
            r#"{{
  "schema_version": 1,
  "product": {{"name": "pulse", "version": "{version}"}},
  "core": {{"version": "2.0.1", "manifest": "src/codex/UPSTREAM.json"}},
  "configuration": {{"schema_version": 13, "source": "src/config.rs"}},
  "database": {{"schema_version": 7, "source": "src-tauri/src/db.rs"}},
  "release": {{"windows_sbom": "pulse-windows-x64.spdx.json", "checksum_manifest": "SHA256SUMS.txt"}}
}}"#
        ),
    );
}

fn release_fixture(version: &str) -> TempDir {
    release_fixture_with_tag(version, true, true)
}

fn release_fixture_with_tag(version: &str, annotated: bool, reachable: bool) -> TempDir {
    let fixture = TempDir::new();
    let root = fixture.path();
    write(
        root.join("Cargo.toml"),
        &format!("[package]\nname = \"cc-discord-presence\"\nversion = \"{version}\"\n"),
    );
    write(
        root.join("src-tauri/Cargo.toml"),
        &format!("[package]\nname = \"pulse\"\nversion = \"{version}\"\n"),
    );
    write(
        root.join("src-tauri/tauri.conf.json"),
        &format!(r#"{{"version":"{version}"}}"#),
    );
    write(
        root.join("package.json"),
        &format!(r#"{{"name":"pulse","version":"{version}"}}"#),
    );
    write(
        root.join("frontend/package.json"),
        &format!(r#"{{"name":"pulse-frontend","version":"{version}"}}"#),
    );
    write(
        root.join("Cargo.lock"),
        &format!(
            "[[package]]\nname = \"cc-discord-presence\"\nversion = \"{version}\"\n\n[[package]]\nname = \"pulse\"\nversion = \"{version}\"\n"
        ),
    );
    write(
        root.join("CHANGELOG.md"),
        &format!(
            "# Changelog\n\n## [{version}] - 2026-07-09\n\n### Fixed\n\n- Validate immutable release inputs before any platform build starts.\n- Publish only artifacts produced after every required quality check succeeds.\n"
        ),
    );
    write(root.join("README.md"), &format!("Download v{version}\n"));
    write(
        root.join("docs/index.md"),
        &format!("App release: **v{version}**\n"),
    );
    git(root, &["init", "-q", "-b", "main"]);
    git_identity(root);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "release fixture"]);
    if !reachable {
        git(root, &["checkout", "-q", "--orphan", "detached-release"]);
        git(root, &["commit", "--allow-empty", "-qm", "detached"]);
    }
    let tag = format!("v{version}");
    if annotated {
        git(root, &["tag", "-a", &tag, "-m", "fixture"]);
    } else {
        git(root, &["tag", &tag]);
    }
    if !reachable {
        git(root, &["checkout", "-q", "main"]);
    }
    let remote = root.join("origin.git");
    let remote_path = remote.to_str().expect("remote");
    git(root, &["init", "-q", "--bare", remote_path]);
    git(root, &["remote", "add", "origin", remote_path]);
    git(root, &["push", "-q", "origin", "main"]);
    git(root, &["push", "-q", "origin", &tag]);
    git(root, &["fetch", "-q", "origin", "main"]);
    fixture
}
