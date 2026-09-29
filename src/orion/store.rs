use super::{Config, Session, Usage};
use anyhow::{Context, Result};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
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

#[derive(Default)]
pub struct Collector {
    cursors: HashMap<PathBuf, (i64, String)>,
    hydrated: HashMap<(PathBuf, String), Session>,
    contexts: HashMap<PathBuf, (Instant, ContextWindows)>,
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
            let path = super::database_path(&root);
            if !path.is_file() {
                batch.diagnostics.push(format!(
                    "Orion App database not found at {}",
                    path.display()
                ));
                continue;
            }
            let contexts = self.context_windows(&root);
            let cursor = self.cursors.get(&path).cloned().unwrap_or_default();
            match read_database(&path, &root, &cursor, now, &contexts, &mut self.hydrated) {
                Ok((changed, live, next, last_activity)) => {
                    batch.cursors.insert(path.clone(), next);
                    batch.last_activity = batch.last_activity.max(last_activity);
                    for session in changed {
                        if seen.insert(session.id.clone()) {
                            batch.changed.push(session);
                        }
                    }
                    batch.live.extend(live);
                }
                Err(error) => batch
                    .diagnostics
                    .push(format!("{}: {error:#}", path.display())),
            }
        }
        batch
            .live
            .sort_by_key(|session| std::cmp::Reverse(session.updated));
        batch
    }

    pub fn acknowledge(&mut self, batch: &Batch) {
        self.cursors.extend(batch.cursors.clone());
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

type ReadResult = (Vec<Session>, Vec<Session>, (i64, String), Option<i64>);

fn read_database(
    path: &Path,
    root: &Path,
    cursor: &(i64, String),
    now: i64,
    contexts: &ContextWindows,
    hydrated: &mut HashMap<(PathBuf, String), Session>,
) -> Result<ReadResult> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
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
    Ok((
        changed,
        live,
        next,
        last_activity.filter(|value| *value > 0 && *value <= now + 60_000),
    ))
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
        .or_else(|| {
            // Orion has no rule for this model: use Pulse's bundled facts.
            let id = session.model.rsplit('/').next().unwrap_or(&session.model);
            if id.starts_with("claude-") {
                Some(if crate::cost::is_ga_1m_context(id) {
                    1_000_000
                } else {
                    200_000
                })
            } else {
                crate::codex::cost::default_model_context_window(id)
            }
        });

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

fn tool_activity(tool: &str) -> &'static str {
    match tool {
        "Read" | "Glob" | "Grep" | "LSP" => "Reading files",
        "Edit" | "Write" => "Editing files",
        "PowerShell" | "Bash" | "Monitor" => "Running command",
        "WebSearch" | "WebFetch" => "Researching",
        "browser" => "Using browser",
        "Agent" | "SendMessage" => "Delegating",
        "AskUserQuestion" | "ExitPlanMode" => "Waiting for input",
        _ => "Using tool",
    }
}

fn string(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let db_dir = dir.path().join("cli").join("db");
        std::fs::create_dir_all(&db_dir).unwrap();
        let path = db_dir.join("db.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (id text primary key, project_id text, parent_id text, \
                   directory text not null, title text not null, time_created integer not null, \
                   time_updated integer not null, time_archived integer, \
                   task_type text not null default 'interactive'); \
                 CREATE TABLE message (id text primary key, session_id text not null, \
                   time_created integer not null, time_updated integer not null, data text not null); \
                 CREATE TABLE part (id text primary key, message_id text, session_id text not null, \
                   time_created integer not null, time_updated integer not null, data text not null); \
                 CREATE TABLE session_entry (id text primary key, session_id text not null, \
                   type text not null, time_created integer not null, time_updated integer not null, \
                   data text not null);",
            )
            .unwrap();
        (dir, path)
    }

    fn assistant(
        model: &str,
        input: u64,
        output: u64,
        read: u64,
        write: u64,
        done: bool,
    ) -> String {
        let completed = if done { ",\"completed\":2" } else { "" };
        format!(
            "{{\"role\":\"assistant\",\"modelId\":\"{model}\",\"providerId\":\"opencodex:anthropic\",\
             \"time\":{{\"created\":1{completed}}},\"cost\":0,\"finish\":\"stop\",\
             \"tokens\":{{\"total\":{},\"input\":{input},\"output\":{output},\"reasoning\":0,\
             \"cache\":{{\"read\":{read},\"write\":{write}}}}}}}",
            input + output
        )
    }

    #[test]
    fn root_session_rolls_up_children_and_reports_live_state() {
        let (dir, path) = fixture();
        let now = chrono::Utc::now().timestamp_millis();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO session VALUES ('root', 'p', NULL, 'D:\\\\work\\\\pulse', 'Title', ?1, ?2, NULL, 'interactive')",
                params![now - 60_000, now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO session VALUES ('child', 'p', 'root', 'D:\\\\work\\\\pulse', 'Child', ?1, ?2, NULL, 'subagent_child')",
                params![now - 30_000, now],
            )
            .unwrap();
        let model = "anthropic/claude-opus-5-5";
        let messages = [
            (
                "m0",
                "root",
                1,
                "{\"role\":\"user\",\"text\":\"private prompt\"}".to_string(),
            ),
            ("m1", "root", 2, assistant(model, 1000, 100, 800, 100, true)),
            ("m2", "child", 3, assistant(model, 500, 50, 0, 0, true)),
            ("m3", "root", 4, assistant(model, 0, 0, 0, 0, false)),
        ];
        for (id, session, order, data) in &messages {
            connection
                .execute(
                    "INSERT INTO message VALUES (?1, ?2, ?3, ?3, ?4)",
                    params![id, session, now - 10_000 + order, data],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO session_entry VALUES ('e1', 'root', 'runtime/model_selection', ?1, ?1, ?2)",
                params![now, "{\"modelSelection\":{\"providerId\":\"opencodex:anthropic\",\"modelId\":\"anthropic/claude-opus-5-5\",\"options\":{\"reasoningLevel\":\"medium\"}}}"],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO part VALUES ('p1', 'm3', 'root', ?1, ?1, ?2)",
                params![now, "{\"type\":\"tool\",\"tool\":\"Edit\",\"state\":{\"status\":\"running\",\"input\":{\"file_path\":\"D:\\\\work\\\\pulse\\\\src\\\\main.rs\"}}}"],
            )
            .unwrap();
        std::fs::create_dir_all(dir.path().join("v2")).unwrap();
        std::fs::write(
            dir.path().join("v2").join("provider_config.json"),
            r#"{"config":{"modelConfigRules":{"providerModelRules":[{"providerId":"opencodex:anthropic","modelId":"anthropic/claude-opus-5-5","config":{"properties":{"contextWindow":1000000}}}]}}}"#,
        )
        .unwrap();
        drop(connection);

        let config = Config {
            data_roots: vec![dir.path().to_path_buf()],
            ..Default::default()
        };
        let mut collector = Collector::default();
        let batch = collector.poll(&config);
        assert!(batch.diagnostics.is_empty(), "{:?}", batch.diagnostics);
        assert_eq!(batch.changed.len(), 1, "children are not separate sessions");
        let live = &batch.live[0];
        assert_eq!(live.id, "root");
        assert_eq!(live.project, "pulse");
        assert_eq!(live.subagent_count, 1);
        assert_eq!(live.usage.input, 1500);
        assert_eq!(live.usage.output, 150);
        assert_eq!(live.usage.fresh_input(), 600);
        assert_eq!(live.model_label(), "Claude Opus 5.5 · Medium");
        assert_eq!(live.activity, "Editing files");
        assert_eq!(live.activity_target.as_deref(), Some("main.rs"));
        assert_eq!(live.context_used, Some(1100));
        assert_eq!(live.context_window, Some(1_000_000));
        // Opus 5.5 over 600 fresh input, 150 output, 100 cache write and 800
        // cache read tokens: $4 / $20 / $5 / $0.20 per million.
        let parts = live.cost_parts;
        assert!((parts.input - 0.0024).abs() < 1e-12, "{parts:?}");
        assert!((parts.output - 0.003).abs() < 1e-12, "{parts:?}");
        assert!((parts.cache_write - 0.0005).abs() < 1e-12, "{parts:?}");
        assert!((parts.cache_read - 0.00016).abs() < 1e-12, "{parts:?}");
        assert!((live.known_cost.unwrap() - 0.00606).abs() < 1e-12);
        assert!((parts.total() - live.known_cost.unwrap()).abs() < 1e-12);
        assert!(live.cost_complete);
        assert_eq!(live.output_tokens_per_sec, None, "no model_usage table");
        assert!(live.is_active(now));
        assert!(super::super::preferred_session(&batch.live).is_some());

        // The child's latest message finished with "stop": it is done.
        assert_eq!(live.running_subagents, 0);
        assert_eq!(batch.last_activity, Some(now));

        collector.acknowledge(&batch);
        assert!(collector.poll(&config).changed.is_empty());

        // A new child starts streaming while the root row stays unchanged: the
        // cached root still reports it live, then stops counting it once done.
        let connection = Connection::open(&path).unwrap();
        let later = now + 1;
        connection
            .execute(
                "INSERT INTO session VALUES ('child2', 'p', 'root', 'x', 'Child 2', ?1, ?1, NULL, 'subagent_child')",
                params![later],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO message VALUES ('m4', 'child2', ?1, ?1, ?2)",
                params![later, assistant(model, 10, 1, 0, 0, false)],
            )
            .unwrap();
        let running = collector.poll(&config);
        assert_eq!(
            running.live[0].running_subagents, 1,
            "streaming child is live"
        );
        assert_eq!(running.live[0].subagent_count, 2);
        connection
            .execute(
                "UPDATE message SET data = ?1 WHERE id = 'm4'",
                params![assistant(model, 10, 1, 0, 0, true)],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE session SET time_updated = ?1 WHERE id = 'child2'",
                params![later + 1],
            )
            .unwrap();
        let finished = collector.poll(&config);
        assert_eq!(
            finished.live[0].running_subagents, 0,
            "finished child never lingers"
        );
        assert_eq!(
            finished.live[0].subagent_count, 2,
            "history keeps the total"
        );
    }

    /// A live root session whose assistant messages use the given models.
    fn priced_session(models: &[&str]) -> (tempfile::TempDir, PathBuf, i64) {
        let (dir, path) = fixture();
        let now = chrono::Utc::now().timestamp_millis();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO session VALUES ('root', 'p', NULL, '/home/dev/pulse', 'T', ?1, ?2, NULL, 'interactive')",
                params![now - 60_000, now],
            )
            .unwrap();
        for (index, model) in models.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO message VALUES (?1, 'root', ?2, ?2, ?3)",
                    params![
                        format!("m{index}"),
                        now - 10_000 + index as i64,
                        assistant(model, 1_000_000, 1_000_000, 0, 0, true)
                    ],
                )
                .unwrap();
        }
        (dir, path, now)
    }

    fn poll_live(dir: &tempfile::TempDir) -> Session {
        let config = Config {
            data_roots: vec![dir.path().to_path_buf()],
            ..Default::default()
        };
        let batch = Collector::default().poll(&config);
        assert!(batch.diagnostics.is_empty(), "{:?}", batch.diagnostics);
        batch.live.into_iter().next().expect("live session")
    }

    #[test]
    fn mixed_priced_and_unpriced_models_report_a_lower_bound() {
        let (dir, _, _) =
            priced_session(&["anthropic/claude-opus-5-5", "opencode-go/unknown-model-x"]);
        let session = poll_live(&dir);
        // Only Opus 5.5 is priced: 1M input + 1M output = $4 + $20.
        assert!((session.known_cost.unwrap() - 24.0).abs() < 1e-9);
        assert!(!session.cost_complete, "an unpriced model makes it partial");
        assert!((session.cost_parts.total() - 24.0).abs() < 1e-9);
        assert_eq!(session.project, "pulse");
    }

    #[test]
    fn a_session_with_no_priced_model_has_no_cost_instead_of_zero() {
        let (dir, _, _) = priced_session(&["opencode-go/unknown-model-x"]);
        let session = poll_live(&dir);
        assert_eq!(session.known_cost, None);
        assert!(!session.cost_complete);
        assert_eq!(session.cost_parts, super::super::CostParts::default());
        assert_eq!(session.usage.total(), 2_000_000, "tokens are still counted");
    }

    #[test]
    fn output_speed_uses_completed_timed_requests_of_the_session_and_its_children() {
        let (dir, path, now) = priced_session(&["anthropic/claude-opus-5-5"]);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO session VALUES ('child', 'p', 'root', '/home/dev/pulse', 'C', ?1, ?1, NULL, 'subagent_child')",
                params![now - 30_000],
            )
            .unwrap();
        connection
            .execute_batch(
                "CREATE TABLE model_usage (id text primary key, session_id text not null, \
                   status text, first_token_at integer, completed_at integer, output_tokens integer);",
            )
            .unwrap();
        for (id, session, status, first, done, output) in [
            // 300 tokens in 2 s and 100 tokens in 2 s: 400 tokens over 4 s.
            ("u1", "root", "completed", Some(1_000), 3_000, 300),
            ("u2", "child", "completed", Some(10_000), 12_000, 100),
            // Not streamed, cancelled, or no output: excluded.
            ("u3", "root", "completed", None, 9_000, 5_000),
            ("u4", "root", "cancelled", Some(1_000), 9_000, 5_000),
            ("u5", "root", "completed", Some(1_000), 9_000, 0),
        ] {
            connection
                .execute(
                    "INSERT INTO model_usage VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![id, session, status, first, done, output],
                )
                .unwrap();
        }
        drop(connection);
        let session = poll_live(&dir);
        assert!((session.output_tokens_per_sec.unwrap() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn project_name_reads_windows_directories_on_any_platform() {
        let (dir, path, now) = priced_session(&["anthropic/claude-opus-5-5"]);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE session SET directory = ?1 WHERE id = 'root'",
                params!["C:\\Users\\dev\\repos\\pulse\\"],
            )
            .unwrap();
        connection
            .execute("UPDATE session SET time_updated = ?1", params![now])
            .unwrap();
        drop(connection);
        assert_eq!(poll_live(&dir).project, "pulse");
    }

    #[test]
    #[ignore = "Reads the local Orion App database; run explicitly"]
    fn local_orion_database_probe() {
        let config = Config::default();
        let mut collector = Collector::default();
        let started = Instant::now();
        let batch = collector.poll(&config);
        let first = started.elapsed();
        collector.acknowledge(&batch);
        let started = Instant::now();
        let again = collector.poll(&config);
        println!(
            "first={first:?} second={:?} changed={} live={} diagnostics={:?}",
            started.elapsed(),
            batch.changed.len(),
            again.live.len(),
            batch.diagnostics
        );
        for session in again.live.iter().take(3) {
            let parts = session.cost_parts;
            println!(
                "{} project={} model={} activity={} tokens={} ctx={:?}/{:?} cost={:?} complete={} \
                 parts=in:{:.4}/out:{:.4}/cw:{:.4}/cr:{:.4} speed={:?} subagents={}",
                session.id,
                session.project,
                session.model_label(),
                session.activity,
                session.usage.total(),
                session.context_used,
                session.context_window,
                session.known_cost,
                session.cost_complete,
                parts.input,
                parts.output,
                parts.cache_write,
                parts.cache_read,
                session.output_tokens_per_sec,
                session.subagent_count
            );
        }
    }

    #[test]
    fn missing_database_is_a_diagnostic_not_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            data_roots: vec![dir.path().to_path_buf()],
            ..Default::default()
        };
        let batch = Collector::default().poll(&config);
        assert!(batch.live.is_empty() && batch.changed.is_empty());
        assert_eq!(batch.diagnostics.len(), 1);
    }

    #[test]
    fn reader_never_opens_the_orion_database_for_writing() {
        let (dir, path) = fixture();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        let config = Config {
            data_roots: vec![dir.path().to_path_buf()],
            ..Default::default()
        };
        let batch = Collector::default().poll(&config);
        assert!(batch.diagnostics.is_empty(), "{:?}", batch.diagnostics);
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).unwrap();
    }
}
