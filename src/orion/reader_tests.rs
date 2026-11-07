//! End-to-end tests of the current Orion layout through `Collector::poll`:
//! every store is built in a temp folder with Orion's real column names.
//! Expected values are worked out from the fixtures and published rates
//! (Claude Opus 5.5: $4 input, $20 output, $5 cache write, $0.20 cache read per
//! million tokens), not read back from the code under test.

use super::testkit::{
    Msg, Row, context_line, create_index, create_legacy, create_usage, insert_session,
    message_line, read_only, restore, selection_line, tool_line, user_line, write_transcript,
};
use super::{Collector, Config, Session};

/// Real model ids Orion writes on this machine (spec section 2).
pub(super) const OPUS: &str = "claude-opus-5-5";
pub(super) const DEEPSEEK: &str = "deepseek/deepseek-v4.1-flash";
use rusqlite::{Connection, params};
use std::path::Path;

pub(super) struct World {
    pub dir: tempfile::TempDir,
    pub index: Connection,
    pub now: i64,
}

pub(super) fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let index = create_index(dir.path());
    World {
        dir,
        index,
        now: chrono::Utc::now().timestamp_millis(),
    }
}

impl World {
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn config(&self) -> Config {
        Config {
            data_roots: vec![self.root().to_path_buf()],
            ..Default::default()
        }
    }

    pub fn poll(&self) -> super::Batch {
        Collector::default().poll(&self.config())
    }

    /// The only live session of a poll with no diagnostics.
    pub fn live(&self) -> Session {
        let batch = self.poll();
        assert!(batch.diagnostics.is_empty(), "{:?}", batch.diagnostics);
        batch.live.into_iter().next().expect("a live session")
    }
}

pub(super) fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

/// A finished request of the given size, `age_ms` before now.
pub(super) fn done(w: &World, id: &str, age_ms: i64, tokens: (u64, u64, u64, u64)) -> String {
    let (input, output, read, write) = tokens;
    message_line(
        1,
        &Msg::new(id)
            .created(w.now - age_ms)
            .tokens(input, output, read, write),
    )
}

#[test]
fn a_root_session_rolls_up_its_subagents_and_reads_orion_context() {
    let w = world();
    let now = w.now;
    insert_session(&w.index, &Row::root("root", now - 3_600_000, now));
    insert_session(
        &w.index,
        &Row::child("child1", "root", now - 1_800_000, now - 1_000),
    );
    insert_session(
        &w.index,
        &Row::child("child2", "root", now - 900_000, now - 500),
    );

    // The root: a user message, a selection, one request written three times
    // (the first write has 0 tokens), one request still streaming, Orion's
    // context entry and a tool that is running.
    let streamed = |msg: Msg| msg.created(now - 30_000);
    let mut text = user_line(1, "u1");
    text += &selection_line(2, "anthropic", OPUS, "medium");
    text += &message_line(3, &streamed(Msg::new("m1")).streaming());
    text += &message_line(
        4,
        &streamed(Msg::new("m1")).tokens(900, 10, 0, 0).streaming(),
    );
    text += &message_line(5, &streamed(Msg::new("m1")).tokens(1_000, 100, 800, 100));
    text += &message_line(6, &Msg::new("m2").created(now - 5_000).streaming());
    text += &context_line(7, 43_758, 1_000_000, 800_000, Some(0.41));
    text += &tool_line(
        8,
        "p1",
        "Edit",
        "running",
        Some("/work/pulse/src/main.rs"),
        now - 1_000,
    );
    write_transcript(w.root(), "-work-pulse/root", &text);
    // child1 finished its request; child2 is still streaming.
    let child1 = done(&w, "c1", 20_000, (500, 50, 0, 0));
    write_transcript(w.root(), "-work-pulse/root/subagents/child1", &child1);
    let child2 = message_line(1, &Msg::new("c2").created(now - 1_000).streaming());
    write_transcript(w.root(), "-work-pulse/root/subagents/child2", &child2);

    let batch = w.poll();
    assert!(batch.diagnostics.is_empty(), "{:?}", batch.diagnostics);
    assert_eq!(
        batch.changed.len(),
        1,
        "subagents are not separate sessions"
    );
    let live = &batch.live[0];
    assert_eq!(live.id, "root");
    assert_eq!(live.project, "pulse");
    assert_eq!(live.title.as_deref(), Some("Title of root"));
    assert_eq!((live.updated, live.tree_updated), (now, now));

    // Tokens: m1 (last write) + child1; the user message and 0-token writes add nothing.
    assert_eq!(live.usage.input, 1_500);
    assert_eq!(live.usage.output, 150);
    assert_eq!(live.usage.cache_read, 800);
    assert_eq!(live.usage.cache_write, 100);
    assert_eq!(live.usage.fresh_input(), 600);
    assert_eq!(live.messages, 4, "m1, m2, c1 and c2");
    // Opus 5.5 over 600 fresh input, 150 output, 100 cache write, 800 cache read.
    let parts = live.cost_parts;
    close(parts.input, 0.0024);
    close(parts.output, 0.003);
    close(parts.cache_write, 0.0005);
    close(parts.cache_read, 0.00016);
    close(live.known_cost.unwrap(), 0.00606);
    assert!(live.cost_complete);

    assert_eq!(live.model_label(), "Claude Opus 5.5 · Medium");
    assert_eq!(live.provider_id, "anthropic");
    assert_eq!(live.activity, "Editing files");
    assert_eq!(live.activity_target.as_deref(), Some("main.rs"));
    assert_eq!(live.subagent_count, 2);
    assert_eq!(live.running_subagents, 1, "only child2 is streaming");
    assert!(live.is_active(now));

    // Orion's own context numbers, not Pulse's bundled values.
    assert_eq!(live.context_used, Some(43_758));
    assert_eq!(live.context_window, Some(1_000_000));
    assert_eq!(live.auto_compact_threshold, Some(800_000));
    assert_eq!(live.cache_hit_rate, Some(0.41));
}

#[test]
fn freshness_is_last_write_ms_and_the_whole_tree_decides_which_roots_are_live() {
    let w = world();
    let now = w.now;
    // Root row not touched for an hour, but written to just now (time_updated lags).
    let lagging = Row {
        time_updated: now - 3_600_000,
        ..Row::root("lagging", now - 7_200_000, now)
    };
    insert_session(&w.index, &lagging);
    // Idle root whose subagent wrote just now.
    insert_session(
        &w.index,
        &Row::root("delegating", now - 7_200_000, now - 3_600_000),
    );
    insert_session(
        &w.index,
        &Row::child("kid", "delegating", now - 600_000, now - 2_000),
    );
    // Idle for an hour: history only.
    insert_session(
        &w.index,
        &Row::root("idle", now - 7_200_000, now - 3_600_000),
    );
    // Never roots of the live list: archived, and another task type.
    insert_session(&w.index, &Row::root("archived", now - 600_000, now));
    insert_session(&w.index, &Row::root("fork", now - 600_000, now));
    w.index
        .execute(
            "UPDATE session SET time_archived = 1 WHERE id = 'archived'",
            [],
        )
        .unwrap();
    w.index
        .execute(
            "UPDATE session SET task_type = 'fork' WHERE id = 'fork'",
            [],
        )
        .unwrap();

    let batch = w.poll();
    assert!(batch.diagnostics.is_empty(), "{:?}", batch.diagnostics);
    let live: Vec<_> = batch.live.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(live.len(), 2);
    assert!(
        live.contains(&"lagging") && live.contains(&"delegating"),
        "{live:?}"
    );
    let delegating = batch.live.iter().find(|s| s.id == "delegating").unwrap();
    assert_eq!(delegating.updated, now - 3_600_000);
    assert_eq!(delegating.tree_updated, now - 2_000);
    let mut changed: Vec<_> = batch.changed.iter().map(|s| s.id.as_str()).collect();
    changed.sort_unstable();
    assert_eq!(
        changed,
        ["delegating", "idle", "lagging"],
        "history roots only"
    );
    assert_eq!(batch.last_activity, Some(now));
}

#[test]
fn activity_follows_the_latest_request_of_the_root_session() {
    let cases = [
        (None, "Idle"),
        (Some(Msg::new("m").streaming()), "Thinking"),
        (Some(Msg::new("m").finish("tool-calls")), "Thinking"),
        (Some(Msg::new("m").finish("stop")), "Waiting for input"),
    ];
    for (message, expected) in cases {
        let w = world();
        insert_session(&w.index, &Row::root("root", w.now - 60_000, w.now));
        if let Some(message) = message {
            let text = message_line(1, &message.created(w.now - 1_000));
            write_transcript(w.root(), "-work-pulse/root", &text);
        }
        assert_eq!(w.live().activity, expected);
    }
}

#[test]
fn running_subagents_need_an_open_request_and_a_recent_write() {
    let w = world();
    let now = w.now;
    insert_session(&w.index, &Row::root("root", now - 600_000, now));
    for (id, last_write) in [
        ("open", now - 1_000),
        ("finished", now - 1_000),
        ("stale", now - 300_000),
    ] {
        insert_session(&w.index, &Row::child(id, "root", now - 500_000, last_write));
    }
    write_transcript(
        w.root(),
        "-work-pulse/root",
        &done(&w, "m1", 5_000, (10, 1, 0, 0)),
    );
    let streaming = |id: &str| message_line(1, &Msg::new(id).created(now - 2_000).streaming());
    write_transcript(w.root(), "-work-pulse/root/subagents/open", &streaming("a"));
    write_transcript(
        w.root(),
        "-work-pulse/root/subagents/stale",
        &streaming("b"),
    );
    write_transcript(
        w.root(),
        "-work-pulse/root/subagents/finished",
        &done(&w, "c", 2_000, (10, 1, 0, 0)),
    );
    let session = w.live();
    assert_eq!(session.subagent_count, 3, "history keeps the total started");
    assert_eq!(session.running_subagents, 1);
    assert_eq!(
        session.activity, "Delegating",
        "the root waits for its subagent"
    );
}

#[test]
fn appended_lines_are_added_on_the_next_poll() {
    let w = world();
    insert_session(&w.index, &Row::root("root", w.now - 60_000, w.now));
    let path = write_transcript(
        w.root(),
        "-work-pulse/root",
        &done(&w, "m1", 2_000, (100, 10, 0, 0)),
    );
    let mut collector = Collector::default();
    let first = collector.poll(&w.config());
    assert_eq!(first.live[0].usage.input, 100);
    collector.acknowledge(&first);
    assert!(
        collector.poll(&w.config()).changed.is_empty(),
        "nothing new, nothing to import"
    );

    let mut text = std::fs::read_to_string(&path).unwrap();
    text += &message_line(
        2,
        &Msg::new("m2").created(w.now - 1_000).tokens(200, 20, 0, 0),
    );
    std::fs::write(&path, text).unwrap();
    w.index
        .execute(
            "UPDATE session SET last_write_ms = ?1 WHERE id = 'root'",
            params![w.now + 1],
        )
        .unwrap();
    let second = collector.poll(&w.config());
    assert_eq!(second.changed.len(), 1, "a newer write is imported again");
    assert_eq!(
        (second.live[0].usage.input, second.live[0].messages),
        (300, 2)
    );
}

#[test]
fn unpriced_and_catalog_models_get_their_own_cost_status() {
    let w = world();
    insert_session(&w.index, &Row::root("deep", w.now - 60_000, w.now));
    let deepseek = Msg::new("m1")
        .model("command-code", DEEPSEEK)
        .tokens(1_000, 100, 0, 0);
    write_transcript(w.root(), "-work-pulse/deep", &message_line(1, &deepseek));
    let session = w.live();
    assert_eq!(session.known_cost, None, "unknown families stay unpriced");
    assert!(!session.cost_complete);
    assert_eq!(session.usage.total(), 1_100, "tokens are still counted");
    // Pulse names an unknown model by the part after the provider prefix.
    assert_eq!(session.model_label(), "deepseek-v4.1-flash");

    // GPT-6.1 Terra: $1 input and $5 output per million.
    let w = world();
    insert_session(&w.index, &Row::root("terra", w.now - 60_000, w.now));
    let terra = Msg::new("m1")
        .model("codex", "gpt-6.1-terra")
        .tokens(100_000, 1_000, 0, 0);
    let text = selection_line(1, "codex", "gpt-6.1-terra", "high") + &message_line(2, &terra);
    write_transcript(w.root(), "-work-pulse/terra", &text);
    let session = w.live();
    assert_eq!(session.model_label(), "GPT-6.1 Terra · High");
    close(session.known_cost.unwrap(), 0.105);
    assert!(session.cost_complete);
}

#[test]
fn unreadable_transcripts_are_diagnostics_and_orion_files_are_never_written() {
    let w = world();
    insert_session(&w.index, &Row::root("broken", w.now - 60_000, w.now));
    let gz = w.root().join("projects/-work-pulse/broken.jsonl.gz");
    std::fs::create_dir_all(gz.parent().unwrap()).unwrap();
    std::fs::write(&gz, b"this is not gzip").unwrap();
    let usage = create_usage(w.root());
    drop(usage);
    let index_path = super::layout::index_path(w.root());
    let watched = [
        index_path.clone(),
        super::layout::usage_path(w.root()),
        gz.clone(),
    ];
    let before: Vec<_> = watched.iter().map(|p| std::fs::read(p).unwrap()).collect();
    let originals: Vec<_> = watched.iter().map(|path| read_only(path)).collect();

    let batch = w.poll();
    assert_eq!(
        batch.live.len(),
        1,
        "the session is listed without its transcript"
    );
    assert_eq!(batch.live[0].usage.total(), 0);
    assert_eq!(batch.diagnostics.len(), 1, "{:?}", batch.diagnostics);
    assert!(
        batch.diagnostics[0].contains("transcript"),
        "{:?}",
        batch.diagnostics
    );

    let after: Vec<_> = watched.iter().map(|p| std::fs::read(p).unwrap()).collect();
    assert_eq!(before, after);
    let siblings: Vec<_> = std::fs::read_dir(index_path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !siblings
            .iter()
            .any(|name| name.ends_with("-wal") || name.ends_with("-journal")),
        "{siblings:?}"
    );
    for (path, original) in watched.iter().zip(originals) {
        restore(path, original);
    }
}

#[test]
fn the_current_layout_wins_and_the_legacy_store_is_read_only_while_it_has_sessions() {
    let now = chrono::Utc::now().timestamp_millis();
    let legacy_row = |connection: &Connection, id: &str| {
        connection
            .execute(
                "INSERT INTO session VALUES (?1, 'p', NULL, '/home/dev/old', 'T', ?2, ?2, NULL, 'interactive')",
                params![id, now],
            )
            .unwrap();
    };
    let ids = |root: &Path| {
        let config = Config {
            data_roots: vec![root.to_path_buf()],
            ..Default::default()
        };
        let batch = Collector::default().poll(&config);
        (
            batch.live.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            batch.diagnostics,
        )
    };

    // Only a legacy database with a session: it is read.
    let dir = tempfile::tempdir().unwrap();
    let (_, legacy) = create_legacy(dir.path());
    legacy_row(&legacy, "old");
    assert_eq!(ids(dir.path()), (vec!["old".to_string()], vec![]));

    // Both layouts: the index wins and the legacy rows are not mixed in.
    let index = create_index(dir.path());
    insert_session(&index, &Row::root("new", now - 60_000, now));
    assert_eq!(ids(dir.path()), (vec!["new".to_string()], vec![]));

    // A legacy database with no sessions is not a store.
    let empty = tempfile::tempdir().unwrap();
    create_legacy(empty.path());
    let (live, diagnostics) = ids(empty.path());
    assert!(live.is_empty());
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
}

/// Reads the real Orion store of this machine without printing message text.
/// Run with `cargo nextest run -p cc-discord-presence --run-ignored ignored-only local_orion_store_probe`.
#[test]
#[ignore = "Reads the local Orion App store; run explicitly"]
fn local_orion_store_probe() {
    let config = Config::default();
    let mut collector = Collector::default();
    let started = std::time::Instant::now();
    let batch = collector.poll(&config);
    let first = started.elapsed();
    collector.acknowledge(&batch);
    let started = std::time::Instant::now();
    let again = collector.poll(&config);
    println!(
        "first={first:?} second={:?} changed={} live={} diagnostics={:?}",
        started.elapsed(),
        batch.changed.len(),
        again.live.len(),
        batch.diagnostics
    );
    for session in batch.changed.iter().chain(again.live.iter()).take(4) {
        println!(
            "{} project={} model={} activity={} tokens={} ctx={:?}/{:?} compact={:?} hit={:?} \
             cost={:?} ttft={:?} speed={:?} subagents={} helper={:?} agents={}",
            session.id,
            session.project,
            session.model_label(),
            session.activity,
            session.usage.total(),
            session.context_used,
            session.context_window,
            session.auto_compact_threshold,
            session.cache_hit_rate,
            session.known_cost,
            session.ttft_ms_median,
            session.output_tokens_per_sec,
            session.subagent_count,
            session.helper_requests,
            session.subagents_by_agent.as_ref().map_or(0, Vec::len),
        );
    }
}
