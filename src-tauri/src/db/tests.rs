//! Unit tests for the analytics database: schema migrations, cost estimation and history queries.

use super::*;
use chrono::TimeZone;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn monetary_provenance_recognizes_only_owned_billing_and_api_equivalent_sources() {
    for source in ["provider_billed", "provider-billed"] {
        assert_eq!(
            monetary_provenance(source),
            MonetaryProvenance::ProviderBilled
        );
    }
    for source in [
        "api_equivalent",
        "anthropic_api_equivalent",
        "session-calculated",
        "legacy-calculated",
        "live_session",
        "versioned-pricing",
    ] {
        assert_eq!(
            monetary_provenance(source),
            MonetaryProvenance::ApiEquivalent
        );
    }
    for source in ["unknown", "legacy", "fixture", "external-ledger"] {
        assert_eq!(monetary_provenance(source), MonetaryProvenance::Other);
    }
}

#[test]
fn api_equivalent_estimate_reconstructs_claude_and_codex_from_real_rates() {
    // Claude Opus 5 ($5 in / $25 out per 1M): 1M pure input + 1M output.
    let claude =
        estimate_api_equivalent_cost("claude", "claude-opus-5", 1_000_000, 1_000_000, 0, 0)
            .expect("claude opus 5 has published API rates");
    assert!((claude.total - 30.0).abs() < 0.01, "got {}", claude.total);

    // Claude Opus 5.5 ($4 in / $20 out per 1M) must not inherit Opus 5 rates.
    let opus_5_5 =
        estimate_api_equivalent_cost("claude", "claude-opus-5-5", 1_000_000, 1_000_000, 0, 0)
            .expect("claude opus 5.5 has published API rates");
    assert!(
        (opus_5_5.total - 24.0).abs() < 0.01,
        "got {}",
        opus_5_5.total
    );

    // Codex GPT-5.6 Sol ($4 in / $20 out per 1M, promotional until 2026-11-30), below the
    // 272,000-token long-context threshold: 0.1 x $4 + 0.1 x $20 = $2.40.
    let codex = estimate_api_equivalent_cost("codex", "gpt-5.6-sol", 100_000, 100_000, 0, 0)
        .expect("gpt-5.6-sol has catalog API pricing");
    assert!((codex.total - 2.4).abs() < 0.01, "got {}", codex.total);
}

#[test]
fn api_equivalent_estimate_prices_gpt_6_family_and_withholds_long_context_totals() {
    // GPT-6.1 Sol: 100K fresh input ($0.20) + 10K output ($0.10).
    let standard = estimate_api_equivalent_cost("codex", "gpt-6.1-sol", 100_000, 10_000, 0, 0)
        .expect("gpt-6.1-sol has catalog API pricing");
    assert!(
        (standard.total - 0.30).abs() < 1e-9,
        "got {}",
        standard.total
    );
    let fast = estimate_api_equivalent_cost_with_speed(
        "codex",
        "gpt-6.1-sol",
        100_000,
        10_000,
        0,
        0,
        "fast",
    )
    .expect("fast has a published multiplier");
    assert!((fast.total - 0.60).abs() < 1e-9, "got {}", fast.total);

    let sol = estimate_api_equivalent_cost("codex", "gpt-6-sol", 100_000, 10_000, 0, 0)
        .expect("gpt-6-sol has catalog API pricing");
    assert!((sol.total - 0.30).abs() < 1e-9, "got {}", sol.total);
    let luna = estimate_api_equivalent_cost("codex", "gpt-6-luna", 100_000, 10_000, 0, 0)
        .expect("gpt-6-luna has catalog API pricing");
    assert!((luna.total - 0.015).abs() < 1e-9, "got {}", luna.total);
    // GPT-6.1 Terra ($1 in / $5 out, 2026-11-07): 100K x $1 + 10K x $5 = $0.15.
    let terra = estimate_api_equivalent_cost("codex", "gpt-6.1-terra", 100_000, 10_000, 0, 0)
        .expect("gpt-6.1-terra has catalog API pricing");
    assert!((terra.total - 0.15).abs() < 1e-9, "got {}", terra.total);
    let terra_fast = estimate_api_equivalent_cost_with_speed(
        "codex",
        "gpt-6.1-terra",
        100_000,
        10_000,
        0,
        0,
        "fast",
    )
    .expect("fast has a published multiplier");
    assert!(
        (terra_fast.total - 0.30).abs() < 1e-9,
        "got {}",
        terra_fast.total
    );

    // Session totals above 272K input cannot tell which requests crossed
    // the long-context threshold, so no estimate is stored.
    for model in [
        "gpt-6.1-sol",
        "gpt-6.1-terra",
        "gpt-6-sol",
        "gpt-6-luna",
        "gpt-6.1-sol-fast",
    ] {
        assert!(
            estimate_api_equivalent_cost("codex", model, 300_000, 1_000, 0, 0).is_none(),
            "{model}"
        );
    }
    assert!(estimate_api_equivalent_cost("codex", "gpt-6.2-sol", 100_000, 10_000, 0, 0).is_none());
}

#[test]
fn api_equivalent_estimate_never_invents_without_tokens_or_model() {
    assert!(estimate_api_equivalent_cost("codex", "gpt-5.6-sol", 0, 0, 0, 0).is_none());
    assert!(estimate_api_equivalent_cost("claude", "", 1_000_000, 1_000_000, 0, 0).is_none());
}

#[test]
fn api_equivalent_estimate_prices_haiku_5_5_only_when_the_tier_is_certain() {
    // Totals of 60,000 input (10,000 write + 20,000 read + 30,000 pure) and
    // 1,000 output, so every request was in the low tier:
    // 30,000 x $0.10 + 10,000 x $0.125 + 20,000 x $0.01 + 1,000 x $0.50 = $0.00495
    let low =
        estimate_api_equivalent_cost("claude", "claude-haiku-5-5", 60_000, 1_000, 10_000, 20_000)
            .expect("low tier is certain below the threshold");
    assert!((low.total - 0.004_95).abs() < 1e-9, "got {}", low.total);

    // Exactly 100,000 is still the low row: 100,000 x $0.10 = $0.01.
    let at = estimate_api_equivalent_cost("claude", "claude-haiku-5-5", 100_000, 0, 0, 0)
        .expect("100,000 is the low tier");
    assert!((at.total - 0.01).abs() < 1e-9, "got {}", at.total);

    // Above it, totals cannot say which requests crossed 100,000 tokens.
    for model in ["claude-haiku-5-5", "claude-haiku-5-5[1m]"] {
        assert!(
            estimate_api_equivalent_cost("claude", model, 100_001, 0, 0, 0).is_none(),
            "{model}"
        );
    }

    // Haiku 4.5 is flat-rate, so large totals are still priced: 1M x $1 = $1.
    let flat = estimate_api_equivalent_cost("claude", "claude-haiku-4-5", 1_000_000, 0, 0, 0)
        .expect("haiku 4.5 has flat rates");
    assert!((flat.total - 1.0).abs() < 1e-9, "got {}", flat.total);
}

#[test]
fn api_equivalent_estimate_uses_sonnet_5_5_cache_read_rate() {
    // 50,000 cache-read tokens x $0.10 = $0.005 (Sonnet 5: $0.20 -> $0.01).
    let sonnet_5_5 =
        estimate_api_equivalent_cost("claude", "claude-sonnet-5-5", 50_000, 0, 0, 50_000)
            .expect("sonnet 5.5 has published API rates");
    assert!((sonnet_5_5.cache_read_cost - 0.005).abs() < 1e-9);
    let sonnet_5 = estimate_api_equivalent_cost("claude", "claude-sonnet-5", 50_000, 0, 0, 50_000)
        .expect("sonnet 5 has published API rates");
    assert!((sonnet_5.cache_read_cost - 0.01).abs() < 1e-9);
}

#[test]
fn api_equivalent_estimate_uses_persisted_fast_speed() {
    let standard = estimate_api_equivalent_cost_with_speed(
        "codex", "gpt-5.4", 100_000, 100_000, 0, 0, "standard",
    )
    .expect("standard cost");
    let fast =
        estimate_api_equivalent_cost_with_speed("codex", "gpt-5.4", 100_000, 100_000, 0, 0, "fast")
            .expect("fast cost");

    assert!((fast.total - standard.total * 2.0).abs() < 0.001);
}

/// An Orion session priced by the core ledger, as the poller would persist it.
fn orion_info(id: &str, models: &[&str]) -> super::super::commands::SessionInfo {
    use cc_discord_presence::orion::{CostLedger, Session, Usage};
    let usage = Usage {
        input: 3_000_000,
        output: 1_000_000,
        cache_read: 1_000_000,
        cache_write: 1_000_000,
        ..Default::default()
    };
    let mut ledger = CostLedger::default();
    let mut session = Session {
        id: id.into(),
        project: "pulse".into(),
        model: models[0].into(),
        model_name: "Claude Opus 5.5".into(),
        created: Utc::now().timestamp_millis() - 3_600_000,
        updated: Utc::now().timestamp_millis(),
        ..Default::default()
    };
    for model in models {
        ledger.add(model, &usage);
        session.usage.add(&usage);
    }
    session.apply_cost(ledger.finish());
    crate::orion::session_info(&session)
}

fn orion_rows(conn: &Connection) -> Vec<HistoricalSession> {
    query_sessions(
        conn, "orion", None, None, None, None, None, None, None, None, None, None,
    )
}

#[test]
fn orion_history_carries_category_costs_and_reads_back_as_a_calculated_estimate() {
    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    init_schema(&conn).expect("schema");
    let info = orion_info("priced", &["anthropic/claude-opus-5-5"]);
    upsert_session_into(&conn, &info, &Utc::now().to_rfc3339()).expect("persist");

    let rows = orion_rows(&conn);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.provider, "orion");
    assert_eq!(row.cost_basis, CostBasis::Estimated);
    assert_eq!(row.cost_source, "api_equivalent");
    assert_eq!(
        monetary_provenance(&row.cost_source),
        MonetaryProvenance::ApiEquivalent
    );
    // Opus 5.5: $4 input, $20 output, $5 cache write, $0.20 cache read per 1M.
    for (actual, expected) in [
        (row.input_cost, 4.0),
        (row.output_cost, 20.0),
        (row.cache_write_cost, 5.0),
        (row.cache_read_cost, 0.20),
        (row.known_cost.unwrap(), 29.20),
    ] {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }
    assert_eq!(row.total_tokens, 4_000_000);

    // The Costs view aggregate reads the same columns.
    let totals = super::super::commands::aggregate_cost_totals(30, &rows);
    assert_eq!(totals.cost_basis, CostBasis::Estimated);
    assert_eq!(totals.priced_sessions, 1);
    assert!((totals.input_cost - 4.0).abs() < 1e-9);
    assert!((totals.cache_read_cost - 0.20).abs() < 1e-9);
    assert!((totals.api_equivalent_usd.unwrap() - 29.20).abs() < 1e-9);
    assert_eq!(totals.billed_spend_usd, None, "never presented as billed");

    // The month-to-date forecast prices the same row once.
    let forecast = cost_forecast_from_connection(&conn, "orion", Utc::now());
    assert!((forecast.api_equivalent_usd.unwrap() - 29.20).abs() < 1e-9);
    assert_eq!(forecast.api_equivalent_sessions, 1);
    assert_eq!(forecast.billed_spend_usd, None);
}

#[test]
fn orion_reingestion_replaces_a_legacy_row_without_double_counting() {
    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    init_schema(&conn).expect("schema");
    // Rows written before per-category costs carried a total and no split.
    let mut legacy = orion_info("upgrade", &["anthropic/claude-opus-5-5"]);
    legacy.input_cost = 0.0;
    legacy.output_cost = 0.0;
    legacy.cache_write_cost = 0.0;
    legacy.cache_read_cost = 0.0;
    legacy.tokens_per_sec = 0.0;
    upsert_session_into(&conn, &legacy, &Utc::now().to_rfc3339()).expect("legacy row");
    assert_eq!(orion_rows(&conn)[0].output_cost, 0.0);

    // The collector re-reads history on every start and upserts by id.
    let current = orion_info("upgrade", &["anthropic/claude-opus-5-5"]);
    upsert_session_into(&conn, &current, &Utc::now().to_rfc3339()).expect("re-ingest");
    let rows = orion_rows(&conn);
    assert_eq!(rows.len(), 1, "same session, same row");
    assert!((rows[0].output_cost - 20.0).abs() < 1e-9);
    assert!((rows[0].known_cost.unwrap() - 29.20).abs() < 1e-9);
    let totals = super::super::commands::aggregate_cost_totals(30, &rows);
    assert!((totals.total_cost - 29.20).abs() < 1e-9);
}

#[test]
fn orion_partial_and_unavailable_costs_are_never_filled_in_on_read() {
    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    init_schema(&conn).expect("schema");
    let now = Utc::now().to_rfc3339();
    // One priced and one unpriced model: a lower bound with its categories.
    let partial = orion_info(
        "partial",
        &["anthropic/claude-opus-5-5", "opencode-go/unknown-model-x"],
    );
    assert_eq!(partial.cost_basis, "partial");
    upsert_session_into(&conn, &partial, &now).expect("partial row");
    // No priced model: unavailable, with real token counts.
    let unavailable = orion_info("unavailable", &["opencode-go/unknown-model-x"]);
    assert_eq!(unavailable.cost_basis, "unavailable");
    upsert_session_into(&conn, &unavailable, &now).expect("unavailable row");

    let rows = orion_rows(&conn);
    let by_id = |id: &str| {
        rows.iter()
            .find(|row| row.id == format!("orion:{id}"))
            .unwrap_or_else(|| panic!("row {id}"))
    };
    let partial = by_id("partial");
    assert_eq!(partial.cost_basis, CostBasis::Partial);
    assert!((partial.known_cost.unwrap() - 29.20).abs() < 1e-9);
    assert!((partial.input_cost - 4.0).abs() < 1e-9);

    // The read-time estimator must not send an unknown model id through the
    // default Claude tier and invent a rate.
    let unavailable = by_id("unavailable");
    assert_eq!(unavailable.cost_basis, CostBasis::Unavailable);
    assert_eq!(unavailable.known_cost, None);
    assert_eq!(unavailable.total_tokens, 4_000_000);
    assert_eq!(unavailable.output_cost, 0.0);

    let totals = super::super::commands::aggregate_cost_totals(30, &rows);
    assert_eq!(totals.cost_basis, CostBasis::Partial);
    assert_eq!(totals.priced_sessions, 1);
    assert_eq!(totals.sessions, 2);
    assert!((totals.total_cost - 29.20).abs() < 1e-9);
    let forecast = cost_forecast_from_connection(&conn, "orion", Utc::now());
    assert!((forecast.api_equivalent_usd.unwrap() - 29.20).abs() < 1e-9);
}

#[test]
fn orion_rows_are_not_re_estimated_from_totals() {
    assert!(
        estimate_api_equivalent_cost_with_speed(
            "orion",
            "opencode-go/unknown-model-x",
            1_000_000,
            1_000_000,
            0,
            0,
            "unknown",
        )
        .is_none()
    );
    assert!(
        estimate_api_equivalent_cost_with_speed(
            "orion",
            "anthropic/claude-opus-5-5",
            1_000_000,
            1_000_000,
            0,
            0,
            "unknown",
        )
        .is_none(),
        "Orion prices each request itself; totals are not repriced"
    );
}

fn temporary_database_path(test_name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "pulse-{test_name}-{}-{nonce}.db",
        std::process::id()
    ))
}

fn remove_database_files(path: &Path) {
    for candidate in [
        path.to_path_buf(),
        migration_backup_path(path),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ] {
        if candidate.exists() {
            std::fs::remove_file(candidate).expect("remove test database file");
        }
    }
}

fn create_v3_schema(conn: &Connection) {
    conn.execute_batch(
        "
            PRAGMA user_version = 3;

            CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                provider TEXT NOT NULL DEFAULT 'claude',
                session_name TEXT DEFAULT NULL,
                project TEXT NOT NULL,
                model TEXT NOT NULL,
                model_id TEXT DEFAULT '',
                context_window TEXT DEFAULT '200K',
                branch TEXT,
                effort TEXT DEFAULT 'Medium',
                started_at TEXT,
                created_at TEXT,
                ended_at TEXT,
                duration_secs INTEGER DEFAULT 0,
                total_cost REAL DEFAULT 0,
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                cache_write_tokens INTEGER DEFAULT 0,
                cache_read_tokens INTEGER DEFAULT 0,
                total_tokens INTEGER DEFAULT 0,
                input_cost REAL DEFAULT 0,
                output_cost REAL DEFAULT 0,
                cache_write_cost REAL DEFAULT 0,
                cache_read_cost REAL DEFAULT 0,
                has_thinking INTEGER DEFAULT 0,
                subagent_count INTEGER DEFAULT 0,
                is_active INTEGER DEFAULT 1,
                updated_at TEXT NOT NULL,
                used_tokens INTEGER DEFAULT 0,
                window_tokens INTEGER DEFAULT 0
            );

            CREATE TABLE daily_stats (
                date TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'claude',
                project TEXT NOT NULL,
                model TEXT NOT NULL,
                session_count INTEGER DEFAULT 0,
                total_cost REAL DEFAULT 0,
                total_tokens INTEGER DEFAULT 0,
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                cache_write_tokens INTEGER DEFAULT 0,
                cache_read_tokens INTEGER DEFAULT 0,
                PRIMARY KEY (date, provider, project, model)
            );

            CREATE VIRTUAL TABLE sessions_fts USING fts5(
                project, model, branch,
                content='sessions',
                content_rowid='rowid',
                tokenize='porter unicode61'
            );

            CREATE TRIGGER sessions_ai AFTER INSERT ON sessions BEGIN
                INSERT INTO sessions_fts(rowid, project, model, branch)
                VALUES (new.rowid, new.project, new.model, COALESCE(new.branch, ''));
            END;

            CREATE TRIGGER sessions_au AFTER UPDATE ON sessions BEGIN
                DELETE FROM sessions_fts WHERE rowid = old.rowid;
                INSERT INTO sessions_fts(rowid, project, model, branch)
                VALUES (new.rowid, new.project, new.model, COALESCE(new.branch, ''));
            END;

            CREATE TABLE budget_config (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                monthly_budget REAL DEFAULT 0,
                alert_threshold_pct REAL DEFAULT 80,
                updated_at TEXT NOT NULL DEFAULT '1970-01-01'
            );
            ",
    )
    .expect("create v3 schema");
}

fn test_conn() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    init_schema(&conn).expect("initialize schema");
    conn
}

#[test]
fn raw_jsonl_reconciles_with_sqlite_costs_dto_and_export() {
    use cc_discord_presence::session::{
        GitBranchCache, SessionParseCache, collect_active_sessions_multi,
    };
    use std::time::Duration;

    let temp = tempfile::tempdir().expect("tempdir");
    let project_root = temp.path().join("projects");
    let transcript_dir = project_root.join("encoded-repo");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&transcript_dir).expect("transcript dir");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    let transcript = transcript_dir.join("4ccf0482-61c0-4611-9d22-becaf1781231.jsonl");
    let observed_at = Utc::now().to_rfc3339();
    let model = "claude-sonnet-4-20250514";
    let lines = [
        (100_u64, 20_u64, 30_u64, 50_u64),
        (40_u64, 10_u64, 5_u64, 15_u64),
    ];
    let raw = lines
        .iter()
        .map(|(input, output, cache_write, cache_read)| {
            serde_json::json!({
                "type": "assistant",
                "timestamp": observed_at,
                "sessionId": "4ccf0482-61c0-4611-9d22-becaf1781231",
                "cwd": workspace,
                "message": {
                    "model": model,
                    "usage": {
                        "input_tokens": input,
                        "output_tokens": output,
                        "cache_creation_input_tokens": cache_write,
                        "cache_read_input_tokens": cache_read
                    },
                    "content": [{"type": "text", "text": "fixture"}]
                }
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&transcript, raw).expect("raw JSONL fixture");

    let expected_input: u64 = lines
        .iter()
        .map(|(input, _, cache_write, cache_read)| input + cache_write + cache_read)
        .sum();
    let expected_output: u64 = lines.iter().map(|(_, output, _, _)| output).sum();
    let expected_cache_write: u64 = lines.iter().map(|(_, _, write, _)| write).sum();
    let expected_cache_read: u64 = lines.iter().map(|(_, _, _, read)| read).sum();
    let expected_total = expected_input + expected_output;
    let expected_cost: f64 = lines
        .iter()
        .map(|(input, output, cache_write, cache_read)| {
            cost::calculate_category_costs(model, *input, *output, *cache_write, *cache_read, false)
                .total()
        })
        .sum();

    let snapshots = collect_active_sessions_multi(
        &[project_root.clone(), project_root],
        Duration::from_secs(3_600),
        Duration::from_secs(3_600),
        &mut GitBranchCache::new(Duration::ZERO),
        &mut SessionParseCache::default(),
        &[],
    )
    .expect("parse raw JSONL");
    assert_eq!(
        snapshots.len(),
        1,
        "duplicate source roots must deduplicate"
    );
    let infos = crate::commands::build_claude_session_infos(&snapshots);
    assert_eq!(infos.len(), 1);
    let info = &infos[0];
    assert_eq!(info.input_tokens, expected_input);
    assert_eq!(info.output_tokens, expected_output);
    assert_eq!(info.cache_write_tokens, expected_cache_write);
    assert_eq!(info.cache_read_tokens, expected_cache_read);
    assert_eq!(info.tokens, expected_total);
    assert!((info.cost - expected_cost).abs() < 1e-12);

    let conn = test_conn();
    upsert_session_into(&conn, info, &observed_at).expect("persist parsed DTO");
    upsert_session_into(&conn, info, &observed_at).expect("idempotent upsert");
    let costs = costs_data_from_connection(&conn, "claude", 30, None, 200, Utc::now());
    assert_eq!(costs.aggregate_sessions.len(), 1);
    let stored = &costs.aggregate_sessions[0];
    assert_eq!(stored.input_tokens, expected_input as i64);
    assert_eq!(stored.output_tokens, expected_output as i64);
    assert_eq!(stored.cache_write_tokens, expected_cache_write as i64);
    assert_eq!(stored.cache_read_tokens, expected_cache_read as i64);
    assert_eq!(stored.total_tokens, expected_total as i64);
    assert_eq!(stored.cost_basis, CostBasis::Estimated);
    assert_eq!(stored.cost_source, "api_equivalent");
    assert!((stored.known_cost.expect("known API equivalent") - expected_cost).abs() < 1e-12);

    let totals = crate::commands::aggregate_cost_totals(30, &costs.aggregate_sessions);
    assert_eq!(totals.sessions, 1);
    assert_eq!(totals.priced_sessions, 1);
    assert_eq!(totals.total_tokens, expected_total as i64);
    assert!((totals.total_cost - expected_cost).abs() < 1e-12);

    let export = export_all_data_from_connection(&conn, "claude", Utc::now());
    assert_eq!(export["summary"]["total_sessions"], 1);
    assert_eq!(export["summary"]["total_tokens"], expected_total);
    assert_eq!(export["sessions"].as_array().map(Vec::len), Some(1));
    assert_eq!(export["sessions"][0]["input_tokens"], expected_input);
    assert_eq!(export["sessions"][0]["output_tokens"], expected_output);
    assert_eq!(
        export["sessions"][0]["cache_write_tokens"],
        expected_cache_write
    );
    assert_eq!(
        export["sessions"][0]["cache_read_tokens"],
        expected_cache_read
    );
    assert_eq!(export["sessions"][0]["cost_basis"], "estimated");
}

#[test]
fn provider_history_inventory_counts_local_sessions_independently_from_cost() {
    let conn = test_conn();
    let timestamp = Utc::now().to_rfc3339();
    for (id, provider) in [
        ("codex:inventory", "codex"),
        ("claude:inventory-1", "claude"),
        ("claude:inventory-2", "claude"),
    ] {
        conn.execute(
            "INSERT INTO sessions (
                    id, provider, project, model, started_at, created_at, updated_at,
                    cost_status, cost_source, known_cost, total_tokens
                 ) VALUES (?1, ?2, 'inventory-repo', 'inventory-model', ?3, ?3, ?3,
                    'unavailable', 'unknown', NULL, 100)",
            params![id, provider, timestamp],
        )
        .expect("insert inventory session");
    }

    let inventory = provider_history_inventory(&conn, None);
    assert_eq!(inventory.get("codex"), Some(&1));
    assert_eq!(inventory.get("claude"), Some(&2));
    assert_eq!(inventory.get("openai"), None);
}

#[test]
fn provider_scope_never_leaks_cross_provider_sessions_or_cost() {
    let conn = test_conn();
    let started_at = Utc::now().to_rfc3339();
    for (id, provider, cost) in [
        ("codex:scope", "codex", 2.0),
        ("claude:scope", "claude", 7.0),
    ] {
        conn.execute(
            "INSERT INTO sessions (
                    id, provider, project, model, started_at, created_at, updated_at,
                    total_cost, cost_status, cost_source, known_cost, total_tokens
                 ) VALUES (?1, ?2, 'scope-repo', 'scope-model', ?3, ?3, ?3,
                    ?4, 'exact', 'session-calculated', ?4, 100)",
            params![id, provider, started_at, cost],
        )
        .expect("insert scoped fixture");
    }

    let codex = query_sessions(
        &conn,
        "codex",
        Some(7),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    let claude = query_sessions(
        &conn,
        "claude",
        Some(7),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    let all = query_sessions(
        &conn,
        "all",
        Some(7),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );

    assert_eq!(
        codex
            .iter()
            .map(|session| session.provider.as_str())
            .collect::<Vec<_>>(),
        vec!["codex"]
    );
    assert_eq!(
        claude
            .iter()
            .map(|session| session.provider.as_str())
            .collect::<Vec<_>>(),
        vec!["claude"]
    );
    assert_eq!(all.len(), 2);

    let codex_summary = analytics_summary_from_connection(&conn, "codex");
    let claude_summary = analytics_summary_from_connection(&conn, "claude");
    let all_summary = analytics_summary_from_connection(&conn, "all");
    assert_eq!(codex_summary.total_sessions, 1);
    assert_eq!(codex_summary.total_cost, 2.0);
    assert_eq!(claude_summary.total_sessions, 1);
    assert_eq!(claude_summary.total_cost, 7.0);
    assert_eq!(all_summary.total_sessions, 2);
    assert_eq!(all_summary.total_cost, 9.0);

    let cutoff_date = (Utc::now() - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let cutoff_time = (Utc::now() - chrono::Duration::days(1)).to_rfc3339();
    let codex_daily = query_daily_stats(&conn, "codex", &cutoff_date);
    let claude_daily = query_daily_stats(&conn, "claude", &cutoff_date);
    let all_daily = query_daily_stats(&conn, "all", &cutoff_date);
    assert_eq!(
        codex_daily.iter().map(|row| row.session_count).sum::<i64>(),
        1
    );
    assert_eq!(
        claude_daily
            .iter()
            .map(|row| row.session_count)
            .sum::<i64>(),
        1
    );
    assert_eq!(
        all_daily.iter().map(|row| row.session_count).sum::<i64>(),
        2
    );
    assert_eq!(all_daily.iter().map(|row| row.total_cost).sum::<f64>(), 9.0);

    let codex_projects = query_project_stats(&conn, "codex", &cutoff_time);
    let claude_projects = query_project_stats(&conn, "claude", &cutoff_time);
    let all_projects = query_project_stats(&conn, "all", &cutoff_time);
    assert_eq!(codex_projects[0].session_count, 1);
    assert_eq!(claude_projects[0].session_count, 1);
    assert_eq!(all_projects[0].session_count, 2);
    assert_eq!(all_projects[0].total_cost, 9.0);

    let now = Utc::now();
    let codex_forecast = cost_forecast_from_connection(&conn, "codex", now);
    let claude_forecast = cost_forecast_from_connection(&conn, "claude", now);
    let all_forecast = cost_forecast_from_connection(&conn, "all", now);
    assert_eq!(codex_forecast.billed_spend_usd, None);
    assert_eq!(claude_forecast.billed_spend_usd, None);
    assert_eq!(all_forecast.billed_spend_usd, None);
    assert_eq!(codex_forecast.api_equivalent_usd, Some(2.0));
    assert_eq!(claude_forecast.api_equivalent_usd, Some(7.0));
    assert_eq!(all_forecast.api_equivalent_usd, Some(9.0));
}

#[test]
fn clear_history_is_scoped_and_keeps_fts_in_sync() {
    let mut conn = test_conn();
    let timestamp = Utc::now().to_rfc3339();
    for (id, provider) in [("codex:clear", "codex"), ("claude:keep", "claude")] {
        conn.execute(
            "INSERT INTO sessions (
                    id, provider, project, model, started_at, created_at, updated_at,
                    cost_status, cost_source, total_tokens
                 ) VALUES (?1, ?2, 'clear-repo', 'clear-model', ?3, ?3, ?3,
                    'unavailable', 'unknown', 100)",
            params![id, provider, timestamp],
        )
        .expect("insert scoped clear fixture");
    }

    assert_eq!(
        clear_history_from_connection(&mut conn, "codex").expect("clear codex history"),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE provider = 'claude'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("count retained history"),
        1
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sessions_fts", [], |row| row
            .get::<_, i64>(0))
            .expect("count retained search rows"),
        1
    );
}

fn sample_session_info(
    context_window: &str,
    model_id: &str,
    input_tokens: u64,
    tokens: u64,
) -> super::super::commands::SessionInfo {
    super::super::commands::SessionInfo {
        opencode: None,
        orion: None,
        provider: "claude".into(),
        app_name: None,
        session_id: "session".into(),
        session_name: None,
        project: "repo".into(),
        model: "Claude Opus".into(),
        model_id: model_id.into(),
        context_window: context_window.into(),
        cost: 0.0,
        cost_available: true,
        cost_basis: "exact".into(),
        cost_source: String::new(),
        tokens,
        input_tokens,
        output_tokens: 0,
        cache_write_tokens: 0,
        cache_read_tokens: 0,
        context_used_tokens: input_tokens,
        context_window_tokens: 0,
        branch: None,
        activity: "Idle".into(),
        activity_target: None,
        effort: "Medium".into(),
        effort_explicit: false,
        is_idle: false,
        started_at: None,
        duration_secs: 0,
        has_thinking: false,
        workflow_label: None,
        subagent_count: 0,
        subagents: Vec::new(),
        tokens_per_sec: 0.0,
        input_cost: 0.0,
        output_cost: 0.0,
        cache_write_cost: 0.0,
        cache_read_cost: 0.0,
        speed: "standard".into(),
        fast: false,
        service_tier: None,
        has_inflated_tokenizer: false,
    }
}

fn repair_session(id: &str, tokens: u64, cost: f64) -> super::super::commands::SessionInfo {
    let mut info = sample_session_info("1M", "claude-opus-5-5", tokens, tokens);
    info.session_id = id.into();
    info.cost = cost;
    info.input_cost = cost;
    info.cache_read_tokens = tokens;
    info.cost_source = "api_equivalent".into();
    info
}

fn stored_tokens_and_cost(conn: &Connection, id: &str) -> (i64, Option<f64>) {
    conn.query_row(
        "SELECT total_tokens, known_cost FROM sessions WHERE id = ?1",
        params![format!("claude:{id}")],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .expect("stored row")
}

/// Four stored rows: inflated, statusline-priced, transcript gone and
/// already correct. Only the first two change, statusline money stays.
#[test]
fn claude_history_repair_corrects_inflated_rows_and_backs_up_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let database = dir.path().join("pulse-analytics.db");
    let conn = Connection::open(&database).expect("database");
    init_schema(&conn).expect("schema");
    let now = Utc::now().to_rfc3339();

    let inflated = repair_session("inflated", 3_000, 9.0);
    let mut statusline = repair_session("statusline", 3_000, 12.0);
    statusline.cost_source = "claude_statusline_api_equivalent".into();
    let gone = repair_session("gone", 5_000, 5.0);
    let correct = repair_session("correct", 1_000, 1.0);
    for info in [&inflated, &statusline, &gone, &correct] {
        upsert_session_into(&conn, info, &now).expect("seed row");
    }

    let fresh = vec![
        repair_session("inflated", 1_000, 3.0),
        {
            let mut info = repair_session("statusline", 1_000, 4.0);
            info.cost_source = "claude_statusline_api_equivalent".into();
            info
        },
        repair_session("correct", 1_000, 1.0),
    ];

    let (preview, _) = plan_claude_history_repair(&conn, &fresh).expect("preview");
    assert_eq!(preview.rows_checked, 4);
    assert_eq!(preview.rows_to_update, 2);
    assert_eq!(preview.rows_without_transcript, 1);
    assert_eq!(preview.tokens_before, 6_000);
    assert_eq!(preview.tokens_after, 2_000);
    assert!((preview.cost_before - 21.0).abs() < 1e-9);
    assert!(
        (preview.cost_after - 15.0).abs() < 1e-9,
        "statusline money is kept"
    );
    assert!(!preview.applied);
    assert_eq!(
        stored_tokens_and_cost(&conn, "inflated"),
        (3_000, Some(9.0))
    );

    let applied = apply_claude_history_repair_in(&conn, &database, &fresh).expect("apply");
    assert!(applied.applied);
    let backup = PathBuf::from(applied.backup_path.expect("backup path"));
    assert!(backup.is_file());
    let backup_conn = Connection::open(&backup).expect("backup");
    assert_eq!(
        stored_tokens_and_cost(&backup_conn, "inflated"),
        (3_000, Some(9.0)),
        "the backup holds the values from before the repair"
    );

    assert_eq!(
        stored_tokens_and_cost(&conn, "inflated"),
        (1_000, Some(3.0))
    );
    assert_eq!(
        stored_tokens_and_cost(&conn, "statusline"),
        (1_000, Some(12.0))
    );
    assert_eq!(stored_tokens_and_cost(&conn, "gone"), (5_000, Some(5.0)));
    assert_eq!(stored_tokens_and_cost(&conn, "correct"), (1_000, Some(1.0)));

    let again = apply_claude_history_repair_in(&conn, &database, &fresh).expect("rerun");
    assert_eq!(again.rows_to_update, 0);
    assert!(!again.applied, "nothing to do writes no second backup");
    assert_eq!(again.backup_path, None);
}

#[test]
fn session_window_tokens_reports_1m_for_1m_context() {
    let one_m = sample_session_info("1M", "claude-opus-4-8", 10, 10);
    assert_eq!(session_window_tokens(&one_m), 1_000_000);

    let ga_1m = sample_session_info("200K", "claude-opus-4-8", 10, 10);
    assert_eq!(session_window_tokens(&ga_1m), 1_000_000);

    let two_hundred_k = sample_session_info("200K", "claude-sonnet-4-5", 10, 10);
    assert_eq!(session_window_tokens(&two_hundred_k), 200_000);
}

/// Opus 5 and Opus 5.5 ship 1M as both default and maximum window, so they must
/// report 1M even when a stale snapshot still says "200K".
#[test]
fn session_window_tokens_reports_1m_for_opus_5() {
    for model in [
        "claude-opus-5",
        "claude-opus-5-20260724",
        "claude-opus-5[1m]",
        "claude-opus-5-5",
        "claude-opus-5-5[1m]",
    ] {
        let stale = sample_session_info("200K", model, 10, 10);
        assert_eq!(session_window_tokens(&stale), 1_000_000, "{model}");
    }
}

#[test]
fn session_window_tokens_reports_1m_for_haiku_5_5_but_not_haiku_4_5() {
    for model in [
        "claude-haiku-5-5",
        "claude-haiku-5-5-20261101",
        "claude-haiku-5-5[1m]",
    ] {
        let stale = sample_session_info("200K", model, 10, 10);
        assert_eq!(session_window_tokens(&stale), 1_000_000, "{model}");
    }
    let haiku_4_5 = sample_session_info("200K", "claude-haiku-4-5", 10, 10);
    assert_eq!(session_window_tokens(&haiku_4_5), 200_000);
}

/// `limit: None` must mean "every matching row". It used to silently
/// default to 100, which truncated aggregate queries: a 299-session window
/// reported the cost of only its 100 newest sessions.
#[test]
fn query_limit_none_is_unbounded_not_a_hidden_hundred() {
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
    init_schema(&conn).expect("schema");

    for i in 0..250 {
        conn.execute(
            "INSERT INTO sessions (id, provider, project, model, model_id, context_window,
                    effort, started_at, total_cost, updated_at)
                 VALUES (?1, 'claude', 'p', 'm', 'claude-opus-5', '1M', 'high',
                    datetime('now'), 1.0, datetime('now'))",
            rusqlite::params![format!("s{i}")],
        )
        .expect("insert");
    }

    let unbounded = query_sessions(
        &conn, "claude", None, None, None, None, None, None, None, None, None, None,
    );
    assert_eq!(unbounded.len(), 250, "None must not cap the result set");

    let capped = query_sessions(
        &conn,
        "claude",
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
    assert_eq!(capped.len(), 10, "an explicit limit still applies");
}

#[test]
fn session_used_tokens_uses_context_snapshot_not_lifetime_total() {
    let mut info = sample_session_info("1M", "claude-opus-4-8", 83_700, 8_580_000);
    info.context_used_tokens = 83_700;
    info.context_window_tokens = 1_000_000;
    assert_eq!(session_used_tokens(&info), 83_700);

    info.context_used_tokens = 1_250_000;
    assert_eq!(session_used_tokens(&info), 1_000_000);
}

#[test]
fn history_query_uses_created_at_when_started_at_missing() {
    let conn = test_conn();
    let created_at = Utc::now().to_rfc3339();
    let provider = active_provider_slug().to_string();
    conn.execute(
        "INSERT INTO sessions (id, provider, project, model, created_at, updated_at, total_cost)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            "session-a",
            provider,
            "repo-a",
            "Claude Opus 4.7",
            created_at,
            created_at,
            12.5
        ],
    )
    .expect("insert session");

    let rows = query_sessions(
        &conn,
        &provider,
        Some(7),
        None,
        None,
        Some("repo-a"),
        None,
        None,
        None,
        None,
        None,
        Some(10),
    );

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "session-a");
}

#[test]
fn seven_day_history_includes_six_days_ago_and_excludes_eight_days_ago() {
    let conn = test_conn();
    let provider = FIXTURE_PROVIDER;
    let now = Utc::now();
    for (id, timestamp) in [
        ("inside", now - chrono::Duration::days(6)),
        ("outside", now - chrono::Duration::days(8)),
    ] {
        conn.execute(
            "INSERT INTO sessions (id, provider, project, model, started_at, updated_at)
                 VALUES (?1, ?2, 'repo', 'model', ?3, ?3)",
            params![id, provider, timestamp.to_rfc3339()],
        )
        .expect("insert dated session");
    }

    let rows = query_sessions(
        &conn,
        provider,
        Some(7),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );

    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        vec!["inside"]
    );
}

#[test]
fn historical_session_round_trips_stored_cost_provenance() {
    let conn = test_conn();
    let now = Utc::now().to_rfc3339();
    for (id, status, source, known_cost) in [
        ("exact", "exact", "session-calculated", Some(1.25)),
        ("partial", "partial", "session-calculated", Some(0.75)),
        ("billed", "exact", "provider_billed", Some(2.5)),
        ("missing", "unavailable", "unknown", None),
    ] {
        conn.execute(
            "INSERT INTO sessions (
                    id, provider, project, model, started_at, updated_at,
                    total_cost, cost_status, cost_source, known_cost
                 ) VALUES (?1, ?2, 'repo', 'model', ?3, ?3, 99, ?4, ?5, ?6)",
            params![id, FIXTURE_PROVIDER, now, status, source, known_cost],
        )
        .expect("insert provenance row");
    }

    let rows = query_sessions(
        &conn,
        FIXTURE_PROVIDER,
        Some(7),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    let by_id = rows
        .into_iter()
        .map(|row| (row.id.clone(), row))
        .collect::<std::collections::HashMap<_, _>>();

    assert_eq!(by_id["exact"].cost_basis, CostBasis::Estimated);
    assert_eq!(by_id["partial"].cost_basis, CostBasis::Partial);
    assert_eq!(by_id["billed"].cost_basis, CostBasis::Exact);
    assert_eq!(by_id["missing"].cost_basis, CostBasis::Unavailable);
    assert_eq!(by_id["exact"].known_cost, Some(1.25));
    assert_eq!(by_id["missing"].known_cost, None);
    assert_eq!(by_id["billed"].cost_source, "provider_billed");
}

#[test]
fn collected_codex_jsonl_persists_into_current_history_and_forecast() {
    let root = tempfile::TempDir::new().expect("session root");
    let session_file = root.path().join("current.jsonl");
    let now = Utc::now();
    let lines = [
        serde_json::json!({
            "timestamp": now.to_rfc3339(),
            "type": "session_meta",
            "payload": {
                "id": "current-codex",
                "cwd": root.path(),
                "originator": "codex_cli_rs"
            }
        }),
        serde_json::json!({
            "timestamp": now.to_rfc3339(),
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": 1200,
                        "cached_input_tokens": 400,
                        "output_tokens": 200,
                        "total_tokens": 1400
                    }
                }
            }
        }),
    ]
    .into_iter()
    .map(|line| line.to_string())
    .collect::<Vec<_>>()
    .join("\n");
    std::fs::write(&session_file, lines).expect("write current JSONL");

    let mut git = cc_discord_presence::codex::session::GitBranchCache::new(
        std::time::Duration::from_secs(30),
    );
    let mut parse = cc_discord_presence::codex::session::SessionParseCache::default();
    let config = cc_discord_presence::codex::config::PresenceConfig::default();
    let snapshots = cc_discord_presence::codex::session::collect_active_sessions_multi(
        &[root.path().to_path_buf()],
        std::time::Duration::from_secs(120),
        std::time::Duration::from_secs(120),
        &mut git,
        &mut parse,
        &config.pricing,
    )
    .expect("collect current Codex session");
    let infos = crate::commands::build_codex_session_infos(
        &snapshots,
        &config,
        cc_discord_presence::codex::config::PresenceSurface::Cli,
    );
    let conn = test_conn();
    for info in &infos {
        upsert_session_into(&conn, info, &now.to_rfc3339()).expect("persist snapshot");
    }

    let history = query_sessions(
        &conn,
        "codex",
        Some(7),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    let forecast = cost_forecast_from_connection(&conn, "codex", now);

    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, "codex:current-codex");
    assert_eq!(history[0].total_tokens, 1400);
    assert_eq!(forecast.sessions, 1);
}

#[test]
fn cost_forecast_reports_partial_coverage_without_promoting_raw_totals() {
    let conn = test_conn();
    let now = Utc
        .with_ymd_and_hms(2026, 8, 15, 12, 0, 0)
        .single()
        .expect("fixed date");
    for (id, status, source, known_cost, raw_cost) in [
        ("exact", "exact", "session-calculated", Some(2.5), 2.5),
        ("missing", "unavailable", "unknown", None, 99.0),
    ] {
        conn.execute(
            "INSERT INTO sessions (
                    id, provider, project, model, started_at, updated_at,
                    total_cost, cost_status, cost_source, known_cost
                 ) VALUES (?1, 'codex', 'repo', 'model', ?2, ?2, ?3, ?4, ?5, ?6)",
            params![id, now.to_rfc3339(), raw_cost, status, source, known_cost],
        )
        .expect("insert forecast row");
    }

    let forecast = cost_forecast_from_connection(&conn, "codex", now);

    assert_eq!(forecast.cost_basis, CostBasis::Partial);
    assert_eq!(forecast.sessions, 2);
    assert_eq!(forecast.priced_sessions, 1);
    assert_eq!(forecast.billed_spend_usd, None);
    assert_eq!(forecast.api_equivalent_usd, Some(2.5));
    assert_eq!(
        forecast.cost_sources,
        vec!["session-calculated".to_string()]
    );
}

#[test]
fn forecast_and_budget_keep_provider_billing_separate_from_api_equivalent_value() {
    let conn = test_conn();
    let now = Utc
        .with_ymd_and_hms(2026, 8, 15, 12, 0, 0)
        .single()
        .expect("fixed date");
    for (id, source, known_cost) in [
        ("billed", "provider_billed", 10.0),
        ("estimated", "api_equivalent", 90.0),
    ] {
        conn.execute(
            "INSERT INTO sessions (
                    id, provider, project, model, started_at, updated_at,
                    total_cost, cost_status, cost_source, known_cost
                 ) VALUES (?1, 'codex', 'repo', 'model', ?2, ?2, ?3, 'exact', ?4, ?3)",
            params![id, now.to_rfc3339(), known_cost, source],
        )
        .expect("insert monetary provenance fixture");
    }
    conn.execute(
        "INSERT INTO budget_config (id, monthly_budget, alert_threshold_pct, updated_at)
             VALUES (1, 50, 80, ?1)",
        params![now.to_rfc3339()],
    )
    .expect("insert budget fixture");

    let forecast = cost_forecast_from_connection(&conn, "codex", now);
    assert_eq!(forecast.billed_spend_usd, Some(10.0));
    assert_eq!(forecast.api_equivalent_usd, Some(90.0));
    assert_eq!(forecast.billed_sessions, 1);
    assert_eq!(forecast.api_equivalent_sessions, 1);

    let budget = budget_status_from_connection(&conn, "codex", now);
    assert_eq!(budget.billed_spend_usd, Some(10.0));
    assert_eq!(budget.api_equivalent_usd, Some(90.0));
    assert_eq!(budget.pct_used, Some(20.0));
    assert!(
        !budget.over_budget,
        "API-equivalent value must not trip a billing budget"
    );
}

#[test]
fn forecast_does_not_promote_unknown_known_cost_to_api_equivalent_value() {
    let conn = test_conn();
    let now = Utc
        .with_ymd_and_hms(2026, 8, 15, 12, 0, 0)
        .single()
        .expect("fixed date");
    for (id, source, known_cost) in [
        ("billed", "provider_billed", 10.0),
        ("calculated", "session-calculated", 20.0),
        ("other", "external-ledger", 30.0),
    ] {
        conn.execute(
            "INSERT INTO sessions (
                    id, provider, project, model, started_at, updated_at,
                    total_cost, cost_status, cost_source, known_cost
                 ) VALUES (?1, 'codex', 'repo', 'model', ?2, ?2, ?3, 'exact', ?4, ?3)",
            params![id, now.to_rfc3339(), known_cost, source],
        )
        .expect("insert monetary provenance fixture");
    }

    let forecast = cost_forecast_from_connection(&conn, "codex", now);

    assert_eq!(forecast.billed_spend_usd, Some(10.0));
    assert_eq!(forecast.api_equivalent_usd, Some(20.0));
    assert_eq!(forecast.billed_sessions, 1);
    assert_eq!(forecast.api_equivalent_sessions, 1);
    assert_eq!(forecast.priced_sessions, 3);
}

#[test]
fn reconstructed_costs_survive_summary_search_filter_and_blank_model_forecast() {
    let conn = test_conn();
    let now = Utc
        .with_ymd_and_hms(2026, 8, 15, 12, 0, 0)
        .single()
        .expect("fixed date");
    conn.execute(
        "INSERT INTO sessions (
                id, provider, session_name, project, model, model_id,
                started_at, updated_at, cost_status, cost_source, known_cost,
                input_tokens, output_tokens, total_tokens
             ) VALUES (
                'estimated', 'codex', 'needle session', 'repo', 'gpt-5.6-sol', '',
                ?1, ?1, 'unavailable', 'unknown', NULL,
                100000, 100000, 200000
             )",
        params![now.to_rfc3339()],
    )
    .expect("insert unpriced subscription session");

    let summary = analytics_summary_from_connection(&conn, "codex");
    assert_eq!(summary.priced_sessions, 1);
    assert_eq!(summary.cost_basis, CostBasis::Estimated);
    assert!(summary.total_cost > 0.0);

    let filtered = query_sessions(
        &conn,
        "codex",
        None,
        None,
        None,
        None,
        None,
        Some(1.0),
        None,
        None,
        None,
        Some(10),
    );
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].cost_basis, CostBasis::Estimated);

    let searched = search_sessions_from_connection(&conn, "codex", "repo", Some(10));
    assert_eq!(searched.len(), 1);
    assert_eq!(searched[0].cost_basis, CostBasis::Estimated);

    let timeline = query_daily_costs(&conn, "codex", "2026-08-01", None);
    assert_eq!(timeline.len(), 1);
    assert_eq!(timeline[0].priced_sessions, 1);
    assert_eq!(timeline[0].cost_basis, CostBasis::Estimated);
    assert!(timeline[0].cost > 0.0);

    let forecast = cost_forecast_from_connection(&conn, "codex", now);
    assert_eq!(forecast.priced_sessions, 1);
    assert_eq!(forecast.cost_basis, CostBasis::Estimated);
    assert!(forecast.billed_spend_usd.is_none());
    assert!(forecast.api_equivalent_usd.is_some_and(|value| value > 0.0));
}

#[test]
fn analytics_top_project_uses_reconstructed_cost_not_session_count() {
    let conn = test_conn();
    for (id, project, input_tokens) in [
        ("cheap-1", "busy", 10_000),
        ("cheap-2", "busy", 10_000),
        // Below the 272,000-token long-context threshold, so the total stays estimable.
        ("expensive", "costly", 250_000),
    ] {
        conn.execute(
            "INSERT INTO sessions (
                    id, provider, project, model, model_id, started_at, updated_at,
                    cost_status, cost_source, known_cost, input_tokens, output_tokens
                 ) VALUES (?1, 'codex', ?2, 'GPT-5.4', 'gpt-5.4',
                    '2026-08-15T12:00:00Z', '2026-08-15T12:00:00Z',
                    'unavailable', 'unknown', NULL, ?3, 1000)",
            params![id, project, input_tokens],
        )
        .expect("insert project session");
    }

    let summary = analytics_summary_from_connection(&conn, "codex");
    assert_eq!(summary.top_project, "costly");
}

#[test]
fn persisted_api_equivalent_cost_remains_estimated_in_sql_aggregates() {
    let conn = test_conn();
    conn.execute(
        "INSERT INTO sessions (
                id, provider, project, model, started_at, updated_at,
                total_cost, cost_status, cost_source, known_cost
             ) VALUES (
                'estimated', 'claude', 'repo', 'Claude Opus 4.8',
                '2026-08-15T12:00:00Z', '2026-08-15T12:00:00Z',
                2.5, 'exact', 'api_equivalent', 2.5
             )",
        [],
    )
    .expect("insert estimated session");

    let daily = query_daily_stats(&conn, "claude", "2026-08-01");
    let projects = query_project_stats(&conn, "claude", "2026-08-01T00:00:00Z");
    let hourly = query_hourly_activity(&conn, "claude", "2026-08-01T00:00:00Z");
    let models = query_model_distribution(&conn, "claude", "2026-08-01T00:00:00Z");

    for basis in [
        daily[0].cost_basis,
        projects[0].cost_basis,
        hourly[0].cost_basis,
        models[0].cost_basis,
    ] {
        assert_eq!(basis, CostBasis::Estimated);
    }
}

#[test]
fn aggregate_views_exclude_unavailable_raw_cost_and_report_partial_coverage() {
    let conn = test_conn();
    let now = Utc
        .with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
        .single()
        .expect("fixed date");
    for (id, status, source, known_cost, raw_cost) in [
        ("priced", "exact", "session-calculated", Some(2.5), 2.5),
        ("unpriced", "unavailable", "unknown", None, 99.0),
    ] {
        conn.execute(
            "INSERT INTO sessions (
                    id, provider, project, model, started_at, updated_at,
                    total_cost, cost_status, cost_source, known_cost, duration_secs
                 ) VALUES (?1, 'codex', 'repo', 'gpt-5.6', ?2, ?2, ?3, ?4, ?5, ?6, 60)",
            params![id, now.to_rfc3339(), raw_cost, status, source, known_cost],
        )
        .expect("insert aggregate row");
    }

    let daily = query_daily_stats(&conn, "codex", "2026-08-01");
    let projects = query_project_stats(&conn, "codex", "2026-08-01T00:00:00+00:00");
    let hourly = query_hourly_activity(&conn, "codex", "2026-08-01T00:00:00+00:00");
    let timeline = query_daily_costs(&conn, "codex", "2026-08-01", None);
    let summary = analytics_summary_from_connection(&conn, "codex");
    let models = query_model_distribution(&conn, "codex", "2026-08-01T00:00:00+00:00");

    assert_eq!(daily[0].total_cost, 2.5);
    assert_eq!(daily[0].priced_sessions, 1);
    assert_eq!(daily[0].cost_basis, CostBasis::Partial);
    assert_eq!(projects[0].total_cost, 2.5);
    assert_eq!(projects[0].avg_session_cost, 2.5);
    assert_eq!(projects[0].priced_sessions, 1);
    assert_eq!(projects[0].cost_basis, CostBasis::Partial);
    assert_eq!(hourly[0].total_cost, 2.5);
    assert_eq!(hourly[0].priced_sessions, 1);
    assert_eq!(hourly[0].cost_basis, CostBasis::Partial);
    assert_eq!(timeline[0].cost, 2.5);
    assert_eq!(timeline[0].priced_sessions, 1);
    assert_eq!(timeline[0].cost_basis, CostBasis::Partial);
    assert_eq!(summary.total_cost, 2.5);
    assert_eq!(summary.avg_cost_per_session, 2.5);
    assert_eq!(summary.priced_sessions, 1);
    assert_eq!(summary.cost_basis, CostBasis::Partial);
    assert_eq!(summary.top_project, "repo");
    assert_eq!(models[0].model, "gpt-5.6");
    assert_eq!(models[0].session_count, 2);
    assert_eq!(models[0].priced_sessions, 1);
    assert_eq!(models[0].cost_basis, CostBasis::Partial);
    assert_eq!(models[0].total_cost, 2.5);
}

/// Provider slug used by the daily-aggregation fixtures. Pinned rather than
/// read from the global active provider, which another test can switch
/// between the insert and the query.
const FIXTURE_PROVIDER: &str = "claude";

/// Inserts one dated session so daily-aggregation tests can build a window
/// without going through the live upsert path.
fn insert_dated_session(conn: &Connection, id: &str, day: &str, project: &str, cost: f64) {
    let provider = FIXTURE_PROVIDER.to_string();
    let started = format!("{day}T12:00:00+00:00");
    conn.execute(
        "INSERT INTO sessions (
                id, provider, project, model, started_at, updated_at, total_cost,
                cost_status, cost_source, known_cost
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'exact', 'fixture', ?7)",
        params![
            id,
            provider,
            project,
            "Claude Opus 5",
            started,
            started,
            cost
        ],
    )
    .expect("insert dated session");
}

#[test]
fn daily_costs_aggregate_every_session_in_the_window() {
    let conn = test_conn();
    let provider = FIXTURE_PROVIDER.to_string();
    insert_dated_session(&conn, "a", "2026-07-01", "repo", 10.0);
    insert_dated_session(&conn, "b", "2026-07-01", "repo", 5.5);
    insert_dated_session(&conn, "c", "2026-07-03", "repo", 2.0);

    let rows = query_daily_costs(&conn, &provider, "2026-07-01", None);

    assert_eq!(rows.len(), 2, "only days with sessions are returned");
    assert_eq!(rows[0].date, "2026-07-01");
    assert!((rows[0].cost - 15.5).abs() < 0.000_001);
    assert_eq!(rows[0].sessions, 2);
    assert_eq!(rows[1].date, "2026-07-03");
}

/// The boundary day is inclusive: a session earlier in the day than the
/// rolling `now - days` instant still belongs to the first plotted date.
#[test]
fn daily_costs_include_the_whole_boundary_day_and_exclude_earlier_days() {
    let conn = test_conn();
    let provider = FIXTURE_PROVIDER.to_string();
    insert_dated_session(&conn, "before", "2026-06-30", "repo", 99.0);
    insert_dated_session(&conn, "boundary", "2026-07-01", "repo", 4.0);

    let rows = query_daily_costs(&conn, &provider, "2026-07-01", None);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].date, "2026-07-01");
    assert!((rows[0].cost - 4.0).abs() < 0.000_001);
}

#[test]
fn daily_costs_respect_the_project_filter() {
    let conn = test_conn();
    let provider = FIXTURE_PROVIDER.to_string();
    insert_dated_session(&conn, "a", "2026-07-01", "repo-a", 10.0);
    insert_dated_session(&conn, "b", "2026-07-01", "repo-b", 3.0);

    let rows = query_daily_costs(&conn, &provider, "2026-07-01", Some("repo-b"));

    assert_eq!(rows.len(), 1);
    assert!((rows[0].cost - 3.0).abs() < 0.000_001);
}

/// Aggregation must cover the whole window even when the analyzer page cap
/// would have dropped the oldest sessions.
#[test]
fn daily_costs_are_not_limited_by_the_analyzer_row_cap() {
    let conn = test_conn();
    let provider = FIXTURE_PROVIDER.to_string();
    for index in 0..120 {
        let day = format!("2026-07-{:02}", (index % 30) + 1);
        insert_dated_session(&conn, &format!("s{index}"), &day, "repo", 1.0);
    }

    let rows = query_daily_costs(&conn, &provider, "2026-07-01", None);
    let total: f64 = rows.iter().map(|row| row.cost).sum();
    let sessions: i64 = rows.iter().map(|row| row.sessions).sum();

    assert_eq!(sessions, 120);
    assert!((total - 120.0).abs() < 0.000_001);
}

#[test]
fn hour_range_filter_uses_fallback_timestamp() {
    let conn = test_conn();
    let provider = active_provider_slug().to_string();
    let local_early_hour: i64 = conn
        .query_row(
            "SELECT CAST(strftime('%H', ?1, 'localtime') AS INTEGER)",
            ["2026-04-18T03:15:00+00:00"],
            |row| row.get(0),
        )
        .expect("local hour");
    conn.execute(
        "INSERT INTO sessions (id, provider, project, model, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            "session-early",
            provider.clone(),
            "repo-a",
            "Claude Opus 4.7",
            "2026-04-18T03:15:00+00:00",
            "2026-04-18T03:15:00+00:00"
        ],
    )
    .expect("insert early");
    conn.execute(
        "INSERT INTO sessions (id, provider, project, model, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            "session-late",
            provider,
            "repo-a",
            "Claude Opus 4.7",
            "2026-04-18T15:45:00+00:00",
            "2026-04-18T15:45:00+00:00"
        ],
    )
    .expect("insert late");

    let rows = query_sessions(
        &conn,
        &provider,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(local_early_hour),
        Some(local_early_hour),
        Some(10),
    );

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "session-early");
}

#[test]
fn init_schema_is_idempotent_and_preserves_rows() {
    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    init_schema(&conn).expect("initialize schema");
    let provider = active_provider_slug().to_string();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO sessions (id, provider, project, model, created_at, updated_at, used_tokens, window_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            "session-keep",
            provider,
            "repo-a",
            "Claude Opus 4.7",
            now,
            now,
            123_456_i64,
            1_000_000_i64
        ],
    )
    .expect("insert session");

    init_schema(&conn).expect("repeat schema initialization");

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
        .expect("count rows");
    assert_eq!(count, 1);

    let rows = query_sessions(
        &conn,
        &provider,
        Some(7),
        None,
        None,
        Some("repo-a"),
        None,
        None,
        None,
        None,
        None,
        Some(10),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].used_tokens, 123_456);
    assert_eq!(rows[0].window_tokens, 1_000_000);
}

#[test]
fn context_tokens_round_trip_through_query() {
    let conn = test_conn();
    let provider = active_provider_slug().to_string();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO sessions (id, provider, project, model, created_at, updated_at, used_tokens, window_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            "session-ctx",
            provider,
            "repo-ctx",
            "Claude Sonnet 4.6",
            now,
            now,
            90_000_i64,
            200_000_i64
        ],
    )
    .expect("insert session");

    let rows = query_sessions(
        &conn,
        &provider,
        Some(7),
        None,
        None,
        Some("repo-ctx"),
        None,
        None,
        None,
        None,
        None,
        Some(10),
    );

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].used_tokens, 90_000);
    assert_eq!(rows[0].window_tokens, 200_000);
}

#[test]
fn window_backfill_maps_context_label_when_zero() {
    let conn = test_conn();
    let provider = active_provider_slug().to_string();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO sessions (id, provider, project, model, context_window, created_at, updated_at, window_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
        params![
            "session-1m",
            provider,
            "repo-1m",
            "Claude Opus 4.8",
            "1M",
            now,
            now
        ],
    )
    .expect("insert session");

    init_schema(&conn).expect("repeat schema initialization");

    let window: i64 = conn
        .query_row(
            "SELECT window_tokens FROM sessions WHERE id = 'session-1m'",
            [],
            |r| r.get(0),
        )
        .expect("read window");
    assert_eq!(window, 1_000_000);
}

#[test]
fn migration_v3_to_v5_preserves_rollback_table_and_adds_history_index() {
    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    create_v3_schema(&conn);

    for (id, label) in [
        ("context-200k", "200K"),
        ("context-400k", "400K"),
        ("context-372k", "372K"),
        ("context-353-4k", "353.4K"),
    ] {
        conn.execute(
            "INSERT INTO sessions (
                    id, provider, project, model, context_window, total_cost, updated_at
                 ) VALUES (?1, 'codex', 'repo', 'GPT-5.6 Sol', ?2, 19.75, '2026-07-10T12:00:00+00:00')",
            params![id, label],
        )
        .expect("insert v3 session");
    }
    conn.execute(
        "INSERT INTO daily_stats (
                date, provider, project, model, session_count, total_cost, total_tokens
             ) VALUES ('2026-07-10', 'codex', 'rollback-sentinel', 'GPT-5.6 Sol', 4, 79.0, 4000)",
        [],
    )
    .expect("insert rollback sentinel");

    init_schema(&conn).expect("migrate schema");

    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("read user_version");
    assert_eq!(version, SCHEMA_VERSION);

    let provider_history_index: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_index_list('sessions') WHERE name = 'idx_sessions_provider_history_ts'",
            [],
            |row| row.get(0),
        )
        .expect("read provider history index");
    assert_eq!(provider_history_index, 1);

    let columns = {
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('sessions')")
            .expect("prepare schema query");
        stmt.query_map([], |row| row.get::<_, String>(0))
            .expect("query schema")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect columns")
    };
    for expected in [
        "speed",
        "speed_source",
        "speed_known",
        "cost_status",
        "cost_source",
        "known_cost",
        "cached_input_savings",
        "context_source",
        "context_raw_source",
        "raw_window_tokens",
        "effective_context_percent",
    ] {
        assert!(
            columns.iter().any(|column| column == expected),
            "{expected}"
        );
    }

    let context_windows = {
        let mut stmt = conn
            .prepare("SELECT id, window_tokens FROM sessions ORDER BY id")
            .expect("prepare context query");
        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .expect("query contexts")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect contexts")
    };
    assert_eq!(
        context_windows,
        vec![
            ("context-200k".into(), 200_000),
            ("context-353-4k".into(), 353_400),
            ("context-372k".into(), 372_000),
            ("context-400k".into(), 400_000),
        ]
    );

    let legacy_state: (i64, String, String, Option<f64>, String, String) = conn
        .query_row(
            "SELECT speed_known, speed_source, cost_status, known_cost,
                        context_source, context_raw_source
                 FROM sessions WHERE id = 'context-372k'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .expect("read legacy state");
    assert_eq!(
        legacy_state,
        (
            0,
            "legacy".into(),
            "unavailable".into(),
            None,
            "legacy".into(),
            "unknown".into(),
        )
    );

    let rollback_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM daily_stats WHERE project = 'rollback-sentinel'",
            [],
            |row| row.get(0),
        )
        .expect("read rollback table");
    assert_eq!(rollback_rows, 1);
}

#[test]
fn migration_v4_to_v5_only_adds_index_and_preserves_provenance() {
    let conn = test_conn();
    conn.execute_batch(
        "DROP INDEX idx_sessions_provider_history_ts;
             PRAGMA user_version = 4;",
    )
    .expect("prepare v4 database");
    conn.execute(
        "INSERT INTO sessions (
                id, provider, project, model, updated_at, total_cost,
                cost_status, cost_source, known_cost, context_source
             ) VALUES (
                'v4-codex', 'codex', 'repo', 'GPT-5.6 Sol',
                '2026-07-16T12:00:00+00:00', 12.5,
                'exact', 'session', 12.5, 'event'
             )",
        [],
    )
    .expect("insert v4 row");

    init_schema(&conn).expect("migrate v4 to v5");

    let provenance: (String, String, Option<f64>, String) = conn
        .query_row(
            "SELECT cost_status, cost_source, known_cost, context_source
                 FROM sessions WHERE id = 'v4-codex'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("read migrated provenance");
    assert_eq!(
        provenance,
        (
            "exact".to_string(),
            "session".to_string(),
            Some(12.5),
            "event".to_string()
        )
    );
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("read user_version");
    assert_eq!(version, SCHEMA_VERSION);
}

#[test]
fn file_migration_creates_valid_v3_backup_once_before_schema_mutation() {
    let path = temporary_database_path("v3-backup");
    {
        let conn = Connection::open(&path).expect("open v3 database");
        create_v3_schema(&conn);
        conn.execute(
            "INSERT INTO sessions (
                    id, provider, project, model, context_window, updated_at
                 ) VALUES (
                    'backup-sentinel', 'codex', 'repo', 'GPT-5.6 Sol', '372K',
                    '2026-07-10T12:00:00+00:00'
                 )",
            [],
        )
        .expect("insert backup sentinel");
    }

    let migrated = open_database(&path).expect("migrate database");
    let backup_path = migration_backup_path(&path);
    assert!(backup_path.exists());

    let backup = Connection::open(&backup_path).expect("open migration backup");
    assert_eq!(schema_version(&backup).expect("backup schema version"), 3);
    let backup_rows: i64 = backup
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = 'backup-sentinel'",
            [],
            |row| row.get(0),
        )
        .expect("backup sentinel count");
    assert_eq!(backup_rows, 1);
    let backup_has_speed: bool = backup
        .query_row(
            "SELECT EXISTS(
                    SELECT 1 FROM pragma_table_info('sessions') WHERE name = 'speed'
                )",
            [],
            |row| row.get(0),
        )
        .expect("backup schema columns");
    assert!(!backup_has_speed);
    assert_eq!(
        schema_version(&migrated).expect("live schema version"),
        SCHEMA_VERSION
    );

    drop(backup);
    drop(migrated);
    let first_backup = std::fs::read(&backup_path).expect("read first backup");
    drop(open_database(&path).expect("repeat database initialization"));
    let second_backup = std::fs::read(&backup_path).expect("read repeated backup");
    assert_eq!(second_backup, first_backup);

    remove_database_files(&path);
}

#[test]
fn daily_stats_are_derived_from_idempotent_session_rows() {
    let conn = test_conn();
    conn.execute(
        "INSERT INTO daily_stats (
                date, provider, project, model, session_count, total_cost, total_tokens
             ) VALUES (
                '2026-07-10', 'codex', 'stale-table', 'GPT-5.6 Sol', 99, 999.0, 999999
             )",
        [],
    )
    .expect("insert stale daily aggregate");

    let upsert = "INSERT INTO sessions (
                id, provider, project, model, started_at, created_at, updated_at,
                total_cost, known_cost, total_tokens, input_tokens, output_tokens,
                cache_write_tokens, cache_read_tokens
             ) VALUES (
                ?1, 'codex', 'repo', 'GPT-5.6 Sol', ?2, ?2, ?2,
                ?3, ?3, ?4, ?5, ?6, ?7, ?8
             )
             ON CONFLICT(id) DO UPDATE SET
                total_cost = excluded.total_cost,
                known_cost = excluded.known_cost,
                total_tokens = excluded.total_tokens,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cache_write_tokens = excluded.cache_write_tokens,
                cache_read_tokens = excluded.cache_read_tokens";
    conn.execute(
        upsert,
        params![
            "codex:one",
            "2026-07-10T10:00:00+00:00",
            1.0,
            1_000_i64,
            600_i64,
            100_i64,
            100_i64,
            200_i64
        ],
    )
    .expect("insert first session");
    conn.execute(
        upsert,
        params![
            "codex:one",
            "2026-07-10T10:00:00+00:00",
            1.5,
            1_500_i64,
            900_i64,
            150_i64,
            150_i64,
            300_i64
        ],
    )
    .expect("repeat first session upsert");
    conn.execute(
        upsert,
        params![
            "codex:two",
            "2026-07-10T11:00:00+00:00",
            2.5,
            2_500_i64,
            1_500_i64,
            250_i64,
            250_i64,
            500_i64
        ],
    )
    .expect("insert second session");

    let rows = query_daily_stats(&conn, "codex", "2026-07-01");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].date, "2026-07-10");
    assert_eq!(rows[0].project, "repo");
    assert_eq!(rows[0].model, "GPT-5.6 Sol");
    assert_eq!(rows[0].session_count, 2);
    assert_eq!(rows[0].total_cost, 4.0);
    assert_eq!(rows[0].total_tokens, 4_000);
    assert_eq!(rows[0].input_tokens, 2_400);
    assert_eq!(rows[0].output_tokens, 400);
    assert_eq!(rows[0].cache_write_tokens, 400);
    assert_eq!(rows[0].cache_read_tokens, 800);

    let stale_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM daily_stats", [], |row| row.get(0))
        .expect("read rollback table");
    assert_eq!(stale_rows, 1);
}

#[test]
fn session_upsert_supports_legacy_non_nullable_raw_window_tokens() {
    let conn = test_conn();
    conn.execute_batch(
        "CREATE TRIGGER sessions_raw_window_tokens_not_null
             BEFORE INSERT ON sessions
             WHEN NEW.raw_window_tokens IS NULL
             BEGIN
                 SELECT RAISE(ABORT, 'raw_window_tokens may not be NULL');
             END;",
    )
    .expect("enforce the legacy v4/v5 column invariant");

    let session = sample_session_info("258.4K", "gpt-5.6-sol", 1_000, 1_500);
    upsert_session_into(&conn, &session, "2026-08-03T09:30:36+00:00")
        .expect("persist against the installed schema invariant");

    let raw_window_tokens: i64 = conn
        .query_row(
            "SELECT raw_window_tokens FROM sessions WHERE id = 'claude:session'",
            [],
            |row| row.get(0),
        )
        .expect("read persisted raw window tokens");
    assert_eq!(raw_window_tokens, 0);
}

#[test]
fn session_upsert_persists_known_live_provenance_as_exact_only_when_flagged() {
    let conn = test_conn();
    let mut session = sample_session_info("353.4K", "gpt-5.6-sol", 1_000, 1_500);
    session.provider = "codex".into();
    session.model = "GPT-5.6 Sol".into();
    session.speed = "fast".into();
    session.fast = true;
    session.cost = 12.5;
    session.context_window_tokens = 353_400;

    upsert_session_into(&conn, &session, "2026-07-10T12:00:00+00:00").expect("insert live session");
    session.cost = 13.0;
    upsert_session_into(&conn, &session, "2026-07-10T12:01:00+00:00")
        .expect("repeat live session upsert");

    type StoredProvenance = (
        i64,
        String,
        String,
        i64,
        String,
        String,
        Option<f64>,
        Option<f64>,
        String,
        String,
        Option<i64>,
        Option<i64>,
    );
    let stored: StoredProvenance = conn
        .query_row(
            "SELECT COUNT(*) OVER (), speed, speed_source, speed_known,
                        cost_status, cost_source, known_cost, cached_input_savings,
                        context_source, context_raw_source, raw_window_tokens,
                        effective_context_percent
                 FROM sessions WHERE id = 'codex:session'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                ))
            },
        )
        .expect("read persisted provenance");
    assert_eq!(
        stored,
        (
            1,
            "fast".into(),
            "session".into(),
            1,
            "exact".into(),
            "session-calculated".into(),
            Some(13.0),
            None,
            "session".into(),
            "unknown".into(),
            Some(0),
            None,
        )
    );

    session.session_id = "unpriced".into();
    session.cost = 0.0;
    session.cost_available = false;
    session.cost_basis = "unavailable".into();
    upsert_session_into(&conn, &session, "2026-07-10T12:02:00+00:00")
        .expect("insert unpriced live session");
    let unpriced: (String, Option<f64>) = conn
        .query_row(
            "SELECT cost_status, known_cost FROM sessions WHERE id = 'codex:unpriced'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read unpriced session");
    assert_eq!(unpriced, ("unavailable".into(), None));

    session.session_id = "partial".into();
    session.cost = 4.0;
    session.cost_available = true;
    session.cost_basis = "partial".into();
    upsert_session_into(&conn, &session, "2026-07-10T12:03:00+00:00")
        .expect("insert partial live session");

    session.session_id = "billed".into();
    session.cost = 7.0;
    session.cost_basis = "provider_billed".into();
    upsert_session_into(&conn, &session, "2026-07-10T12:04:00+00:00")
        .expect("insert provider-billed live session");

    let read_provenance = |id: &str| {
        conn.query_row(
            "SELECT cost_status, cost_source, known_cost FROM sessions WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<f64>>(2)?,
                ))
            },
        )
        .expect("read stored cost provenance")
    };
    assert_eq!(
        read_provenance("codex:partial"),
        (
            "partial".to_string(),
            "session-calculated".to_string(),
            Some(4.0)
        )
    );
    assert_eq!(
        read_provenance("codex:billed"),
        (
            "exact".to_string(),
            "provider_billed".to_string(),
            Some(7.0)
        )
    );
}
