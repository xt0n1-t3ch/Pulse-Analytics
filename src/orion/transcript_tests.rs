//! Tests for the Orion transcript reader.

use super::super::testkit::{Msg, context_line, gzip, message_line, selection_line, tool_line};
use super::*;
use std::io::Write;

fn refreshed<'a>(cache: &'a mut TranscriptCache, path: &Path) -> &'a Transcript {
    cache.refresh("s", &[path.to_path_buf()]).unwrap();
    cache.get("s").expect("transcript")
}

#[test]
fn the_last_line_of_a_message_id_wins_and_the_first_write_has_no_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let mut text = String::new();
    // The same assistant message streamed three times.
    for (seq, msg) in [
        Msg::new("m1").tokens(0, 0, 0, 0).streaming(),
        Msg::new("m1").tokens(900, 10, 0, 0).streaming(),
        Msg::new("m1").tokens(1_000, 100, 800, 100),
    ]
    .into_iter()
    .enumerate()
    {
        text += &message_line(seq as u64 + 1, &msg);
    }
    std::fs::write(&path, text).unwrap();
    let mut cache = TranscriptCache::default();
    let state = refreshed(&mut cache, &path);
    assert_eq!(state.messages.len(), 1);
    let message = &state.messages["m1"];
    assert_eq!(message.usage.input, 1_000);
    assert_eq!(message.usage.output, 100);
    assert_eq!(message.usage.cache_read, 800);
    assert_eq!(message.usage.cache_write, 100);
    assert!(message.completed);
    assert_eq!(message.fill, 1_100);
    // 100 fresh input, 100 output, 100 cache write, 800 cache read on Opus 5.5.
    let parts = message.estimate.unwrap().parts;
    assert!((parts.total() - (0.0004 + 0.002 + 0.0005 + 0.00016)).abs() < 1e-12);
}

#[test]
fn a_plain_file_is_parsed_incrementally_and_a_partial_line_waits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let first = message_line(1, &Msg::new("m1").tokens(10, 1, 0, 0));
    std::fs::write(&path, &first).unwrap();
    let mut cache = TranscriptCache::default();
    assert_eq!(refreshed(&mut cache, &path).messages.len(), 1);
    let offset = cache.get("s").unwrap().offset;
    assert_eq!(offset, first.len() as u64);

    // A second message arrives with its newline still unwritten.
    let second = message_line(2, &Msg::new("m2").tokens(20, 2, 0, 0));
    let (head, tail) = second.trim_end().split_at(30);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(head.as_bytes()).unwrap();
    assert_eq!(refreshed(&mut cache, &path).messages.len(), 1);
    assert_eq!(
        cache.get("s").unwrap().offset,
        offset,
        "partial line not consumed"
    );

    file.write_all(tail.as_bytes()).unwrap();
    file.write_all(b"\n").unwrap();
    let state = refreshed(&mut cache, &path);
    assert_eq!(state.messages.len(), 2);
    assert_eq!(state.offset, (first.len() + second.len()) as u64);
}

#[test]
fn a_shrunk_file_resets_the_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let one = message_line(1, &Msg::new("m1").tokens(10, 1, 0, 0));
    let two = message_line(2, &Msg::new("m2").tokens(20, 2, 0, 0));
    std::fs::write(&path, format!("{one}{two}")).unwrap();
    let mut cache = TranscriptCache::default();
    assert_eq!(refreshed(&mut cache, &path).messages.len(), 2);
    // Orion truncated a torn tail: the file is shorter than what was read.
    std::fs::write(&path, &one).unwrap();
    let state = refreshed(&mut cache, &path);
    assert_eq!(state.messages.keys().collect::<Vec<_>>(), ["m1"]);
}

#[test]
fn a_gzip_transcript_is_read_whole_and_a_removed_message_disappears() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl.gz");
    let mut text = message_line(1, &Msg::new("m1").tokens(10, 1, 0, 0));
    text += &message_line(2, &Msg::new("m2").tokens(20, 2, 0, 0));
    text += "{\"v\":1,\"seq\":3,\"kind\":\"message\",\"op\":\"remove\",\"id\":\"m2\",\"at\":9}\n";
    gzip(&path, &text);
    let mut cache = TranscriptCache::default();
    let state = refreshed(&mut cache, &path);
    assert_eq!(state.messages.keys().collect::<Vec<_>>(), ["m1"]);
    // Unchanged file: the same state, no second parse.
    cache.refresh("s", std::slice::from_ref(&path)).unwrap();
    assert_eq!(cache.get("s").unwrap().messages.len(), 1);
}

#[test]
fn entries_and_tool_parts_fill_selection_context_and_the_running_tool() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let mut text = selection_line(1, "anthropic", "claude-opus-5-5", "max");
    text += &context_line(2, 43_758, 1_000_000, 800_000, Some(0.41));
    text += &tool_line(
        3,
        "p1",
        "Edit",
        "running",
        Some("/work/pulse/src/main.rs"),
        500,
    );
    text += &tool_line(4, "p2", "Read", "running", None, 600);
    text += &tool_line(5, "p2", "Read", "completed", None, 700);
    std::fs::write(&path, text).unwrap();
    let mut cache = TranscriptCache::default();
    let state = refreshed(&mut cache, &path);
    assert_eq!(
        state.selection,
        Some(Selection {
            provider: "anthropic".into(),
            model: "claude-opus-5-5".into(),
            effort: Some("max".into())
        })
    );
    assert_eq!(
        state.context,
        Some(ContextUsage {
            used: Some(43_758),
            max: Some(1_000_000),
            auto_compact_threshold: Some(800_000),
            cache_hit_rate: Some(0.41)
        })
    );
    assert_eq!(state.running_tools.len(), 1);
    let mark = &state.running_tools["p1"];
    assert_eq!((mark.tool.as_str(), mark.at), ("Edit", 500));
    assert_eq!(mark.target.as_deref(), Some("/work/pulse/src/main.rs"));
}

#[test]
fn an_overlong_line_is_skipped_without_losing_the_next_one() {
    let big = format!("{}\n", "x".repeat(MAX_LINE_BYTES as usize + 10));
    let small = message_line(1, &Msg::new("m1").tokens(10, 1, 0, 0));
    let mut state = Transcript::default();
    let mut reader = io::Cursor::new(format!("{big}{small}").into_bytes());
    let used = read_lines(&mut reader, |line| state.apply(line)).unwrap();
    assert_eq!(used, (big.len() + small.len()) as u64);
    assert_eq!(state.messages.len(), 1);
}
