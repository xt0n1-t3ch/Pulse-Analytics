//! Polls every configured Orion data root and picks the layout it holds: the
//! current one (`reader.rs`) whenever `projects/index.sqlite` exists, else the
//! pre-cutover SQL store (`legacy.rs`) while it still has sessions.

use super::legacy::{self, Legacy};
use super::reader::{self, HydratedMap};
use super::transcript::TranscriptCache;
use super::{Config, Session, layout};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// One poll of one root.
pub(super) struct Read {
    pub changed: Vec<Session>,
    pub live: Vec<Session>,
    /// History cursor after this page: (change time, session id).
    pub next: (i64, String),
    pub last_activity: Option<i64>,
    pub diagnostics: Vec<String>,
}

#[derive(Default)]
pub struct Collector {
    cursors: HashMap<PathBuf, (i64, String)>,
    hydrated: HydratedMap,
    /// Parsed transcripts, one cache per data root.
    transcripts: HashMap<PathBuf, TranscriptCache>,
    legacy: Legacy,
}

#[derive(Default)]
pub struct Batch {
    pub changed: Vec<Session>,
    pub live: Vec<Session>,
    /// Latest change to any Orion session, for the idle timer.
    pub last_activity: Option<i64>,
    pub diagnostics: Vec<String>,
    cursors: HashMap<PathBuf, (i64, String)>,
}

impl Collector {
    pub fn poll(&mut self, config: &Config) -> Batch {
        let mut batch = Batch::default();
        let now = chrono::Utc::now().timestamp_millis();
        let mut seen = HashSet::new();
        for root in config.roots() {
            let (path, result) = self.read_root(&root, now);
            match result {
                Ok(read) => batch.merge(path, read, &mut seen),
                Err(error) => batch
                    .diagnostics
                    .push(format!("{}: {error:#}", path.display())),
            }
        }
        batch
            .live
            .sort_by_key(|session| std::cmp::Reverse(session.tree_updated));
        batch
    }

    pub fn acknowledge(&mut self, batch: &Batch) {
        self.cursors.extend(batch.cursors.clone());
    }

    /// Reads a root with the layout it holds and returns the file it read, which
    /// also keys the history cursor.
    fn read_root(&mut self, root: &Path, now: i64) -> (PathBuf, anyhow::Result<Read>) {
        let index = layout::index_path(root);
        if index.is_file() {
            let cursor = self.cursors.get(&index).cloned().unwrap_or_default();
            let cache = self.transcripts.entry(root.to_path_buf()).or_default();
            let result = reader::read(&index, root, &cursor, now, &mut self.hydrated, cache);
            return (index, result);
        }
        let old_database = layout::legacy_database_path(root);
        if old_database.is_file() && legacy::has_sessions(&old_database) {
            let cursor = self.cursors.get(&old_database).cloned().unwrap_or_default();
            let result = self.legacy.read(&old_database, root, &cursor, now);
            return (old_database, result);
        }
        (
            index,
            Err(anyhow::anyhow!(
                "Orion App session index not found (and no legacy database with sessions)"
            )),
        )
    }
}

impl Batch {
    fn merge(&mut self, path: PathBuf, read: Read, seen: &mut HashSet<String>) {
        self.cursors.insert(path, read.next);
        self.last_activity = self.last_activity.max(read.last_activity);
        self.diagnostics.extend(read.diagnostics);
        for session in read.changed {
            if seen.insert(session.id.clone()) {
                self.changed.push(session);
            }
        }
        self.live.extend(read.live);
    }
}
