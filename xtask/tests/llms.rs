//! `llms.txt` and `llms-full.txt` generation and the `--check` mode.

mod common;

use common::{TempDir, assert_failure_contains, assert_success, read, write};
use xtask::llms;

fn fixture() -> TempDir {
    let root = TempDir::new();
    write(
        root.path().join(".github/repository.toml"),
        "repository = \"o/r\"\ndescription = \"d\"\nhomepage = \"https://example.invalid\"\ntopics = []\n",
    );
    write(
        root.path().join("docs/llms.toml"),
        r#"name = "Pulse"
summary = "Local analytics."
facts = ["It is local-first."]

[[section]]
title = "Docs"
[[section.link]]
path = "docs/index.md"
title = "Documentation index"
description = "Task-based guide."

[[section]]
title = "Optional"
[[section.link]]
path = "CHANGELOG.md"
title = "Changelog"
description = "What changed."
"#,
    );
    write(
        root.path().join("README.md"),
        "<p align=\"center\"><img src=\"a.png\" alt=\"Banner\"></p>\n\n# Pulse\n\nReadme text.\n",
    );
    write(root.path().join("docs/index.md"), "# Docs\n\nIndex text.\n");
    write(root.path().join("CHANGELOG.md"), "# Changelog\n");
    root
}

#[test]
fn generated_files_follow_the_format_and_pass_their_own_check() {
    let root = fixture();

    assert_success(llms::run(root.path(), false));

    let index = read(root.path().join("llms.txt"));
    assert!(
        index.starts_with(
            "# Pulse\n\n> Local analytics.\n\nKey facts:\n\n- It is local-first.\n\n## Docs\n"
        ),
        "{index}"
    );
    assert!(index.contains(
        "- [Documentation index](https://raw.githubusercontent.com/o/r/main/docs/index.md): Task-based guide."
    ));
    assert!(index.ends_with(
        "- [Everything above in one file](https://raw.githubusercontent.com/o/r/main/llms-full.txt): The text of every document listed here.\n"
    ));

    let full = read(root.path().join("llms-full.txt"));
    let readme = full.find("# README.md\n").expect("README section");
    let docs = full.find("# docs/index.md\n").expect("docs section");
    let changelog = full.find("# CHANGELOG.md\n").expect("changelog section");
    assert!(readme < docs && docs < changelog, "{full}");
    assert!(full.contains("Source: https://raw.githubusercontent.com/o/r/main/docs/index.md"));
    assert!(
        full.contains("(Picture: Banner)") && !full.contains("<img"),
        "{full}"
    );

    assert_success(llms::run(root.path(), true));
}

#[test]
fn check_fails_when_a_source_document_changes_or_a_file_is_missing() {
    let root = fixture();
    assert_success(llms::run(root.path(), false));

    write(
        root.path().join("docs/index.md"),
        "# Docs\n\nEdited after generation.\n",
    );
    assert_failure_contains(llms::run(root.path(), true), "llms-full.txt is out of date");

    assert_success(llms::run(root.path(), false));
    std::fs::remove_file(root.path().join("llms.txt")).expect("remove");
    assert_failure_contains(llms::run(root.path(), true), "llms.txt is out of date");
}

#[test]
fn a_link_to_a_missing_document_is_an_error_naming_the_path() {
    let root = fixture();
    std::fs::remove_file(root.path().join("docs/index.md")).expect("remove");

    assert_failure_contains(llms::run(root.path(), false), "docs/index.md");
}

#[test]
fn check_tolerates_windows_line_endings_on_disk() {
    let root = fixture();
    assert_success(llms::run(root.path(), false));
    for name in ["llms.txt", "llms-full.txt"] {
        let text = read(root.path().join(name));
        write(root.path().join(name), &text.replace('\n', "\r\n"));
    }

    assert_success(llms::run(root.path(), true));
}
