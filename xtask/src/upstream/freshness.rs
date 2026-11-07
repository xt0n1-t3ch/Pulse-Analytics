//! Keeps one GitHub issue open while Pulse's pinned `codex-presence-core` commit is
//! behind the canonical `main`, and closes it when the pin catches up.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;

use crate::fsx;
use crate::semver::is_full_sha;

pub const ISSUE_TITLE: &str = "sync(codex): refresh vendored upstream";

#[derive(Debug, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub body: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Create(String),
    Edit { number: u64, body: String },
    Close { number: u64 },
    Nothing,
}

/// The pinned commit: `canonical_commit` for schema 3, `commit` for the older mirror schema.
pub fn pinned_commit(manifest: &Value) -> Result<String> {
    let key = if manifest.get("schema_version").and_then(Value::as_i64) == Some(3) {
        "canonical_commit"
    } else {
        "commit"
    };
    let commit = manifest
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !is_full_sha(commit) {
        bail!("UPSTREAM.json has an invalid commit");
    }
    Ok(commit.to_owned())
}

pub fn issue_body(pinned: &str, latest: &str) -> String {
    format!(
        "Pulse's immutable `codex-presence-core` pin is behind canonical `main`.\n\n- Pinned commit: `{pinned}`\n- Upstream commit: `{latest}`\n\nUpdate only after selecting an immutable canonical release tag and full commit SHA. Change the Cargo Git `rev` and schema-3 canonical manifest together, then rerun the full core compatibility and release gates.\n"
    )
}

fn normalized(text: &str) -> String {
    text.replace("\r\n", "\n").trim().to_owned()
}

/// What to do with the open issues given whether the pin is stale.
pub fn decide(pinned: &str, latest: &str, issues: &[Issue]) -> Result<Action> {
    let matching: Vec<&Issue> = issues
        .iter()
        .filter(|issue| issue.title == ISSUE_TITLE)
        .collect();
    if matching.len() > 1 {
        bail!("Multiple open Codex upstream drift issues exist");
    }
    let stale = !pinned.eq_ignore_ascii_case(latest);
    Ok(match (stale, matching.first()) {
        (true, None) => Action::Create(issue_body(pinned, latest)),
        (true, Some(issue)) => {
            let body = issue_body(pinned, latest);
            if normalized(&issue.body) == normalized(&body) {
                Action::Nothing
            } else {
                Action::Edit {
                    number: issue.number,
                    body,
                }
            }
        }
        (false, Some(issue)) => Action::Close {
            number: issue.number,
        },
        (false, None) => Action::Nothing,
    })
}

fn gh(arguments: &[&str]) -> Result<String> {
    let output = Command::new("gh")
        .args(arguments)
        .output()
        .context("failed to start gh")?;
    if !output.status.success() {
        bail!(
            "gh {} failed: {}",
            arguments.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Reads the pin and upstream `main`, then opens, updates or closes the single drift issue.
pub fn run(root: &Path) -> Result<String> {
    let manifest: Value =
        serde_json::from_str(&fsx::read_text(&root.join("src/codex/UPSTREAM.json"))?)
            .context("parse src/codex/UPSTREAM.json")?;
    let pinned = pinned_commit(&manifest)?;
    let repository = manifest
        .get("repository")
        .and_then(Value::as_str)
        .and_then(|url| url.strip_prefix("https://github.com/"))
        .context("UPSTREAM.json has an invalid repository")?;
    let latest = gh(&[
        "api",
        &format!("repos/{repository}/commits/main"),
        "--jq",
        ".sha",
    ])?;
    if !is_full_sha(&latest) {
        bail!("GitHub returned an invalid upstream commit");
    }

    let listing = gh(&[
        "issue",
        "list",
        "--state",
        "open",
        "--limit",
        "100",
        "--json",
        "number,title,body",
    ])?;
    let issues: Vec<Issue> =
        serde_json::from_str(&listing).context("parse `gh issue list` output")?;
    let action = decide(&pinned, &latest, &issues)?;
    match &action {
        Action::Create(body) => {
            gh(&["issue", "create", "--title", ISSUE_TITLE, "--body", body])?;
        }
        Action::Edit { number, body } => {
            gh(&["issue", "edit", &number.to_string(), "--body", body])?;
        }
        Action::Close { number } => {
            gh(&[
                "issue",
                "close",
                &number.to_string(),
                "--reason",
                "completed",
            ])?;
        }
        Action::Nothing => {}
    }
    Ok(format!(
        "Upstream freshness: pinned={pinned} latest={latest} action={}",
        describe(&action)
    ))
}

fn describe(action: &Action) -> &'static str {
    match action {
        Action::Create(_) => "opened the drift issue",
        Action::Edit { .. } => "updated the drift issue",
        Action::Close { .. } => "closed the drift issue",
        Action::Nothing => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const OLD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const NEW: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn issue(number: u64, title: &str, body: &str) -> Issue {
        Issue {
            number,
            title: title.to_owned(),
            body: body.to_owned(),
        }
    }

    #[test]
    fn the_pin_comes_from_the_schema_specific_key() {
        let v3 = json!({"schema_version": 3, "canonical_commit": OLD, "commit": NEW});
        assert_eq!(pinned_commit(&v3).expect("v3"), OLD);
        let v2 = json!({"schema_version": 2, "commit": NEW});
        assert_eq!(pinned_commit(&v2).expect("v2"), NEW);
        assert!(pinned_commit(&json!({"schema_version": 3, "canonical_commit": "short"})).is_err());
    }

    #[test]
    fn a_stale_pin_opens_one_issue_then_leaves_an_identical_one_alone() {
        assert_eq!(
            decide(OLD, NEW, &[]).expect("create"),
            Action::Create(issue_body(OLD, NEW))
        );
        let open = issue(7, ISSUE_TITLE, &issue_body(OLD, NEW).replace('\n', "\r\n"));
        assert_eq!(decide(OLD, NEW, &[open]).expect("same"), Action::Nothing);
    }

    #[test]
    fn a_moved_upstream_updates_the_issue_and_a_caught_up_pin_closes_it() {
        let stale = issue(7, ISSUE_TITLE, &issue_body(OLD, OLD));
        assert_eq!(
            decide(OLD, NEW, &[stale]).expect("edit"),
            Action::Edit {
                number: 7,
                body: issue_body(OLD, NEW)
            }
        );
        let open = issue(9, ISSUE_TITLE, "anything");
        assert_eq!(
            decide(NEW, NEW, &[open]).expect("close"),
            Action::Close { number: 9 }
        );
        assert_eq!(decide(NEW, NEW, &[]).expect("nothing"), Action::Nothing);
    }

    #[test]
    fn unrelated_issues_are_ignored_and_duplicates_are_an_error() {
        let other = issue(1, "something else", "x");
        assert_eq!(
            decide(NEW, NEW, &[other]).expect("ignored"),
            Action::Nothing
        );
        let twins = [issue(1, ISSUE_TITLE, "a"), issue(2, ISSUE_TITLE, "b")];
        assert!(decide(OLD, NEW, &twins).is_err());
    }
}
