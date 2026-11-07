//! GitHub description, homepage and topics, kept in `.github/repository.toml` and
//! compared with (`--check`) or sent to (`--apply`) the live repository through `gh`.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::repo_config::RepoConfig;

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RemoteMeta {
    pub description: Option<String>,
    pub homepage_url: Option<String>,
    pub repository_topics: Option<Vec<RemoteTopic>>,
}

#[derive(Debug, Deserialize)]
pub struct RemoteTopic {
    pub name: String,
}

impl RemoteMeta {
    fn topics(&self) -> Vec<&str> {
        self.repository_topics
            .iter()
            .flatten()
            .map(|topic| topic.name.as_str())
            .collect()
    }
}

/// What differs between the wanted metadata and the live one, in words.
pub fn differences(wanted: &RepoConfig, remote: &RemoteMeta) -> Vec<String> {
    let mut found = Vec::new();
    if remote.description.as_deref().unwrap_or_default() != wanted.description {
        found.push(format!(
            "description\n    now: {}\n    new: {}",
            remote
                .description
                .as_deref()
                .filter(|d| !d.is_empty())
                .unwrap_or("(none)"),
            wanted.description
        ));
    }
    if remote.homepage_url.as_deref().unwrap_or_default() != wanted.homepage {
        found.push(format!(
            "homepage\n    now: {}\n    new: {}",
            remote
                .homepage_url
                .as_deref()
                .filter(|h| !h.is_empty())
                .unwrap_or("(none)"),
            wanted.homepage
        ));
    }
    let current = remote.topics();
    let to_add: Vec<&str> = wanted
        .topics
        .iter()
        .map(String::as_str)
        .filter(|topic| !current.contains(topic))
        .collect();
    let to_remove: Vec<&str> = current
        .iter()
        .copied()
        .filter(|topic| {
            !wanted
                .topics
                .iter()
                .any(|wanted_topic| wanted_topic == topic)
        })
        .collect();
    if !to_add.is_empty() {
        found.push(format!("topics to add: {}", to_add.join(", ")));
    }
    if !to_remove.is_empty() {
        found.push(format!("topics to remove: {}", to_remove.join(", ")));
    }
    found
}

/// Arguments for `gh repo edit` that bring the repository to `wanted`; only changed parts.
pub fn edit_arguments(wanted: &RepoConfig, remote: &RemoteMeta) -> Vec<String> {
    let mut arguments = vec![
        "repo".to_owned(),
        "edit".to_owned(),
        wanted.repository.clone(),
    ];
    if remote.description.as_deref().unwrap_or_default() != wanted.description {
        arguments.extend(["--description".to_owned(), wanted.description.clone()]);
    }
    if remote.homepage_url.as_deref().unwrap_or_default() != wanted.homepage {
        arguments.extend(["--homepage".to_owned(), wanted.homepage.clone()]);
    }
    let current = remote.topics();
    for topic in wanted
        .topics
        .iter()
        .filter(|topic| !current.contains(&topic.as_str()))
    {
        arguments.extend(["--add-topic".to_owned(), topic.clone()]);
    }
    for topic in current
        .iter()
        .filter(|topic| !wanted.topics.iter().any(|w| w == **topic))
    {
        arguments.extend(["--remove-topic".to_owned(), (*topic).to_owned()]);
    }
    arguments
}

fn gh(arguments: &[String]) -> Result<String> {
    let output = Command::new("gh")
        .args(arguments)
        .output()
        .context("failed to start gh (install the GitHub CLI and run `gh auth login`)")?;
    if !output.status.success() {
        bail!(
            "gh {} failed: {}",
            arguments.first().map_or("", String::as_str),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn fetch_remote(repository: &str) -> Result<RemoteMeta> {
    let json = gh(&[
        "repo".to_owned(),
        "view".to_owned(),
        repository.to_owned(),
        "--json".to_owned(),
        "description,homepageUrl,repositoryTopics".to_owned(),
    ])?;
    serde_json::from_str(&json).context("parse `gh repo view` output")
}

/// `--check`: fails when the live repository differs. `--apply`: edits it, then re-checks.
pub fn run(root: &Path, apply: bool) -> Result<String> {
    let wanted = RepoConfig::load(root)?;
    let remote = fetch_remote(&wanted.repository)?;
    let found = differences(&wanted, &remote);
    if found.is_empty() {
        return Ok(format!(
            "GitHub metadata of {} matches .github/repository.toml ({} topics)",
            wanted.repository,
            wanted.topics.len()
        ));
    }
    if !apply {
        bail!(
            "GitHub metadata of {} differs from .github/repository.toml; run `cargo xtask meta --apply`:\n- {}",
            wanted.repository,
            found.join("\n- ")
        );
    }
    gh(&edit_arguments(&wanted, &remote))?;
    let after = differences(&wanted, &fetch_remote(&wanted.repository)?);
    if !after.is_empty() {
        bail!(
            "GitHub still differs after `gh repo edit`:\n- {}",
            after.join("\n- ")
        );
    }
    Ok(format!(
        "Updated GitHub metadata of {}:\n- {}",
        wanted.repository,
        found.join("\n- ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted() -> RepoConfig {
        RepoConfig {
            repository: "o/r".to_owned(),
            description: "Wanted description".to_owned(),
            homepage: "https://example.com/latest".to_owned(),
            topics: vec!["rust".to_owned(), "tauri".to_owned()],
        }
    }

    fn remote(json: &str) -> RemoteMeta {
        serde_json::from_str(json).expect("remote json")
    }

    #[test]
    fn matching_metadata_has_no_differences_and_no_edit() {
        let live = remote(
            r#"{"description":"Wanted description","homepageUrl":"https://example.com/latest","repositoryTopics":[{"name":"tauri"},{"name":"rust"}]}"#,
        );
        assert!(differences(&wanted(), &live).is_empty());
        assert_eq!(edit_arguments(&wanted(), &live), ["repo", "edit", "o/r"]);
    }

    #[test]
    fn differences_and_edit_arguments_cover_every_field() {
        let live = remote(
            r#"{"description":"Old","homepageUrl":"","repositoryTopics":[{"name":"rust"},{"name":"stale"}]}"#,
        );
        let found = differences(&wanted(), &live);
        assert_eq!(found.len(), 4, "{found:?}");
        assert!(found[0].contains("now: Old") && found[0].contains("new: Wanted description"));
        assert!(found[1].contains("now: (none)"));
        assert_eq!(found[2], "topics to add: tauri");
        assert_eq!(found[3], "topics to remove: stale");
        assert_eq!(
            edit_arguments(&wanted(), &live),
            [
                "repo",
                "edit",
                "o/r",
                "--description",
                "Wanted description",
                "--homepage",
                "https://example.com/latest",
                "--add-topic",
                "tauri",
                "--remove-topic",
                "stale"
            ]
        );
    }

    #[test]
    fn a_repository_without_topics_or_description_still_parses() {
        let live = remote(r#"{"description":null,"homepageUrl":null,"repositoryTopics":null}"#);
        assert_eq!(differences(&wanted(), &live).len(), 3);
        assert_eq!(remote("{}").topics(), Vec::<&str>::new());
    }
}
