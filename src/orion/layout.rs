//! Where Orion keeps its data and which of its two layouts a root holds.
//!
//! Orion owns the rules: `orion-app-process/src/data_root.rs` (base folder, then
//! `.orion`) and `orion-agent-session-files/src/constants.rs` (file names). Since
//! its 2026-10-17 session-storage cutover Orion writes `projects/index.sqlite`
//! plus one JSONL transcript per session and no longer writes conversations to
//! SQL tables. The pre-cutover store (`cli/db/db.sqlite`, also under the legacy
//! `~/.zcode` link) stays readable while it still holds sessions.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

const PULSE_HOME_ENV: &str = "PULSE_ORION_HOME";
/// Orion's own override of the folder that contains `.orion`.
const DATA_BASE_DIR_ENV: &str = "ORION_DATA_BASE_DIR";
const DATA_DIR_NAME: &str = ".orion";
const LEGACY_DIR_NAME: &str = ".zcode";

const TRANSCRIPT_SUFFIX: &str = ".jsonl";
const TRANSCRIPT_GZ_SUFFIX: &str = ".jsonl.gz";
/// Layout v1 kept the transcript inside the session folder.
const LEGACY_TRANSCRIPT: &str = "transcript.jsonl";
const LEGACY_TRANSCRIPT_GZ: &str = "transcript.jsonl.gz";

/// Orion data root, first match wins: `PULSE_ORION_HOME`; an absolute
/// `ORION_DATA_BASE_DIR` joined with `.orion`; `~/.orion`; the legacy
/// `~/.zcode`. The two home folders are tried in that order only for a store
/// that exists, and `~/.orion` is the answer when neither does.
pub fn home() -> PathBuf {
    resolve_home(
        std::env::var_os(PULSE_HOME_ENV),
        std::env::var_os(DATA_BASE_DIR_ENV),
        dirs::home_dir(),
    )
}

/// `home()` with its three inputs injected, so the order is testable without
/// touching the process environment.
pub(super) fn resolve_home(
    pulse_home: Option<OsString>,
    base_dir: Option<OsString>,
    home: Option<PathBuf>,
) -> PathBuf {
    if let Some(path) = pulse_home.filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    // Orion itself never falls back from an explicit base folder, and neither
    // does Pulse, so an empty folder there is reported, not skipped.
    if let Some(base) = base_dir
        .map(PathBuf::from)
        .filter(|base| base.is_absolute())
    {
        return base.join(DATA_DIR_NAME);
    }
    let home = home.unwrap_or_default();
    let orion = home.join(DATA_DIR_NAME);
    if holds_store(&orion) {
        return orion;
    }
    let legacy = home.join(LEGACY_DIR_NAME);
    if holds_store(&legacy) { legacy } else { orion }
}

fn holds_store(root: &Path) -> bool {
    index_path(root).is_file() || legacy_database_path(root).is_file()
}

/// Rebuildable session index of the current layout.
pub fn index_path(root: &Path) -> PathBuf {
    projects_dir(root).join("index.sqlite")
}

pub fn projects_dir(root: &Path) -> PathBuf {
    root.join("projects")
}

/// Per-request usage telemetry, kept by Orion for 30 days.
pub fn usage_path(root: &Path) -> PathBuf {
    root.join("telemetry").join("usage.sqlite")
}

/// Pre-cutover session store.
pub fn legacy_database_path(root: &Path) -> PathBuf {
    root.join("cli").join("db").join("db.sqlite")
}

/// Places a session's transcript can be, most current first: beside its folder
/// as plain JSONL, as gzip once cold, then the layout v1 spots inside the
/// folder. `rel_dir` comes from the index with `/` separators and is split into
/// components, so no platform separator is assumed. A value that could leave
/// the projects folder yields `None`.
pub fn transcript_candidates(projects: &Path, rel_dir: &str) -> Option<Vec<PathBuf>> {
    let mut folder = projects.to_path_buf();
    let mut parts = rel_dir
        .split('/')
        .filter(|part| !part.is_empty())
        .peekable();
    parts.peek()?;
    let mut last = "";
    for part in parts {
        let mut components = Path::new(part).components();
        let plain =
            matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
        if !plain || part.contains('\\') {
            return None;
        }
        if !last.is_empty() {
            folder.push(last);
        }
        last = part;
    }
    let session_dir = folder.join(last);
    Some(vec![
        folder.join(format!("{last}{TRANSCRIPT_SUFFIX}")),
        folder.join(format!("{last}{TRANSCRIPT_GZ_SUFFIX}")),
        session_dir.join(LEGACY_TRANSCRIPT),
        session_dir.join(LEGACY_TRANSCRIPT_GZ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    const ABSOLUTE: &str = r"D:\profiles\orion";
    #[cfg(not(windows))]
    const ABSOLUTE: &str = "/srv/profiles/orion";

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }

    #[test]
    fn root_resolution_order_is_pulse_override_then_base_dir_then_home_then_legacy() {
        let home = tempfile::tempdir().unwrap();
        let some = |value: &str| Some(OsString::from(value));
        let home_path = Some(home.path().to_path_buf());

        // 1. Pulse's own override wins over everything and is used verbatim.
        assert_eq!(
            resolve_home(some("/custom"), some(ABSOLUTE), home_path.clone()),
            PathBuf::from("/custom")
        );
        // An empty override counts as unset.
        assert_eq!(
            resolve_home(some(""), None, home_path.clone()),
            home.path().join(".orion")
        );
        // 2. An absolute base folder gets `.orion` and never falls back,
        //    even when `~/.orion` holds a store.
        touch(&index_path(&home.path().join(".orion")));
        assert_eq!(
            resolve_home(None, some(ABSOLUTE), home_path.clone()),
            Path::new(ABSOLUTE).join(".orion")
        );
        // A relative base folder is ignored, as Orion ignores it.
        assert_eq!(
            resolve_home(None, some("relative"), home_path.clone()),
            home.path().join(".orion")
        );
        // 3. `~/.orion` with the current layout.
        assert_eq!(
            resolve_home(None, None, home_path.clone()),
            home.path().join(".orion")
        );
    }

    #[test]
    fn the_legacy_zcode_link_is_used_only_when_it_holds_a_store_and_orion_does_not() {
        let home = tempfile::tempdir().unwrap();
        let home_path = Some(home.path().to_path_buf());
        // Nothing anywhere: report the current default location.
        assert_eq!(
            resolve_home(None, None, home_path.clone()),
            home.path().join(".orion")
        );
        touch(&legacy_database_path(&home.path().join(".zcode")));
        assert_eq!(
            resolve_home(None, None, home_path.clone()),
            home.path().join(".zcode")
        );
        // The pre-cutover database in `~/.orion` beats `~/.zcode`.
        touch(&legacy_database_path(&home.path().join(".orion")));
        assert_eq!(
            resolve_home(None, None, home_path),
            home.path().join(".orion")
        );
    }

    #[test]
    fn transcripts_sit_beside_the_session_folder_and_never_escape_the_projects_folder() {
        let projects = Path::new("projects");
        let nested =
            transcript_candidates(projects, "-work-pulse/sess_a/subagents/sess_b").unwrap();
        let base = projects.join("-work-pulse/sess_a/subagents");
        assert_eq!(nested[0], base.join("sess_b.jsonl"));
        assert_eq!(nested[1], base.join("sess_b.jsonl.gz"));
        assert_eq!(nested[2], base.join("sess_b/transcript.jsonl"));
        assert_eq!(nested[3], base.join("sess_b/transcript.jsonl.gz"));
        // Dots in an id stay part of the name.
        let dotted = transcript_candidates(projects, "w/sess.v2").unwrap();
        assert_eq!(dotted[0], projects.join("w").join("sess.v2.jsonl"));
        for bad in ["", "/", "../x", "a/../b", "a/./b", "a\\b"] {
            assert_eq!(transcript_candidates(projects, bad), None, "{bad:?}");
        }
        // A leading `/` is an empty component, so an absolute path cannot
        // replace the projects folder.
        let rooted = transcript_candidates(projects, "/abs/x").unwrap();
        assert_eq!(rooted[0], projects.join("abs").join("x.jsonl"));
    }
}
