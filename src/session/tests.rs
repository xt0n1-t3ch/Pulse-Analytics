//! Unit tests for Claude session parsing: effort, activity, cache-write pricing and checkpoints.

use super::*;
use std::io::Write;

fn parser_test_line(cwd: &Path, input_tokens: u64) -> String {
    serde_json::json!({
        "type": "assistant",
        "timestamp": "2026-08-12T12:00:00Z",
        "sessionId": "4ccf0482-61c0-4611-9d22-becaf1781231",
        "cwd": cwd,
        "message": {
            "model": "claude-sonnet-4-20250514",
            "usage": {
                "input_tokens": input_tokens,
                "output_tokens": 1,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0
            },
            "content": [{"type": "text", "text": "ok"}]
        }
    })
    .to_string()
}

fn parse_test_session(
    path: &Path,
    modified_tick: u64,
    cache: &mut SessionParseCache,
) -> ClaudeSessionSnapshot {
    let metadata = fs::metadata(path).expect("session metadata");
    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(modified_tick);
    parse_session_file_cached(
        path,
        &metadata,
        modified,
        "test-project",
        &mut GitBranchCache::new(Duration::ZERO),
        cache,
        ReasoningEffort::Medium,
        &[],
    )
    .expect("parse session")
    .expect("session snapshot")
}

#[test]
fn cached_parser_reads_only_the_appended_delta_when_mtime_advances() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let first = format!("{}\n", parser_test_line(dir.path(), 10));
    fs::write(&path, first.as_bytes()).expect("initial JSONL");
    let mut cache = SessionParseCache::default();

    let initial = parse_test_session(&path, 1, &mut cache);
    assert_eq!(initial.input_tokens, 10);
    let first_cursor = cache.entries[&path].cursor;

    let appended = format!("{}\n", parser_test_line(dir.path(), 20));
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open append");
    file.write_all(appended.as_bytes()).expect("append JSONL");

    let updated = parse_test_session(&path, 2, &mut cache);
    let entry = &cache.entries[&path];
    assert_eq!(updated.input_tokens, 30);
    assert!(entry.cursor > first_cursor);
    assert_eq!(entry.body_bytes_read_last_poll, appended.len() as u64);
    assert!(entry.anchor_bytes_read_last_poll <= 64);

    let unchanged = parse_test_session(&path, 2, &mut cache);
    assert_eq!(unchanged.input_tokens, 30);
    let entry = &cache.entries[&path];
    assert_eq!(entry.body_bytes_read_last_poll, 0);
    assert_eq!(entry.anchor_bytes_read_last_poll, 0);
}

#[test]
fn cached_parser_accepts_a_complete_final_record_without_a_newline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let record = parser_test_line(dir.path(), 19);
    fs::write(&path, record.as_bytes()).expect("JSONL without trailing newline");
    let mut cache = SessionParseCache::default();

    let snapshot = parse_test_session(&path, 1, &mut cache);

    assert_eq!(snapshot.input_tokens, 19);
    assert_eq!(cache.entries[&path].cursor, record.len() as u64);
}

#[test]
fn cached_parser_waits_for_a_complete_line_without_losing_or_double_counting_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let first = format!("{}\n", parser_test_line(dir.path(), 10));
    fs::write(&path, first.as_bytes()).expect("initial JSONL");
    let mut cache = SessionParseCache::default();
    let initial = parse_test_session(&path, 1, &mut cache);
    assert_eq!(initial.input_tokens, 10);
    let stable_cursor = cache.entries[&path].cursor;

    let second = parser_test_line(dir.path(), 20);
    let split = second.len() / 2;
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(&second.as_bytes()[..split]).unwrap();
    let partial = parse_test_session(&path, 2, &mut cache);
    assert_eq!(partial.input_tokens, 10);
    assert_eq!(cache.entries[&path].cursor, stable_cursor);

    file.write_all(&second.as_bytes()[split..]).unwrap();
    file.write_all(b"\n").unwrap();
    drop(file);
    let complete = parse_test_session(&path, 3, &mut cache);
    assert_eq!(complete.input_tokens, 30);
}

#[test]
fn cached_parser_resets_on_truncation_or_tail_anchor_mismatch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let original = format!(
        "{}\n{}\n",
        parser_test_line(dir.path(), 10),
        parser_test_line(dir.path(), 20)
    );
    fs::write(&path, original).expect("original JSONL");
    let mut cache = SessionParseCache::default();
    assert_eq!(parse_test_session(&path, 1, &mut cache).input_tokens, 30);

    let replacement = format!("{}\n", parser_test_line(dir.path(), 7));
    fs::write(&path, replacement).expect("truncated replacement");
    assert_eq!(parse_test_session(&path, 2, &mut cache).input_tokens, 7);

    let same_or_larger = format!(
        "{}\n{}\n",
        parser_test_line(dir.path(), 3).replace("\"ok\"", "\"replacement\""),
        parser_test_line(dir.path(), 4)
    );
    fs::write(&path, same_or_larger).expect("same-size replacement");
    assert_eq!(parse_test_session(&path, 3, &mut cache).input_tokens, 7);
    assert_eq!(
        cache.entries[&path].body_bytes_read_last_poll,
        fs::metadata(&path).unwrap().len()
    );
}

/// FNV-1a reference values; a change here would orphan every checkpoint.
#[test]
fn checkpoint_file_names_use_a_stable_hash() {
    assert_eq!(stable_path_hash(Path::new("")), 0xcbf2_9ce4_8422_2325);
    assert_eq!(stable_path_hash(Path::new("a")), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(
        session_checkpoint_path(Path::new("root"), Path::new("a")),
        Path::new("root").join("af63dc4c8601ec8c.json")
    );
}

#[test]
fn pruning_keeps_current_checkpoints_and_removes_stale_ones() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("checkpoints");
    let live = dir.path().join("live.jsonl");
    let gone = dir.path().join("gone.jsonl");
    for path in [&live, &gone] {
        fs::write(path, format!("{}\n", parser_test_line(dir.path(), 10))).expect("JSONL");
        let mut cache = SessionParseCache::default();
        parse_test_session(path, 1, &mut cache);
        persist_session_checkpoint_to(&root, path, &cache.entries[path]).expect("persist");
    }
    fs::remove_file(&gone).expect("remove transcript");

    let live_checkpoint = session_checkpoint_path(&root, &live);
    let mut other_build: serde_json::Value =
        serde_json::from_slice(&fs::read(&live_checkpoint).unwrap()).unwrap();
    other_build["app_version"] = serde_json::json!("0.0.0-older-build");
    let other_path = dir.path().join("other.jsonl");
    fs::write(&other_path, "{}\n").unwrap();
    other_build["source_path"] = serde_json::json!(other_path);
    fs::write(
        session_checkpoint_path(&root, &other_path),
        serde_json::to_vec(&other_build).unwrap(),
    )
    .unwrap();
    fs::write(root.join("corrupt.json"), b"{").unwrap();
    let fresh_temp = root.join(".tmpA1b2C3");
    fs::write(&fresh_temp, b"partial").unwrap();

    assert_eq!(prune_session_checkpoints_in(&root), 3);
    assert!(fresh_temp.exists(), "a temporary write in progress is kept");
    fs::remove_file(&fresh_temp).unwrap();
    let remaining: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(remaining, vec![live_checkpoint]);
    assert_eq!(prune_session_checkpoints_in(&dir.path().join("missing")), 0);
}

#[test]
fn legacy_checkpoint_directory_is_removed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let legacy = dir.path().join("pulse-session-checkpoints");
    fs::create_dir_all(legacy.join("v1")).unwrap();
    fs::write(legacy.join("v1").join("0123456789abcdef.json"), b"{}").unwrap();
    fs::write(legacy.join("v1").join(".tmpXYZ"), b"").unwrap();

    assert_eq!(remove_legacy_session_checkpoints(&legacy), 2);
    assert!(!legacy.exists());
    assert!(dir.path().exists(), "the parent Claude directory is kept");
}

#[test]
fn persistent_checkpoint_round_trips_only_when_source_identity_still_matches() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let checkpoint_root = dir.path().join("checkpoints");
    let original = format!("{}\n", parser_test_line(dir.path(), 10));
    fs::write(&path, &original).expect("initial JSONL");
    let mut cache = SessionParseCache::default();
    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
    assert_eq!(parse_test_session(&path, 1, &mut cache).input_tokens, 10);

    persist_session_checkpoint_to(&checkpoint_root, &path, &cache.entries[&path])
        .expect("persist checkpoint");
    let restored = load_session_checkpoint_from(
        &checkpoint_root,
        &path,
        &fs::metadata(&path).unwrap(),
        modified,
    )
    .expect("valid checkpoint");
    assert_eq!(restored.cursor, original.len() as u64);
    assert_eq!(restored.accumulator.total_input_tokens, 10);
    assert_eq!(restored.snapshot.unwrap().input_tokens, 10);
    assert_eq!(
        fs::read_dir(&checkpoint_root).unwrap().count(),
        1,
        "atomic checkpointing must leave no temporary files behind"
    );

    let mut replacement = original.as_bytes().to_vec();
    let last_json_byte = replacement.len() - 2;
    replacement[last_json_byte] = b']';
    fs::write(&path, replacement).expect("same-size replacement");
    assert!(
        load_session_checkpoint_from(
            &checkpoint_root,
            &path,
            &fs::metadata(&path).unwrap(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(2),
        )
        .is_none(),
        "a matching size cannot promote a checkpoint with a stale tail anchor"
    );
}

/// A checkpoint written by another build holds costs priced at that build's
/// rates, so it must be discarded and the session re-parsed from JSONL.
#[test]
fn checkpoint_from_another_build_is_not_restored() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let checkpoint_root = dir.path().join("checkpoints");
    fs::write(
        &path,
        format!(
            "{}
",
            parser_test_line(dir.path(), 10)
        ),
    )
    .expect("JSONL");
    let mut cache = SessionParseCache::default();
    parse_test_session(&path, 1, &mut cache);
    persist_session_checkpoint_to(&checkpoint_root, &path, &cache.entries[&path])
        .expect("persist checkpoint");
    let checkpoint_path = session_checkpoint_path(&checkpoint_root, &path);
    let load = || {
        load_session_checkpoint_from(
            &checkpoint_root,
            &path,
            &fs::metadata(&path).unwrap(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        )
    };
    assert!(load().is_some(), "same build restores its own checkpoint");

    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&checkpoint_path).unwrap()).unwrap();
    value["app_version"] = serde_json::json!("0.0.0-older-build");
    fs::write(&checkpoint_path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(load().is_none(), "another build's costs are not reused");

    value.as_object_mut().unwrap().remove("app_version");
    fs::write(&checkpoint_path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(load().is_none(), "unversioned checkpoints are not reused");
}

#[test]
fn corrupt_persistent_checkpoint_falls_back_to_the_jsonl_source() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let checkpoint_root = dir.path().join("checkpoints");
    fs::create_dir_all(&checkpoint_root).unwrap();
    fs::write(&path, format!("{}\n", parser_test_line(dir.path(), 17))).expect("source JSONL");
    fs::write(session_checkpoint_path(&checkpoint_root, &path), b"{").expect("corrupt checkpoint");

    assert!(
        load_session_checkpoint_from(
            &checkpoint_root,
            &path,
            &fs::metadata(&path).unwrap(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        )
        .is_none()
    );
    let mut source_cache = SessionParseCache::default();
    assert_eq!(
        parse_test_session(&path, 1, &mut source_cache).input_tokens,
        17
    );
}

#[test]
fn cold_scan_selects_only_fresh_transcripts_and_their_subagent_parent() {
    let root = PathBuf::from("projects").join("encoded-workspace");
    let parent_id = "4ccf0482-61c0-4611-9d22-becaf1781231";
    let parent = root.join(format!("{parent_id}.jsonl"));
    let active_subagent = root
        .join(parent_id)
        .join("subagents")
        .join("agent-active.jsonl");
    let stale_unrelated = root.join("stale.jsonl");
    let fresh_main = root.join("fresh.jsonl");
    let cutoff = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(50);
    let fresh = SystemTime::UNIX_EPOCH + Duration::from_secs(150);

    let selected = selected_session_paths(
        &[
            (parent.clone(), old, false),
            (active_subagent.clone(), fresh, true),
            (stale_unrelated.clone(), old, false),
            (fresh_main.clone(), fresh, false),
        ],
        cutoff,
    );

    assert!(selected.contains(&active_subagent));
    assert!(selected.contains(&parent));
    assert!(selected.contains(&fresh_main));
    assert!(!selected.contains(&stale_unrelated));
}

#[test]
fn claude_session_scan_respects_depth_limit() {
    let temp = tempfile::tempdir().unwrap();
    let nested = temp.path().join("one").join("two").join("three");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("session.jsonl"), b"{}\n").unwrap();

    let files_seen = walk_session_root_with_limits(temp.path(), 2, 100)
        .filter(|entry| entry.file_type().is_file())
        .count();

    assert_eq!(files_seen, 0);
}

#[test]
fn claude_session_scan_respects_entry_limit() {
    let temp = tempfile::tempdir().unwrap();
    for idx in 0..3 {
        fs::write(temp.path().join(format!("{idx}.jsonl")), b"{}\n").unwrap();
    }

    let files_seen = walk_session_root_with_limits(temp.path(), usize::MAX, 3)
        .filter(|entry| entry.file_type().is_file())
        .count();

    assert_eq!(files_seen, 2);
}

#[test]
fn effort_label_all_variants() {
    assert_eq!(ReasoningEffort::Low.label(), "Low");
    assert_eq!(ReasoningEffort::Medium.label(), "Medium");
    assert_eq!(ReasoningEffort::High.label(), "High");
    assert_eq!(ReasoningEffort::ExtraHigh.label(), "Extra High");
    assert_eq!(ReasoningEffort::Max.label(), "Max");
}

#[test]
fn effort_short_label_all_variants() {
    assert_eq!(ReasoningEffort::Low.short_label(), "Low");
    assert_eq!(ReasoningEffort::Medium.short_label(), "Med");
    assert_eq!(ReasoningEffort::High.short_label(), "High");
    assert_eq!(ReasoningEffort::ExtraHigh.short_label(), "X-High");
    assert_eq!(ReasoningEffort::Max.short_label(), "Max");
}

#[test]
fn effort_api_value_matches_anthropic_spec() {
    assert_eq!(ReasoningEffort::Low.api_value(), "low");
    assert_eq!(ReasoningEffort::Medium.api_value(), "medium");
    assert_eq!(ReasoningEffort::High.api_value(), "high");
    assert_eq!(ReasoningEffort::ExtraHigh.api_value(), "xhigh");
    assert_eq!(ReasoningEffort::Max.api_value(), "max");
}

#[test]
fn effort_from_api_roundtrip() {
    for e in [
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::ExtraHigh,
        ReasoningEffort::Max,
    ] {
        assert_eq!(ReasoningEffort::from_api(e.api_value()), Some(e));
    }
}

#[test]
fn effort_from_api_aliases() {
    assert_eq!(
        ReasoningEffort::from_api("xhigh"),
        Some(ReasoningEffort::ExtraHigh)
    );
    assert_eq!(
        ReasoningEffort::from_api("x-high"),
        Some(ReasoningEffort::ExtraHigh)
    );
    assert_eq!(
        ReasoningEffort::from_api("extra_high"),
        Some(ReasoningEffort::ExtraHigh)
    );
    assert_eq!(
        ReasoningEffort::from_api("extra high"),
        Some(ReasoningEffort::ExtraHigh)
    );
    assert_eq!(
        ReasoningEffort::from_api("EXTRAHIGH"),
        Some(ReasoningEffort::ExtraHigh)
    );
    assert_eq!(
        ReasoningEffort::from_api("med"),
        Some(ReasoningEffort::Medium)
    );
    assert_eq!(
        ReasoningEffort::from_api("maximum"),
        Some(ReasoningEffort::Max)
    );
    assert_eq!(ReasoningEffort::from_api("bogus"), None);
}

#[test]
fn effort_is_high_includes_extra_high() {
    assert!(!ReasoningEffort::Low.is_high());
    assert!(!ReasoningEffort::Medium.is_high());
    assert!(ReasoningEffort::High.is_high());
    assert!(ReasoningEffort::ExtraHigh.is_high());
    assert!(ReasoningEffort::Max.is_high());
}

#[test]
fn effort_serde_snake_case_preserves_extra_high() {
    let json = serde_json::to_string(&ReasoningEffort::ExtraHigh).unwrap();
    assert_eq!(json, "\"extra_high\"");
    let parsed: ReasoningEffort = serde_json::from_str("\"extra_high\"").unwrap();
    assert_eq!(parsed, ReasoningEffort::ExtraHigh);
}

/// Builds an `assistant` transcript line in the exact shape Claude Code writes
/// today: the effort selector value lives in a **top-level** `effort` field,
/// sibling to `type`/`message`, not inside a system-reminder text block.
fn assistant_line_with_effort(effort: Option<&str>, sidechain: bool) -> JsonlMessage {
    let mut line = serde_json::json!({
        "type": "assistant",
        "isSidechain": sidechain,
        "message": {
            "model": "claude-opus-5",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "ok"}]
        }
    });
    if let Some(effort) = effort {
        line["effort"] = serde_json::json!(effort);
    }
    serde_json::from_value(line).unwrap()
}

#[test]
fn assistant_effort_field_drives_reasoning_effort() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    process_jsonl_message(&mut acc, &assistant_line_with_effort(Some("high"), false));

    assert_eq!(acc.reasoning_effort, ReasoningEffort::High);
    assert!(acc.reasoning_effort_explicitly_set);
    assert_eq!(infer_effort(&acc), ReasoningEffort::High);
}

#[test]
fn assistant_effort_field_maps_every_api_tier() {
    for (raw, expected) in [
        ("low", ReasoningEffort::Low),
        ("medium", ReasoningEffort::Medium),
        ("high", ReasoningEffort::High),
        ("xhigh", ReasoningEffort::ExtraHigh),
        ("max", ReasoningEffort::Max),
    ] {
        let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Low);
        process_jsonl_message(&mut acc, &assistant_line_with_effort(Some(raw), false));
        assert_eq!(acc.reasoning_effort, expected, "raw effort {raw}");
    }
}

#[test]
fn latest_assistant_effort_wins_when_user_switches_mid_session() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    process_jsonl_message(&mut acc, &assistant_line_with_effort(Some("high"), false));
    process_jsonl_message(&mut acc, &assistant_line_with_effort(Some("xhigh"), false));

    assert_eq!(acc.reasoning_effort, ReasoningEffort::ExtraHigh);
}

#[test]
fn assistant_line_without_effort_preserves_last_known_effort() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    process_jsonl_message(&mut acc, &assistant_line_with_effort(Some("high"), false));
    process_jsonl_message(&mut acc, &assistant_line_with_effort(None, false));

    assert_eq!(acc.reasoning_effort, ReasoningEffort::High);
    assert!(acc.reasoning_effort_explicitly_set);
}

#[test]
fn sidechain_effort_never_shadows_the_main_chain_selector() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    process_jsonl_message(&mut acc, &assistant_line_with_effort(Some("high"), false));
    // A subagent spawned with an explicit `effort: low` override must not
    // rewrite what the operator picked for the parent session.
    process_jsonl_message(&mut acc, &assistant_line_with_effort(Some("low"), true));

    assert_eq!(acc.reasoning_effort, ReasoningEffort::High);
}

#[test]
fn an_early_sidechain_does_not_define_the_parent_effort() {
    // A subagent can answer before the parent's first turn. Honouring its
    // override here would report the subagent's tier as the session's for
    // the whole period before the parent responds.
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    process_jsonl_message(&mut acc, &assistant_line_with_effort(Some("low"), true));

    assert_eq!(acc.reasoning_effort, ReasoningEffort::Medium);
    assert!(!acc.reasoning_effort_explicitly_set);

    process_jsonl_message(&mut acc, &assistant_line_with_effort(Some("high"), false));
    assert_eq!(acc.reasoning_effort, ReasoningEffort::High);
}

#[test]
fn a_legacy_reminder_cannot_override_an_observed_effort_field() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    process_jsonl_message(&mut acc, &assistant_line_with_effort(Some("high"), false));

    // A later user event carrying the old system-reminder text, typically
    // describing a subagent, must not rewrite the authoritative value.
    let reminder: JsonlMessage = serde_json::from_value(serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": "<system-reminder>reasoning effort level: low</system-reminder>"}]
        }
    }))
    .unwrap();
    process_jsonl_message(&mut acc, &reminder);

    assert_eq!(acc.reasoning_effort, ReasoningEffort::High);
}

#[test]
fn a_legacy_reminder_still_applies_to_transcripts_without_the_effort_field() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    let reminder: JsonlMessage = serde_json::from_value(serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": "<system-reminder>reasoning effort level: max</system-reminder>"}]
        }
    }))
    .unwrap();
    process_jsonl_message(&mut acc, &reminder);

    assert_eq!(acc.reasoning_effort, ReasoningEffort::Max);
}

#[test]
fn unknown_effort_string_leaves_effort_untouched() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    process_jsonl_message(&mut acc, &assistant_line_with_effort(Some("turbo"), false));

    assert_eq!(acc.reasoning_effort, ReasoningEffort::Medium);
    assert!(!acc.reasoning_effort_explicitly_set);
}

#[test]
fn decode_project_paths() {
    let decoded = decode_project_path("home-user-projects-my--app");
    let decoded_str = decoded.to_string_lossy();
    assert!(decoded_str.contains("my-app") || decoded_str.contains("my-app"));
}

#[test]
fn subagent_parent_session_id_handles_direct_and_workflow_subagents() {
    let parent = "ecff266d-ecec-4662-800d-7d4e6cb6323a";
    let base = std::env::temp_dir()
        .join(".claude")
        .join("projects")
        .join("repo")
        .join(parent);
    let direct = base.join("subagents").join("agent-a.jsonl");
    let workflow = base
        .join("subagents")
        .join("workflows")
        .join("wf_4913aa25-d45")
        .join("agent-b.jsonl");

    assert_eq!(subagent_parent_session_id(&direct).as_deref(), Some(parent));
    assert_eq!(
        subagent_parent_session_id(&workflow).as_deref(),
        Some(parent)
    );
}

#[test]
fn git_branch_cache_ttl() {
    let mut cache = GitBranchCache::new(Duration::from_secs(30));
    assert_eq!(cache.get(Path::new("/nonexistent/path")), None);
}

#[test]
fn activity_kind_text() {
    let snap = ActivitySnapshot {
        kind: ActivityKind::EditingFile,
        target: Some("src/main.rs".to_string()),
        ..Default::default()
    };
    assert_eq!(snap.action_text(), "Editing");
    assert_eq!(snap.to_text(true), "Editing main.rs");
    assert_eq!(snap.to_text(false), "Editing");

    let snap = ActivitySnapshot {
        kind: ActivityKind::RunningCommand,
        target: Some("cargo build".to_string()),
        ..Default::default()
    };
    assert_eq!(snap.to_text(true), "Running command cargo build");
}

#[test]
fn activity_file_target_hides_project_and_branch_context() {
    let snap = ActivitySnapshot {
        kind: ActivityKind::ReadingFile,
        target: Some(
            "channel-events.ts - PropertyAlpha-Agent (feat/marketplace-addtochat-liveview-management)"
                .to_string(),
        ),
        ..Default::default()
    };

    assert_eq!(snap.to_text(true), "Reading channel-events.ts");
}

#[test]
fn classify_read_tool() {
    let content = serde_json::json!({
        "name": "Read",
        "input": {"file_path": "/foo/bar.rs"}
    });
    let (kind, target) = classify_tool_call(&content);
    assert_eq!(kind, ActivityKind::ReadingFile);
    assert_eq!(target, Some("bar.rs".to_string()));
}

#[test]
fn classify_bash_tool() {
    let content = serde_json::json!({
        "name": "Bash",
        "input": {"command": "cargo build"}
    });
    let (kind, target) = classify_tool_call(&content);
    assert_eq!(kind, ActivityKind::RunningCommand);
    assert_eq!(target, Some("cargo build".to_string()));
}

#[test]
fn classify_bash_tool_hides_noisy_env_path_target() {
    let content = serde_json::json!({
        "name": "Bash",
        "input": {"command": "D=\"C:/Users/xt0n1/.claude/jobs/ecff266d/tmp\" - PropertyAlpha-Agent"}
    });
    let (kind, target) = classify_tool_call(&content);
    assert_eq!(kind, ActivityKind::Thinking);
    assert_eq!(target, None);

    let snap = ActivitySnapshot {
        kind,
        target,
        ..Default::default()
    };
    assert_eq!(snap.to_text(true), "Thinking");
}

#[test]
fn classify_bash_tool_hides_decorative_echo_banner() {
    let content = serde_json::json!({
        "name": "Bash",
        "input": {"command": "echo ===== PropertyAlpha-Agent ====="}
    });
    let (kind, target) = classify_tool_call(&content);
    assert_eq!(kind, ActivityKind::Thinking);
    assert_eq!(target, None);

    let snap = ActivitySnapshot {
        kind,
        target,
        ..Default::default()
    };
    assert_eq!(snap.to_text(true), "Thinking");
}

#[test]
fn orphaned_pending_call_finalizes_as_idle() {
    let old =
        Utc::now() - chrono::Duration::seconds(ORPHANED_PENDING_CALL_SECS + IDLE_DEBOUNCE_SECS + 1);
    let mut tracker = ActivityTracker::default();
    tracker.register_call(
        "call_1",
        ActivityKind::RunningCommand,
        Some("cargo test".to_string()),
        Some(old),
    );

    let snapshot = tracker.finalize(Utc::now()).expect("activity");
    assert_eq!(snapshot.kind, ActivityKind::Idle);
    assert_eq!(snapshot.target, None);
    assert_eq!(snapshot.pending_calls, 0);
}

#[test]
fn recent_live_file_activity_reactivates_idle_snapshot() {
    let now = Utc::now();
    let mut tracker = ActivityTracker::default();
    tracker.mark_activity(
        ActivityKind::RunningCommand,
        Some("cargo test".to_string()),
        Some(now - chrono::Duration::seconds(IDLE_DEBOUNCE_SECS + 1)),
    );
    tracker.apply_idle_debounce(now);
    assert_eq!(
        tracker.finalize(now).expect("idle").kind,
        ActivityKind::Idle
    );

    tracker.note_live_file_activity(now, now);
    let snapshot = tracker.finalize(now).expect("activity");
    assert_eq!(snapshot.kind, ActivityKind::Thinking);
    assert_eq!(snapshot.target, None);
}

#[test]
fn idle_debounce_does_not_refresh_effective_activity_to_now() {
    let old = Utc::now() - chrono::Duration::seconds(IDLE_DEBOUNCE_SECS + 30);
    let now = Utc::now();
    let mut tracker = ActivityTracker::default();
    tracker.mark_activity(
        ActivityKind::RunningCommand,
        Some("cargo test".to_string()),
        Some(old),
    );

    tracker.apply_idle_debounce(now);
    let snapshot = tracker.finalize(now).expect("activity");

    assert_eq!(snapshot.kind, ActivityKind::Idle);
    assert_eq!(snapshot.last_effective_signal_at, Some(old));
    assert_ne!(snapshot.last_effective_signal_at, Some(now));
}

#[test]
fn classify_write_tool() {
    let content = serde_json::json!({
        "name": "Write",
        "input": {"file_path": "/home/user/project/src/lib.rs"}
    });
    let (kind, target) = classify_tool_call(&content);
    assert_eq!(kind, ActivityKind::EditingFile);
    assert_eq!(target, Some("lib.rs".to_string()));
}

#[test]
fn classify_edit_tool() {
    let content = serde_json::json!({
        "name": "Edit",
        "input": {"file_path": "/home/dev/project/main.rs"}
    });
    let (kind, target) = classify_tool_call(&content);
    assert_eq!(kind, ActivityKind::EditingFile);
    assert_eq!(target, Some("main.rs".to_string()));
}

#[cfg(windows)]
#[test]
fn classify_edit_tool_windows_path() {
    let content = serde_json::json!({
        "name": "Edit",
        "input": {"file_path": "C:\\Users\\dev\\project\\main.rs"}
    });
    let (kind, target) = classify_tool_call(&content);
    assert_eq!(kind, ActivityKind::EditingFile);
    assert_eq!(target, Some("main.rs".to_string()));
}

#[test]
fn classify_glob_tool() {
    let content = serde_json::json!({
        "name": "Glob",
        "input": {"pattern": "**/*.rs"}
    });
    let (kind, target) = classify_tool_call(&content);
    assert_eq!(kind, ActivityKind::ReadingFile);
    assert_eq!(target, Some("**/*.rs".to_string()));
}

#[test]
fn classify_ask_user() {
    let content = serde_json::json!({"name": "AskUserQuestion"});
    let (kind, target) = classify_tool_call(&content);
    assert_eq!(kind, ActivityKind::WaitingInput);
    assert_eq!(target, None);
}

#[test]
fn classify_web_search() {
    let content = serde_json::json!({"name": "WebSearch"});
    let (kind, _) = classify_tool_call(&content);
    assert_eq!(kind, ActivityKind::Thinking);
}

#[test]
fn classify_unknown_tool() {
    let content = serde_json::json!({"name": "CustomTool"});
    let (kind, _) = classify_tool_call(&content);
    assert_eq!(kind, ActivityKind::Thinking);
}

#[test]
fn extract_filename_from_path_variants() {
    let input = serde_json::json!({"file_path": "/a/b/c.rs"});
    assert_eq!(extract_filename(Some(&input)), Some("c.rs".to_string()));

    let input = serde_json::json!({"path": "/a/b/d.ts"});
    assert_eq!(extract_filename(Some(&input)), Some("d.ts".to_string()));

    let input = serde_json::json!({"notebook_path": "/a/b/notebook.ipynb"});
    assert_eq!(
        extract_filename(Some(&input)),
        Some("notebook.ipynb".to_string())
    );

    let input = serde_json::json!({"other": "value"});
    assert_eq!(extract_filename(Some(&input)), None);

    assert_eq!(extract_filename(None), None);
}

#[test]
fn truncate_target_short() {
    assert_eq!(truncate_target("hello", 40), "hello");
}

#[test]
fn truncate_target_long() {
    let long = "a".repeat(60);
    let result = truncate_target(&long, 40);
    assert!(result.len() <= 40);
    assert!(result.ends_with("..."));
}

#[test]
fn activity_snapshot_default() {
    let snap = ActivitySnapshot::default();
    assert_eq!(snap.kind, ActivityKind::Idle);
    assert_eq!(snap.target, None);
    assert_eq!(snap.action_text(), "Idle");
}

#[test]
fn activity_kind_all_variants() {
    assert_eq!(
        ActivitySnapshot {
            kind: ActivityKind::Thinking,
            ..Default::default()
        }
        .action_text(),
        "Thinking"
    );
    assert_eq!(
        ActivitySnapshot {
            kind: ActivityKind::ReadingFile,
            ..Default::default()
        }
        .action_text(),
        "Reading"
    );
    assert_eq!(
        ActivitySnapshot {
            kind: ActivityKind::RunningCommand,
            ..Default::default()
        }
        .action_text(),
        "Running command"
    );
    assert_eq!(
        ActivitySnapshot {
            kind: ActivityKind::WaitingInput,
            ..Default::default()
        }
        .action_text(),
        "Waiting for input"
    );
}

#[test]
fn synthetic_model_is_ignored() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    acc.model = Some("claude-opus-4-6".to_string());

    let msg: JsonlMessage = serde_json::from_value(serde_json::json!({
        "type": "assistant",
        "is_api_error_message": true,
        "message": {
            "model": "<synthetic>",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "API Error"}]
        }
    }))
    .unwrap();

    process_jsonl_message(&mut acc, &msg);
    assert_eq!(acc.model.as_deref(), Some("claude-opus-4-6"));
}

#[test]
fn speed_from_label_is_case_insensitive() {
    assert_eq!(Speed::from_label("fast"), Speed::Fast);
    assert_eq!(Speed::from_label("FAST"), Speed::Fast);
    assert_eq!(Speed::from_label(" Fast "), Speed::Fast);
    assert_eq!(Speed::from_label("standard"), Speed::Standard);
    assert_eq!(Speed::from_label("anything-else"), Speed::Standard);
    assert_eq!(Speed::Fast.as_str(), "fast");
    assert_eq!(Speed::Standard.as_str(), "standard");
}

fn assistant_usage_message(
    model: &str,
    input: u64,
    output: u64,
    speed: Option<&str>,
    service_tier: Option<&str>,
) -> JsonlMessage {
    let mut usage = serde_json::json!({
        "input_tokens": input,
        "output_tokens": output,
        "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": 0,
    });
    if let Some(speed) = speed {
        usage["speed"] = serde_json::json!(speed);
    }
    if let Some(tier) = service_tier {
        usage["service_tier"] = serde_json::json!(tier);
    }
    serde_json::from_value(serde_json::json!({
        "type": "assistant",
        "message": {
            "model": model,
            "type": "message",
            "role": "assistant",
            "usage": usage,
            "content": [{"type": "text", "text": "ok"}]
        }
    }))
    .unwrap()
}

#[test]
fn mixed_fast_and_standard_session_sums_cost_speed_correctly() {
    let model = "claude-opus-4-8";
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);

    let fast_msg = assistant_usage_message(model, 100_000, 20_000, Some("fast"), Some("priority"));
    let standard_msg =
        assistant_usage_message(model, 50_000, 10_000, Some("standard"), Some("standard"));
    let absent_speed_msg = assistant_usage_message(model, 30_000, 5_000, None, None);

    process_jsonl_message(&mut acc, &fast_msg);
    process_jsonl_message(&mut acc, &standard_msg);
    process_jsonl_message(&mut acc, &absent_speed_msg);

    let expected = cost::calculate_cost_with_context_and_speed(model, 100_000, 20_000, 0, 0, true)
        + cost::calculate_cost_with_context_and_speed(model, 50_000, 10_000, 0, 0, false)
        + cost::calculate_cost_with_context_and_speed(model, 30_000, 5_000, 0, 0, false);

    assert!((acc.total_cost - expected).abs() < 0.0000001);
    assert_eq!(acc.speed, Speed::Standard);
    assert_eq!(acc.service_tier, None);
}

fn compact_boundary_message(post_tokens: u64) -> JsonlMessage {
    serde_json::from_value(serde_json::json!({
        "type": "system",
        "subtype": "compact_boundary",
        "content": "Conversation compacted",
        "compactMetadata": {
            "trigger": "manual",
            "preTokens": 959_012,
            "durationMs": 108_397,
            "postTokens": post_tokens,
        }
    }))
    .unwrap()
}

#[test]
fn compact_boundary_resets_current_context_tokens_but_not_the_historical_max() {
    let model = "claude-sonnet-5";
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);

    process_jsonl_message(
        &mut acc,
        &assistant_usage_message_with_cache(model, 100_000, 5_000, 50_000, 800_000, None),
    );
    assert_eq!(acc.max_turn_api_input, 950_000);
    assert_eq!(acc.current_context_tokens, 950_000);

    process_jsonl_message(&mut acc, &compact_boundary_message(25_500));
    assert_eq!(
        acc.max_turn_api_input, 950_000,
        "the historical peak must survive a compaction -- it still answers whether this \
             session ever needed the 1M tier"
    );
    assert_eq!(
        acc.current_context_tokens, 25_500,
        "current fill must reset to the real post-compaction size Claude Code reported, \
             not stay frozen at the pre-compaction peak"
    );

    process_jsonl_message(
        &mut acc,
        &assistant_usage_message_with_cache(model, 6_000, 500, 0, 24_000, None),
    );
    assert_eq!(
        acc.current_context_tokens, 30_000,
        "current fill tracks the latest turn after a compaction, not a running max"
    );
    assert_eq!(
        acc.max_turn_api_input, 950_000,
        "a small post-compaction turn must not lower the historical peak"
    );
}

#[test]
fn compact_boundary_with_missing_post_tokens_does_not_reset_current_context_tokens() {
    let model = "claude-sonnet-5";
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);

    process_jsonl_message(
        &mut acc,
        &assistant_usage_message_with_cache(model, 100_000, 5_000, 0, 0, None),
    );
    assert_eq!(acc.current_context_tokens, 100_000);

    let malformed: JsonlMessage = serde_json::from_value(serde_json::json!({
        "type": "system",
        "subtype": "compact_boundary",
        "content": "Conversation compacted",
    }))
    .unwrap();
    process_jsonl_message(&mut acc, &malformed);

    assert_eq!(
        acc.current_context_tokens, 100_000,
        "a compact_boundary with no compactMetadata.postTokens must be a no-op, not a \
             silent reset to zero"
    );
}

#[test]
fn compact_boundary_as_the_first_message_initializes_cleanly() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    process_jsonl_message(&mut acc, &compact_boundary_message(13_269));
    assert_eq!(acc.current_context_tokens, 13_269);
    assert_eq!(acc.max_turn_api_input, 0);
}

#[test]
fn current_context_tokens_matches_latest_turn_when_no_compaction_ever_happened() {
    let model = "claude-sonnet-5";
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);

    process_jsonl_message(
        &mut acc,
        &assistant_usage_message_with_cache(model, 100_000, 5_000, 10_000, 20_000, None),
    );
    process_jsonl_message(
        &mut acc,
        &assistant_usage_message_with_cache(model, 50_000, 3_000, 5_000, 80_000, None),
    );

    assert_eq!(
        acc.current_context_tokens, 135_000,
        "with zero compactions, current fill is simply the latest turn's total"
    );
    assert_eq!(acc.max_turn_api_input, 135_000);
}

fn assistant_usage_message_with_cache(
    model: &str,
    input: u64,
    output: u64,
    cache_write: u64,
    cache_read: u64,
    speed: Option<&str>,
) -> JsonlMessage {
    let mut usage = serde_json::json!({
        "input_tokens": input,
        "output_tokens": output,
        "cache_creation_input_tokens": cache_write,
        "cache_read_input_tokens": cache_read,
    });
    if let Some(speed) = speed {
        usage["speed"] = serde_json::json!(speed);
    }
    serde_json::from_value(serde_json::json!({
        "type": "assistant",
        "message": {
            "model": model,
            "type": "message",
            "role": "assistant",
            "usage": usage,
            "content": [{"type": "text", "text": "ok"}]
        }
    }))
    .unwrap()
}

/// One API response as Claude Code writes it: one line per content block,
/// each carrying the same `message.id` and the response's usage.
fn response_block_line(id: &str, block: &str, output: u64) -> JsonlMessage {
    serde_json::from_value(serde_json::json!({
        "type": "assistant",
        "requestId": format!("req_{id}"),
        "message": {
            "id": id,
            "model": "claude-opus-5-5",
            "type": "message",
            "role": "assistant",
            "usage": {
                "input_tokens": 2,
                "output_tokens": output,
                "cache_creation_input_tokens": 12_491,
                "cache_read_input_tokens": 79_255
            },
            "content": [{"type": block}]
        }
    }))
    .unwrap()
}

#[test]
fn repeated_content_block_lines_count_one_response_once() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    for block in ["thinking", "text", "tool_use", "tool_use"] {
        process_jsonl_message(&mut acc, &response_block_line("msg_a", block, 467));
    }

    let once = cost::calculate_category_costs("claude-opus-5-5", 2, 467, 12_491, 79_255, false);
    assert_eq!(acc.total_output_tokens, 467);
    assert_eq!(acc.total_cache_creation_tokens, 12_491);
    assert_eq!(acc.total_cache_read_tokens, 79_255);
    assert_eq!(acc.total_input_tokens, 2 + 12_491 + 79_255);
    assert!((acc.total_cost - once.total()).abs() < 1e-9);
    assert!((acc.cache_read_cost - once.cache_read_cost).abs() < 1e-9);
    assert_eq!(acc.session_total_tokens, Some(2 + 12_491 + 79_255 + 467));
}

/// `output_tokens` can grow across a streamed response's lines; the
/// latest value replaces the earlier one, even when another response's
/// lines are interleaved.
#[test]
fn streamed_response_keeps_its_latest_usage_across_interleaved_lines() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    process_jsonl_message(&mut acc, &response_block_line("msg_a", "thinking", 6));
    process_jsonl_message(&mut acc, &response_block_line("msg_b", "text", 100));
    process_jsonl_message(&mut acc, &response_block_line("msg_a", "tool_use", 368));

    let a = cost::calculate_category_costs("claude-opus-5-5", 2, 368, 12_491, 79_255, false);
    let b = cost::calculate_category_costs("claude-opus-5-5", 2, 100, 12_491, 79_255, false);
    assert_eq!(acc.total_output_tokens, 468);
    assert_eq!(acc.total_cache_read_tokens, 2 * 79_255);
    assert!((acc.total_cost - (a.total() + b.total())).abs() < 1e-9);
    let sum = acc.input_cost + acc.output_cost + acc.cache_write_cost + acc.cache_read_cost;
    assert!((sum - acc.total_cost).abs() < 1e-9);
}

#[test]
fn lines_without_a_message_id_still_count_individually() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    let line = assistant_usage_message_with_cache("claude-opus-5-5", 10, 20, 30, 40, None);
    process_jsonl_message(&mut acc, &line);
    process_jsonl_message(&mut acc, &line);
    assert_eq!(acc.total_output_tokens, 40);
    assert!(acc.counted_usage.is_empty());
}

fn assistant_line_with_usage(model: Option<&str>, usage: serde_json::Value) -> JsonlMessage {
    let mut message = serde_json::json!({
        "type": "message",
        "role": "assistant",
        "usage": usage,
        "content": [{"type": "text", "text": "ok"}]
    });
    if let Some(model) = model {
        message["model"] = serde_json::json!(model);
    }
    serde_json::from_value(serde_json::json!({"type": "assistant", "message": message})).unwrap()
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

/// Claude Code's real shape (checked against local transcripts on
/// 2026-11-07): `cache_creation` carries both lifetimes next to the total.
fn usage_with_cache_ttl_split(
    input: u64,
    output: u64,
    write_5m: u64,
    write_1h: u64,
    cache_read: u64,
) -> serde_json::Value {
    serde_json::json!({
        "input_tokens": input,
        "output_tokens": output,
        "cache_creation_input_tokens": write_5m + write_1h,
        "cache_read_input_tokens": cache_read,
        "cache_creation": {
            "ephemeral_5m_input_tokens": write_5m,
            "ephemeral_1h_input_tokens": write_1h
        }
    })
}

#[test]
fn one_hour_cache_writes_are_billed_at_the_one_hour_rate() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    // Opus 5.5 writes: 100,000 x $8 = $0.80 for 1h (5m would be $0.50).
    let line = assistant_line_with_usage(
        Some("claude-opus-5-5"),
        usage_with_cache_ttl_split(0, 0, 0, 100_000, 0),
    );
    process_jsonl_message(&mut acc, &line);
    close(acc.cache_write_cost, 0.80);
    close(acc.total_cost, 0.80);
    assert_eq!(acc.total_cache_creation_tokens, 100_000);
}

#[test]
fn mixed_five_minute_and_one_hour_writes_are_billed_each_at_their_rate() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    // 40,000 x $5 + 60,000 x $8 = $0.68 on Opus 5.5.
    let line = assistant_line_with_usage(
        Some("claude-opus-5-5"),
        usage_with_cache_ttl_split(0, 0, 40_000, 60_000, 0),
    );
    process_jsonl_message(&mut acc, &line);
    close(acc.cache_write_cost, 0.68);
    assert_eq!(acc.total_cache_creation_tokens, 100_000);
}

#[test]
fn cache_writes_without_a_ttl_split_are_five_minute_writes() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    // 100,000 x $5 = $0.50 on Opus 5.5.
    let line = assistant_usage_message_with_cache("claude-opus-5-5", 0, 0, 100_000, 0, None);
    process_jsonl_message(&mut acc, &line);
    close(acc.cache_write_cost, 0.50);
}

#[test]
fn a_cache_creation_object_without_one_hour_tokens_stays_five_minute() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    let line = assistant_line_with_usage(
        Some("claude-opus-5-5"),
        serde_json::json!({
            "input_tokens": 0,
            "output_tokens": 0,
            "cache_creation_input_tokens": 100_000,
            "cache_read_input_tokens": 0,
            "cache_creation": {"ephemeral_5m_input_tokens": 100_000}
        }),
    );
    process_jsonl_message(&mut acc, &line);
    close(acc.cache_write_cost, 0.50);
}

#[test]
fn haiku_5_5_requests_are_priced_by_their_own_prompt_length() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    // Low tier: 50,000 x $0.10 + 1,000 x $0.50 = $0.0055.
    process_jsonl_message(
        &mut acc,
        &assistant_usage_message("claude-haiku-5-5", 50_000, 1_000, None, None),
    );
    close(acc.total_cost, 0.0055);
    // High tier (prompt 200,000): 20,000 x $0.50 + 30,000 x $0.625
    // + 150,000 x $0.05 + 2,000 x $2.50 = $0.04125.
    process_jsonl_message(
        &mut acc,
        &assistant_usage_message_with_cache(
            "claude-haiku-5-5",
            20_000,
            2_000,
            30_000,
            150_000,
            None,
        ),
    );
    close(acc.total_cost, 0.0055 + 0.04125);
    // Two 60,000-token requests stay in the low tier: 2 x 60,000 x $0.10 = $0.012.
    let mut two = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    for _ in 0..2 {
        process_jsonl_message(
            &mut two,
            &assistant_usage_message("claude-haiku-5-5", 60_000, 0, None, None),
        );
    }
    close(two.total_cost, 0.012);
    let sum = two.input_cost + two.output_cost + two.cache_write_cost + two.cache_read_cost;
    close(sum, two.total_cost);
}

#[test]
fn haiku_5_5_prompt_boundary_is_100_000_inclusive_for_the_low_tier() {
    let mut at = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    process_jsonl_message(
        &mut at,
        &assistant_usage_message_with_cache("claude-haiku-5-5", 40_000, 0, 10_000, 50_000, None),
    );
    // 40,000 x $0.10 + 10,000 x $0.125 + 50,000 x $0.01 = $0.00575
    close(at.total_cost, 0.005_75);
    let mut over = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    process_jsonl_message(
        &mut over,
        &assistant_usage_message_with_cache("claude-haiku-5-5", 40_001, 0, 10_000, 50_000, None),
    );
    // 40,001 x $0.50 + 10,000 x $0.625 + 50,000 x $0.05 = $0.0287505
    close(over.total_cost, 0.028_750_5);
}

#[test]
fn sonnet_5_5_cache_reads_are_billed_at_ten_cents_per_million() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    // 50,000 x $0.10 = $0.005
    process_jsonl_message(
        &mut acc,
        &assistant_usage_message_with_cache("claude-sonnet-5-5", 0, 0, 0, 50_000, None),
    );
    close(acc.cache_read_cost, 0.005);
}

#[test]
fn a_turn_without_a_model_is_unpriced_not_priced_as_sonnet_4() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    process_jsonl_message(
        &mut acc,
        &assistant_line_with_usage(
            None,
            usage_with_cache_ttl_split(1_000_000, 100_000, 0, 0, 0),
        ),
    );
    assert_eq!(acc.model, None);
    assert_eq!(acc.total_cost, 0.0, "no model, no price");
    assert_eq!(acc.input_cost, 0.0);
    assert_eq!(acc.output_cost, 0.0);
    // The tokens are still counted, so the session reads as unpriced
    // (tokens without cost), not as a free session.
    assert_eq!(acc.total_input_tokens, 1_000_000);
    assert_eq!(acc.total_output_tokens, 100_000);
}

#[test]
fn a_synthetic_model_turn_before_any_real_model_is_unpriced() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    process_jsonl_message(
        &mut acc,
        &assistant_line_with_usage(
            Some("<synthetic>"),
            usage_with_cache_ttl_split(1_000_000, 0, 0, 0, 0),
        ),
    );
    assert_eq!(acc.total_cost, 0.0);
    assert_eq!(acc.total_input_tokens, 1_000_000);
}

#[test]
fn turns_after_the_model_is_known_are_priced_while_the_earlier_one_stays_unpriced() {
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);
    process_jsonl_message(
        &mut acc,
        &assistant_line_with_usage(None, usage_with_cache_ttl_split(1_000_000, 0, 0, 0, 0)),
    );
    // Opus 5.5: 1,000,000 x $4 = $4.00 for the priced turn only.
    process_jsonl_message(
        &mut acc,
        &assistant_usage_message("claude-opus-5-5", 1_000_000, 0, None, None),
    );
    close(acc.total_cost, 4.0);
    assert_eq!(acc.total_input_tokens, 2_000_000);
}

#[test]
fn fast_opus_48_session_categories_reconcile_and_double_standard() {
    let model = "claude-opus-4-8";
    let (input, output, cache_write, cache_read) = (100_000u64, 20_000u64, 8_000u64, 60_000u64);
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);

    process_jsonl_message(
        &mut acc,
        &assistant_usage_message_with_cache(
            model,
            input,
            output,
            cache_write,
            cache_read,
            Some("fast"),
        ),
    );

    let sum = acc.input_cost + acc.output_cost + acc.cache_write_cost + acc.cache_read_cost;
    assert!((sum - acc.total_cost).abs() < 1e-7);

    let standard =
        cost::calculate_category_costs(model, input, output, cache_write, cache_read, false);
    assert!((acc.input_cost - standard.input_cost * 2.0).abs() < 1e-7);
    assert!((acc.output_cost - standard.output_cost * 2.0).abs() < 1e-7);
    assert!((acc.cache_write_cost - standard.cache_write_cost * 2.0).abs() < 1e-7);
    assert!((acc.cache_read_cost - standard.cache_read_cost * 2.0).abs() < 1e-7);
}

#[test]
fn mixed_speed_session_categories_sum_to_total_per_turn() {
    let model = "claude-opus-4-8";
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);

    process_jsonl_message(
        &mut acc,
        &assistant_usage_message_with_cache(model, 100_000, 20_000, 8_000, 60_000, Some("fast")),
    );
    process_jsonl_message(
        &mut acc,
        &assistant_usage_message_with_cache(model, 50_000, 10_000, 4_000, 30_000, Some("standard")),
    );

    let sum = acc.input_cost + acc.output_cost + acc.cache_write_cost + acc.cache_read_cost;
    assert!((sum - acc.total_cost).abs() < 1e-7);

    let fast_turn = cost::calculate_category_costs(model, 100_000, 20_000, 8_000, 60_000, true);
    let std_turn = cost::calculate_category_costs(model, 50_000, 10_000, 4_000, 30_000, false);
    let expected_total = fast_turn.total() + std_turn.total();
    assert!((acc.total_cost - expected_total).abs() < 1e-7);
    assert!((acc.input_cost - (fast_turn.input_cost + std_turn.input_cost)).abs() < 1e-7);
    assert!((acc.output_cost - (fast_turn.output_cost + std_turn.output_cost)).abs() < 1e-7);
    assert!(
        (acc.cache_write_cost - (fast_turn.cache_write_cost + std_turn.cache_write_cost)).abs()
            < 1e-7
    );
    assert!(
        (acc.cache_read_cost - (fast_turn.cache_read_cost + std_turn.cache_read_cost)).abs() < 1e-7
    );
}

#[test]
fn category_scale_apportions_authoritative_total_and_reconciles() {
    let jsonl_total = 10.0_f64;
    let authoritative_total = 12.0_f64;
    let scale = category_scale(authoritative_total, jsonl_total);
    assert!((scale - 1.2).abs() < 1e-12);
    let categories = [3.0_f64, 5.0, 1.0, 1.0];
    assert!((categories.iter().sum::<f64>() - jsonl_total).abs() < 1e-12);
    let scaled_sum: f64 = categories.iter().map(|c| c * scale).sum();
    assert!((scaled_sum - authoritative_total).abs() < 1e-7);
}

#[test]
fn category_scale_is_zero_without_a_jsonl_cost_basis() {
    assert_eq!(category_scale(9.99, 0.0), 0.0);
}

#[test]
fn most_recent_turn_speed_and_service_tier_win_for_display() {
    let model = "claude-opus-4-8";
    let mut acc = SessionAccumulator::with_default_effort(ReasoningEffort::Medium);

    process_jsonl_message(
        &mut acc,
        &assistant_usage_message(model, 10_000, 1_000, Some("standard"), Some("standard")),
    );
    process_jsonl_message(
        &mut acc,
        &assistant_usage_message(model, 10_000, 1_000, Some("fast"), Some("priority")),
    );

    assert_eq!(acc.speed, Speed::Fast);
    assert_eq!(acc.service_tier.as_deref(), Some("priority"));
}

#[test]
fn session_id_validation_accepts_uuid_shape() {
    assert!(is_valid_session_id("4ccf0482-61c0-4611-9d22-becaf1781231"));
    assert!(!is_valid_session_id("encoded_c--foo"));
    assert!(is_synthetic_model_id("<synthetic>"));
}

#[test]
fn derive_name_normal_path() {
    assert_eq!(
        derive_project_name(Path::new("/home/user/my-project")),
        "my-project"
    );
}

#[cfg(windows)]
#[test]
fn derive_name_normal_path_windows() {
    assert_eq!(
        derive_project_name(Path::new("D:\\X\\Work\\Property Alpha")),
        "Property Alpha"
    );
}

#[cfg(windows)]
#[test]
fn derive_name_drive_root() {
    assert_eq!(derive_project_name(Path::new("C:\\")), "C: Drive");
    assert_eq!(derive_project_name(Path::new("D:\\")), "D: Drive");
    assert_eq!(derive_project_name(Path::new("C:/")), "C: Drive");
}

#[test]
fn derive_name_from_package_json() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"name": "my-cool-app", "version": "1.0.0"}"#,
    )
    .unwrap();
    assert_eq!(
        read_project_name_from_metadata(dir.path()),
        Some("my-cool-app".to_string())
    );
}

#[test]
fn derive_name_from_cargo_toml() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"discord-presence\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    assert_eq!(
        read_project_name_from_metadata(dir.path()),
        Some("discord-presence".to_string())
    );
}

#[test]
fn derive_name_git_repo() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    let name = find_git_repo_name(dir.path(), 3);
    assert!(name.is_some());
}
