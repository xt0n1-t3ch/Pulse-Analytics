//! Strict SemVer 2.0.0 release tags (`v1.2.3`, `v1.2.3-rc.1`, no build metadata).

use std::cmp::Ordering;

/// A parsed version. Numeric identifiers stay as text so arbitrarily long numbers
/// compare correctly without overflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    core: [String; 3],
    pre: Vec<String>,
}

fn is_numeric_identifier(text: &str) -> bool {
    !text.is_empty()
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'))
}

fn is_all_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_prerelease_identifier(text: &str) -> bool {
    if text.is_empty()
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return false;
    }
    // All-digit identifiers must be canonical numbers; anything with a letter or hyphen is fine.
    !is_all_digits(text) || is_numeric_identifier(text)
}

fn compare_numeric(left: &str, right: &str) -> Ordering {
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

impl Version {
    /// Parses `1.2.3` or `1.2.3-pre.release`. Build metadata (`+x`) is rejected.
    pub fn parse(text: &str) -> Option<Self> {
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut parts = core.split('.');
        let (major, minor, patch) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() || ![major, minor, patch].into_iter().all(is_numeric_identifier) {
            return None;
        }
        let pre = match pre {
            Some(pre) => {
                let identifiers: Vec<String> = pre.split('.').map(str::to_owned).collect();
                if !identifiers.iter().all(|id| is_prerelease_identifier(id)) {
                    return None;
                }
                identifiers
            }
            None => Vec::new(),
        };
        Some(Self {
            core: [major.to_owned(), minor.to_owned(), patch.to_owned()],
            pre,
        })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        for (left, right) in self.core.iter().zip(&other.core) {
            let order = compare_numeric(left, right);
            if order != Ordering::Equal {
                return order;
            }
        }
        match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            (false, false) => {}
        }
        for (left, right) in self.pre.iter().zip(&other.pre) {
            let order = match (is_all_digits(left), is_all_digits(right)) {
                (true, true) => compare_numeric(left, right),
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                (false, false) => left.cmp(right),
            };
            if order != Ordering::Equal {
                return order;
            }
        }
        self.pre.len().cmp(&other.pre.len())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The version inside a `v<version>` release tag, or `None` when the tag is not strict SemVer.
pub fn release_tag_version(tag: &str) -> Option<&str> {
    let version = tag.strip_prefix('v')?;
    Version::parse(version).map(|_| version)
}

/// A full 40-character hexadecimal Git object id.
pub fn is_full_sha(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_stable_and_prerelease_tags() {
        for (tag, version) in [
            ("v1.5.2", "1.5.2"),
            ("v0.0.0", "0.0.0"),
            ("v1.5.3-rc.1", "1.5.3-rc.1"),
            ("v1.0.0-0", "1.0.0-0"),
            ("v1.0.0-alpha-1", "1.0.0-alpha-1"),
            ("v10.20.30-x.7.z.92", "10.20.30-x.7.z.92"),
        ] {
            assert_eq!(release_tag_version(tag), Some(version), "{tag}");
        }
    }

    #[test]
    fn rejects_everything_outside_strict_semver() {
        for tag in [
            "1.5.2",
            "release-1.5.2",
            "v01.5.2",
            "v1.5",
            "v1.5.2.1",
            "v1.5.2-rc..1",
            "v1.7.2-01",
            "v1.7.2-.beta",
            "v1.5.2-",
            "v1.5.2+build",
            "v1.5.2-rc.1+build",
            "v",
            "",
        ] {
            assert_eq!(release_tag_version(tag), None, "{tag}");
        }
    }

    #[test]
    fn orders_versions_as_the_semver_specification_lists_them() {
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.9.7",
            "1.10.0",
            "2.0.0",
        ];
        for pair in ordered.windows(2) {
            let low = Version::parse(pair[0]).expect("low");
            let high = Version::parse(pair[1]).expect("high");
            assert!(low < high, "{} < {}", pair[0], pair[1]);
        }
    }

    #[test]
    fn full_sha_needs_forty_hex_digits() {
        assert!(is_full_sha(&"a".repeat(40)));
        assert!(is_full_sha(&"A1".repeat(20)));
        assert!(!is_full_sha(&"a".repeat(39)));
        assert!(!is_full_sha(&"g".repeat(40)));
    }
}
