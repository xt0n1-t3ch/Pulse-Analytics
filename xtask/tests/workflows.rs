//! Structure of the release and freshness workflows: pins, gates, native matrix and the
//! rule that every non-trivial step is a `cargo xtask` command that exists.

mod common;

use std::process::Command;

use common::{TempDir, assert_success, read, repository_root};
use xtask::ci;
use xtask::platform_assets::Platform;
use xtask::release_assets::REQUIREMENTS;

fn workflow(name: &str) -> String {
    read(repository_root().join(".github/workflows").join(name))
}

#[test]
fn manual_release_workflow_pins_actions_and_gates_publication_on_preflight() {
    let release = workflow("release.yml");
    let freshness = workflow("upstream-freshness.yml");
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
    assert!(release.contains("contents: write"));
    assert!(release.contains("cancel-in-progress: false"));
    assert!(release.contains("immutable-releases"));
    assert!(!release.contains("gh run list --workflow ci.yml"));
    assert!(release.contains("trap cleanup_draft EXIT INT TERM"));
    assert!(release.contains("cargo xtask audit"));
    assert!(
        repository_root()
            .join("scripts/rustsec-accepted-warnings.json")
            .is_file()
    );
    assert!(release.contains(
        "cargo xtask release-contract --tag \"$RELEASE_TAG\" --expected-commit \"$tag_commit\""
    ));
    assert!(release.contains("tag_commit: ${{ steps.release_target.outputs.tag_commit }}"));
    assert!(release.contains("tag_commit=$tag_commit"));
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
        "git fetch --force origin \"refs/tags/${RELEASE_TAG}:refs/tags/${RELEASE_TAG}\""
    ));
    assert!(release.contains("gh release view \"$TAG\" --json databaseId --jq .databaseId"));
    assert!(
        !release.contains("gh api \"repos/${GITHUB_REPOSITORY}/releases/tags/${TAG}\" --jq .id")
    );
    assert!(release.contains("cargo xtask upstream check"));
    assert!(release.contains("cargo xtask changelog check"));
    assert!(release.contains("cargo xtask llms --check"));
    assert!(release.contains("cargo xtask changelog notes \"${TAG#v}\" --output release-body.md"));
    assert!(release.contains("BUN_VERSION: \"1.4.4\""));
    assert!(release.contains("bun install --frozen-lockfile"));
    assert!(release.contains("run: bun run typecheck"));
    assert!(release.contains("run: bun run test"));
    assert!(release.contains("run: bun run build:frontend"));
    assert!(release.contains(
        "- name: Generate and verify Windows SPDX SBOM\n        if: runner.os == 'Windows'\n        run: cargo xtask sbom ci --dist dist --platform ${{ matrix.label }}"
    ));
    assert!(
        release.contains("Verify updater manifest targets every platform"),
        "the release must prove latest.json covers every updater target"
    );
    assert!(release.contains("release-assets/latest.json"));
    assert!(freshness.contains("workflow_dispatch:"));
    assert!(freshness.contains("issues: write"));
    assert!(freshness.contains("cargo xtask upstream freshness"));

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
fn every_workflow_clears_rustflags_because_hosted_runners_have_no_mold() {
    for name in ["release.yml", "upstream-freshness.yml"] {
        let text = workflow(name);
        let top_level_env = text
            .split("\nenv:\n")
            .nth(1)
            .and_then(|rest| rest.split("\n\n").next())
            .unwrap_or_else(|| panic!("{name} has no top-level env block"));
        assert!(
            top_level_env
                .lines()
                .any(|line| line == "  RUSTFLAGS: \"\""),
            "{name} must set RUSTFLAGS to an empty string for every job:\n{top_level_env}"
        );
    }
}

#[test]
fn workflows_use_bun_and_cargo_xtask_instead_of_npm_and_powershell() {
    for (name, text) in [
        ("release.yml", workflow("release.yml")),
        ("upstream-freshness.yml", workflow("upstream-freshness.yml")),
    ] {
        for forbidden in ["pwsh", "npm ", "setup-node", "NODE_VERSION", "python3"] {
            assert!(!text.contains(forbidden), "{name} still uses {forbidden:?}");
        }
    }
    assert!(workflow("release.yml").contains("oven-sh/setup-bun@"));
}

#[test]
fn every_xtask_command_a_workflow_runs_exists() {
    let mut commands = Vec::new();
    for name in ["release.yml", "upstream-freshness.yml"] {
        for line in workflow(name).lines() {
            let Some((_, after)) = line.split_once("cargo xtask ") else {
                continue;
            };
            let words: Vec<String> = after
                .split_whitespace()
                .take_while(|word| !word.starts_with(['-', '"', '$', '\'']))
                .map(str::to_owned)
                .collect();
            if !words.is_empty() {
                commands.push(words);
            }
        }
    }
    assert!(
        commands.len() >= 12,
        "expected the workflows to call xtask often: {commands:?}"
    );

    for words in commands {
        let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(&words)
            .arg("--help")
            .output()
            .expect("run xtask");
        assert!(
            output.status.success(),
            "`cargo xtask {}` is not a command: {}",
            words.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn release_matrix_requires_native_arm64_artifacts_for_windows_and_linux() {
    let root = repository_root();
    let release = workflow("release.yml");
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
        assert!(release.contains(value), "release matrix is missing {value}");
    }
    for platform in [Platform::WindowsArm64, Platform::LinuxArm64] {
        assert!(
            Platform::ALL.contains(&platform),
            "platform collector is missing {platform:?}"
        );
        assert!(
            REQUIREMENTS
                .iter()
                .any(|r| r.prefix.contains(platform.as_str())),
            "release assembler is missing {platform:?}"
        );
    }
    for updater_target in ["windows-aarch64", "linux-aarch64"] {
        assert!(
            REQUIREMENTS
                .iter()
                .any(|r| r.updater_target == Some(updater_target)),
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
    let release = workflow("release.yml");
    let build = release
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
    assert!(build.contains("run: cargo xtask verify-native --target ${{ matrix.target }}"));
    assert!(release.contains("publish_release:"));
    assert!(release.contains("type: boolean\n        default: false"));
    let publish = release.split("\n  publish:").nth(1).expect("publish job");
    assert!(publish.contains("if: inputs.publish_release"));
    assert!(publish.contains("needs: [preflight, build]"));
    assert!(build.contains("name: frontend-dist"));
    assert!(
        !build.contains("bun run build:frontend"),
        "native jobs must consume the shared frontend artifact"
    );
    assert_eq!(release.matches("bun run build:frontend").count(), 1);
}

/// The body of one job of the release workflow.
fn release_job(name: &str) -> String {
    let release = workflow("release.yml");
    let body = release
        .split(&format!("\n  {name}:\n"))
        .nth(1)
        .unwrap_or_else(|| panic!("release.yml has no {name} job"));
    let end = body
        .lines()
        .scan(0, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .find(|(_, line)| line.starts_with("  ") && !line.starts_with("   ") && line.ends_with(':'))
        .map_or(body.len(), |(start, _)| start);
    body[..end].to_owned()
}

/// Preflight ran Clippy, the tests and a workspace build on Linux before the six native
/// jobs ran Clippy and the tests again, which held every build back by about five minutes.
#[test]
fn preflight_checks_the_contract_and_leaves_compiling_to_the_native_jobs() {
    let preflight = release_job("preflight");
    for duplicated in ["cargo clippy", "cargo test", "cargo build", "apt-get"] {
        assert!(
            !preflight.contains(duplicated),
            "preflight repeats native work: {duplicated}"
        );
    }
    for kept in [
        "cargo xtask release-contract",
        "cargo xtask upstream check",
        "cargo xtask changelog check",
        "cargo xtask llms --check",
        "cargo xtask audit",
        "cargo fmt --all -- --check",
        "run: bun run typecheck",
        "run: bun run test",
        "run: bun run build:frontend",
    ] {
        assert!(preflight.contains(kept), "preflight lost {kept}");
    }
}

/// A hung job would otherwise run to the six-hour default.
#[test]
fn every_release_job_has_a_timeout() {
    for (job, limit) in [("preflight", 15), ("build", 45), ("publish", 15)] {
        assert!(
            release_job(job).contains(&format!("\n    timeout-minutes: {limit}\n")),
            "{job} must stop after {limit} minutes"
        );
    }
}

/// The local gate lints the Linux host and the Windows target, so the release lints only
/// the code no local run reaches: macOS, once.
#[test]
fn native_jobs_test_every_target_and_lint_only_macos() {
    let build = release_job("build");
    assert_eq!(build.matches("lint: true").count(), 1);
    assert!(build.contains(
        "- label: macos-arm64\n            os: macos-latest\n            target: aarch64-apple-darwin\n            bundles: app,dmg\n            lint: true"
    ));
    assert!(build.contains(
        "run: cargo xtask verify-native --target ${{ matrix.target }} ${{ matrix.lint && '--lint' || '' }}"
    ));

    let tests_only = ci::native_verification_commands("aarch64-apple-darwin", ci::NativeLint::Skip);
    assert_eq!(
        tests_only,
        [[
            "test",
            "--release",
            "--locked",
            "--workspace",
            "--target",
            "aarch64-apple-darwin"
        ]]
    );
    let linted = ci::native_verification_commands("aarch64-apple-darwin", ci::NativeLint::Clippy);
    assert_eq!(linted.len(), 2);
    assert_eq!(linted[0][0], "clippy");
    assert!(linted[0].ends_with(&["--", "-D", "warnings"]));
    assert_eq!(linted[1], tests_only[0]);
}

#[test]
fn native_build_steps_avoid_shell_specific_syntax() {
    // The build job runs on Windows (PowerShell) and Unix (bash) runners.
    let release = workflow("release.yml");
    let build = release
        .split("\n  build:")
        .nth(1)
        .expect("build job")
        .split("\n  publish:")
        .next()
        .expect("build body");
    for line in build.lines().filter(|line| line.contains("cargo xtask")) {
        assert!(
            !line.contains("$TARGET") && !line.contains("$PLATFORM") && !line.contains("$env:"),
            "{line}"
        );
    }
}

#[test]
fn github_macos_packages_require_updater_signatures_not_apple_credentials() {
    let release = workflow("release.yml");
    let gate = release
        .find("Require updater signing credentials")
        .expect("updater gate");
    let native_build = release.find("Build Tauri bundle").expect("native build");
    assert!(gate < native_build);
    assert!(release.contains("TAURI_SIGNING_PRIVATE_KEY"));
    assert!(!release.contains("secrets.APPLE_"));
    assert!(release.contains("Verify macOS package architecture"));
    assert!(release.contains(r#"lipo "$app/Contents/MacOS/$executable" -verify_arch "$arch""#));
    assert!(release.contains("no Apple Developer ID signature or notarization"));
}

#[test]
fn verification_builds_turn_updater_artifacts_off_and_publication_builds_do_not() {
    let root = TempDir::new();

    let verification = assert_success(ci::bundle_args(
        root.path(),
        "x86_64-apple-darwin",
        "app,dmg",
        false,
    ));
    assert_eq!(
        verification,
        "args=--target x86_64-apple-darwin --bundles app,dmg --config .build/platform-verification.json -- --locked"
    );
    assert_eq!(
        read(
            root.path()
                .join("src-tauri/.build/platform-verification.json")
        ),
        "{\"bundle\":{\"createUpdaterArtifacts\":false}}\n"
    );

    let publication = TempDir::new();
    let line = assert_success(ci::bundle_args(
        publication.path(),
        "x86_64-apple-darwin",
        "app,dmg",
        true,
    ));
    assert_eq!(
        line,
        "args=--target x86_64-apple-darwin --bundles app,dmg -- --locked"
    );
    assert!(!publication.path().join("src-tauri/.build").exists());
}

#[test]
fn release_artifact_actions_use_node24_pins_and_repository_retention() {
    let release = workflow("release.yml");
    assert!(release.contains("actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9"));
    assert!(release.contains("actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333"));
    assert_eq!(release.matches("retention-days: 0").count(), 2);
    assert!(!release.contains("retention-days: 7"));
}

#[test]
fn dependabot_watches_bun_at_the_workspace_root() {
    let dependabot = read(repository_root().join(".github/dependabot.yml"));
    assert!(dependabot.contains("package-ecosystem: bun\n    directory: \"/\""));
    assert!(!dependabot.contains("package-ecosystem: npm"));
}
