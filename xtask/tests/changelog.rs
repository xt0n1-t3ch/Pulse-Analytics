//! `changelog check` and `changelog notes` on a fixture repository.

mod common;

use common::{TempDir, assert_failure_contains, assert_success, write};
use xtask::changelog;

const REPOSITORY_TOML: &str = "repository = \"o/r\"\ndescription = \"d\"\nhomepage = \"https://example.invalid\"\ntopics = []\n";
const WEB: &str = "https://github.com/o/r";

fn changelog_text(summary_lines: usize) -> String {
    let bullets: String = (0..summary_lines)
        .map(|n| format!("- Entry number {n}.\n"))
        .collect();
    format!(
        "# Changelog\n\nThe format is based on [Keep a Changelog](https://keepachangelog.com/en/2.0.0/),\nand this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).\n\nThe version covers the commands and the files they write.\n\n## [Unreleased]\n\n## [1.1.0] - 2026-11-07\n\nThis release adds entries; the database format is unchanged and 1.0.0 remains a valid rollback.\n\n### Added\n\n{bullets}\n## [1.0.0] - 2026-10-01\n\nFirst release.\n\n### Added\n\n- Everything.\n\n[Unreleased]: {WEB}/compare/v1.1.0...HEAD\n[1.1.0]: {WEB}/compare/v1.0.0...v1.1.0\n[1.0.0]: {WEB}/releases/tag/v1.0.0\n"
    )
}

fn fixture(text: &str) -> TempDir {
    let root = TempDir::new();
    write(root.path().join(".github/repository.toml"), REPOSITORY_TOML);
    write(root.path().join("CHANGELOG.md"), text);
    root
}

#[test]
fn changelog_check_accepts_a_well_formed_file_outside_git() {
    let root = fixture(&changelog_text(3));

    let message = assert_success(changelog::check(root.path()));

    assert!(message.contains("2 releases"), "{message}");
}

#[test]
fn changelog_check_names_every_problem_it_finds() {
    let broken = changelog_text(3).replace(
        "[1.1.0]: https://github.com/o/r/compare/v1.0.0...v1.1.0",
        "[1.1.0]: https://github.com/old/name/compare/v1.0.0...v1.1.0",
    );
    let root = fixture(&broken);

    let error = common::failure_text(changelog::check(root.path()));

    assert!(error.contains("CHANGELOG.md breaks the format"), "{error}");
    assert!(
        error.contains("[1.1.0] links https://github.com/old/name"),
        "{error}"
    );
    assert!(
        error.contains("expected https://github.com/o/r/compare/v1.0.0...v1.1.0"),
        "{error}"
    );
}

#[test]
fn release_notes_are_the_section_verbatim_for_a_short_release() {
    let root = fixture(&changelog_text(3));

    let notes = assert_success(changelog::notes(root.path(), "1.1.0"));

    assert_eq!(
        notes,
        "This release adds entries; the database format is unchanged and 1.0.0 remains a valid rollback.\n\n### Added\n\n- Entry number 0.\n- Entry number 1.\n- Entry number 2.\n"
    );
    assert!(
        !notes
            .lines()
            .any(|line| line.starts_with("# ") || line.starts_with("## ")),
        "no H1 or H2 in a short release body"
    );
    assert!(
        !notes.contains("compare"),
        "no compare link in a short release body"
    );
}

#[test]
fn only_a_section_longer_than_150_lines_gets_the_appendix() {
    let at_limit = fixture(&changelog_text(150 - 4));
    let notes = assert_success(changelog::notes(at_limit.path(), "1.1.0"));
    assert_eq!(
        notes.trim_end().lines().count(),
        changelog::LONG_NOTES_LINES
    );
    assert!(!notes.contains("## Release assets"));

    let long = fixture(&changelog_text(150 - 3));
    let notes = assert_success(changelog::notes(long.path(), "1.1.0"));
    for heading in [
        "\n\n## Release assets\n",
        "\n\n## Integrity\n",
        "\n\n## Full changelog\n",
    ] {
        assert!(notes.contains(heading), "{heading:?} missing");
    }
    assert!(notes.contains("`pulse-windows-x64-*.exe`"));
    assert!(notes.contains("`pulse-windows-arm64.spdx.json`"));
    assert!(notes.contains("`latest.json`") && notes.contains("`SHA256SUMS.txt`"));
    assert!(notes.contains(&format!(
        "[All changes in v1.1.0]({WEB}/compare/v1.0.0...v1.1.0)"
    )));
    assert!(notes.contains("no Apple Developer ID signature or notarization"));
}

#[test]
fn release_notes_reject_unknown_versions_and_stub_sections() {
    let root = fixture(&changelog_text(3));
    assert_failure_contains(
        changelog::notes(root.path(), "9.9.9"),
        "no section for 9.9.9",
    );

    write(
        root.path().join("CHANGELOG.md"),
        "# Changelog\n\n## [1.0.0] - 2026-10-01\n\nTiny.\n",
    );
    assert_failure_contains(
        changelog::notes(root.path(), "1.0.0"),
        "shorter than 80 characters",
    );
}
