//! Fixtures shared by the integration tests. Not every test file uses every helper.
#![allow(dead_code)]

use std::fmt::Debug;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Scratch folder removed on drop; std only, so the crate needs no `tempfile`.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("pulse-xtask-test-{}", xtask::fsx::unique_suffix()));
        fs::create_dir_all(&path).expect("create scratch folder");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn repository_root() -> PathBuf {
    xtask::repo_root()
}

pub fn contract_path() -> PathBuf {
    repository_root().join("scripts/codex-vendor-contract.json")
}

pub fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(path, contents).expect("write fixture");
}

pub fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).expect("read file")
}

fn run_git(directory: &Path, arguments: &[&str]) -> std::process::Output {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

pub fn git(directory: &Path, arguments: &[&str]) {
    run_git(directory, arguments);
}

pub fn git_output(directory: &Path, arguments: &[&str]) -> String {
    String::from_utf8_lossy(&run_git(directory, arguments).stdout)
        .trim()
        .to_owned()
}

/// Identity and signing settings so fixture commits work on any machine.
pub fn git_identity(directory: &Path) {
    git(directory, &["config", "user.name", "Pulse Tests"]);
    git(
        directory,
        &["config", "user.email", "pulse-tests@example.invalid"],
    );
    git(directory, &["config", "commit.gpgsign", "false"]);
    git(directory, &["config", "tag.gpgsign", "false"]);
}

/// The error chain of a failed command, or a panic when it succeeded.
pub fn failure_text<T: Debug>(result: anyhow::Result<T>) -> String {
    match result {
        Ok(value) => panic!("command unexpectedly passed: {value:?}"),
        Err(error) => format!("{error:#}"),
    }
}

pub fn assert_failure_contains<T: Debug>(result: anyhow::Result<T>, expected: &str) {
    let text = failure_text(result);
    assert!(text.contains(expected), "expected {expected:?} in:\n{text}");
}

pub fn assert_success<T>(result: anyhow::Result<T>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("command failed: {error:#}"),
    }
}
