//! Tests for the pre-cutover Orion SQL reader.

use super::*;
use crate::orion::{Collector, Config};

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let (path, _connection) = crate::orion::testkit::create_legacy(dir.path());
    (dir, path)
}

fn assistant(model: &str, input: u64, output: u64, read: u64, write: u64, done: bool) -> String {
    let completed = if done { ",\"completed\":2" } else { "" };
    format!(
        "{{\"role\":\"assistant\",\"modelId\":\"{model}\",\"providerId\":\"anthropic\",\
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
    let model = "claude-opus-5-5";
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
            params![now, "{\"modelSelection\":{\"providerId\":\"anthropic\",\"modelId\":\"claude-opus-5-5\",\"options\":{\"reasoningLevel\":\"medium\"}}}"],
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
        r#"{"config":{"modelConfigRules":{"providerModelRules":[{"providerId":"anthropic","modelId":"claude-opus-5-5","config":{"properties":{"contextWindow":1000000}}}]}}}"#,
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
    let (dir, _, _) = priced_session(&["claude-opus-5-5", "deepseek/deepseek-v4.1-flash"]);
    let session = poll_live(&dir);
    // Only Opus 5.5 is priced: 1M input + 1M output = $4 + $20.
    assert!((session.known_cost.unwrap() - 24.0).abs() < 1e-9);
    assert!(!session.cost_complete, "an unpriced model makes it partial");
    assert!((session.cost_parts.total() - 24.0).abs() < 1e-9);
    assert_eq!(session.project, "pulse");
}

#[test]
fn a_session_with_no_priced_model_has_no_cost_instead_of_zero() {
    let (dir, _, _) = priced_session(&["deepseek/deepseek-v4.1-flash"]);
    let session = poll_live(&dir);
    assert_eq!(session.known_cost, None);
    assert!(!session.cost_complete);
    assert_eq!(session.cost_parts, super::super::CostParts::default());
    assert_eq!(session.usage.total(), 2_000_000, "tokens are still counted");
}

#[test]
fn output_speed_uses_completed_timed_requests_of_the_session_and_its_children() {
    let (dir, path, now) = priced_session(&["claude-opus-5-5"]);
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
    let (dir, path, now) = priced_session(&["claude-opus-5-5"]);
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
#[ignore = "Reads the local pre-cutover Orion App database; run explicitly"]
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
    let now = chrono::Utc::now().timestamp_millis();
    Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO session VALUES ('root', 'p', NULL, '/home/dev/pulse', 'T', ?1, ?1, NULL, 'interactive')",
            params![now],
        )
        .unwrap();
    let original = crate::orion::testkit::read_only(&path);
    let config = Config {
        data_roots: vec![dir.path().to_path_buf()],
        ..Default::default()
    };
    let batch = Collector::default().poll(&config);
    assert!(batch.diagnostics.is_empty(), "{:?}", batch.diagnostics);
    crate::orion::testkit::restore(&path, original);
}
