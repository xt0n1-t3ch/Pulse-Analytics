//! Reads the current Orion layout: `projects/index.sqlite` lists the sessions,
//! JSONL transcripts hold the conversations and `telemetry/usage.sqlite` adds
//! per-request timings (`layout.rs` has the paths).
//!
//! Orion rebuilds the index from the transcripts, so it is only ever opened
//! read-only and only the columns Pulse needs are selected. Freshness is
//! `last_write_ms`: `time_updated` lags by up to 30 seconds.

use super::store::Read;
use super::transcript::{Transcript, TranscriptCache};
use super::{CostLedger, Session, layout, tool_activity, usage};
use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// History rows imported per poll. Initial import advances in pages so a
/// large Orion history never blocks one GUI tick.
const BATCH_SIZE: usize = 16;
/// Live window: sessions touched in the last ten minutes are hydrated on every poll.
const LIVE_WINDOW_MS: i64 = 600_000;
/// A subagent or tool that has not written anything for this long is no longer running.
const RUNNING_WINDOW_MS: i64 = 120_000;
/// Roots returned for the live view.
const LIVE_LIMIT: usize = 16;
/// A write stamped further ahead than this is a clock error, not activity.
const FUTURE_SKEW_MS: i64 = 60_000;
const BUSY_TIMEOUT: Duration = Duration::from_millis(200);
/// Longest file name shown as the current activity target.
const TARGET_CHARS: usize = 64;

/// Root sessions with the newest write of the session or any direct subagent.
/// Subagents write to their own transcripts, so the parent can stay unchanged
/// while they work: the tree decides freshness.
const ROOTS: &str = "SELECT id, directory, title, time_created, last_write, tree_write FROM ( \
        SELECT s.id, s.directory, s.title, s.time_created, \
               COALESCE(s.last_write_ms, s.time_updated, 0) AS last_write, \
               MAX(COALESCE(s.last_write_ms, s.time_updated, 0), \
                   COALESCE((SELECT MAX(COALESCE(c.last_write_ms, c.time_updated, 0)) \
                             FROM session c WHERE c.parent_id = s.id), 0)) AS tree_write \
        FROM session s \
        WHERE s.parent_id IS NULL AND s.task_type = 'interactive' AND s.time_archived IS NULL)";

/// A subagent of a hydrated root session.
struct Child {
    last_write: i64,
    /// Not archived, and its latest request is streaming or ended on tool calls
    /// (or it has not written a request yet).
    open: bool,
}

/// A summarized session kept while it stays recent.
pub(super) struct Hydrated {
    session: Session,
    /// Root and subagent ids, which own the cached transcripts.
    members: Vec<String>,
    children: Vec<Child>,
}

pub(super) type HydratedMap = HashMap<(PathBuf, String), Hydrated>;

/// Subagents still working: a child counts while it is open and changed in the
/// last two minutes, so a crashed child never stays "running".
fn running_subagents(children: &[Child], now: i64) -> usize {
    children
        .iter()
        .filter(|child| child.open && child.last_write >= now - RUNNING_WINDOW_MS)
        .count()
}

struct Member {
    id: String,
    rel_dir: String,
    last_write: i64,
    archived: bool,
}

/// Everything one poll needs from the environment of a root.
struct Env<'a> {
    connection: &'a Connection,
    usage_db: Option<&'a Connection>,
    projects: &'a Path,
    now: i64,
}

struct Roots {
    changed: Vec<Session>,
    live: Vec<Session>,
    next: (i64, String),
}

pub(super) fn read(
    path: &Path,
    root: &Path,
    cursor: &(i64, String),
    now: i64,
    hydrated: &mut HydratedMap,
    transcripts: &mut TranscriptCache,
) -> Result<Read> {
    let connection = crate::foreign_sqlite::open_read_only(path)?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    let usage_db = usage::open(root);
    let projects = layout::projects_dir(root);
    let env = Env {
        connection: &connection,
        usage_db: usage_db.as_ref(),
        projects: &projects,
        now,
    };
    let Roots {
        mut changed,
        mut live,
        next,
    } = list_roots(&connection, root, cursor, now)?;
    let mut diagnostics = Vec::new();
    hydrated.retain(|_, entry| entry.session.is_recent(now));
    for session in changed.iter_mut().chain(live.iter_mut()) {
        let key = (path.to_path_buf(), session.id.clone());
        if let Some(previous) = hydrated.get(&key).filter(|previous| {
            previous.session.updated == session.updated
                && previous.session.tree_updated == session.tree_updated
        }) {
            *session = previous.session.clone();
            session.running_subagents = running_subagents(&previous.children, now);
            continue;
        }
        let (members, children) = hydrate(&env, session, transcripts, &mut diagnostics)
            .with_context(|| format!("Orion session {}", session.id))?;
        if session.is_recent(now) {
            let session = session.clone();
            hydrated.insert(
                key,
                Hydrated {
                    session,
                    members,
                    children,
                },
            );
        }
    }
    // Only sessions that stay recent keep a parsed transcript; the rest was
    // summarized once and is parsed again if it ever changes.
    let kept: HashSet<&str> = hydrated
        .iter()
        .filter(|((owner, _), _)| owner == path)
        .flat_map(|(_, entry)| entry.members.iter().map(String::as_str))
        .collect();
    transcripts.retain(|id| kept.contains(id));

    let last_activity: Option<i64> = connection
        .prepare_cached(
            "SELECT MAX(COALESCE(last_write_ms, time_updated)) FROM session \
             WHERE time_archived IS NULL",
        )?
        .query_row([], |row| row.get(0))?;
    Ok(Read {
        changed,
        live,
        next,
        last_activity: last_activity.filter(|value| *value > 0 && *value <= now + FUTURE_SKEW_MS),
        diagnostics,
    })
}

/// Root sessions past the history cursor (one page) and the live ones.
fn list_roots(
    connection: &Connection,
    root: &Path,
    cursor: &(i64, String),
    now: i64,
) -> Result<Roots> {
    let read_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<Session> {
        let raw_directory = row.get::<_, Option<String>>(1)?.unwrap_or_default();
        // Orion stores the path of the machine that wrote it, which may be a
        // Windows path read on Linux or macOS; `PathBuf::file_name` would not
        // split it.
        let project = super::project_name(&raw_directory);
        Ok(Session {
            id: row.get(0)?,
            root: root.to_path_buf(),
            directory: PathBuf::from(raw_directory),
            title: row
                .get::<_, Option<String>>(2)?
                .filter(|title| !title.trim().is_empty()),
            project,
            created: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
            updated: row.get(4)?,
            tree_updated: row.get(5)?,
            ..Default::default()
        })
    };
    let changed: Vec<Session> = connection
        .prepare(&format!(
            "{ROOTS} WHERE tree_write > ?1 OR (tree_write = ?1 AND id > ?2) \
             ORDER BY tree_write, id LIMIT {BATCH_SIZE}"
        ))?
        .query_map(params![cursor.0, cursor.1], read_row)?
        .collect::<rusqlite::Result<_>>()?;
    let next = changed
        .last()
        .map(|session| (session.tree_updated, session.id.clone()))
        .unwrap_or_else(|| cursor.clone());
    let live: Vec<Session> = connection
        .prepare(&format!(
            "{ROOTS} WHERE tree_write >= ?1 ORDER BY tree_write DESC LIMIT {LIVE_LIMIT}"
        ))?
        .query_map([now - LIVE_WINDOW_MS], read_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Roots {
        changed,
        live,
        next,
    })
}

/// The root session and every subagent below it, root first.
fn members(connection: &Connection, root_id: &str) -> Result<Vec<Member>> {
    let mut members: Vec<Member> = connection
        .prepare_cached(
            "WITH RECURSIVE tree(id) AS ( \
                 SELECT ?1 UNION SELECT s.id FROM session s JOIN tree t ON s.parent_id = t.id) \
             SELECT s.id, s.rel_dir, COALESCE(s.last_write_ms, s.time_updated, 0), \
                    s.time_archived IS NOT NULL \
             FROM session s WHERE s.id IN (SELECT id FROM tree)",
        )?
        .query_map([root_id], |row| {
            Ok(Member {
                id: row.get(0)?,
                rel_dir: row.get(1)?,
                last_write: row.get(2)?,
                archived: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    members.sort_by_key(|member| member.id != root_id);
    Ok(members)
}

fn hydrate(
    env: &Env<'_>,
    session: &mut Session,
    transcripts: &mut TranscriptCache,
    diagnostics: &mut Vec<String>,
) -> Result<(Vec<String>, Vec<Child>)> {
    let members = members(env.connection, &session.id)?;
    for member in &members {
        let Some(candidates) = layout::transcript_candidates(env.projects, &member.rel_dir) else {
            continue;
        };
        if let Err(error) = transcripts.refresh(&member.id, &candidates) {
            diagnostics.push(format!(
                "Orion transcript of session {}: {error}",
                member.id
            ));
        }
    }
    roll_up(session, &members, transcripts);
    let children = children_of(session, &members, transcripts);
    session.running_subagents = running_subagents(&children, env.now);
    let root = transcripts.get(&session.id);
    describe_model(session, root);
    describe_activity(session, root, env.now);
    describe_context(session, root);
    let ids: Vec<String> = members.into_iter().map(|member| member.id).collect();
    apply_usage(session, env, &ids);
    Ok((ids, children))
}

/// Tokens and cost roll up the root session and its subagents, the same way
/// Orion's own task usage totals them. Each assistant message is one model
/// request and was priced on its own when read: long-context and fast-mode
/// rates depend on the request, not on the session total.
fn roll_up(session: &mut Session, members: &[Member], transcripts: &TranscriptCache) {
    let mut ledger = CostLedger::default();
    for member in members {
        let Some(transcript) = transcripts.get(&member.id) else {
            continue;
        };
        for message in transcript.messages.values() {
            session.messages = session.messages.saturating_add(1);
            session.usage.add(&message.usage);
            ledger.add_estimate(&message.usage, message.estimate);
        }
    }
    session.apply_cost(ledger.finish());
    session.subagent_count = members.len().saturating_sub(1);
}

fn children_of(session: &Session, members: &[Member], transcripts: &TranscriptCache) -> Vec<Child> {
    members
        .iter()
        .filter(|member| member.id != session.id)
        .map(|member| Child {
            last_write: member.last_write,
            open: !member.archived
                && transcripts
                    .get(&member.id)
                    .and_then(Transcript::last_message)
                    .is_none_or(|message| !message.completed || message.ended_on_tool_calls),
        })
        .collect()
}

fn describe_model(session: &mut Session, root: Option<&Transcript>) {
    if let Some(selection) = root.and_then(|transcript| transcript.selection.as_ref()) {
        session.provider_id = selection.provider.clone();
        session.model = selection.model.clone();
        session.effort = selection.effort.as_deref().and_then(super::effort_label);
    }
    if let Some(transcript) = root
        && session.model.is_empty()
        && let Some(message) = transcript.last_message()
    {
        session.model = message.model.clone();
        session.provider_id = transcript.provider.clone();
    }
    session.model_name = super::display_model_name(&session.model);
}

fn describe_activity(session: &mut Session, root: Option<&Transcript>, now: i64) {
    session.activity = match root.and_then(Transcript::last_message) {
        Some(message) if message.completed && !message.ended_on_tool_calls => "Waiting for input",
        Some(_) => "Thinking",
        None => "Idle",
    }
    .into();
    if session.running_subagents > 0
        && matches!(session.activity.as_str(), "Waiting for input" | "Idle")
    {
        session.activity = "Delegating".into();
    }
    let running_tool = root.and_then(|transcript| {
        transcript
            .running_tools
            .values()
            .filter(|mark| mark.at >= now - RUNNING_WINDOW_MS)
            .max_by_key(|mark| mark.at)
    });
    if let Some(mark) = running_tool {
        session.activity = tool_activity(&mark.tool).into();
        session.activity_target = mark
            .target
            .as_deref()
            .map(|path| crate::codex::session::sanitize_file_target(path, TARGET_CHARS))
            .filter(|target| !target.is_empty());
    }
}

/// Orion reports the real window and fill. Without that entry the fill is the
/// latest completed request's prompt size and the window is Pulse's bundled
/// value for the model.
fn describe_context(session: &mut Session, root: Option<&Transcript>) {
    let context = root.and_then(|transcript| transcript.context.as_ref());
    session.context_used = context
        .and_then(|context| context.used)
        .filter(|used| *used > 0)
        .or_else(|| {
            root?
                .messages
                .values()
                .filter(|message| message.completed && message.usage.input > 0)
                .max_by_key(|message| message.created)
                .map(|message| message.fill)
                .filter(|fill| *fill > 0)
        });
    session.context_window = context
        .and_then(|context| context.max)
        .or_else(|| super::bundled_context_window(&session.model));
    session.auto_compact_threshold = context.and_then(|context| context.auto_compact_threshold);
    session.cache_hit_rate = context.and_then(|context| context.cache_hit_rate);
}

fn apply_usage(session: &mut Session, env: &Env<'_>, ids: &[String]) {
    let covered = usage::covers(session.created, env.now);
    let telemetry = env
        .usage_db
        .and_then(|db| usage::read(db, ids, covered).ok())
        .flatten();
    if let Some(telemetry) = telemetry {
        session.ttft_ms_median = telemetry.ttft_ms_median;
        session.output_tokens_per_sec = telemetry.output_tokens_per_sec;
        session.helper_requests = telemetry.helper_requests;
        session.subagents_by_agent = telemetry.subagents_by_agent;
    }
}
