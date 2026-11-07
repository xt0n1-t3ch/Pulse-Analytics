//! `llms.txt` and `llms-full.txt` (llmstxt.org), generated from `docs/llms.toml`.
//!
//! `llms.txt` is the short index of links to raw Markdown on `main`; `llms-full.txt`
//! holds the text of README.md and every linked document in one file. Both carry facts
//! and links only, so a model that reads them is never handed instructions.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::fsx;
use crate::repo_config::RepoConfig;

pub const CONFIG_PATH: &str = "docs/llms.toml";
const FULL_FILE: &str = "llms-full.txt";
const INDEX_FILE: &str = "llms.txt";
/// llmstxt.org wants a short index; the detail belongs behind the links.
const MAX_INDEX_BYTES: usize = 10_000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmsConfig {
    pub name: String,
    pub summary: String,
    #[serde(default)]
    pub facts: Vec<String>,
    #[serde(rename = "section")]
    pub sections: Vec<Section>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub title: String,
    #[serde(rename = "link", default)]
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub path: String,
    pub title: String,
    pub description: String,
}

/// HTML a README lays its pictures out with; text for a model carries none of it.
const LAYOUT_TAGS: [&str; 17] = [
    "a", "p", "br", "table", "thead", "tbody", "tr", "td", "th", "div", "picture", "source", "sub",
    "sup", "span", "center", "details",
];

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    for quote in ['"', '\''] {
        let marker = format!("{name}={quote}");
        if let Some(start) = tag.find(&marker) {
            let value = &tag[start + marker.len()..];
            return value.find(quote).map(|end| &value[..end]);
        }
    }
    None
}

/// Rewrites one chunk of Markdown outside code fences: pictures become `(Picture: alt)`,
/// a heading drawn as a picture becomes a Markdown heading, layout tags and HTML comments go.
fn strip_html(chunk: &str) -> String {
    let mut output = String::with_capacity(chunk.len());
    let mut rest = chunk;
    let mut heading_level: Option<usize> = None;
    while let Some(open) = rest.find('<') {
        output.push_str(&rest[..open]);
        rest = &rest[open..];
        if let Some(after) = rest.strip_prefix("<!--") {
            match after.find("-->") {
                Some(end) => {
                    rest = after[end + 3..]
                        .strip_prefix('\n')
                        .unwrap_or(&after[end + 3..]);
                }
                None => {
                    output.push_str(rest);
                    rest = "";
                }
            }
            continue;
        }
        let Some(close) = rest.find('>') else {
            break;
        };
        let tag = &rest[1..close];
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        let heading = name
            .strip_prefix('h')
            .and_then(|digit| digit.parse::<usize>().ok())
            .filter(|level| (1..=6).contains(level));
        if name == "img" && !closing {
            match (attribute(tag, "alt"), heading_level) {
                (Some(alt), Some(_)) => {
                    output.push_str(alt.split(':').next().unwrap_or(alt).trim())
                }
                (Some(alt), None) => output.push_str(&format!("(Picture: {alt})")),
                (None, _) => {}
            }
        } else if let Some(level) = heading {
            if closing {
                heading_level = None;
                output.push('\n');
            } else {
                heading_level = Some(level);
                output.push_str(&format!("\n{} ", "#".repeat(level)));
            }
        } else if name == "br" {
            output.push('\n');
        } else if !LAYOUT_TAGS.contains(&name.as_str()) {
            output.push('<');
            rest = &rest[1..];
            continue;
        }
        rest = &rest[close + 1..];
    }
    output.push_str(rest);
    output
}

/// README text without its HTML layout, code blocks untouched.
pub fn plain_markdown(markdown: &str) -> String {
    let mut output = String::new();
    let mut chunk = String::new();
    let mut in_fence = false;
    for line in markdown.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            if !in_fence {
                output.push_str(&strip_html(&chunk));
                chunk.clear();
            }
            in_fence = !in_fence;
            output.push_str(line);
        } else if in_fence {
            output.push_str(line);
        } else {
            chunk.push_str(line);
        }
    }
    output.push_str(&strip_html(&chunk));

    // Picture lines are left indented by table layouts; four spaces would make a code block.
    let tidy: Vec<&str> = output
        .lines()
        .map(|line| {
            if line.contains("(Picture:") || line.trim().is_empty() {
                line.trim()
            } else {
                line
            }
        })
        .collect();
    let mut collapsed = String::new();
    let mut blanks = 0;
    for line in tidy {
        blanks = if line.is_empty() { blanks + 1 } else { 0 };
        if blanks <= 1 {
            collapsed.push_str(line);
            collapsed.push('\n');
        }
    }
    collapsed
}

pub fn load(root: &Path) -> Result<LlmsConfig> {
    let text = fsx::read_text(&root.join(CONFIG_PATH))?;
    toml::from_str(&text).with_context(|| format!("parse {CONFIG_PATH}"))
}

/// The links in order, with the pointer to `llms-full.txt` guaranteed last in `Optional`
/// (or the last section when none is called that).
fn sections_with_full_link(config: &LlmsConfig) -> Vec<(String, Vec<Link>)> {
    let mut sections: Vec<(String, Vec<Link>)> = config
        .sections
        .iter()
        .map(|section| (section.title.clone(), section.links.clone()))
        .collect();
    let has_full_link = sections
        .iter()
        .flat_map(|(_, links)| links)
        .any(|link| link.path == FULL_FILE);
    if !has_full_link {
        let target = sections
            .iter()
            .position(|(title, _)| title == "Optional")
            .or(sections.len().checked_sub(1));
        if let Some(target) = target {
            sections[target].1.push(Link {
                path: FULL_FILE.to_owned(),
                title: "Everything above in one file".to_owned(),
                description: "The text of every document listed here.".to_owned(),
            });
        }
    }
    sections
}

pub fn index_text(config: &LlmsConfig, raw_url: &str) -> String {
    let mut lines = vec![
        format!("# {}", config.name),
        String::new(),
        format!("> {}", config.summary),
    ];
    if !config.facts.is_empty() {
        lines.push(String::new());
        lines.push("Key facts:".to_owned());
        lines.push(String::new());
        lines.extend(config.facts.iter().map(|fact| format!("- {fact}")));
    }
    for (title, links) in sections_with_full_link(config) {
        lines.push(String::new());
        lines.push(format!("## {title}"));
        lines.push(String::new());
        lines.extend(links.iter().map(|link| {
            format!(
                "- [{}]({raw_url}{}): {}",
                link.title, link.path, link.description
            )
        }));
    }
    lines.join("\n") + "\n"
}

/// Paths whose text goes into `llms-full.txt`: README.md first, then each linked document once.
fn full_paths(config: &LlmsConfig) -> Vec<String> {
    let mut paths = vec!["README.md".to_owned()];
    for link in config.sections.iter().flat_map(|section| &section.links) {
        if link.path != FULL_FILE && link.path != INDEX_FILE && !paths.contains(&link.path) {
            paths.push(link.path.clone());
        }
    }
    paths
}

pub fn full_text(config: &LlmsConfig, raw_url: &str, root: &Path) -> Result<String> {
    let mut text = format!(
        "# {}, the full documentation\n\n> {}\n",
        config.name, config.summary
    );
    for path in full_paths(config) {
        let content = fsx::read_text(&root.join(&path))
            .with_context(|| format!("{CONFIG_PATH} links {path}, which cannot be read"))?;
        text.push_str(&format!(
            "\n---\n\n# {path}\n\nSource: {raw_url}{path}\n\n{}\n",
            plain_markdown(&content).trim()
        ));
    }
    Ok(text)
}

/// What makes `llms.txt` break the format or point at nothing.
pub fn index_problems(text: &str, raw_url: &str, root: &Path) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    if !lines.first().is_some_and(|line| line.starts_with("# ")) {
        found.push("the first line is not the H1 with the name".to_owned());
    }
    if !lines
        .iter()
        .skip(1)
        .take(3)
        .any(|line| line.starts_with("> "))
    {
        found.push("no blockquote summary right after the H1".to_owned());
    }
    for line in &lines {
        let Some(url) = line
            .strip_prefix("- [")
            .and_then(|rest| rest.split_once("](")?.1.split_once(')'))
            .map(|(url, _)| url)
        else {
            continue;
        };
        match url.strip_prefix(raw_url) {
            None => found.push(format!("a link outside the repository: {url}")),
            Some(path) if path != FULL_FILE && !root.join(path).is_file() => {
                found.push(format!("a link to a file that does not exist: {path}"));
            }
            Some(_) => {}
        }
    }
    if text.len() > MAX_INDEX_BYTES {
        found.push(format!(
            "llms.txt is over {MAX_INDEX_BYTES} bytes; the detail belongs behind its links"
        ));
    }
    found
}

/// Writes both files, or with `check_only` fails when either is out of date or malformed.
pub fn run(root: &Path, check_only: bool) -> Result<String> {
    let repo = RepoConfig::load(root)?;
    let raw_url = repo.raw_url();
    let config = load(root)?;
    let files = [
        (INDEX_FILE, index_text(&config, &raw_url)),
        (FULL_FILE, full_text(&config, &raw_url, root)?),
    ];

    if !check_only {
        for (name, text) in &files {
            fsx::write_text(&root.join(name), text)?;
        }
        return Ok(format!(
            "Wrote {INDEX_FILE} ({} bytes) and {FULL_FILE} ({} bytes)",
            files[0].1.len(),
            files[1].1.len()
        ));
    }

    let mut found = index_problems(&files[0].1, &raw_url, root);
    for (name, text) in &files {
        let on_disk =
            std::fs::read_to_string(root.join(name)).map(|disk| disk.replace("\r\n", "\n"));
        if on_disk.as_deref().ok() != Some(text.as_str()) {
            found.push(format!("{name} is out of date: run `cargo xtask llms`"));
        }
    }
    if !found.is_empty() {
        bail!("{}", found.join("\n"));
    }
    Ok(format!(
        "{INDEX_FILE} and {FULL_FILE} are current and {INDEX_FILE} follows the format"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_become_text_and_headings_stay_headings() {
        let readme = "<h1 align=\"center\"><img src=\"x.png\" alt=\"Pulse: the logo\"></h1>\n<p align=\"center\">\n  <img src=\"y.png\" alt=\"Home screen\">\n</p>\n<!-- hidden -->\nText with a<br>break and <https://example.com> kept.\n\n\n\n```html\n<div>code stays</div>\n```\n";
        let plain = plain_markdown(readme);
        assert!(
            plain.contains("\n# Pulse\n") || plain.starts_with("# Pulse\n"),
            "{plain}"
        );
        assert!(plain.contains("(Picture: Home screen)"), "{plain}");
        assert!(!plain.contains("hidden"), "{plain}");
        assert!(plain.contains("a\nbreak"), "{plain}");
        assert!(plain.contains("<https://example.com>"), "{plain}");
        assert!(plain.contains("<div>code stays</div>"), "{plain}");
        assert!(!plain.contains("\n\n\n"), "{plain}");
    }

    fn config() -> LlmsConfig {
        toml::from_str(
            r#"
name = "Pulse"
summary = "One paragraph."
facts = ["Fact one.", "Fact two."]

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
        )
        .expect("config")
    }

    #[test]
    fn index_follows_llmstxt_and_ends_with_the_full_file_link() {
        let text = index_text(&config(), "https://raw.example/o/r/main/");
        let expected = "# Pulse\n\n> One paragraph.\n\nKey facts:\n\n- Fact one.\n- Fact two.\n\n## Docs\n\n- [Documentation index](https://raw.example/o/r/main/docs/index.md): Task-based guide.\n\n## Optional\n\n- [Changelog](https://raw.example/o/r/main/CHANGELOG.md): What changed.\n- [Everything above in one file](https://raw.example/o/r/main/llms-full.txt): The text of every document listed here.\n";
        assert_eq!(text, expected);
    }

    #[test]
    fn an_explicit_full_file_link_is_not_duplicated() {
        let mut config = config();
        config.sections[1].links.push(Link {
            path: FULL_FILE.to_owned(),
            title: "All".to_owned(),
            description: "Everything.".to_owned(),
        });
        let text = index_text(&config, "https://raw.example/o/r/main/");
        assert_eq!(text.matches("llms-full.txt").count(), 1);
        assert_eq!(
            full_paths(&config),
            ["README.md", "docs/index.md", "CHANGELOG.md"]
        );
    }
}
