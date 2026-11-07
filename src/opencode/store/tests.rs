//! Tests for the OpenCode store over fixture databases of both schema versions.

use super::*;
use serde_json::json;

fn fixture(v2: bool) -> (tempfile::TempDir, PathBuf, Connection) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("opencode.db");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, title TEXT, model TEXT, parent_id TEXT, time_created INTEGER, time_updated INTEGER, time_archived INTEGER);").unwrap();
    connection.execute_batch(&format!("CREATE TABLE {} (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);", if v2 { "session_message" } else { "message" })).unwrap();
    (temp, path, connection)
}

fn insert_session(connection: &Connection, id: &str, updated: i64) {
    connection
        .execute(
            "INSERT INTO session VALUES (?1,'C:/project','Private prompt title',?2,NULL,1,?3,NULL)",
            params![
                id,
                r#"{"id":"selected-not-used","providerID":"custom","variant":"high"}"#,
                updated
            ],
        )
        .unwrap();
}

fn insert_message(
    connection: &Connection,
    v2: bool,
    id: &str,
    session: &str,
    model: &str,
    cost: Option<f64>,
) {
    let data = json!({"role":"assistant","modelID":model,"providerID":"custom","variant":"high","cost":cost,"tokens":{"input":10,"output":2,"reasoning":1,"cache":{"read":5,"write":3}},"time":{"completed":100}});
    connection
        .execute(
            &format!(
                "INSERT INTO {} VALUES (?1,?2,100,100,?3)",
                if v2 { "session_message" } else { "message" }
            ),
            params![id, session, data.to_string()],
        )
        .unwrap();
}

#[test]
fn all_model_families_and_both_schemas_preserve_real_identity() {
    for v2 in [false, true] {
        let (_temp, path, connection) = fixture(v2);
        for (index, model) in [
            "gpt-6-astra",
            "claude-opus-4-7",
            "gemini-3",
            "deepseek-v4",
            "qwen/qwen3",
            "grok-4.5",
            "glm-5.3-flash",
            "unknown-custom",
        ]
        .iter()
        .enumerate()
        {
            let id = index.to_string();
            insert_session(&connection, &id, 100);
            insert_message(&connection, v2, &id, &id, model, Some(0.0));
        }
        let (sessions, _) = read_database(&path, &(0, String::new()), 100).unwrap();
        assert_eq!(sessions.len(), 8);
        for session in sessions {
            assert_eq!(session.metadata.model_provider, "custom");
            assert_ne!(session.metadata.model_id, "selected-not-used");
            assert_eq!(session.cost, Some(0.0));
            assert_eq!(session.usage.total_tokens(), 21);
            assert!(session.context_window.is_none());
        }
    }
}

#[test]
fn unknown_cost_and_model_switch_do_not_invent_attribution() {
    let (_temp, path, connection) = fixture(false);
    insert_session(&connection, "one", 100);
    insert_message(&connection, false, "a", "one", "claude-opus-4-7", Some(0.2));
    insert_message(&connection, false, "b", "one", "gpt-6-astra", None);
    let (sessions, _) = read_database(&path, &(0, String::new()), 100).unwrap();
    let s = &sessions[0];
    assert!(s.cost.is_none());
    assert_eq!(s.metadata.models.len(), 2);
    assert!(
        s.metadata
            .models
            .iter()
            .any(|model| model.cost == Some(0.2))
    );
}

#[test]
fn importer_pages_and_retries_without_duplicates() {
    let (_temp, path, connection) = fixture(false);
    for i in 0..70 {
        insert_session(&connection, &format!("{i:03}"), 100);
    }
    let (first, next) = read_database(&path, &(0, String::new()), 1_000_000).unwrap();
    assert_eq!(first.len(), 64);
    let (second, _) = read_database(&path, &next, 1_000_000).unwrap();
    assert_eq!(second.len(), 6);
    let (retry, _) = read_database(&path, &(0, String::new()), 1_000_000).unwrap();
    assert_eq!(retry.len(), 64);
    connection
        .execute("UPDATE session SET time_updated=200 WHERE id='000'", [])
        .unwrap();
    let (changed, _) = read_database(&path, &(100, "069".into()), 1_000_000).unwrap();
    assert_eq!(changed[0].id, "000");
}

#[test]
fn readonly_reader_reports_schema_and_lock_errors() {
    let (_temp, path, connection) = fixture(false);
    connection.execute_batch("BEGIN EXCLUSIVE").unwrap();
    assert!(read_database(&path, &(0, String::new()), 0).is_err());
    connection
        .execute_batch("ROLLBACK; DROP TABLE session")
        .unwrap();
    assert!(
        read_database(&path, &(0, String::new()), 0)
            .unwrap_err()
            .to_string()
            .contains("schema")
    );
}

#[test]
fn explicit_database_paths_are_canonicalized_and_deduplicated() {
    let (_temp, path, _connection) = fixture(false);
    let config = Config {
        database_paths: vec![path.clone(), path.clone()],
        ..Default::default()
    };
    let found = discover_databases(&config);
    let canonical = path.canonicalize().unwrap();
    assert_eq!(
        found
            .iter()
            .filter(|candidate| **candidate == canonical)
            .count(),
        1
    );
}

#[test]
fn unchanged_recent_sessions_use_the_metadata_cache() {
    let (_temp, path, connection) = fixture(false);
    insert_session(&connection, "cache", 100);
    insert_message(&connection, false, "one", "cache", "custom", Some(0.0));
    let mut cache = HashMap::new();
    read_database_cached(&path, &(0, String::new()), 100, &mut cache).unwrap();
    connection
        .execute("UPDATE message SET data='invalid JSON'", [])
        .unwrap();
    assert!(read_database_cached(&path, &(0, String::new()), 100, &mut cache).is_ok());
    connection
        .execute("UPDATE session SET time_updated=101", [])
        .unwrap();
    assert!(read_database_cached(&path, &(0, String::new()), 101, &mut cache).is_err());
}

#[test]
fn completion_null_is_active_and_completed_messages_override_old_tool_state() {
    let (_temp, path, connection) = fixture(false);
    insert_session(&connection, "one", 100);
    insert_message(
        &connection,
        false,
        "message",
        "one",
        "mimo-v2.5-free",
        Some(0.0),
    );
    connection
        .execute(
            "UPDATE message SET data = json_set(data, '$.time.completed', NULL)",
            [],
        )
        .unwrap();
    let (active, _) = read_database(&path, &(0, String::new()), 100).unwrap();
    assert_eq!(active[0].activity, "Thinking");
    connection
        .execute(
            "UPDATE message SET data = json_set(data, '$.time.completed', 100)",
            [],
        )
        .unwrap();
    connection
        .execute_batch(
            "CREATE TABLE part (id TEXT, session_id TEXT, time_updated INTEGER, data TEXT);",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO part VALUES ('old','one',99,?1)",
            params![json!({"type":"tool","tool":"bash","state":{"status":"running"}}).to_string()],
        )
        .unwrap();
    let (completed, _) = read_database(&path, &(0, String::new()), 100).unwrap();
    assert_eq!(completed[0].activity, "Waiting for input");
    assert!(completed[0].is_idle(100));
    assert_eq!(completed[0].metadata.model_id, "mimo-v2.5-free");
}
