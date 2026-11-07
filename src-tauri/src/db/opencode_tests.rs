//! Tests for how OpenCode-reported costs are stored and read back.

use super::*;
#[test]
fn dashboard_summary_uses_seven_days_and_exact_provider_scope() {
    let connection = Connection::open_in_memory().unwrap();
    init_schema(&connection).unwrap();
    let now = Utc::now();
    for (id, provider, age, tokens) in [
        ("recent", "opencode", 1, 100),
        ("old", "opencode", 8, 900),
        ("other", "codex", 1, 500),
    ] {
        let timestamp = (now - chrono::Duration::days(age)).to_rfc3339();
        let session = crate::commands::SessionInfo {
            provider: provider.into(),
            session_id: id.into(),
            started_at: Some(timestamp.clone()),
            tokens,
            input_tokens: tokens,
            model: id.into(),
            project: id.into(),
            is_idle: true,
            ..Default::default()
        };
        upsert_session_into(&connection, &session, &timestamp).unwrap();
    }
    let scoped = analytics_summary_for_window(&connection, "opencode", 7);
    assert_eq!(scoped.total_sessions, 1);
    assert_eq!(scoped.total_tokens, 100);
    assert_eq!(scoped.top_model, "recent");
    let all = analytics_summary_for_window(&connection, "all", 7);
    assert_eq!(all.total_sessions, 2);
    assert_eq!(all.total_tokens, 600);
}

#[test]
fn native_metadata_survives_history_and_unknown_cost_stays_unknown() {
    let connection = Connection::open_in_memory().unwrap();
    init_schema(&connection).unwrap();
    let metadata = cc_discord_presence::opencode::Metadata {
        model_provider: "custom".into(),
        model_id: "gpt-6-astra".into(),
        variant: Some("max".into()),
        ..Default::default()
    };
    let session = crate::commands::SessionInfo {
        provider: "opencode".into(),
        session_id: "native".into(),
        model_id: "gpt-6-astra".into(),
        opencode: Some(metadata.clone()),
        input_tokens: 1000,
        is_idle: true,
        ..Default::default()
    };
    upsert_session_into(&connection, &session, "2026-09-04T00:00:00Z").unwrap();
    let history = query_sessions(
        &connection,
        "opencode",
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(10),
    );
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].opencode.as_ref(), Some(&metadata));
    assert!(history[0].known_cost.is_none());
    assert_eq!(history[0].window_tokens, 0);
    let free = crate::commands::SessionInfo {
        cost_available: true,
        cost_basis: "opencode_reported".into(),
        ..session
    };
    upsert_session_into(&connection, &free, "2026-09-04T00:00:00Z").unwrap();
    let (cost, source): (Option<f64>, String) = connection
        .query_row(
            "SELECT known_cost,cost_source FROM sessions WHERE provider='opencode'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(cost, Some(0.0));
    assert_eq!(source, "opencode_reported");
    assert_eq!(schema_version(&connection).unwrap(), SCHEMA_VERSION);
}

#[test]
fn rereading_an_unchanged_external_session_writes_nothing() {
    let connection = Connection::open_in_memory().unwrap();
    init_schema(&connection).unwrap();
    let session = crate::commands::SessionInfo {
        provider: "orion".into(),
        session_id: "history".into(),
        input_tokens: 10,
        is_idle: true,
        ..Default::default()
    };
    let stamp = "2026-11-01T00:00:00+00:00";
    assert!(upsert_external_session_into(&connection, &session, stamp).unwrap());
    let before = connection.total_changes();

    assert!(!upsert_external_session_into(&connection, &session, stamp).unwrap());
    assert_eq!(
        connection.total_changes(),
        before,
        "the same source timestamp is a no-op"
    );

    assert!(
        upsert_external_session_into(&connection, &session, "2026-11-02T00:00:00+00:00").unwrap(),
        "a newer source timestamp is written"
    );
}
