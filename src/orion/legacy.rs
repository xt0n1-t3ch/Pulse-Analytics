//! Pre-cutover Orion store: conversations in SQL tables of `cli/db/db.sqlite`
//! (also under the legacy `~/.zcode` link). Orion stopped writing it on
//! 2026-10-17, so Pulse reads it only while its `session` table still holds
//! rows, and the current layout (`reader.rs`) wins whenever it exists.

use super::store::Read;
use super::{Session, Usage, tool_activity};
use anyhow::{Context, Result};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// History rows imported per poll. Initial import advances in pages so a
/// large Orion history never blocks one GUI tick.
const BATCH_SIZE: usize = 16;
/// Live window: sessions touched in the last ten minutes are hydrated on every poll.
const LIVE_WINDOW_MS: i64 = 600_000;
/// A subagent that has not written anything for this long is no longer running.
const RUNNING_WINDOW_MS: i64 = 120_000;
const MAX_JSON_BYTES: usize = 1024 * 1024;
const CONTEXT_REFRESH: Duration = Duration::from_secs(60);

/// Context window per Orion `(providerId, modelId)` rule.
type ContextWindows = HashMap<(String, String), u64>;

/// What the legacy reader remembers between polls.
#[derive(Default)]
pub(super) struct Legacy {
    hydrated: HashMap<(PathBuf, String), Session>,
    contexts: HashMap<PathBuf, (Instant, ContextWindows)>,
}

impl Legacy {
    pub(super) fn read(
        &mut self,
        path: &Path,
        root: &Path,
        cursor: &(i64, String),
        now: i64,
    ) -> Result<Read> {
        let contexts = self.context_windows(root);
        read_database(path, root, cursor, now, &contexts, &mut self.hydrated)
    }

    fn context_windows(&mut self, root: &Path) -> ContextWindows {
        if let Some((read_at, map)) = self.contexts.get(root)
            && read_at.elapsed() < CONTEXT_REFRESH
        {
            return map.clone();
        }
        let map = read_context_windows(&root.join("v2").join("provider_config.json"));
        self.contexts
            .insert(root.to_path_buf(), (Instant::now(), map.clone()));
        map
    }
}

/// Whether the legacy database exists and still holds a session.
pub(super) fn has_sessions(path: &Path) -> bool {
    let Ok(connection) = crate::foreign_sqlite::open_read_only(path) else {
        return false;
    };
    connection
        .busy_timeout(Duration::from_millis(200))
        .and_then(|()| {
            connection.query_row("SELECT EXISTS(SELECT 1 FROM session)", [], |row| {
                row.get::<_, bool>(0)
            })
        })
        .unwrap_or(false)
}

/// Orion's per-model context rules: `providerId` + `modelId` -> `contextWindow`.
fn read_context_windows(path: &Path) -> ContextWindows {
    let mut map = HashMap::new();
    let Ok(bytes) = std::fs::read(path) else {
        return map;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return map;
    };
    let Some(rules) = value
        .pointer("/config/modelConfigRules/providerModelRules")
        .and_then(Value::as_array)
    else {
        return map;
    };
    for rule in rules {
        let (Some(provider), Some(model), Some(window)) = (
            rule.get("providerId").and_then(Value::as_str),
            rule.get("modelId").and_then(Value::as_str),
            rule.pointer("/config/properties/contextWindow")
                .and_then(Value::as_u64)
                .filter(|value| *value > 0),
        ) else {
            continue;
        };
        map.insert((provider.to_string(), model.to_string()), window);
    }
    map
}

fn read_database(
    path: &Path,
    root: &Path,
    cursor: &(i64, String),
    now: i64,
    contexts: &ContextWindows,
    hydrated: &mut HashMap<(PathBuf, String), Session>,
) -> Result<Read> {
    let connection = crate::foreign_sqlite::open_read_only(path)?;
    connection.busy_timeout(Duration::from_millis(200))?;
    let base = "SELECT id, directory, title, time_created, time_updated FROM session \
                WHERE parent_id IS NULL AND task_type = 'interactive' AND time_archived IS NULL";
    let read_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<Session> {
        let raw_directory = row.get::<_, String>(1)?;
        // Orion stores the path of the machine that wrote it, which may be a
        // Windows path read on Linux or macOS; `PathBuf::file_name` would not
        // split it.
        let project = super::project_name(&raw_directory);
        let directory = PathBuf::from(raw_directory);
        Ok(Session {
            id: row.get(0)?,
            root: root.to_path_buf(),
            directory,
            title: row
                .get::<_, Option<String>>(2)?
                .filter(|title| !title.trim().is_empty()),
            project,
            created: row.get(3)?,
            updated: row.get(4)?,
            ..Default::default()
        })
    };
    let mut changed: Vec<Session> = connection
        .prepare(&format!(
            "{base} AND (time_updated > ?1 OR (time_updated = ?1 AND id > ?2)) \
             ORDER BY time_updated, id LIMIT {BATCH_SIZE}"
        ))?
        .query_map(params![cursor.0, cursor.1], read_row)?
        .collect::<rusqlite::Result<_>>()?;
    let next = changed
        .last()
        .map(|session| (session.updated, session.id.clone()))
        .unwrap_or_else(|| cursor.clone());
    let mut live: Vec<Session> = connection
        .prepare(&format!(
            "{base} AND time_updated >= ?1 ORDER BY time_updated DESC LIMIT 16"
        ))?
        .query_map([now - LIVE_WINDOW_MS], read_row)?
        .collect::<rusqlite::Result<_>>()?;

    hydrated.retain(|_, session| session.is_recent(now));
    for session in changed.iter_mut().chain(live.iter_mut()) {
        let key = (path.to_path_buf(), session.id.clone());
        // Subagents write to their own rows, so the parent row can stay
        // unchanged while they work: the whole tree decides freshness.
        session.tree_updated = tree_updated(&connection, &session.id)?.max(session.updated);
        if let Some(previous) = hydrated.get(&key).filter(|previous| {
            previous.updated == session.updated && previous.tree_updated == session.tree_updated
        }) {
            *session = previous.clone();
            session.running_subagents = running_subagents(&connection, &session.id, now)
                .with_context(|| format!("Orion session {}", session.id))?;
            continue;
        }
        hydrate(&connection, session, contexts, now)
            .with_context(|| format!("Orion session {}", session.id))?;
        if session.is_recent(now) {
            hydrated.insert(key, session.clone());
        }
    }
    let last_activity: Option<i64> = connection
        .prepare_cached("SELECT MAX(time_updated) FROM session WHERE time_archived IS NULL")?
        .query_row([], |row| row.get(0))?;
    Ok(Read {
        changed,
        live,
        next,
        last_activity: last_activity.filter(|value| *value > 0 && *value <= now + 60_000),
        diagnostics: Vec::new(),
    })
}

fn hydrate(
    connection: &Connection,
    session: &mut Session,
    contexts: &ContextWindows,
    now: i64,
) -> Result<()> {
    // Tokens and cost roll up the root session and its subagent children, the
    // same way Orion's own task usage totals them. Each assistant message is
    // one model request, so it is priced on its own: long-context and
    // fast-mode rates depend on the request, not on the session total.
    let mut stmt = connection.prepare_cached(
        "SELECT json_extract(m.data, '$.modelId'), \
                json_extract(m.data, '$.tokens.input'), \
                json_extract(m.data, '$.tokens.output'), \
                json_extract(m.data, '$.tokens.reasoning'), \
                json_extract(m.data, '$.tokens.cache.read'), \
                json_extract(m.data, '$.tokens.cache.write') \
         FROM message m \
         WHERE m.session_id IN (SELECT id FROM session WHERE id = ?1 OR parent_id = ?1) \
           AND json_extract(m.data, '$.role') = 'assistant'",
    )?;
    let requests = stmt.query_map([&session.id], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?.unwrap_or_default(),
            Usage {
                input: count(row, 1)?,
                output: count(row, 2)?,
                reasoning: count(row, 3)?,
                cache_read: count(row, 4)?,
                cache_write: count(row, 5)?,
            },
        ))
    })?;
    let mut ledger = super::CostLedger::default();
    for request in requests {
        let (model, usage) = request?;
        session.messages = session.messages.saturating_add(1);
        session.usage.add(&usage);
        ledger.add(&model, &usage);
    }
    session.apply_cost(ledger.finish());
    session.output_tokens_per_sec = output_speed(connection, &session.id);

    session.subagent_count = connection
        .prepare_cached("SELECT COUNT(*) FROM session WHERE parent_id = ?1")?
        .query_row([&session.id], |row| row.get::<_, i64>(0))?
        .max(0) as usize;
    session.running_subagents = running_subagents(connection, &session.id, now)?;

    let selection: Option<String> = connection
        .prepare_cached(
            "SELECT data FROM session_entry WHERE session_id = ?1 \
             AND type = 'runtime/model_selection' ORDER BY time_updated DESC LIMIT 1",
        )?
        .query_row([&session.id], |row| row.get(0))
        .optional()?;
    if let Some(value) = selection
        .filter(|raw| raw.len() <= MAX_JSON_BYTES)
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    {
        session.provider_id = string(&value, "/modelSelection/providerId");
        session.model = string(&value, "/modelSelection/modelId");
        session.effort = value
            .pointer("/modelSelection/options/reasoningLevel")
            .and_then(Value::as_str)
            .and_then(super::effort_label);
    }

    let last = connection
        .prepare_cached(
            "SELECT data FROM message WHERE session_id = ?1 \
             AND json_extract(data, '$.role') = 'assistant' \
             ORDER BY time_created DESC, id DESC LIMIT 1",
        )?
        .query_row([&session.id], |row| row.get::<_, String>(0))
        .optional()?
        .filter(|raw| raw.len() <= MAX_JSON_BYTES)
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
    if let Some(message) = &last {
        if session.model.is_empty() {
            session.model = string(message, "/modelId");
            session.provider_id = string(message, "/providerId");
        }
        let completed = message.pointer("/time/completed").is_some();
        session.activity = if completed {
            if message.pointer("/finish").and_then(Value::as_str) == Some("tool-calls") {
                "Thinking"
            } else {
                "Waiting for input"
            }
        } else {
            "Thinking"
        }
        .into();
    } else {
        session.activity = "Idle".into();
    }
    if session.running_subagents > 0
        && matches!(session.activity.as_str(), "Waiting for input" | "Idle")
    {
        session.activity = "Delegating".into();
    }

    // Current fill: the latest assistant message with reported tokens.
    let fill: Option<i64> = connection
        .prepare_cached(
            "SELECT COALESCE(json_extract(data, '$.tokens.total'), \
                    json_extract(data, '$.tokens.input') + json_extract(data, '$.tokens.output')) \
             FROM message WHERE session_id = ?1 AND json_extract(data, '$.role') = 'assistant' \
               AND json_extract(data, '$.time.completed') IS NOT NULL \
               AND COALESCE(json_extract(data, '$.tokens.input'), 0) > 0 \
             ORDER BY time_created DESC LIMIT 1",
        )?
        .query_row([&session.id], |row| row.get(0))
        .optional()?
        .flatten();
    session.context_used = fill.filter(|value| *value > 0).map(|value| value as u64);
    session.context_window = contexts
        .get(&(session.provider_id.clone(), session.model.clone()))
        .copied()
        .or_else(|| super::bundled_context_window(&session.model));

    let running: Option<String> = connection
        .prepare_cached(
            "SELECT data FROM part WHERE session_id = ?1 AND time_updated >= ?2 \
             AND json_extract(data, '$.type') = 'tool' \
             AND json_extract(data, '$.state.status') = 'running' \
             ORDER BY time_updated DESC LIMIT 1",
        )?
        .query_row(params![&session.id, now - 120_000], |row| row.get(0))
        .optional()?;
    if let Some(part) = running
        .filter(|raw| raw.len() <= MAX_JSON_BYTES)
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    {
        let tool = part.get("tool").and_then(Value::as_str).unwrap_or_default();
        session.activity = tool_activity(tool).into();
        session.activity_target = ["/state/input/file_path", "/state/input/filePath"]
            .iter()
            .find_map(|pointer| part.pointer(pointer).and_then(Value::as_str))
            .map(|path| crate::codex::session::sanitize_file_target(path, 64));
    }
    session.model_name = super::display_model_name(&session.model);
    Ok(())
}

/// Latest change anywhere in a root session and its subagents.
fn tree_updated(connection: &Connection, session_id: &str) -> Result<i64> {
    Ok(connection
        .prepare_cached(
            "SELECT COALESCE(MAX(time_updated), 0) FROM session WHERE id = ?1 OR parent_id = ?1",
        )?
        .query_row([session_id], |row| row.get(0))?)
}

/// Subagents that are still working. A child counts while its latest assistant
/// message is streaming or ended on tool calls, and only while it changed in the
/// last two minutes, so a crashed child never stays "running". A child whose
/// latest message finished with any other reason is done.
fn running_subagents(connection: &Connection, session_id: &str, now: i64) -> Result<usize> {
    let count: i64 = connection
        .prepare_cached(
            "SELECT COUNT(*) FROM session s WHERE s.parent_id = ?1 AND s.time_updated >= ?2 \
               AND s.time_archived IS NULL \
               AND COALESCE((SELECT json_extract(m.data, '$.time.completed') IS NULL \
                                    OR json_extract(m.data, '$.finish') = 'tool-calls' \
                             FROM message m WHERE m.session_id = s.id \
                               AND json_extract(m.data, '$.role') = 'assistant' \
                             ORDER BY m.time_created DESC, m.id DESC LIMIT 1), 1)",
        )?
        .query_row(params![session_id, now - RUNNING_WINDOW_MS], |row| {
            row.get(0)
        })?;
    Ok(count.max(0) as usize)
}

/// A token count from a JSON number column. Missing, negative or non-numeric
/// values count as zero.
fn count(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    Ok(match row.get_ref(index)? {
        ValueRef::Integer(value) => value.max(0) as u64,
        ValueRef::Real(value) if value.is_finite() => value.max(0.0) as u64,
        _ => 0,
    })
}

/// Output tokens per second over the requests that streamed, from Orion's
/// per-request timings (`model_usage`). Both sums use the same requests, so the
/// rate is generated tokens divided by time spent generating them, the same
/// meaning Claude's output speed has. Older stores without the table, and
/// sessions without a timed request, report no speed.
fn output_speed(connection: &Connection, session_id: &str) -> Option<f64> {
    let (tokens, millis): (i64, i64) = connection
        .prepare_cached(
            "SELECT COALESCE(SUM(output_tokens), 0), COALESCE(SUM(completed_at - first_token_at), 0) \
             FROM model_usage \
             WHERE session_id IN (SELECT id FROM session WHERE id = ?1 OR parent_id = ?1) \
               AND status = 'completed' AND output_tokens > 0 \
               AND first_token_at IS NOT NULL AND completed_at > first_token_at",
        )
        .ok()?
        .query_row([session_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .ok()?;
    super::output_speed(tokens.max(0) as u64, millis.max(0) as u64)
}

fn string(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
#[path = "legacy_tests.rs"]
mod tests;
