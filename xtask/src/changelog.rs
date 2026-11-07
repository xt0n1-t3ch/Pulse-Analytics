//! `CHANGELOG.md` in the Keep a Changelog 2.0.0 format with SemVer 2.0.0: the format
//! check (`changelog check`) and the release notes (`changelog notes <version>`).

use std::path::Path;

use anyhow::{Result, bail};

use crate::clock;
use crate::fsx;
use crate::git;
use crate::release_assets::REQUIREMENTS;
use crate::repo_config::RepoConfig;
use crate::semver::Version;

/// Section names in the only order Keep a Changelog 2.0.0 allows.
pub const TYPES: [&str; 6] = [
    "Added",
    "Changed",
    "Deprecated",
    "Removed",
    "Fixed",
    "Security",
];
const UNRELEASED: &str = "Unreleased";
const KEEP_A_CHANGELOG_LINE: &str =
    "The format is based on [Keep a Changelog](https://keepachangelog.com/en/2.0.0/),";
const SEMVER_LINE: &str =
    "and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).";
/// A release body longer than this gets the asset, integrity and compare sections appended.
pub const LONG_NOTES_LINES: usize = 150;
const MIN_NOTES_CHARS: usize = 80;

struct Heading<'a> {
    label: &'a str,
    rest: &'a str,
    start: usize,
    end: usize,
}

/// `## [label]rest`
fn heading_parts(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("## [")?;
    let close = rest.find(']')?;
    Some((&rest[..close], &rest[close + 1..]))
}

/// `[label]: url`
fn link_definition(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix('[')?;
    let close = rest.find("]:")?;
    let url = rest[close + 2..].strip_prefix(char::is_whitespace)?.trim();
    (!url.is_empty() && !url.contains(char::is_whitespace)).then_some((&rest[..close], url))
}

/// Index where the trailing block of link definitions (and blank lines) starts.
fn links_start(lines: &[&str]) -> usize {
    let mut start = lines.len();
    while start > 0
        && (lines[start - 1].trim().is_empty() || link_definition(lines[start - 1]).is_some())
    {
        start -= 1;
    }
    start
}

fn headings<'a>(lines: &[&'a str]) -> Vec<Heading<'a>> {
    let starts: Vec<(usize, &str, &str)> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| heading_parts(line).map(|(label, rest)| (index, label, rest)))
        .collect();
    let links = links_start(lines);
    starts
        .iter()
        .enumerate()
        .map(|(position, &(start, label, rest))| Heading {
            label,
            rest,
            start,
            end: starts
                .get(position + 1)
                .map_or(links.max(start + 1), |next| next.0),
        })
        .collect()
}

fn trim_blank_lines<'a>(lines: &'a [&'a str]) -> &'a [&'a str] {
    let first = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(lines.len());
    let last = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(first, |index| index + 1);
    &lines[first..last]
}

/// Body of the `## [version]` section: no heading, no surrounding blank lines, no trailing
/// link definitions. `None` when the file has no such section.
pub fn section_body(text: &str, version: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let heading = headings(&lines)
        .into_iter()
        .find(|heading| heading.label == version)?;
    Some(trim_blank_lines(&lines[heading.start + 1..heading.end]).join("\n"))
}

/// ` - YYYY-MM-DD` with an optional ` [YANKED]` tail.
fn is_release_date(rest: &str) -> bool {
    let Some(rest) = rest.strip_prefix(" - ") else {
        return false;
    };
    rest.get(..10)
        .is_some_and(|date| clock::parse_date(date).is_some())
        && matches!(&rest[10..], "" | " [YANKED]")
}

fn check_section_body(label: &str, body: &[&str], is_version: bool, found: &mut Vec<String>) {
    let mut in_fence = false;
    let mut subsections: Vec<(usize, &str)> = Vec::new(); // (line index in body, name)
    for (index, line) in body.iter().enumerate() {
        if line.starts_with("```") {
            in_fence = !in_fence;
        } else if !in_fence && let Some(name) = line.strip_prefix("### ") {
            subsections.push((index, name.trim()));
        }
    }

    if is_version {
        let first = body.iter().position(|line| !line.trim().is_empty());
        let summary_ok = first.is_some_and(|first| {
            let opening = body[first];
            let paragraph_end = body[first..]
                .iter()
                .position(|line| line.trim().is_empty())
                .map_or(body.len(), |offset| first + offset);
            let next = body[paragraph_end..]
                .iter()
                .find(|line| !line.trim().is_empty());
            !opening.starts_with('#')
                && !opening.starts_with("- ")
                && !opening.starts_with("* ")
                && next.is_none_or(|line| line.starts_with("### "))
        });
        if !summary_ok {
            found.push(format!(
                "[{label}] must open with one summary paragraph, then ### sections"
            ));
        }
        if subsections.is_empty() {
            found.push(format!("[{label}] has no ### section"));
        }
    }

    let mut previous: Option<usize> = None;
    for (position, &(index, name)) in subsections.iter().enumerate() {
        let Some(type_index) = TYPES.iter().position(|kind| *kind == name) else {
            found.push(format!(
                "### {name} in [{label}] is not one of {}",
                TYPES.join(", ")
            ));
            continue;
        };
        if previous.is_some_and(|before| type_index <= before) {
            found.push(format!(
                "### {name} in [{label}] repeats or comes out of the order {}",
                TYPES.join(", ")
            ));
        }
        previous = Some(type_index);
        let end = subsections
            .get(position + 1)
            .map_or(body.len(), |next| next.0);
        if body[index + 1..end]
            .iter()
            .all(|line| line.trim().is_empty())
        {
            found.push(format!("### {name} in [{label}] is empty"));
        }
    }
}

/// Every way `text` breaks the format. `web_url` is the repository's web address;
/// `tags` is the output of `git tag` when known.
pub fn problems(text: &str, web_url: &str, tags: Option<&[String]>) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    if lines.first() != Some(&"# Changelog") {
        found.push("the file must open with # Changelog".to_owned());
    }

    let sections = headings(&lines);
    let header_end = sections
        .first()
        .map_or(lines.len(), |heading| heading.start);
    match (0..header_end.saturating_sub(1))
        .find(|&index| lines[index] == KEEP_A_CHANGELOG_LINE && lines[index + 1] == SEMVER_LINE)
    {
        None => found.push(
            "the two header lines naming Keep a Changelog 2.0.0 and Semantic Versioning 2.0.0 are missing or changed"
                .to_owned(),
        ),
        Some(index) => {
            let after = &lines[index + 2..header_end];
            let promise = after.iter().position(|line| !line.trim().is_empty());
            if promise.is_none_or(|offset| offset == 0 || after[offset].starts_with('#')) {
                found.push(
                    "a paragraph on what the version number promises must follow the header lines"
                        .to_owned(),
                );
            }
        }
    }

    let unreleased = sections
        .iter()
        .filter(|heading| heading.label == UNRELEASED)
        .count();
    if unreleased != 1 {
        found.push(if unreleased == 0 {
            "no ## [Unreleased] section".to_owned()
        } else {
            "more than one ## [Unreleased]".to_owned()
        });
    } else if sections[0].label != UNRELEASED {
        found.push("## [Unreleased] must come first".to_owned());
    }

    let mut versions: Vec<(&str, Version)> = Vec::new();
    for heading in &sections {
        let is_version = heading.label != UNRELEASED;
        if is_version {
            match Version::parse(heading.label) {
                Some(parsed) => versions.push((heading.label, parsed)),
                None => found.push(format!(
                    "## [{}] is not a Semantic Versioning version",
                    heading.label
                )),
            }
            if !is_release_date(heading.rest) {
                found.push(format!(
                    "## [{}] needs its release date as ` - YYYY-MM-DD`",
                    heading.label
                ));
            }
        }
        check_section_body(
            heading.label,
            &lines[heading.start + 1..heading.end],
            is_version,
            &mut found,
        );
    }
    if versions.windows(2).any(|pair| pair[0].1 <= pair[1].1) {
        found.push("versions are out of order or repeated: the newest comes first".to_owned());
    }

    let labels: Vec<&str> = sections.iter().map(|heading| heading.label).collect();
    let definitions: Vec<(&str, &str)> = lines[links_start(&lines)..]
        .iter()
        .filter_map(|line| link_definition(line))
        .collect();
    let defined: Vec<&str> = definitions.iter().map(|(label, _)| *label).collect();
    if defined != labels {
        found.push(format!(
            "the link definitions at the end must be [{}] in that order, found [{}]",
            labels.join("], ["),
            defined.join("], [")
        ));
    } else {
        let names: Vec<&str> = versions.iter().map(|(name, _)| *name).collect();
        // A version released before the repository had tags links to the commit that shipped it.
        // The newest version may still be waiting for its tag, so it never counts as untagged.
        let untagged = |name: &str| {
            names.first() != Some(&name)
                && tags
                    .filter(|tags| !tags.is_empty())
                    .is_some_and(|tags| !tags.iter().any(|tag| *tag == format!("v{name}")))
        };
        for (label, url) in &definitions {
            let found_problem = if *label == UNRELEASED {
                names.first().and_then(|newest| {
                    let expected = format!("{web_url}/compare/v{newest}...HEAD");
                    (*url != expected).then_some(expected)
                })
            } else {
                names
                    .iter()
                    .position(|name| name == label)
                    .and_then(|index| {
                        let to = if untagged(label) {
                            None
                        } else {
                            Some(format!("v{label}"))
                        };
                        let expected = match names.get(index + 1) {
                            Some(previous) => {
                                let from = if untagged(previous) {
                                    None
                                } else {
                                    Some(format!("v{previous}"))
                                };
                                if compare_matches(url, web_url, from.as_deref(), to.as_deref()) {
                                    return None;
                                }
                                format!(
                                    "{web_url}/compare/{}...{}",
                                    from.as_deref().unwrap_or("<commit>"),
                                    to.as_deref().unwrap_or("<commit>")
                                )
                            }
                            None => {
                                let tag_link = format!("{web_url}/releases/tag/v{label}");
                                let commit_link = url
                                    .strip_prefix(&format!("{web_url}/commit/"))
                                    .is_some_and(is_commit_id);
                                if *url == tag_link || (to.is_none() && commit_link) {
                                    return None;
                                }
                                tag_link
                            }
                        };
                        Some(expected)
                    })
            };
            if let Some(expected) = found_problem {
                found.push(format!("[{label}] links {url}, expected {expected}"));
            }
        }
    }

    if let Some(tags) = tags.filter(|tags| !tags.is_empty()) {
        // The newest version may be waiting for its tag; every older one must exist.
        for (name, _) in versions.iter().skip(1) {
            let linked_to_commit = definitions.iter().any(|(label, url)| {
                label == name && url.rsplit(['/', '.']).next().is_some_and(is_commit_id)
            });
            if !linked_to_commit && !tags.iter().any(|tag| *tag == format!("v{name}")) {
                found.push(format!(
                    "[{name}] links the tag v{name}, which `git tag` does not list"
                ));
            }
        }
    }
    found
}

fn is_commit_id(text: &str) -> bool {
    (7..=40).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// True when `url` compares `from` with `to`; `None` accepts a commit id in that place.
fn compare_matches(url: &str, web_url: &str, from: Option<&str>, to: Option<&str>) -> bool {
    let Some(range) = url.strip_prefix(&format!("{web_url}/compare/")) else {
        return false;
    };
    let Some((left, right)) = range.split_once("...") else {
        return false;
    };
    let side = |actual: &str, wanted: Option<&str>| match wanted {
        Some(tag) => actual == tag,
        None => is_commit_id(actual),
    };
    side(left, from) && side(right, to)
}

/// Checks `CHANGELOG.md` under `root` against the repository named in `.github/repository.toml`
/// and the tags `git tag` lists (skipped outside a git checkout).
pub fn check(root: &Path) -> Result<String> {
    let text = fsx::read_text(&root.join("CHANGELOG.md"))?;
    let config = RepoConfig::load(root)?;
    let tags = git::run(Some(root), &["tag", "--list"])
        .ok()
        .filter(|out| out.success)
        .map(|out| out.lines);
    let found = problems(&text, &config.web_url(), tags.as_deref());
    if !found.is_empty() {
        bail!("CHANGELOG.md breaks the format:\n- {}", found.join("\n- "));
    }
    let lines: Vec<&str> = text.lines().collect();
    let released = headings(&lines)
        .iter()
        .filter(|heading| heading.label != UNRELEASED)
        .count();
    Ok(format!(
        "CHANGELOG.md follows Keep a Changelog 2.0.0 ({released} releases)"
    ))
}

/// `pulse-windows-x64-*.exe`, or the exact name when the pattern has no variable part.
fn asset_pattern(prefix: &str, suffix: &str) -> String {
    if prefix.ends_with('-') {
        format!("`{prefix}*{suffix}`")
    } else {
        format!("`{prefix}{suffix}`")
    }
}

fn appendix(text: &str, version: &str) -> Result<String> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(url) = lines[links_start(&lines)..]
        .iter()
        .filter_map(|line| link_definition(line))
        .find_map(|(label, url)| (label == version).then_some(url))
    else {
        bail!("CHANGELOG.md has no link definition for [{version}]");
    };
    let mut assets: Vec<String> = REQUIREMENTS
        .iter()
        .map(|requirement| asset_pattern(requirement.prefix, requirement.suffix))
        .collect();
    assets.push("`latest.json`".to_owned());
    assets.push("`SHA256SUMS.txt`".to_owned());
    Ok(format!(
        "\n\n## Release assets\n\n{}\n\n## Integrity\n\nEvery asset is listed in `SHA256SUMS.txt`; check them with `sha256sum --check SHA256SUMS.txt`. The Windows packages ship SPDX 2.3 SBOMs. GitHub macOS packages have no Apple Developer ID signature or notarization.\n\n## Full changelog\n\n[All changes in v{version}]({url})",
        assets
            .iter()
            .map(|asset| format!("- {asset}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

/// Release notes for `version`: its changelog section verbatim. Only a section longer than
/// [`LONG_NOTES_LINES`] lines gets the assets, integrity and compare sections appended.
pub fn notes(root: &Path, version: &str) -> Result<String> {
    let text = fsx::read_text(&root.join("CHANGELOG.md"))?;
    let Some(mut body) = section_body(&text, version) else {
        bail!("CHANGELOG.md has no section for {version}");
    };
    if body.chars().count() < MIN_NOTES_CHARS {
        bail!("Release notes for {version} are shorter than {MIN_NOTES_CHARS} characters");
    }
    if body.lines().count() > LONG_NOTES_LINES {
        body.push_str(&appendix(&text, version)?);
    }
    body.push('\n');
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEB: &str = "https://github.com/o/r";

    fn valid() -> String {
        format!(
            "# Changelog\n\nAll notable changes are documented here.\n\n{KEEP_A_CHANGELOG_LINE}\n{SEMVER_LINE}\n\nThe version covers the commands and the files they write.\n\n## [Unreleased]\n\n### Added\n\n- Something new.\n\n## [1.1.0] - 2026-11-07\n\nOne summary paragraph for 1.1.0.\n\n### Changed\n\n- A change.\n\n### Fixed\n\n- A fix.\n\n## [1.0.0] - 2026-10-01\n\nFirst release.\n\n### Added\n\n- Everything.\n\n[Unreleased]: {WEB}/compare/v1.1.0...HEAD\n[1.1.0]: {WEB}/compare/v1.0.0...v1.1.0\n[1.0.0]: {WEB}/releases/tag/v1.0.0\n"
        )
    }

    fn tags(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn an_untagged_older_version_links_to_the_commit_that_shipped_it() {
        let sha = "fc17857f2ca66743fd84bb715b1eec4a008bda3c";
        let text = valid()
            .replace(
                &format!("[1.1.0]: {WEB}/compare/v1.0.0...v1.1.0"),
                &format!("[1.1.0]: {WEB}/compare/{sha}...v1.1.0"),
            )
            .replace(
                &format!("[1.0.0]: {WEB}/releases/tag/v1.0.0"),
                &format!("[1.0.0]: {WEB}/commit/{sha}"),
            );
        assert_eq!(
            problems(&text, WEB, Some(&tags(&["v1.1.0"]))),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_tagged_older_version_may_not_hide_behind_a_commit_link() {
        let text = valid().replace(
            &format!("[1.0.0]: {WEB}/releases/tag/v1.0.0"),
            &format!("[1.0.0]: {WEB}/commit/fc17857"),
        );
        let found = problems(&text, WEB, Some(&tags(&["v1.0.0", "v1.1.0"])));
        assert!(
            found
                .iter()
                .any(|problem| problem.starts_with("[1.0.0] links")),
            "{found:?}"
        );
    }

    #[test]
    fn a_valid_changelog_has_no_problems() {
        assert_eq!(
            problems(&valid(), WEB, Some(&tags(&["v1.0.0"]))),
            Vec::<String>::new()
        );
    }

    #[test]
    fn section_body_stops_before_the_link_definitions() {
        let body = section_body(&valid(), "1.0.0").expect("section");
        assert_eq!(body, "First release.\n\n### Added\n\n- Everything.");
        assert!(section_body(&valid(), "9.9.9").is_none());
    }

    #[test]
    fn each_rule_reports_its_own_problem() {
        let broken = [
            (
                valid().replace("# Changelog", "# Log"),
                "must open with # Changelog",
            ),
            (valid().replace("en/2.0.0/", "en/1.1.0/"), "header lines"),
            (
                valid().replace("## [Unreleased]", "## [Next]"),
                "no ## [Unreleased]",
            ),
            (
                valid().replace("2026-11-07", "2026-02-30"),
                "needs its release date",
            ),
            (valid().replace("### Fixed", "### Fixes"), "is not one of"),
            (
                valid().replace("### Changed", "### Fixed\n\n- x\n\n### Added"),
                "out of the order",
            ),
            (
                valid().replace("One summary paragraph for 1.1.0.\n\n", ""),
                "summary paragraph",
            ),
            (valid().replace("- A fix.", ""), "is empty"),
            (
                valid().replace("compare/v1.0.0...v1.1.0", "compare/v1.0.0...main"),
                "expected",
            ),
            (
                valid().replace("[1.0.0]: ", "[0.9.0]: "),
                "link definitions",
            ),
        ];
        for (text, expected) in broken {
            let found = problems(&text, WEB, None);
            assert!(
                found.iter().any(|problem| problem.contains(expected)),
                "{expected}: {found:?}"
            );
        }
    }

    #[test]
    fn versions_must_descend_and_old_tags_must_exist() {
        let swapped = valid()
            .replace("## [1.1.0] - 2026-11-07", "## [0.9.0] - 2026-11-07")
            .replace("[1.1.0]:", "[0.9.0]:");
        assert!(
            problems(&swapped, WEB, None)
                .iter()
                .any(|p| p.contains("out of order"))
        );

        let found = problems(&valid(), WEB, Some(&tags(&["v0.1.0"])));
        assert!(
            found.iter().any(|p| p.contains("`git tag` does not list")),
            "{found:?}"
        );
        // The newest version may still be waiting for its tag.
        assert!(problems(&valid(), WEB, Some(&tags(&["v1.0.0"]))).is_empty());
    }
}
