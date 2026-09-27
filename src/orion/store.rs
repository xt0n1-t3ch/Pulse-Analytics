use super::{Config, Session, Usage};
use anyhow::{Context, Result};
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
                Ok((changed, live, next)) => {
                    batch.cursors.insert(path.clone(), next);
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

type ReadResult = (Vec<Session>, Vec<Session>, (i64, String));

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
        let directory = PathBuf::from(row.get::<_, String>(1)?);
        let project = directory
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| super::NAME.to_string());
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
        if let Some(previous) = hydrated
            .get(&key)
            .filter(|previous| previous.updated == session.updated)
        {
            *session = previous.clone();
            continue;
        }
        hydrate(&connection, session, contexts, now)
            .with_context(|| format!("Orion session {}", session.id))?;
        if session.is_recent(now) {
            hydrated.insert(key, session.clone());
        }
    }
    Ok((changed, live, next))
}

fn hydrate(
    connection: &Connection,
    session: &mut Session,
    contexts: &ContextWindows,
    now: i64,
) -> Result<()> {
    // Tokens and cost roll up the root session and its subagent children, the
    // same way Orion's own task usage totals them.
    let mut stmt = connection.prepare_cached(
        "SELECT json_extract(m.data, '$.modelId'), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.input')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.output')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.reasoning')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.cache.read')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.cache.write')), 0), \
                COUNT(*) \
         FROM message m \
         WHERE m.session_id IN (SELECT id FROM session WHERE id = ?1 OR parent_id = ?1) \
           AND json_extract(m.data, '$.role') = 'assistant' \
         GROUP BY 1",
    )?;
    let groups = stmt.query_map([&session.id], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?.unwrap_or_default(),
            Usage {
                input: row.get::<_, i64>(1)?.max(0) as u64,
                output: row.get::<_, i64>(2)?.max(0) as u64,
                reasoning: row.get::<_, i64>(3)?.max(0) as u64,
                cache_read: row.get::<_, i64>(4)?.max(0) as u64,
                cache_write: row.get::<_, i64>(5)?.max(0) as u64,
            },
            row.get::<_, i64>(6)?.max(0) as u64,
        ))
    })?;
    let mut cost = Some(0.0);
    let mut complete = true;
    for group in groups {
        let (model, usage, count) = group?;
        session.messages = session.messages.saturating_add(count);
        session.usage.add(&usage);
        if usage.total() == 0 {
            continue;
        }
        match super::estimate_cost(&model, &usage) {
            Some((value, exact)) => {
                cost = cost.map(|sum| sum + value);
                complete &= exact;
            }
            None => complete = false,
        }
    }
    let priced = cost.filter(|value| value.is_finite() && *value > 0.0);
    session.known_cost = priced;
    session.cost_complete = priced.is_some() && complete;

    session.subagent_count = connection
        .prepare_cached("SELECT COUNT(*) FROM session WHERE parent_id = ?1")?
        .query_row([&session.id], |row| row.get::<_, i64>(0))?
        .max(0) as usize;

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
        assert!(live.known_cost.is_some_and(|cost| cost > 0.0));
        assert!(live.is_active(now));
        assert!(super::super::preferred_session(&batch.live).is_some());

        collector.acknowledge(&batch);
        assert!(collector.poll(&config).changed.is_empty());
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
            println!(
                "{} project={} model={} activity={} tokens={} ctx={:?}/{:?} cost={:?} complete={} subagents={}",
                session.id,
                session.project,
                session.model_label(),
                session.activity,
                session.usage.total(),
                session.context_used,
                session.context_window,
                session.known_cost,
                session.cost_complete,
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
