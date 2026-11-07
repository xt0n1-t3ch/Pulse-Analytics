//! Builders for the Orion stores the tests write into temp folders. Column and
//! record names are copied from a real Orion install (2026-11-07): the session
//! index `projects/index.sqlite` (schema version 2), JSONL records of
//! `docs/specs/session-storage.md`, and `telemetry/usage.sqlite`.

use flate2::Compression;
use flate2::write::GzEncoder;
use rusqlite::{Connection, params};
use std::io::Write;
use std::path::{Path, PathBuf};

/// One assistant message record. `streaming()` is a partial write: not
/// completed and with no finish reason yet.
#[derive(Clone)]
pub(super) struct Msg {
    id: String,
    model: String,
    provider: String,
    input: u64,
    output: u64,
    read: u64,
    write: u64,
    created: i64,
    completed: bool,
    finish: Option<&'static str>,
}

impl Msg {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.into(),
            model: "claude-opus-5-5".into(),
            provider: "anthropic".into(),
            input: 0,
            output: 0,
            read: 0,
            write: 0,
            created: 1_000,
            completed: true,
            finish: Some("stop"),
        }
    }
    /// `input` already contains `read` and `write`, as Orion records it.
    pub fn tokens(mut self, input: u64, output: u64, read: u64, write: u64) -> Self {
        (self.input, self.output, self.read, self.write) = (input, output, read, write);
        self
    }
    pub fn model(mut self, provider: &str, model: &str) -> Self {
        self.provider = provider.into();
        self.model = model.into();
        self
    }
    pub fn created(mut self, created: i64) -> Self {
        self.created = created;
        self
    }
    pub fn finish(mut self, finish: &'static str) -> Self {
        self.finish = Some(finish);
        self
    }
    pub fn streaming(mut self) -> Self {
        self.completed = false;
        self.finish = None;
        self
    }
}

pub(super) fn message_line(seq: u64, msg: &Msg) -> String {
    let completed = if msg.completed {
        format!(",\"completed\":{}", msg.created + 1)
    } else {
        String::new()
    };
    let finish = msg
        .finish
        .map(|finish| format!(",\"finish\":\"{finish}\""))
        .unwrap_or_default();
    format!(
        "{{\"v\":1,\"seq\":{seq},\"batch\":{seq},\"commit\":true,\"kind\":\"message\",\"op\":\"put\",\
         \"id\":\"{id}\",\"at\":{at},\"row\":{{\"id\":\"{id}\"}},\"data\":{{\"role\":\"assistant\",\
         \"time\":{{\"created\":{at}{completed}}},\"modelId\":\"{model}\",\"providerId\":\"{provider}\",\
         \"cost\":0{finish},\"tokens\":{{\"total\":{total},\"input\":{input},\"output\":{output},\
         \"reasoning\":0,\"cache\":{{\"read\":{read},\"write\":{write}}}}}}}}}\n",
        id = msg.id,
        at = msg.created,
        model = msg.model,
        provider = msg.provider,
        total = msg.input + msg.output,
        input = msg.input,
        output = msg.output,
        read = msg.read,
        write = msg.write,
    )
}

/// A user message: the reader must never count it or keep its text.
pub(super) fn user_line(seq: u64, id: &str) -> String {
    format!(
        "{{\"v\":1,\"seq\":{seq},\"batch\":{seq},\"commit\":true,\"kind\":\"message\",\"op\":\"put\",\
         \"id\":\"{id}\",\"at\":1,\"data\":{{\"role\":\"user\",\"time\":{{\"created\":1}},\
         \"text\":\"private prompt\"}}}}\n"
    )
}

pub(super) fn selection_line(seq: u64, provider: &str, model: &str, effort: &str) -> String {
    format!(
        "{{\"v\":1,\"seq\":{seq},\"batch\":{seq},\"commit\":true,\"kind\":\"entry\",\"op\":\"put\",\
         \"id\":\"s:runtime-model-selection\",\"at\":5,\"row\":{{\"type\":\"runtime/model_selection\"}},\
         \"data\":{{\"modelSelection\":{{\"providerId\":\"{provider}\",\"modelId\":\"{model}\",\
         \"options\":{{\"reasoningLevel\":\"{effort}\"}}}}}}}}\n"
    )
}

pub(super) fn context_line(
    seq: u64,
    used: u64,
    max: u64,
    threshold: u64,
    hit_rate: Option<f64>,
) -> String {
    let hit_rate = hit_rate.map_or("null".to_string(), |rate| rate.to_string());
    format!(
        "{{\"v\":1,\"seq\":{seq},\"batch\":{seq},\"commit\":true,\"kind\":\"entry\",\"op\":\"put\",\
         \"id\":\"s:runtime-context-usage\",\"at\":6,\"row\":{{\"type\":\"runtime/context_usage\"}},\
         \"data\":{{\"anchorMessageId\":\"m\",\"window\":{{\"usedTokens\":{used},\"maxTokens\":{max},\
         \"autoCompactThresholdTokens\":{threshold},\"cache\":{{\"hitRate\":{hit_rate}}}}}}}}}\n"
    )
}

pub(super) fn tool_line(
    seq: u64,
    id: &str,
    tool: &str,
    status: &str,
    file_path: Option<&str>,
    at: i64,
) -> String {
    let input = file_path
        .map(|path| format!("{{\"file_path\":\"{path}\"}}"))
        .unwrap_or_else(|| "{}".into());
    format!(
        "{{\"v\":1,\"seq\":{seq},\"batch\":{seq},\"commit\":true,\"kind\":\"part\",\"op\":\"put\",\
         \"id\":\"{id}\",\"parent\":\"m\",\"at\":{at},\"data\":{{\"type\":\"tool\",\"tool\":\"{tool}\",\
         \"state\":{{\"status\":\"{status}\",\"input\":{input}}}}}}}\n"
    )
}

pub(super) fn gzip(path: &Path, text: &str) {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(text.as_bytes()).unwrap();
    std::fs::write(path, encoder.finish().unwrap()).unwrap();
}

/// Creates `projects/index.sqlite` with Orion's schema version 2.
pub(super) fn create_index(root: &Path) -> Connection {
    let path = super::layout::index_path(root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session (id text primary key, parent_id text, workspace_key text not null, \
               rel_dir text not null unique, nested integer not null default 0, task_type text, \
               title text, time_created integer, time_updated integer, time_archived integer, \
               last_write_ms integer, indexed_bytes integer not null default 0, \
               last_seq integer not null default 0, last_batch integer not null default 0, \
               codec text, directory text, listed integer not null default 0, \
               row_json text not null default '{}'); \
             CREATE INDEX session_parent_idx on session(parent_id, time_created, id); \
             CREATE TABLE record (session_id text not null, kind text not null, \
               record_id text not null, parent_id text, first_seq integer not null, \
               last_seq integer not null, offset integer not null, length integer not null, \
               removed integer not null default 0, lists integer not null default 0, \
               primary key (session_id, kind, record_id));",
        )
        .unwrap();
    connection
}

/// A session row. `last_write_ms` is the freshness signal; `time_updated` lags.
pub(super) struct Row<'a> {
    pub id: &'a str,
    pub parent: Option<&'a str>,
    pub directory: &'a str,
    pub rel_dir: String,
    pub created: i64,
    pub last_write_ms: Option<i64>,
    pub time_updated: i64,
}

impl<'a> Row<'a> {
    /// A root session in the workspace folder `-work-pulse`.
    pub fn root(id: &'a str, created: i64, last_write_ms: i64) -> Self {
        Self {
            id,
            parent: None,
            directory: "/work/pulse",
            rel_dir: format!("-work-pulse/{id}"),
            created,
            last_write_ms: Some(last_write_ms),
            time_updated: created,
        }
    }
    /// A `subagent_child` nested under its parent's `subagents/` folder.
    pub fn child(id: &'a str, parent: &'a str, created: i64, last_write_ms: i64) -> Self {
        Self {
            parent: Some(parent),
            rel_dir: format!("-work-pulse/{parent}/subagents/{id}"),
            ..Self::root(id, created, last_write_ms)
        }
    }
}

pub(super) fn insert_session(connection: &Connection, row: &Row<'_>) {
    let task_type = if row.parent.is_some() {
        "subagent_child"
    } else {
        "interactive"
    };
    connection
        .execute(
            "INSERT INTO session (id, parent_id, workspace_key, rel_dir, nested, task_type, title, \
               time_created, time_updated, last_write_ms, directory, listed) \
             VALUES (?1, ?2, '-work-pulse', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1)",
            params![
                row.id,
                row.parent,
                row.rel_dir,
                i64::from(row.parent.is_some()),
                task_type,
                format!("Title of {}", row.id),
                row.created,
                row.time_updated,
                row.last_write_ms,
                row.directory
            ],
        )
        .unwrap();
}

/// Writes the plain transcript `projects/<rel_dir>.jsonl` and returns its path.
pub(super) fn write_transcript(root: &Path, rel_dir: &str, text: &str) -> PathBuf {
    let path = super::layout::transcript_candidates(&super::layout::projects_dir(root), rel_dir)
        .unwrap()
        .remove(0);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

/// Creates `telemetry/usage.sqlite` with Orion's `model_usage` table.
pub(super) fn create_usage(root: &Path) -> Connection {
    let path = super::layout::usage_path(root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE model_usage (id text primary key, logical_request_id text not null, \
               attempt_index integer not null default 0, session_id text not null, turn_id text, \
               trace_id text, span_id text, assistant_message_id text, parent_user_message_id text, \
               query_source text not null, provider_id text not null, model_id text not null, \
               variant text, agent text, mode text, task_type text, \
               status text not null check(status in ('running', 'completed', 'error', 'cancelled')), \
               started_at integer not null, first_token_at integer, completed_at integer, \
               duration_ms integer, time_to_first_token_ms integer, finish_reason text, \
               tool_call_count integer not null default 0, input_tokens integer not null default 0, \
               output_tokens integer not null default 0, reasoning_tokens integer not null default 0, \
               cache_creation_input_tokens integer not null default 0, \
               cache_read_input_tokens integer not null default 0, provider_total_tokens integer, \
               computed_total_tokens integer not null default 0);",
        )
        .unwrap();
    connection
}

/// One `model_usage` row. Tokens: `input` includes both cache categories.
pub(super) struct Usage<'a> {
    pub session: &'a str,
    pub source: &'a str,
    pub agent: Option<&'a str>,
    pub model: &'a str,
    pub status: &'a str,
    pub started_at: i64,
    /// Milliseconds from start to the first token, and from the first token to completion.
    pub ttft_and_stream: Option<(i64, i64)>,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
}

impl<'a> Usage<'a> {
    pub fn new(session: &'a str, source: &'a str, model: &'a str) -> Self {
        Self {
            session,
            source,
            agent: None,
            model,
            status: "completed",
            started_at: 1_000_000,
            ttft_and_stream: None,
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
        }
    }
}

pub(super) fn insert_usage(connection: &Connection, row: &Usage<'_>) {
    let (first_token, completed, ttft) = match row.ttft_and_stream {
        Some((ttft, stream)) => (
            Some(row.started_at + ttft),
            Some(row.started_at + ttft + stream),
            Some(ttft),
        ),
        None => (None, Some(row.started_at + 100), None),
    };
    let id: i64 = connection
        .query_row("SELECT COUNT(*) FROM model_usage", [], |r| r.get(0))
        .unwrap();
    connection
        .execute(
            "INSERT INTO model_usage (id, logical_request_id, session_id, query_source, provider_id, \
               model_id, agent, status, started_at, first_token_at, completed_at, \
               time_to_first_token_ms, input_tokens, output_tokens, cache_creation_input_tokens, \
               cache_read_input_tokens) \
             VALUES (?1, ?1, ?2, ?3, 'anthropic', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                format!("u{id}"),
                row.session,
                row.source,
                row.model,
                row.agent,
                row.status,
                row.started_at,
                first_token,
                completed,
                ttft,
                row.input,
                row.output,
                row.cache_write,
                row.cache_read
            ],
        )
        .unwrap();
}

/// Creates the pre-cutover `cli/db/db.sqlite` with the tables the legacy reader
/// queries, and returns its path.
pub(super) fn create_legacy(root: &Path) -> (PathBuf, Connection) {
    let path = super::layout::legacy_database_path(root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
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
    (path, connection)
}

/// Makes a file read-only and returns its previous permissions, so a test can
/// prove a reader never writes to it and then put it back.
pub(super) fn read_only(path: &Path) -> std::fs::Permissions {
    let original = std::fs::metadata(path).unwrap().permissions();
    let mut locked = original.clone();
    locked.set_readonly(true);
    std::fs::set_permissions(path, locked).unwrap();
    original
}

pub(super) fn restore(path: &Path, original: std::fs::Permissions) {
    std::fs::set_permissions(path, original).unwrap();
}
