//! Read-only SQLite collector for OpenCode sessions across channel databases.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde_json::Value;

use super::{Config, Metadata, ModelUsage, Session};

const BATCH_SIZE: usize = 64;
const MAX_JSON_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub struct Collector {
    cursors: HashMap<PathBuf, (i64, String)>,
    live: HashMap<String, Session>,
    hydrated: HashMap<(PathBuf, String), Session>,
}

#[derive(Default)]
pub struct ImportBatch {
    pub sessions: Vec<Session>,
    pub live: Vec<Session>,
    pub diagnostics: Vec<String>,
    cursors: HashMap<PathBuf, (i64, String)>,
}

impl Collector {
    pub fn poll(&mut self, config: &Config) -> ImportBatch {
        let mut batch = ImportBatch::default();
        let mut seen = HashSet::new();
        let now = chrono::Utc::now().timestamp_millis();
        for path in discover_databases(config) {
            let cursor = self.cursors.get(&path).cloned().unwrap_or_default();
            match read_database_cached(&path, &cursor, now, &mut self.hydrated) {
                Ok((sessions, next)) => {
                    batch.cursors.insert(path.clone(), next);
                    for session in sessions {
                        if session.is_recent(now) {
                            self.live.insert(session.id.clone(), session.clone());
                        } else {
                            self.live.remove(&session.id);
                        }
                        if seen.insert(session.id.clone()) {
                            batch.sessions.push(session);
                        }
                    }
                }
                Err(error) => batch
                    .diagnostics
                    .push(format!("{}: {error}", path.display())),
            }
        }
        self.live.retain(|_, session| session.is_recent(now));
        batch.live = self.live.values().cloned().collect();
        batch
            .live
            .sort_by_key(|session| std::cmp::Reverse(session.updated));
        batch
    }

    pub fn acknowledge(&mut self, batch: &ImportBatch) {
        self.cursors.extend(batch.cursors.clone());
    }
}

pub fn discover_databases(config: &Config) -> Vec<PathBuf> {
    let root = super::data_dir();
    let mut paths = config.database_paths.clone();
    if let Some(path) = std::env::var_os("OPENCODE_DB") {
        let path = PathBuf::from(path);
        if path != Path::new(":memory:") {
            paths.push(if path.is_absolute() {
                path
            } else {
                root.join(path)
            });
        }
    }
    let mut roots = vec![root];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".local/share/opencode"));
    }
    for root in roots {
        paths.push(root.join("opencode.db"));
        if let Ok(entries) = std::fs::read_dir(&root) {
            paths.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
                path.extension().is_some_and(|ext| ext == "db")
                    && path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("opencode-"))
            }));
        }
    }
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| path.is_file())
        .filter_map(|path| path.canonicalize().ok())
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

fn columns(connection: &Connection, table: &str) -> Result<HashSet<String>> {
    Ok(connection
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |row| row.get(1))?
        .collect::<rusqlite::Result<_>>()?)
}

fn optional_column(
    columns: &HashSet<String>,
    name: &'static str,
    fallback: &'static str,
) -> &'static str {
    if columns.contains(name) {
        name
    } else {
        fallback
    }
}

#[cfg(test)]
fn read_database(
    path: &Path,
    cursor: &(i64, String),
    now: i64,
) -> Result<(Vec<Session>, (i64, String))> {
    read_database_cached(path, cursor, now, &mut HashMap::new())
}

fn read_database_cached(
    path: &Path,
    cursor: &(i64, String),
    now: i64,
    hydrated: &mut HashMap<(PathBuf, String), Session>,
) -> Result<(Vec<Session>, (i64, String))> {
    let connection = crate::foreign_sqlite::open_read_only(path)?;
    connection.busy_timeout(Duration::from_millis(150))?;
    let cols = columns(&connection, "session")?;
    anyhow::ensure!(
        ["id", "directory", "time_created", "time_updated"]
            .iter()
            .all(|key| cols.contains(*key)),
        "Unsupported OpenCode session schema"
    );
    let message_columns = columns(&connection, "message")?;
    let v2_columns = columns(&connection, "session_message")?;
    anyhow::ensure!(
        message_columns.contains("data") || v2_columns.contains("data"),
        "Unsupported OpenCode message schema"
    );
    let projection = format!(
        "id, directory, {}, {}, {}, time_created, time_updated, {}",
        optional_column(&cols, "title", "NULL"),
        optional_column(&cols, "model", "NULL"),
        optional_column(&cols, "parent_id", "NULL"),
        optional_column(&cols, "time_archived", "NULL")
    );
    let sql = format!(
        "SELECT {projection} FROM session WHERE time_updated > ?1 OR (time_updated = ?1 AND id > ?2) ORDER BY time_updated, id LIMIT {BATCH_SIZE}"
    );
    let read_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<Session> {
        let directory = PathBuf::from(row.get::<_, String>(1)?);
        let model: Option<String> = row.get(3)?;
        let descriptor = model
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .unwrap_or_default();
        let project = directory
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "OpenCode".to_string());
        Ok(Session {
            id: row.get(0)?,
            directory,
            project,
            title: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            parent_id: row.get(4)?,
            created: row.get(5)?,
            updated: row.get(6)?,
            archived: row.get::<_, Option<i64>>(7)?.is_some(),
            metadata: Metadata {
                model_provider: string(&descriptor, "providerID"),
                model_id: string(&descriptor, "id"),
                variant: descriptor
                    .get("variant")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                source_database: path.to_path_buf(),
                surface: "OpenCode".into(),
                ..Default::default()
            },
            ..Default::default()
        })
    };
    let mut sessions: Vec<Session> = connection
        .prepare(&sql)?
        .query_map(params![cursor.0, cursor.1], read_row)?
        .collect::<rusqlite::Result<_>>()?;
    let next = sessions
        .last()
        .map(|session| (session.updated, session.id.clone()))
        .unwrap_or_else(|| cursor.clone());
    let recent_sql = format!(
        "SELECT {projection} FROM session WHERE time_updated >= ?1 ORDER BY time_updated DESC LIMIT 64"
    );
    let recent: Vec<Session> = connection
        .prepare(&recent_sql)?
        .query_map([now - 600_000], read_row)?
        .collect::<rusqlite::Result<_>>()?;
    let mut seen: HashSet<String> = sessions.iter().map(|session| session.id.clone()).collect();
    sessions.extend(
        recent
            .into_iter()
            .filter(|session| seen.insert(session.id.clone())),
    );
    hydrated.retain(|_, session| session.is_recent(now));
    for session in &mut sessions {
        let key = (path.to_path_buf(), session.id.clone());
        if let Some(previous) = hydrated.get(&key).filter(|previous| {
            previous.updated == session.updated && previous.archived == session.archived
        }) {
            *session = previous.clone();
            continue;
        }
        read_messages(&connection, session, &message_columns, &v2_columns)?;
        resolve_metadata(session);
        if session.is_recent(now) {
            hydrated.insert(key, session.clone());
        }
    }
    Ok((sessions, next))
}

fn read_messages(
    connection: &Connection,
    session: &mut Session,
    v1: &HashSet<String>,
    v2: &HashSet<String>,
) -> Result<()> {
    let mut messages = BTreeMap::new();
    for (table, cols) in [("message", v1), ("session_message", v2)] {
        if !["id", "data", "session_id", "time_created"]
            .iter()
            .all(|key| cols.contains(*key))
        {
            continue;
        }
        let sql = format!(
            "SELECT id, data, time_created FROM {table} WHERE session_id = ?1 ORDER BY time_created, id"
        );
        let mut stmt = connection.prepare(&sql)?;
        let mut rows = stmt.query([&session.id])?;
        while let Some(row) = rows.next()? {
            let raw: String = row.get(1)?;
            anyhow::ensure!(
                raw.len() <= MAX_JSON_BYTES,
                "OpenCode message exceeds bounded metadata size"
            );
            let mut data: Value =
                serde_json::from_str(&raw).context("Invalid OpenCode message JSON")?;
            if let Some(info) = data.get("info").cloned() {
                data = info;
            }
            let id: String = row.get(0)?;
            let created: i64 = row.get(2)?;
            if data.get("role").and_then(Value::as_str) == Some("assistant")
                || data.get("modelID").is_some()
            {
                messages.insert(id, (created, data));
            }
        }
    }
    let mut ordered: Vec<_> = messages.into_values().collect();
    ordered.sort_by_key(|(created, _)| *created);
    let mut groups: BTreeMap<(String, String), ModelUsage> = BTreeMap::new();
    let mut all_costs_known = !ordered.is_empty();
    let mut total_cost = 0.0;
    for (_, message) in &ordered {
        let provider = string(message, "providerID");
        let model = string(message, "modelID");
        if !model.is_empty() {
            session.metadata.model_id = model.clone();
            session.metadata.model_provider = provider.clone();
            session.metadata.variant = message
                .get("variant")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        let tokens = message.get("tokens").unwrap_or(&Value::Null);
        let usage = ModelUsage {
            provider_id: provider.clone(),
            model_id: model.clone(),
            input: number(tokens.get("input")),
            output: number(tokens.get("output")),
            reasoning: number(tokens.get("reasoning")),
            cache_read: number(tokens.pointer("/cache/read")),
            cache_write: number(tokens.pointer("/cache/write")),
            cost: message
                .get("cost")
                .and_then(Value::as_f64)
                .filter(|cost| cost.is_finite() && *cost >= 0.0),
        };
        all_costs_known &= usage.cost.is_some();
        total_cost += usage.cost.unwrap_or(0.0);
        let group = groups
            .entry((provider, model))
            .or_insert_with(|| ModelUsage {
                provider_id: usage.provider_id.clone(),
                model_id: usage.model_id.clone(),
                cost: Some(0.0),
                ..Default::default()
            });
        add_usage(group, &usage);
        add_usage(&mut session.usage, &usage);
        session.context_used = tokens
            .get("total")
            .and_then(Value::as_u64)
            .or_else(|| (usage.total_tokens() > 0).then(|| usage.total_tokens()));
        session.context_window = message
            .get("contextWindow")
            .and_then(Value::as_u64)
            .filter(|tokens| *tokens > 0);
        if session.context_window.is_some() {
            session.metadata.context_source = Some("message".into());
        }
        session.activity = if message
            .pointer("/time/completed")
            .and_then(Value::as_i64)
            .is_some()
        {
            "Waiting for input"
        } else {
            "Thinking"
        }
        .into();
    }
    session.cost = (all_costs_known && total_cost.is_finite()).then_some(total_cost);
    session.metadata.models = groups.into_values().collect();
    let part_cols = columns(connection, "part")?;
    if part_cols.contains("data") {
        let mut stmt = connection.prepare(
            "SELECT data FROM part WHERE session_id = ?1 ORDER BY time_updated DESC LIMIT 16",
        )?;
        let parts = stmt.query_map([&session.id], |row| row.get::<_, String>(0))?;
        for raw in parts {
            let raw = raw?;
            if raw.len() > MAX_JSON_BYTES {
                continue;
            }
            let Ok(part) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            if part.get("type").and_then(Value::as_str) != Some("tool") {
                continue;
            }
            if session.activity == "Thinking"
                && part.pointer("/state/status").and_then(Value::as_str) == Some("running")
            {
                session.activity = match part.get("tool").and_then(Value::as_str).unwrap_or("") {
                    "read" | "glob" | "grep" => "Reading files",
                    "edit" | "write" | "apply_patch" => "Editing files",
                    "bash" | "shell" => "Running command",
                    _ => "Using tool",
                }
                .into();
                session.activity_target = part
                    .pointer("/state/input/filePath")
                    .and_then(Value::as_str)
                    .map(|path| crate::codex::session::sanitize_file_target(path, 64));
            }
            break;
        }
    }
    Ok(())
}

fn add_usage(total: &mut ModelUsage, usage: &ModelUsage) {
    total.input = total.input.saturating_add(usage.input);
    total.output = total.output.saturating_add(usage.output);
    total.reasoning = total.reasoning.saturating_add(usage.reasoning);
    total.cache_read = total.cache_read.saturating_add(usage.cache_read);
    total.cache_write = total.cache_write.saturating_add(usage.cache_write);
    total.cost = total
        .cost
        .zip(usage.cost)
        .map(|(a, b)| a + b)
        .filter(|value| value.is_finite());
}

fn resolve_metadata(session: &mut Session) {
    session.metadata.model_name = session.metadata.model_id.clone();
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"));
    let path = config_home.join("opencode/opencode.json");
    if let Ok(bytes) = std::fs::read(path)
        && bytes.len() <= MAX_JSON_BYTES
        && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        && let Some(model) = value
            .get("provider")
            .and_then(|providers| providers.get(&session.metadata.model_provider))
            .and_then(|provider| provider.get("models"))
            .and_then(|models| models.get(&session.metadata.model_id))
    {
        if let Some(name) = model.get("name").and_then(Value::as_str) {
            session.metadata.model_name = name.to_string();
        }
        if session.context_window.is_none() {
            session.context_window = model
                .pointer("/limit/context")
                .and_then(Value::as_u64)
                .filter(|value| *value > 0);
            if session.context_window.is_some() {
                session.metadata.context_source = Some("opencode_config".into());
            }
        }
    }
    if session.metadata.model_id == "gpt-6-astra" || session.metadata.model_id == "gpt-6-astra-fast"
    {
        session.metadata.model_name = if session.metadata.model_id.ends_with("-fast") {
            "GPT-6 Astra · Fast"
        } else {
            "GPT-6 Astra"
        }
        .into();
    }
    if session.activity.is_empty() {
        session.activity = "Idle".into();
    }
}

fn number(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or(0)
}
fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests;
