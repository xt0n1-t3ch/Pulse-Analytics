//! `.github/repository.toml`: the one place that names the GitHub repository, its
//! description, homepage and topics. `meta`, `changelog` and `llms` all read it.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::fsx;

pub const CONFIG_PATH: &str = ".github/repository.toml";
const MAX_DESCRIPTION_CHARS: usize = 350;
const MAX_TOPICS: usize = 20;
const MAX_TOPIC_CHARS: usize = 50;

/// `owner/name` with GitHub's allowed characters.
pub fn is_owner_name(text: &str) -> bool {
    text.split_once('/').is_some_and(|(owner, name)| {
        let part_ok = |part: &str| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        };
        part_ok(owner) && part_ok(name)
    })
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepoConfig {
    /// `owner/name` on GitHub.
    pub repository: String,
    pub description: String,
    pub homepage: String,
    pub topics: Vec<String>,
}

impl RepoConfig {
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(CONFIG_PATH);
        let text = fsx::read_text(&path)?;
        let config: Self = toml::from_str(&text).with_context(|| format!("parse {CONFIG_PATH}"))?;
        config.validate()?;
        Ok(config)
    }

    /// `https://github.com/<owner>/<name>`, the base of every release and compare link.
    pub fn web_url(&self) -> String {
        format!("https://github.com/{}", self.repository)
    }

    /// `https://raw.githubusercontent.com/<owner>/<name>/main/`, the base of llms.txt links.
    pub fn raw_url(&self) -> String {
        format!(
            "https://raw.githubusercontent.com/{}/main/",
            self.repository
        )
    }

    /// Enforces GitHub's own limits so `--apply` cannot be rejected halfway.
    pub fn validate(&self) -> Result<()> {
        let mut problems = Vec::new();
        if !is_owner_name(&self.repository) {
            problems.push(format!(
                "repository {:?} must be in owner/name form",
                self.repository
            ));
        }
        let description_chars = self.description.chars().count();
        if self.description.trim().is_empty() {
            problems.push("description is empty".to_owned());
        } else if description_chars > MAX_DESCRIPTION_CHARS {
            problems.push(format!(
                "description is {description_chars} characters; GitHub takes {MAX_DESCRIPTION_CHARS}"
            ));
        }
        if !self.homepage.starts_with("https://") {
            problems.push(format!("homepage {:?} must be an https URL", self.homepage));
        }
        if self.topics.len() > MAX_TOPICS {
            problems.push(format!(
                "{} topics; GitHub takes {MAX_TOPICS}",
                self.topics.len()
            ));
        }
        for (index, topic) in self.topics.iter().enumerate() {
            let valid = !topic.is_empty()
                && topic.len() <= MAX_TOPIC_CHARS
                && topic
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                && !topic.starts_with('-');
            if !valid {
                problems.push(format!(
                    "topic {topic:?} must be lowercase letters, digits and hyphens, at most {MAX_TOPIC_CHARS} characters"
                ));
            }
            if self.topics[..index].contains(topic) {
                problems.push(format!("topic {topic:?} is listed twice"));
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            bail!("{CONFIG_PATH} is invalid:\n- {}", problems.join("\n- "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RepoConfig {
        RepoConfig {
            repository: "xt0n1-t3ch/Pulse-Analytics".to_owned(),
            description: "A description.".to_owned(),
            homepage: "https://example.com/releases/latest".to_owned(),
            topics: vec!["rust".to_owned(), "tauri".to_owned()],
        }
    }

    #[test]
    fn a_complete_config_is_valid_and_derives_its_urls() {
        let config = sample();
        config.validate().expect("valid");
        assert_eq!(
            config.web_url(),
            "https://github.com/xt0n1-t3ch/Pulse-Analytics"
        );
        assert_eq!(
            config.raw_url(),
            "https://raw.githubusercontent.com/xt0n1-t3ch/Pulse-Analytics/main/"
        );
    }

    #[test]
    fn github_limits_are_enforced_before_anything_is_sent() {
        let mut config = sample();
        config.topics = (0..21).map(|n| format!("topic-{n}")).collect();
        assert!(format!("{:#}", config.validate().unwrap_err()).contains("21 topics"));

        let mut config = sample();
        config.topics = vec!["Rust".to_owned(), "rust".to_owned(), "rust".to_owned()];
        let message = format!("{:#}", config.validate().unwrap_err());
        assert!(message.contains("\"Rust\""), "{message}");
        assert!(message.contains("listed twice"), "{message}");

        let mut config = sample();
        config.description = "x".repeat(351);
        assert!(format!("{:#}", config.validate().unwrap_err()).contains("351 characters"));

        let mut config = sample();
        config.repository = "no-slash".to_owned();
        assert!(format!("{:#}", config.validate().unwrap_err()).contains("owner/name"));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let text = "repository = \"a/b\"\ndescription = \"d\"\nhomepage = \"https://x\"\ntopics = []\nextra = 1\n";
        assert!(toml::from_str::<RepoConfig>(text).is_err());
    }
}
