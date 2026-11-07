//! Pulse analytics database: SQLite schema and migrations, session history writes and analytics
//! queries.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result, bail};
use chrono::{Datelike, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use tracing::{debug, warn};

use cc_discord_presence::cost;
use cc_discord_presence::provider::Provider;

static DB: OnceLock<Arc<Mutex<Connection>>> = OnceLock::new();
static WRITES_SINCE_CHECKPOINT: AtomicUsize = AtomicUsize::new(0);

const SCHEMA_VERSION: i64 = 7;

fn context_label_tokens(label: &str) -> Option<i64> {
    let normalized = label
        .trim()
        .replace([',', '_', ' '], "")
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }

    let (number, multiplier) = if let Some(number) = normalized.strip_suffix('k') {
        (number, 1_000.0)
    } else if let Some(number) = normalized.strip_suffix('m') {
        (number, 1_000_000.0)
    } else {
        (normalized.as_str(), 1.0)
    };
    let value = number.parse::<f64>().ok()?;
    let tokens = (value * multiplier).round();
    if !tokens.is_finite() || tokens < 1.0 || tokens > i64::MAX as f64 {
        return None;
    }
    Some(tokens as i64)
}

fn session_window_tokens(s: &super::commands::SessionInfo) -> i64 {
    if s.context_window_tokens > 0 {
        return s.context_window_tokens.min(i64::MAX as u64) as i64;
    }
    if matches!(s.provider.as_str(), "opencode" | "commandcode" | "orion") {
        return 0;
    }
    if cost::is_ga_1m_context(&s.model_id) {
        return 1_000_000;
    }
    context_label_tokens(&s.context_window).unwrap_or(0)
}

fn session_used_tokens(s: &super::commands::SessionInfo) -> i64 {
    let window = session_window_tokens(s).max(0) as u64;
    s.context_used_tokens.min(window).min(i64::MAX as u64) as i64
}

fn db_path() -> PathBuf {
    #[cfg(test)]
    {
        static TEST_STORAGE: OnceLock<PathBuf> = OnceLock::new();
        TEST_STORAGE
            .get_or_init(test_storage_dir)
            .join("pulse-analytics.db")
    }
    #[cfg(not(test))]
    cc_discord_presence::storage::database_path()
}

/// One folder per test process. Statics are never dropped, so a process cannot
/// remove its own folder; each new one removes those of exited processes.
#[cfg(test)]
fn test_storage_dir() -> PathBuf {
    let parent = std::env::temp_dir().join("pulse-unit-tests");
    let own = std::process::id();
    for entry in std::fs::read_dir(&parent).into_iter().flatten().flatten() {
        let pid = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok());
        if pid.is_some_and(|pid| pid != own && !cc_discord_presence::process_list::is_alive(pid)) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    let dir = parent.join(own.to_string());
    std::fs::create_dir_all(&dir).expect("isolated analytics storage");
    dir
}

fn active_provider() -> Provider {
    cc_discord_presence::provider::load_active_provider()
}

fn active_provider_slug() -> &'static str {
    active_provider().as_str()
}

fn analytics_provider_scope(provider: Option<&str>) -> String {
    match provider.map(str::trim).filter(|value| !value.is_empty()) {
        Some(provider) => provider.to_ascii_lowercase(),
        None => active_provider_slug().to_string(),
    }
}

fn storage_session_id(provider: &str, session_id: &str) -> String {
    format!("{provider}:{session_id}")
}

fn migration_backup_path(path: &Path) -> PathBuf {
    let mut file_name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new("pulse-analytics.db"))
        .to_os_string();
    file_name.push(format!(".pre-v{SCHEMA_VERSION}.bak"));
    path.with_file_name(file_name)
}

fn schema_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
}

fn has_user_schema(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master
            WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
        )",
        [],
        |row| row.get(0),
    )
}

fn validate_database(path: &Path, expected_version: i64) -> Result<()> {
    let backup = Connection::open(path)
        .with_context(|| format!("failed to open migration backup {}", path.display()))?;
    let result: String = backup
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .with_context(|| format!("failed to validate migration backup {}", path.display()))?;
    if result != "ok" {
        bail!(
            "migration backup {} failed quick_check: {result}",
            path.display()
        );
    }
    let actual_version = schema_version(&backup)?;
    if actual_version != expected_version {
        bail!(
            "migration backup {} has schema {actual_version}, expected {expected_version}",
            path.display()
        );
    }
    Ok(())
}

fn create_migration_backup(conn: &Connection, path: &Path) -> Result<Option<PathBuf>> {
    let source_version = schema_version(conn)?;
    if source_version >= SCHEMA_VERSION || !has_user_schema(conn)? {
        return Ok(None);
    }

    let backup_path = migration_backup_path(path);
    if !backup_path.exists() {
        let backup_file = backup_path.to_str().with_context(|| {
            format!(
                "backup path is not valid Unicode: {}",
                backup_path.display()
            )
        })?;
        conn.execute("VACUUM INTO ?1", params![backup_file])
            .with_context(|| {
                format!(
                    "failed to create pre-v5 migration backup {}",
                    backup_path.display()
                )
            })?;
    }
    validate_database(&backup_path, source_version)?;
    Ok(Some(backup_path))
}

fn open_database(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create database directory {}", parent.display()))?;
    }
    let existed = path.exists();
    let conn = Connection::open(path)
        .with_context(|| format!("failed to open Pulse database {}", path.display()))?;
    if existed {
        create_migration_backup(&conn, path)?;
    }
    init_schema(&conn)?;
    Ok(conn)
}

fn db() -> &'static Arc<Mutex<Connection>> {
    DB.get_or_init(|| {
        let path = db_path();
        let conn = open_database(&path).expect("failed to initialize pulse-analytics.db");
        Arc::new(Mutex::new(conn))
    })
}

pub(crate) fn with_connection<T>(operation: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    let connection = db()
        .lock()
        .map_err(|_| anyhow::anyhow!("Analytics database is unavailable"))?;
    operation(&connection)
}

fn ensure_column(
    transaction: &Transaction<'_>,
    table: &str,
    column: &str,
    definition: &str,
) -> rusqlite::Result<()> {
    let sql = format!("SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1)");
    let exists: bool = transaction.query_row(&sql, params![column], |row| row.get(0))?;
    if !exists {
        transaction.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition}"
        ))?;
    }
    Ok(())
}

fn backfill_context_windows(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    let missing_windows = {
        let mut statement = transaction.prepare(
            "SELECT id, context_window FROM sessions
             WHERE window_tokens IS NULL OR window_tokens <= 0",
        )?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, label) in missing_windows {
        if let Some(tokens) = context_label_tokens(&label) {
            transaction.execute(
                "UPDATE sessions SET window_tokens = ?1 WHERE id = ?2",
                params![tokens, id],
            )?;
        }
    }
    Ok(())
}

fn migrate_schema(conn: &Connection) -> Result<()> {
    let previous_version = schema_version(conn)?;
    if previous_version > SCHEMA_VERSION {
        bail!(
            "Pulse database schema {previous_version} is newer than supported schema {SCHEMA_VERSION}"
        );
    }

    let transaction = conn.unchecked_transaction()?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS accounts (
            id TEXT PRIMARY KEY,
            provider TEXT NOT NULL,
            origin TEXT NOT NULL,
            source_key TEXT NOT NULL UNIQUE,
            source_path TEXT NOT NULL,
            label TEXT NOT NULL DEFAULT '',
            removed INTEGER NOT NULL DEFAULT 0 CHECK(removed IN (0,1)),
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS account_snapshots (
            account_id TEXT PRIMARY KEY REFERENCES accounts(id),
            payload TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );",
    )?;
    transaction.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            provider TEXT NOT NULL DEFAULT 'claude',
            session_name TEXT DEFAULT NULL,
            project TEXT NOT NULL,
            model TEXT NOT NULL,
            model_id TEXT DEFAULT '',
            context_window TEXT DEFAULT 'Unknown',
            branch TEXT,
            effort TEXT DEFAULT 'Medium',
            speed TEXT NOT NULL DEFAULT 'unknown',
            speed_source TEXT NOT NULL DEFAULT 'unknown',
            speed_known INTEGER NOT NULL DEFAULT 0 CHECK (speed_known IN (0, 1)),
            started_at TEXT,
            created_at TEXT,
            ended_at TEXT,
            duration_secs INTEGER DEFAULT 0,
            total_cost REAL DEFAULT 0,
            cost_status TEXT NOT NULL DEFAULT 'unavailable'
                CHECK (cost_status IN ('exact', 'partial', 'unavailable')),
            cost_source TEXT NOT NULL DEFAULT 'unknown',
            known_cost REAL DEFAULT NULL CHECK (known_cost IS NULL OR known_cost >= 0),
            cached_input_savings REAL DEFAULT NULL
                CHECK (cached_input_savings IS NULL OR cached_input_savings >= 0),
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
            window_tokens INTEGER DEFAULT 0,
            context_source TEXT NOT NULL DEFAULT 'unknown',
            context_raw_source TEXT NOT NULL DEFAULT 'unknown',
            raw_window_tokens INTEGER NOT NULL DEFAULT 0
                CHECK (raw_window_tokens >= 0),
            effective_context_percent INTEGER DEFAULT NULL
                CHECK (
                    effective_context_percent IS NULL
                    OR effective_context_percent BETWEEN 1 AND 100
                )
        );

        CREATE TABLE IF NOT EXISTS daily_stats (
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

        CREATE TABLE IF NOT EXISTS budget_config (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            monthly_budget REAL DEFAULT 0,
            alert_threshold_pct REAL DEFAULT 80,
            updated_at TEXT NOT NULL DEFAULT '1970-01-01'
        );
        ",
    )?;

    for (column, definition) in [
        ("provider", "TEXT DEFAULT 'claude'"),
        ("session_name", "TEXT DEFAULT NULL"),
        ("opencode_json", "TEXT DEFAULT NULL"),
        ("created_at", "TEXT DEFAULT NULL"),
        ("used_tokens", "INTEGER DEFAULT 0"),
        ("window_tokens", "INTEGER DEFAULT 0"),
        ("speed", "TEXT NOT NULL DEFAULT 'unknown'"),
        ("speed_source", "TEXT NOT NULL DEFAULT 'unknown'"),
        (
            "speed_known",
            "INTEGER NOT NULL DEFAULT 0 CHECK (speed_known IN (0, 1))",
        ),
        (
            "cost_status",
            "TEXT NOT NULL DEFAULT 'unavailable' CHECK (cost_status IN ('exact', 'partial', 'unavailable'))",
        ),
        ("cost_source", "TEXT NOT NULL DEFAULT 'unknown'"),
        (
            "known_cost",
            "REAL DEFAULT NULL CHECK (known_cost IS NULL OR known_cost >= 0)",
        ),
        (
            "cached_input_savings",
            "REAL DEFAULT NULL CHECK (cached_input_savings IS NULL OR cached_input_savings >= 0)",
        ),
        ("context_source", "TEXT NOT NULL DEFAULT 'unknown'"),
        ("context_raw_source", "TEXT NOT NULL DEFAULT 'unknown'"),
        (
            "raw_window_tokens",
            "INTEGER NOT NULL DEFAULT 0 CHECK (raw_window_tokens >= 0)",
        ),
        (
            "effective_context_percent",
            "INTEGER DEFAULT NULL CHECK (effective_context_percent IS NULL OR effective_context_percent BETWEEN 1 AND 100)",
        ),
    ] {
        ensure_column(&transaction, "sessions", column, definition)?;
    }
    ensure_column(
        &transaction,
        "daily_stats",
        "provider",
        "TEXT DEFAULT 'claude'",
    )?;

    backfill_context_windows(&transaction)?;
    // These provenance repairs belong to the v4 migration. A v4 -> v5 upgrade
    // only adds the provider/history index and must not relabel valid rows.
    if previous_version < 4 {
        transaction.execute(
            "UPDATE sessions
             SET provider = 'claude'
             WHERE provider IS NULL OR trim(provider) = ''",
            [],
        )?;
        transaction.execute(
            "UPDATE sessions
             SET input_tokens = MAX(total_tokens - output_tokens, 0)
             WHERE lower(provider) = 'codex'
               AND total_tokens > 0
               AND input_tokens > MAX(total_tokens - output_tokens, 0)",
            [],
        )?;
        transaction.execute(
            "UPDATE sessions
             SET created_at = COALESCE(created_at, started_at, updated_at)
             WHERE created_at IS NULL",
            [],
        )?;
        transaction.execute(
            "UPDATE sessions
             SET started_at = updated_at
             WHERE started_at IS NOT NULL
               AND instr(started_at, 'T') = 0
               AND updated_at IS NOT NULL",
            [],
        )?;
        transaction.execute(
            "UPDATE sessions
             SET started_at = COALESCE(started_at, created_at, updated_at)
             WHERE started_at IS NULL",
            [],
        )?;
        transaction.execute(
            "UPDATE sessions
             SET speed = 'unknown',
                 speed_source = 'legacy',
                 speed_known = 0,
                 cost_status = 'unavailable',
                 cost_source = 'legacy',
                 known_cost = NULL,
                 cached_input_savings = NULL,
                 context_source = 'legacy',
                 context_raw_source = 'unknown',
                 raw_window_tokens = 0,
                 effective_context_percent = NULL
             WHERE lower(provider) = 'codex'",
            [],
        )?;
        transaction.execute(
            "UPDATE sessions
             SET cost_status = 'exact',
                 cost_source = 'legacy-calculated',
                 known_cost = MAX(total_cost, 0),
                 context_source = CASE
                     WHEN window_tokens > 0 THEN 'legacy'
                     ELSE context_source
                 END
             WHERE lower(provider) = 'claude'",
            [],
        )?;
    }
    transaction.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project);
        CREATE INDEX IF NOT EXISTS idx_sessions_started ON sessions(started_at);
        CREATE INDEX IF NOT EXISTS idx_sessions_active ON sessions(is_active);
        CREATE INDEX IF NOT EXISTS idx_sessions_model ON sessions(model);
        CREATE INDEX IF NOT EXISTS idx_sessions_provider ON sessions(provider);
        CREATE INDEX IF NOT EXISTS idx_sessions_provider_active ON sessions(provider, is_active);
        CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at);
        CREATE INDEX IF NOT EXISTS idx_sessions_created ON sessions(created_at);
        CREATE INDEX IF NOT EXISTS idx_sessions_history_ts
            ON sessions(COALESCE(started_at, created_at, updated_at));
        CREATE INDEX IF NOT EXISTS idx_sessions_provider_history_ts
            ON sessions(provider, COALESCE(started_at, created_at, updated_at) DESC);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_daily_stats_provider_key
            ON daily_stats(provider, date, project, model);

        CREATE VIRTUAL TABLE IF NOT EXISTS sessions_fts USING fts5(
            project, model, branch,
            content='sessions',
            content_rowid='rowid',
            tokenize='porter unicode61'
        );

        CREATE TRIGGER IF NOT EXISTS sessions_ai AFTER INSERT ON sessions BEGIN
            INSERT INTO sessions_fts(rowid, project, model, branch)
            VALUES (new.rowid, new.project, new.model, COALESCE(new.branch, ''));
        END;

        CREATE TRIGGER IF NOT EXISTS sessions_au AFTER UPDATE ON sessions BEGIN
            DELETE FROM sessions_fts WHERE rowid = old.rowid;
            INSERT INTO sessions_fts(rowid, project, model, branch)
            VALUES (new.rowid, new.project, new.model, COALESCE(new.branch, ''));
        END;

        ",
    )?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA cache_size = -8000;
        ",
    )
    .context("failed to configure pulse-analytics database")?;
    migrate_schema(conn)?;
    debug!("Pulse analytics DB initialized at {}", db_path().display());
    Ok(())
}

#[derive(Debug, Serialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CostBasis {
    Exact,
    Partial,
    /// Cost was not billed by the provider (subscription usage) but is
    /// reconstructed from real token counts x published per-model API rates.
    /// An API-equivalent estimate, never provider-billed spend.
    Estimated,
    #[default]
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MonetaryProvenance {
    ProviderBilled,
    ApiEquivalent,
    Other,
}

/// Classifies a monetary source without treating every non-billing value as an
/// API-equivalent estimate. This is the shared truth boundary for forecasts,
/// budgets, reports, and rolling totals.
pub(crate) fn monetary_provenance(source: &str) -> MonetaryProvenance {
    let normalized = source.trim().to_ascii_lowercase().replace('-', "_");
    if normalized == "provider_billed" {
        MonetaryProvenance::ProviderBilled
    } else if normalized == "api_equivalent"
        || normalized.ends_with("_api_equivalent")
        || matches!(
            normalized.as_str(),
            "session_calculated" | "legacy_calculated" | "live_session"
        )
        || normalized.contains("pricing")
    {
        MonetaryProvenance::ApiEquivalent
    } else {
        MonetaryProvenance::Other
    }
}

impl CostBasis {
    fn from_storage(status: &str, source: &str, known_cost: Option<f64>) -> Self {
        if known_cost.is_none() {
            return Self::Unavailable;
        }
        match status {
            "exact" if monetary_provenance(source) == MonetaryProvenance::ApiEquivalent => {
                Self::Estimated
            }
            "exact" => Self::Exact,
            "partial" => Self::Partial,
            _ => Self::Unavailable,
        }
    }
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct HistoricalSession {
    pub opencode: Option<cc_discord_presence::opencode::Metadata>,
    pub id: String,
    pub provider: String,
    pub session_name: Option<String>,
    pub project: String,
    pub model: String,
    pub model_id: String,
    pub context_window: String,
    pub branch: Option<String>,
    pub effort: String,
    pub speed: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub duration_secs: i64,
    pub total_cost: f64,
    pub cost_basis: CostBasis,
    pub cost_source: String,
    pub known_cost: Option<f64>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_write_tokens: i64,
    pub cache_read_tokens: i64,
    pub total_tokens: i64,
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_write_cost: f64,
    pub cache_read_cost: f64,
    pub has_thinking: bool,
    pub subagent_count: i64,
    pub is_active: bool,
    pub used_tokens: i64,
    pub window_tokens: i64,
}

fn history_timestamp_expr() -> &'static str {
    "COALESCE(started_at, created_at, updated_at)"
}

/// API-equivalent cost reconstructed from real token counts x published
/// per-model API rates. Used for subscription sessions the provider never
/// billed per-token. It is an estimate, never provider-billed spend, and is
/// `None` whenever the model has no resolvable published rate (we never invent
/// a rate to force a number).
pub(crate) struct EstimatedCost {
    pub total: f64,
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_write_cost: f64,
    pub cache_read_cost: f64,
}

/// True when a model id belongs to the OpenAI/Codex family (priced from the
/// bundled model catalog) rather than the Claude family (priced from
/// `cc_discord_presence::cost`).
fn is_codex_family(provider: &str, model_id: &str) -> bool {
    let p = provider.trim().to_lowercase();
    if p == "codex" || p == "openai" {
        return true;
    }
    let m = model_id.trim().to_lowercase();
    m.starts_with("gpt") || m.contains("codex")
}

/// Reconstruct API-equivalent spend for one session. DB token columns follow
/// the accumulator contract where `input_tokens` already includes cache write
/// and cache read, so pure (non-cached) input is `input - cache_write -
/// cache_read`.
#[cfg(test)]
pub(crate) fn estimate_api_equivalent_cost(
    provider: &str,
    model_id: &str,
    input_tokens: i64,
    output_tokens: i64,
    cache_write_tokens: i64,
    cache_read_tokens: i64,
) -> Option<EstimatedCost> {
    if provider == "opencode" {
        return None;
    }
    estimate_api_equivalent_cost_with_speed(
        provider,
        model_id,
        input_tokens,
        output_tokens,
        cache_write_tokens,
        cache_read_tokens,
        "standard",
    )
}

fn estimate_api_equivalent_cost_with_speed(
    provider: &str,
    model_id: &str,
    input_tokens: i64,
    output_tokens: i64,
    cache_write_tokens: i64,
    cache_read_tokens: i64,
    speed: &str,
) -> Option<EstimatedCost> {
    // OpenCode reports its own cost. Orion prices every request when it reads
    // the session store, so a stored row without a cost means no model in it has
    // a published rate. Pricing the totals here would send an unknown model id
    // through the default Claude tier and invent a rate.
    if provider.eq_ignore_ascii_case("opencode") || provider.eq_ignore_ascii_case("orion") {
        return None;
    }
    let model_id = model_id.trim();
    if model_id.is_empty() {
        return None;
    }
    let input = input_tokens.max(0) as u64;
    let output = output_tokens.max(0) as u64;
    let cache_write = cache_write_tokens.max(0) as u64;
    let cache_read = cache_read_tokens.max(0) as u64;
    if input == 0 && output == 0 {
        return None;
    }
    let pure_input = input.saturating_sub(cache_write).saturating_sub(cache_read);

    if is_codex_family(provider, model_id) {
        use cc_discord_presence::codex::config::PricingConfig;
        use cc_discord_presence::codex::cost::{TokenUsage, compute_cost};
        use cc_discord_presence::codex::model::{SessionSpeed, SpeedMode, SpeedSource};

        // `compute_cost` bills `input_tokens - cached_input_tokens` at the input
        // rate, so pass pure+cache_read as input and cache_read as cached to
        // avoid double-charging cache write, which is billed on its own line.
        let usage = TokenUsage {
            input_tokens: pure_input + cache_read,
            cached_input_tokens: cache_read,
            cache_write_tokens: Some(cache_write),
            output_tokens: output,
        };
        let speed_mode = if speed.trim().eq_ignore_ascii_case("fast") {
            SpeedMode::Fast
        } else {
            SpeedMode::Standard
        };
        let computed = compute_cost(
            model_id,
            usage,
            SessionSpeed::explicit(speed_mode, SpeedSource::LegacyDefault),
            &PricingConfig::default(),
        );
        // Session totals cannot show which requests crossed a long-context
        // threshold, so a model with long-context rates is only estimated when
        // every component is priced. An unknown speed only matters when Fast
        // changes the price (GPT-5.6 has no published Fast multiplier).
        let has_long_context_rates = cc_discord_presence::codex::model::resolve_model(model_id)
            .and_then(|model| model.long_context_pricing())
            .is_some();
        let fast_changes_price =
            cc_discord_presence::codex::cost::speed_multiplier(model_id, true) != 1.0;
        if has_long_context_rates
            && (computed.status != cc_discord_presence::codex::cost::PricingStatus::Exact
                || (fast_changes_price && !matches!(speed, "standard" | "fast")))
        {
            return None;
        }
        let total = computed.known_total_cost_usd?;
        if !total.is_finite() || total < 0.0 {
            return None;
        }
        Some(EstimatedCost {
            total,
            input_cost: computed.breakdown.input_cost_usd,
            output_cost: computed.breakdown.output_cost_usd,
            cache_write_cost: computed.breakdown.cache_write_cost_usd,
            cache_read_cost: computed.breakdown.cached_input_cost_usd,
        })
    } else {
        // Totals cannot show which requests crossed a prompt-length price tier
        // (Haiku 5.5, 100,000 tokens). At or below the threshold every request
        // is below it too, so the low row is exact; above it nothing is stored.
        if cost::has_prompt_length_tiers(model_id) && input > cost::HAIKU_5_5_LONG_PROMPT_THRESHOLD
        {
            return None;
        }
        // Totals carry no cache-lifetime split, so every write is a five-minute write.
        let b = cost::calculate_category_costs(
            model_id,
            pure_input,
            output,
            cache_write,
            cache_read,
            speed.trim().eq_ignore_ascii_case("fast"),
        );
        let total = b.total();
        if !total.is_finite() || total <= 0.0 {
            return None;
        }
        Some(EstimatedCost {
            total,
            input_cost: b.input_cost,
            output_cost: b.output_cost,
            cache_write_cost: b.cache_write_cost,
            cache_read_cost: b.cache_read_cost,
        })
    }
}

/// Fills in an API-equivalent estimate for any session the provider left
/// unpriced but that still carries token counts and a resolvable model. Exact
/// and provider-billed sessions are left untouched.
fn apply_api_equivalent_estimates(sessions: &mut [HistoricalSession]) {
    for s in sessions.iter_mut() {
        if s.cost_basis != CostBasis::Unavailable || s.known_cost.is_some() {
            continue;
        }
        let model = if s.model_id.trim().is_empty() {
            s.model.as_str()
        } else {
            s.model_id.as_str()
        };
        if let Some(est) = estimate_api_equivalent_cost_with_speed(
            &s.provider,
            model,
            s.input_tokens,
            s.output_tokens,
            s.cache_write_tokens,
            s.cache_read_tokens,
            &s.speed,
        ) {
            s.total_cost = est.total;
            s.known_cost = Some(est.total);
            s.cost_basis = CostBasis::Estimated;
            s.cost_source = "api_equivalent".to_string();
            s.input_cost = est.input_cost;
            s.output_cost = est.output_cost;
            s.cache_write_cost = est.cache_write_cost;
            s.cache_read_cost = est.cache_read_cost;
        }
    }
}

fn provider_history_inventory(conn: &Connection, days: Option<i64>) -> BTreeMap<String, u64> {
    let history_ts = history_timestamp_expr();
    let sql = format!(
        "SELECT lower(provider), COUNT(*)
         FROM sessions
         WHERE trim(provider) <> ''
           AND (?1 IS NULL OR COALESCE({history_ts}, datetime('now')) >= ?1)
         GROUP BY lower(provider)
         ORDER BY lower(provider)"
    );
    let cutoff = days.map(|days| (Utc::now() - chrono::Duration::days(days)).to_rfc3339());

    let Ok(mut stmt) = conn.prepare(&sql) else {
        return BTreeMap::new();
    };
    let rows = stmt.query_map(params![cutoff], |row| {
        let sessions = u64::try_from(row.get::<_, i64>(1)?).unwrap_or_default();
        Ok((row.get::<_, String>(0)?, sessions))
    });

    rows.ok()
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

pub fn get_provider_history_inventory(days: Option<i64>) -> BTreeMap<String, u64> {
    let Ok(conn) = db().lock() else {
        return BTreeMap::new();
    };
    provider_history_inventory(&conn, days)
}

#[expect(
    clippy::too_many_arguments,
    reason = "every filter is an independent optional SQL predicate"
)]
fn query_sessions(
    conn: &Connection,
    provider: &str,
    days: Option<i64>,
    from_iso: Option<&str>,
    to_iso: Option<&str>,
    project: Option<&str>,
    model: Option<&str>,
    min_cost: Option<f64>,
    max_cost: Option<f64>,
    start_hour: Option<i64>,
    end_hour: Option<i64>,
    limit: Option<i64>,
) -> Vec<HistoricalSession> {
    let history_ts = history_timestamp_expr();
    let mut sql = String::from(
        "SELECT id, provider, session_name, project, model, model_id, context_window, branch, effort,
            started_at, ended_at, duration_secs, COALESCE(known_cost, 0), cost_status, cost_source, known_cost,
            input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, total_tokens,
            input_cost, output_cost, cache_write_cost, cache_read_cost,
            has_thinking, subagent_count, is_active, used_tokens, window_tokens, speed, opencode_json
         FROM sessions
         WHERE (?1 = 'all' OR provider = ?1)",
    );
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(provider.to_string())];
    let mut param_idx = 2;

    if let Some(d) = days {
        let cutoff = (Utc::now() - chrono::Duration::days(d)).to_rfc3339();
        sql.push_str(&format!(
            " AND COALESCE({history_ts}, datetime('now')) >= ?{param_idx}"
        ));
        params_vec.push(Box::new(cutoff));
        param_idx += 1;
    }

    if let Some(from_iso) = from_iso {
        sql.push_str(&format!(
            " AND COALESCE({history_ts}, datetime('now')) >= ?{param_idx}"
        ));
        params_vec.push(Box::new(from_iso.to_string()));
        param_idx += 1;
    }

    if let Some(to_iso) = to_iso {
        sql.push_str(&format!(
            " AND COALESCE({history_ts}, datetime('now')) <= ?{param_idx}"
        ));
        params_vec.push(Box::new(to_iso.to_string()));
        param_idx += 1;
    }

    if let Some(p) = project {
        sql.push_str(&format!(" AND project = ?{param_idx}"));
        params_vec.push(Box::new(p.to_string()));
        param_idx += 1;
    }

    if let Some(m) = model {
        sql.push_str(&format!(" AND model = ?{param_idx}"));
        params_vec.push(Box::new(m.to_string()));
        param_idx += 1;
    }

    if let Some(start_hour) = start_hour {
        sql.push_str(&format!(
            " AND CAST(strftime('%H', COALESCE({history_ts}, ''), 'localtime') AS INTEGER) >= ?{param_idx}"
        ));
        params_vec.push(Box::new(start_hour));
        param_idx += 1;
    }

    if let Some(end_hour) = end_hour {
        sql.push_str(&format!(
            " AND CAST(strftime('%H', COALESCE({history_ts}, ''), 'localtime') AS INTEGER) <= ?{param_idx}"
        ));
        params_vec.push(Box::new(end_hour));
        param_idx += 1;
    }

    sql.push_str(&format!(
        " ORDER BY COALESCE({history_ts}, datetime('now')) DESC, updated_at DESC"
    ));

    // `None` means "every row in the window", which is what aggregate callers
    // need. Defaulting it to 100 silently truncated totals: a 299-session
    // window reported the cost of its 100 newest sessions. SQLite treats a
    // negative LIMIT as unlimited.
    let lim = if min_cost.is_some() || max_cost.is_some() {
        -1
    } else {
        limit.unwrap_or(-1)
    };
    sql.push_str(&format!(" LIMIT ?{param_idx}"));
    params_vec.push(Box::new(lim));

    let refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to prepare history query: {e}");
            return vec![];
        }
    };

    let rows = stmt
        .query_map(refs.as_slice(), |row| {
            Ok(HistoricalSession {
                opencode: row
                    .get::<_, Option<String>>(31)?
                    .and_then(|raw| serde_json::from_str(&raw).ok()),
                id: row.get(0)?,
                provider: row.get(1)?,
                session_name: row.get(2)?,
                project: row.get(3)?,
                model: row.get(4)?,
                model_id: row.get(5)?,
                context_window: row.get(6)?,
                branch: row.get(7)?,
                effort: row.get(8)?,
                speed: row.get(30)?,
                started_at: row.get(9)?,
                ended_at: row.get(10)?,
                duration_secs: row.get(11)?,
                total_cost: row.get(12)?,
                cost_basis: CostBasis::from_storage(
                    row.get::<_, String>(13)?.as_str(),
                    row.get::<_, String>(14)?.as_str(),
                    row.get(15)?,
                ),
                cost_source: row.get(14)?,
                known_cost: row.get(15)?,
                input_tokens: row.get(16)?,
                output_tokens: row.get(17)?,
                cache_write_tokens: row.get(18)?,
                cache_read_tokens: row.get(19)?,
                total_tokens: row.get(20)?,
                input_cost: row.get(21)?,
                output_cost: row.get(22)?,
                cache_write_cost: row.get(23)?,
                cache_read_cost: row.get(24)?,
                has_thinking: row.get::<_, i32>(25)? != 0,
                subagent_count: row.get(26)?,
                is_active: row.get::<_, i32>(27)? != 0,
                used_tokens: row.get(28)?,
                window_tokens: row.get(29)?,
            })
        })
        .ok();

    let mut sessions: Vec<HistoricalSession> = rows
        .map(|r| r.filter_map(|x| x.ok()).collect())
        .unwrap_or_default();
    // Subscription sessions arrive unpriced; reconstruct API-equivalent spend
    // from real tokens x published rates so cost views are complete instead of
    // blank. Provider-billed and exact rows are left untouched.
    apply_api_equivalent_estimates(&mut sessions);
    sessions.retain(|session| {
        let cost = session.known_cost;
        min_cost.is_none_or(|minimum| cost.is_some_and(|value| value >= minimum))
            && max_cost.is_none_or(|maximum| cost.is_some_and(|value| value <= maximum))
    });
    if let Some(limit) = limit {
        sessions.truncate(limit.max(0) as usize);
    }
    sessions
}

#[derive(Debug, Serialize, Clone)]
pub struct DailyStat {
    pub date: String,
    pub project: String,
    pub model: String,
    pub session_count: i64,
    pub priced_sessions: i64,
    pub cost_basis: CostBasis,
    pub cost_sources: Vec<String>,
    pub total_cost: f64,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_write_tokens: i64,
    pub cache_read_tokens: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct ProjectStat {
    pub project: String,
    pub session_count: i64,
    pub priced_sessions: i64,
    pub cost_basis: CostBasis,
    pub cost_sources: Vec<String>,
    pub total_cost: f64,
    pub total_tokens: i64,
    pub avg_session_cost: f64,
    pub avg_duration_secs: f64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub top_model: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct HourlyActivity {
    pub hour: i64,
    pub session_count: i64,
    pub priced_sessions: i64,
    pub cost_basis: CostBasis,
    pub cost_sources: Vec<String>,
    pub total_cost: f64,
}

#[derive(Debug, Serialize, Clone)]
pub struct ModelStat {
    pub model: String,
    pub session_count: i64,
    pub priced_sessions: i64,
    pub cost_basis: CostBasis,
    pub cost_sources: Vec<String>,
    pub total_cost: f64,
}

#[derive(Debug, Serialize, Clone)]
pub struct CostForecast {
    pub billed_spend_usd: Option<f64>,
    pub daily_billed_spend_usd: Option<f64>,
    pub projected_billed_spend_usd: Option<f64>,
    pub api_equivalent_usd: Option<f64>,
    pub daily_api_equivalent_usd: Option<f64>,
    pub projected_api_equivalent_usd: Option<f64>,
    pub days_elapsed: i64,
    pub days_in_month: i64,
    pub cost_basis: CostBasis,
    pub cost_sources: Vec<String>,
    pub sessions: usize,
    pub priced_sessions: usize,
    pub billed_sessions: usize,
    pub api_equivalent_sessions: usize,
    pub refreshed_at: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct BudgetStatus {
    pub monthly_budget: f64,
    pub alert_threshold_pct: f64,
    pub billed_spend_usd: Option<f64>,
    pub projected_billed_spend_usd: Option<f64>,
    pub api_equivalent_usd: Option<f64>,
    pub projected_api_equivalent_usd: Option<f64>,
    pub pct_used: Option<f64>,
    pub over_budget: bool,
    pub cost_basis: CostBasis,
    pub cost_sources: Vec<String>,
    pub sessions: usize,
    pub priced_sessions: usize,
    pub billed_sessions: usize,
    pub api_equivalent_sessions: usize,
    pub refreshed_at: String,
}

#[derive(Debug)]
pub struct DashboardDataSnapshot {
    pub summary: AnalyticsSummary,
    pub sessions: Vec<HistoricalSession>,
    pub forecast: CostForecast,
    pub hourly_activity: Vec<HourlyActivity>,
}

#[derive(Debug)]
pub struct CostsDataSnapshot {
    pub history: Vec<HistoricalSession>,
    pub aggregate_sessions: Vec<HistoricalSession>,
    pub forecast: CostForecast,
    pub budget: BudgetStatus,
    pub daily_usage: Vec<DailyStat>,
}

fn bounded_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn nonnegative_finite(value: f64) -> f64 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CostCoverage {
    pub cost_basis: CostBasis,
    pub cost_sources: Vec<String>,
    pub sessions: usize,
    pub priced_sessions: usize,
}

pub(crate) fn summarize_cost_provenance<'a, I>(observations: I) -> CostCoverage
where
    I: IntoIterator<Item = (CostBasis, &'a str, Option<f64>)>,
{
    let mut sources = BTreeSet::new();
    let mut sessions = 0usize;
    let mut priced_sessions = 0usize;
    let mut exact = 0usize;
    let mut partial = 0usize;
    let mut estimated = 0usize;

    for (basis, source, known_cost) in observations {
        sessions += 1;
        if !known_cost.is_some_and(|cost| cost.is_finite() && cost >= 0.0)
            || basis == CostBasis::Unavailable
        {
            continue;
        }
        priced_sessions += 1;
        match basis {
            CostBasis::Exact => exact += 1,
            CostBasis::Partial => partial += 1,
            CostBasis::Estimated => estimated += 1,
            CostBasis::Unavailable => {}
        }
        let source = source.trim();
        if !source.is_empty() && source != "unknown" {
            sources.insert(source.to_string());
        }
    }

    // An API-equivalent estimate is weaker than provider-billed exact cost but
    // stronger than nothing. Any estimate in the mix downgrades the aggregate to
    // Estimated so the UI never presents reconstructed spend as billed. A true
    // partial (a provider-billed lower bound) still takes precedence as the
    // most conservative "known" label.
    let cost_basis = if priced_sessions == 0 {
        CostBasis::Unavailable
    } else if partial > 0 || priced_sessions != sessions {
        // Any unpriced session, or a provider-billed lower bound, keeps the
        // aggregate at Partial: coverage is genuinely incomplete.
        CostBasis::Partial
    } else if estimated > 0 {
        // Fully covered, but at least one figure is an API-equivalent estimate
        // rather than provider-billed spend.
        CostBasis::Estimated
    } else if exact == priced_sessions {
        CostBasis::Exact
    } else {
        CostBasis::Partial
    };

    CostCoverage {
        cost_basis,
        cost_sources: sources.into_iter().collect(),
        sessions,
        priced_sessions,
    }
}

fn coverage_from_sql(
    sessions: i64,
    exact: i64,
    partial: i64,
    provider_billed: i64,
    estimated: i64,
    sources: Option<String>,
) -> CostCoverage {
    let priced_sessions = (exact + partial + provider_billed + estimated).max(0) as usize;
    let sessions = sessions.max(0) as usize;
    let cost_basis = if priced_sessions == 0 {
        CostBasis::Unavailable
    } else if priced_sessions != sessions || partial > 0 {
        CostBasis::Partial
    } else if estimated > 0 {
        CostBasis::Estimated
    } else if (exact + provider_billed) as usize == priced_sessions {
        CostBasis::Exact
    } else {
        CostBasis::Partial
    };
    let cost_sources = sources
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|source| !source.is_empty() && *source != "unknown")
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    CostCoverage {
        cost_basis,
        cost_sources,
        sessions,
        priced_sessions,
    }
}

const COST_COVERAGE_SQL: &str = "
    COALESCE(SUM(CASE
        WHEN known_cost IS NOT NULL
         AND cost_status = 'exact'
         AND lower(replace(cost_source, '-', '_')) NOT IN (
             'provider_billed', 'api_equivalent', 'session_calculated',
             'legacy_calculated', 'live_session'
         )
         AND lower(replace(cost_source, '-', '_')) NOT LIKE '%api_equivalent'
         AND lower(replace(cost_source, '-', '_')) NOT LIKE '%pricing%'
        THEN 1 ELSE 0 END), 0),
    COALESCE(SUM(CASE
        WHEN known_cost IS NOT NULL AND cost_status = 'partial'
        THEN 1 ELSE 0 END), 0),
    COALESCE(SUM(CASE
        WHEN known_cost IS NOT NULL
         AND cost_status = 'exact'
         AND lower(replace(cost_source, '-', '_')) = 'provider_billed'
        THEN 1 ELSE 0 END), 0),
    COALESCE(SUM(CASE
        WHEN known_cost IS NOT NULL
         AND cost_status = 'exact'
         AND (
             lower(replace(cost_source, '-', '_')) IN (
                 'api_equivalent', 'session_calculated',
                 'legacy_calculated', 'live_session'
             )
             OR lower(replace(cost_source, '-', '_')) LIKE '%api_equivalent'
             OR lower(replace(cost_source, '-', '_')) LIKE '%pricing%'
         )
        THEN 1 ELSE 0 END), 0),
    GROUP_CONCAT(DISTINCT CASE
        WHEN known_cost IS NOT NULL
         AND cost_status IN ('exact', 'partial')
         AND trim(cost_source) NOT IN ('', 'unknown')
        THEN cost_source END)";

fn session_cost_provenance(
    session: &super::commands::SessionInfo,
) -> (&'static str, &'static str, Option<f64>) {
    if !session.cost_available {
        return ("unavailable", "unknown", None);
    }
    let cost = nonnegative_finite(session.cost);
    if session.provider == "opencode" {
        return ("exact", "opencode_reported", Some(cost));
    }
    let source = match session.cost_source.as_str() {
        "api_equivalent" => "api_equivalent",
        "codex_reported" => "codex_reported",
        "claude_statusline_api_equivalent" => "claude_statusline_api_equivalent",
        "commandcode_reported" => "commandcode_reported",
        _ => "session-calculated",
    };
    match session.cost_basis.as_str() {
        "exact" => ("exact", source, Some(cost)),
        "estimated" => ("exact", "api_equivalent", Some(cost)),
        "partial" => ("partial", source, Some(cost)),
        "provider_billed" => ("exact", "provider_billed", Some(cost)),
        "opencode_reported" => ("exact", "opencode_reported", Some(cost)),
        _ => ("unavailable", "unknown", None),
    }
}

fn upsert_session_into(
    conn: &Connection,
    s: &super::commands::SessionInfo,
    updated_at: &str,
) -> rusqlite::Result<()> {
    let storage_id = storage_session_id(&s.provider, &s.session_id);
    let speed = s.speed.trim().to_ascii_lowercase();
    let speed_known = matches!(speed.as_str(), "standard" | "fast");
    let persisted_speed = if speed_known {
        speed.as_str()
    } else {
        "unknown"
    };
    let speed_source = if speed_known { "session" } else { "unknown" };
    let (cost_status, cost_source, known_cost) = session_cost_provenance(s);
    let window_tokens = session_window_tokens(s);
    let context_source = if window_tokens > 0 {
        "session"
    } else {
        "unknown"
    };
    conn.execute(
        "INSERT INTO sessions (id, provider, session_name, project, model, model_id, context_window, branch, effort,
            speed, speed_source, speed_known,
            started_at, duration_secs, total_cost, cost_status, cost_source, known_cost, cached_input_savings,
            input_tokens, output_tokens,
            cache_write_tokens, cache_read_tokens, total_tokens,
            input_cost, output_cost, cache_write_cost, cache_read_cost,
            has_thinking, subagent_count, is_active, updated_at, used_tokens, window_tokens,
            context_source, context_raw_source, raw_window_tokens, effective_context_percent)
        VALUES (
            ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,NULL,
             ?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,1,?30,?31,?32,?33,'unknown',0,NULL
        )
        ON CONFLICT(id) DO UPDATE SET
            provider=?2,
            session_name=COALESCE(?3, session_name),
            project=?4, model=?5, model_id=?6, context_window=?7, branch=?8, effort=?9,
            speed=?10, speed_source=?11, speed_known=?12,
            started_at=CASE
                WHEN sessions.started_at IS NULL OR instr(sessions.started_at, 'T') = 0 THEN ?13
                ELSE sessions.started_at
            END,
            ended_at=NULL,
            duration_secs=?14, total_cost=?15,
            cost_status=?16, cost_source=?17, known_cost=?18, cached_input_savings=NULL,
            input_tokens=?19, output_tokens=?20,
            cache_write_tokens=?21, cache_read_tokens=?22, total_tokens=?23,
            input_cost=?24, output_cost=?25, cache_write_cost=?26, cache_read_cost=?27,
            has_thinking=?28, subagent_count=?29, is_active=1, updated_at=?30,
            used_tokens=?31, window_tokens=?32,
            context_source=?33, context_raw_source='unknown', raw_window_tokens=0,
            effective_context_percent=NULL",
        params![
            storage_id,
            s.provider,
            s.session_name,
            s.project,
            s.model,
            s.model_id,
            s.context_window,
            s.branch,
            s.effort,
            persisted_speed,
            speed_source,
            speed_known as i32,
            s.started_at,
            bounded_i64(s.duration_secs),
            nonnegative_finite(s.cost),
            cost_status,
            cost_source,
            known_cost,
            bounded_i64(s.input_tokens),
            bounded_i64(s.output_tokens),
            bounded_i64(s.cache_write_tokens),
            bounded_i64(s.cache_read_tokens),
            bounded_i64(s.tokens),
            nonnegative_finite(s.input_cost),
            nonnegative_finite(s.output_cost),
            nonnegative_finite(s.cache_write_cost),
            nonnegative_finite(s.cache_read_cost),
            s.has_thinking as i32,
            s.subagent_count.min(i32::MAX as usize) as i32,
            updated_at,
            session_used_tokens(s),
            window_tokens,
            context_source,
        ],
    )?;
    if s.provider == "opencode" {
        let metadata = s
            .opencode
            .as_ref()
            .and_then(|value| serde_json::to_string(value).ok());
        conn.execute("UPDATE sessions SET opencode_json=?2, is_active=?3, ended_at=CASE WHEN ?3=0 THEN ?4 ELSE NULL END WHERE id=?1", params![storage_id, metadata, !s.is_idle, updated_at])?;
    }
    conn.execute(
        "UPDATE sessions
         SET created_at = COALESCE(created_at, started_at, updated_at),
             started_at = COALESCE(started_at, created_at, updated_at)
         WHERE id = ?1",
        params![storage_id],
    )?;
    Ok(())
}

/// Values of one stored Claude history row that a transcript re-read can
/// correct.
#[derive(Debug, Clone, PartialEq)]
struct ClaudeHistoryValues {
    input_tokens: i64,
    output_tokens: i64,
    cache_write_tokens: i64,
    cache_read_tokens: i64,
    total_tokens: i64,
    input_cost: f64,
    output_cost: f64,
    cache_write_cost: f64,
    cache_read_cost: f64,
    total_cost: f64,
    cost_status: String,
    cost_source: String,
    known_cost: Option<f64>,
}

impl ClaudeHistoryValues {
    fn differs_from(&self, other: &Self) -> bool {
        let money = |a: f64, b: f64| (a - b).abs() > 1e-9;
        self.input_tokens != other.input_tokens
            || self.output_tokens != other.output_tokens
            || self.cache_write_tokens != other.cache_write_tokens
            || self.cache_read_tokens != other.cache_read_tokens
            || self.total_tokens != other.total_tokens
            || money(self.input_cost, other.input_cost)
            || money(self.output_cost, other.output_cost)
            || money(self.cache_write_cost, other.cache_write_cost)
            || money(self.cache_read_cost, other.cache_read_cost)
            || money(self.total_cost, other.total_cost)
            || self.cost_status != other.cost_status
            || self.cost_source != other.cost_source
            || match (self.known_cost, other.known_cost) {
                (Some(a), Some(b)) => money(a, b),
                (a, b) => a.is_some() != b.is_some(),
            }
    }
}

/// Result of checking stored Claude history against the transcripts.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClaudeHistoryRepairSummary {
    /// Stored Claude sessions examined.
    pub rows_checked: usize,
    /// Rows whose tokens or costs differ from a fresh transcript read.
    pub rows_to_update: usize,
    /// Rows whose transcript no longer exists; they are left unchanged.
    pub rows_without_transcript: usize,
    pub tokens_before: u64,
    pub tokens_after: u64,
    /// Known cost of the rows to update, before and after.
    pub cost_before: f64,
    pub cost_after: f64,
    /// Backup written before the update, when the repair was applied.
    pub backup_path: Option<String>,
    pub applied: bool,
}

type ClaudeHistoryPlan = (
    ClaudeHistoryRepairSummary,
    Vec<(String, ClaudeHistoryValues)>,
);

/// Compares stored Claude rows with sessions re-read from transcripts.
/// Costs that came from Claude's statusline are authoritative, so those rows
/// keep their money columns and only receive corrected token counts.
fn plan_claude_history_repair(
    conn: &Connection,
    infos: &[super::commands::SessionInfo],
) -> rusqlite::Result<ClaudeHistoryPlan> {
    let fresh: HashMap<String, &super::commands::SessionInfo> = infos
        .iter()
        .filter(|info| info.provider == "claude")
        .map(|info| (storage_session_id(&info.provider, &info.session_id), info))
        .collect();
    let mut statement = conn.prepare(
        "SELECT id, input_tokens, output_tokens, cache_write_tokens, cache_read_tokens,
                total_tokens, input_cost, output_cost, cache_write_cost, cache_read_cost,
                total_cost, cost_status, cost_source, known_cost
         FROM sessions WHERE provider = 'claude' ORDER BY id",
    )?;
    let stored = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                ClaudeHistoryValues {
                    input_tokens: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    output_tokens: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    cache_write_tokens: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    cache_read_tokens: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    total_tokens: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                    input_cost: row.get::<_, Option<f64>>(6)?.unwrap_or(0.0),
                    output_cost: row.get::<_, Option<f64>>(7)?.unwrap_or(0.0),
                    cache_write_cost: row.get::<_, Option<f64>>(8)?.unwrap_or(0.0),
                    cache_read_cost: row.get::<_, Option<f64>>(9)?.unwrap_or(0.0),
                    total_cost: row.get::<_, Option<f64>>(10)?.unwrap_or(0.0),
                    cost_status: row.get::<_, Option<String>>(11)?.unwrap_or_default(),
                    cost_source: row.get::<_, Option<String>>(12)?.unwrap_or_default(),
                    known_cost: row.get::<_, Option<f64>>(13)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut summary = ClaudeHistoryRepairSummary {
        rows_checked: stored.len(),
        rows_to_update: 0,
        rows_without_transcript: 0,
        tokens_before: 0,
        tokens_after: 0,
        cost_before: 0.0,
        cost_after: 0.0,
        backup_path: None,
        applied: false,
    };
    let mut corrections = Vec::new();
    for (id, before) in stored {
        let Some(info) = fresh.get(&id) else {
            summary.rows_without_transcript += 1;
            continue;
        };
        let (cost_status, cost_source, known_cost) = session_cost_provenance(info);
        let mut after = ClaudeHistoryValues {
            input_tokens: bounded_i64(info.input_tokens),
            output_tokens: bounded_i64(info.output_tokens),
            cache_write_tokens: bounded_i64(info.cache_write_tokens),
            cache_read_tokens: bounded_i64(info.cache_read_tokens),
            total_tokens: bounded_i64(info.tokens),
            input_cost: nonnegative_finite(info.input_cost),
            output_cost: nonnegative_finite(info.output_cost),
            cache_write_cost: nonnegative_finite(info.cache_write_cost),
            cache_read_cost: nonnegative_finite(info.cache_read_cost),
            total_cost: nonnegative_finite(info.cost),
            cost_status: cost_status.to_string(),
            cost_source: cost_source.to_string(),
            known_cost,
        };
        if before.cost_source == "claude_statusline_api_equivalent" {
            after.input_cost = before.input_cost;
            after.output_cost = before.output_cost;
            after.cache_write_cost = before.cache_write_cost;
            after.cache_read_cost = before.cache_read_cost;
            after.total_cost = before.total_cost;
            after.cost_status = before.cost_status.clone();
            after.cost_source = before.cost_source.clone();
            after.known_cost = before.known_cost;
        }
        if !after.differs_from(&before) {
            continue;
        }
        summary.rows_to_update += 1;
        summary.tokens_before += before.total_tokens.max(0) as u64;
        summary.tokens_after += after.total_tokens.max(0) as u64;
        summary.cost_before += before.known_cost.unwrap_or(0.0);
        summary.cost_after += after.known_cost.unwrap_or(0.0);
        corrections.push((id, after));
    }
    Ok((summary, corrections))
}

fn history_repair_backup_path(database: &Path) -> PathBuf {
    let mut file_name = database
        .file_name()
        .unwrap_or_else(|| OsStr::new("pulse-analytics.db"))
        .to_os_string();
    file_name.push(format!(
        ".pre-history-repair-{}.bak",
        Utc::now().format("%Y%m%dT%H%M%SZ")
    ));
    database.with_file_name(file_name)
}

fn apply_claude_history_repair_in(
    conn: &Connection,
    database: &Path,
    infos: &[super::commands::SessionInfo],
) -> Result<ClaudeHistoryRepairSummary> {
    let (mut summary, corrections) = plan_claude_history_repair(conn, infos)?;
    if corrections.is_empty() {
        return Ok(summary);
    }
    let backup = history_repair_backup_path(database);
    let backup_file = backup
        .to_str()
        .with_context(|| format!("backup path is not valid Unicode: {}", backup.display()))?;
    conn.execute("VACUUM INTO ?1", params![backup_file])
        .with_context(|| format!("failed to create history backup {}", backup.display()))?;
    validate_database(&backup, schema_version(conn)?)?;

    let transaction = conn.unchecked_transaction()?;
    for (id, values) in &corrections {
        transaction.execute(
            "UPDATE sessions SET
                input_tokens=?2, output_tokens=?3, cache_write_tokens=?4, cache_read_tokens=?5,
                total_tokens=?6, input_cost=?7, output_cost=?8, cache_write_cost=?9,
                cache_read_cost=?10, total_cost=?11, cost_status=?12, cost_source=?13,
                known_cost=?14
             WHERE id=?1",
            params![
                id,
                values.input_tokens,
                values.output_tokens,
                values.cache_write_tokens,
                values.cache_read_tokens,
                values.total_tokens,
                values.input_cost,
                values.output_cost,
                values.cache_write_cost,
                values.cache_read_cost,
                values.total_cost,
                values.cost_status,
                values.cost_source,
                values.known_cost,
            ],
        )?;
    }
    transaction.commit()?;
    summary.backup_path = Some(backup.display().to_string());
    summary.applied = true;
    Ok(summary)
}

/// Previews the repair without changing anything.
pub fn preview_claude_history_repair(
    infos: &[super::commands::SessionInfo],
) -> Result<ClaudeHistoryRepairSummary> {
    with_connection(|conn| Ok(plan_claude_history_repair(conn, infos)?.0))
}

/// Backs up the database, then corrects stored Claude rows in one
/// transaction. Rows without a transcript are left unchanged.
pub fn apply_claude_history_repair(
    infos: &[super::commands::SessionInfo],
) -> Result<ClaudeHistoryRepairSummary> {
    let database = db_path();
    with_connection(|conn| apply_claude_history_repair_in(conn, &database, infos))
}

pub fn upsert_session(s: &super::commands::SessionInfo) -> bool {
    let Ok(conn) = db().lock() else { return false };
    match upsert_session_into(&conn, s, &Utc::now().to_rfc3339()) {
        Ok(()) => true,
        Err(error) => {
            warn!("Failed to persist analytics session: {error}");
            false
        }
    }
}

pub fn mark_inactive(provider: &str, active_ids: &[String]) {
    let Ok(conn) = db().lock() else { return };
    let storage_ids: Vec<String> = active_ids
        .iter()
        .map(|id| storage_session_id(provider, id))
        .collect();
    if active_ids.is_empty() {
        let _ = conn.execute(
            "UPDATE sessions SET is_active = 0, ended_at = ?1 WHERE provider = ?2 AND is_active = 1",
            params![Utc::now().to_rfc3339(), provider],
        );
        return;
    }
    let placeholders: Vec<String> = storage_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 3))
        .collect();
    let sql = format!(
        "UPDATE sessions SET is_active = 0, ended_at = ?1 WHERE provider = ?2 AND is_active = 1 AND id NOT IN ({})",
        placeholders.join(",")
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return,
    };
    let now = Utc::now().to_rfc3339();
    let mut p: Vec<Box<dyn rusqlite::types::ToSql>> =
        vec![Box::new(now), Box::new(provider.to_string())];
    for id in &storage_ids {
        p.push(Box::new(id.clone()));
    }
    let refs: Vec<&dyn rusqlite::types::ToSql> = p.iter().map(|b| b.as_ref()).collect();
    let _ = stmt.execute(refs.as_slice());
}

pub fn checkpoint_wal_after_writes(changed_count: usize) {
    const CHECKPOINT_INTERVAL: usize = 256;
    if changed_count == 0 {
        return;
    }
    let pending =
        WRITES_SINCE_CHECKPOINT.fetch_add(changed_count, Ordering::Relaxed) + changed_count;
    if pending < CHECKPOINT_INTERVAL {
        return;
    }
    let Ok(conn) = db().lock() else { return };
    if conn
        .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .is_ok()
    {
        WRITES_SINCE_CHECKPOINT.store(0, Ordering::Relaxed);
    }
}

pub fn get_session_history(
    days: Option<i64>,
    project: Option<&str>,
    limit: Option<i64>,
) -> Vec<HistoricalSession> {
    get_session_history_scoped(None, days, project, limit)
}

pub fn get_session_history_scoped(
    provider: Option<&str>,
    days: Option<i64>,
    project: Option<&str>,
    limit: Option<i64>,
) -> Vec<HistoricalSession> {
    let provider = analytics_provider_scope(provider);
    let Ok(conn) = db().lock() else {
        return vec![];
    };
    query_sessions(
        &conn, &provider, days, None, None, project, None, None, None, None, None, limit,
    )
}

pub fn get_session_history_filtered(
    from_iso: Option<&str>,
    to_iso: Option<&str>,
    project: Option<&str>,
    model: Option<&str>,
    min_cost: Option<f64>,
    max_cost: Option<f64>,
    limit: Option<i64>,
) -> Vec<HistoricalSession> {
    get_session_history_filtered_scoped(
        None, from_iso, to_iso, project, model, min_cost, max_cost, limit,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the flat filter arguments of the IPC command"
)]
pub fn get_session_history_filtered_scoped(
    provider: Option<&str>,
    from_iso: Option<&str>,
    to_iso: Option<&str>,
    project: Option<&str>,
    model: Option<&str>,
    min_cost: Option<f64>,
    max_cost: Option<f64>,
    limit: Option<i64>,
) -> Vec<HistoricalSession> {
    let provider = analytics_provider_scope(provider);
    let Ok(conn) = db().lock() else {
        return vec![];
    };
    query_sessions(
        &conn, &provider, None, from_iso, to_iso, project, model, min_cost, max_cost, None, None,
        limit,
    )
}

pub fn get_sessions_by_hour_range(
    start_hour: i64,
    end_hour: i64,
    days: Option<i64>,
) -> Vec<HistoricalSession> {
    get_sessions_by_hour_range_scoped(None, start_hour, end_hour, days)
}

pub fn get_sessions_by_hour_range_scoped(
    provider: Option<&str>,
    start_hour: i64,
    end_hour: i64,
    days: Option<i64>,
) -> Vec<HistoricalSession> {
    let provider = analytics_provider_scope(provider);
    let Ok(conn) = db().lock() else {
        return vec![];
    };
    query_sessions(
        &conn,
        &provider,
        days,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(start_hour.clamp(0, 23)),
        Some(end_hour.clamp(0, 23)),
        Some(500),
    )
}

pub fn search_sessions(query: &str, limit: Option<i64>) -> Vec<HistoricalSession> {
    search_sessions_scoped(None, query, limit)
}

pub fn search_sessions_scoped(
    provider: Option<&str>,
    query: &str,
    limit: Option<i64>,
) -> Vec<HistoricalSession> {
    let Ok(conn) = db().lock() else {
        return vec![];
    };
    let provider = analytics_provider_scope(provider);
    search_sessions_from_connection(&conn, &provider, query, limit)
}

fn search_sessions_from_connection(
    conn: &Connection,
    provider: &str,
    query: &str,
    limit: Option<i64>,
) -> Vec<HistoricalSession> {
    let lim = limit.unwrap_or(50);
    let sql = "SELECT s.id, s.provider, s.session_name, s.project, s.model, s.model_id, s.context_window, s.branch, s.effort,
            s.started_at, s.ended_at, s.duration_secs, COALESCE(s.known_cost, 0),
            s.cost_status, s.cost_source, s.known_cost,
            s.input_tokens, s.output_tokens, s.cache_write_tokens, s.cache_read_tokens, s.total_tokens,
            s.input_cost, s.output_cost, s.cache_write_cost, s.cache_read_cost,
             s.has_thinking, s.subagent_count, s.is_active, s.used_tokens, s.window_tokens, s.speed, s.opencode_json
        FROM sessions_fts fts
        JOIN sessions s ON s.rowid = fts.rowid
        WHERE (?1 = 'all' OR s.provider = ?1) AND sessions_fts MATCH ?2
        ORDER BY bm25(sessions_fts)
        LIMIT ?3";

    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            warn!("FTS search failed: {e}");
            return vec![];
        }
    };

    let rows = stmt
        .query_map(params![provider, query, lim], |row| {
            Ok(HistoricalSession {
                opencode: row
                    .get::<_, Option<String>>(31)?
                    .and_then(|raw| serde_json::from_str(&raw).ok()),
                id: row.get(0)?,
                provider: row.get(1)?,
                session_name: row.get(2)?,
                project: row.get(3)?,
                model: row.get(4)?,
                model_id: row.get(5)?,
                context_window: row.get(6)?,
                branch: row.get(7)?,
                effort: row.get(8)?,
                speed: row.get(30)?,
                started_at: row.get(9)?,
                ended_at: row.get(10)?,
                duration_secs: row.get(11)?,
                total_cost: row.get(12)?,
                cost_basis: CostBasis::from_storage(
                    row.get::<_, String>(13)?.as_str(),
                    row.get::<_, String>(14)?.as_str(),
                    row.get(15)?,
                ),
                cost_source: row.get(14)?,
                known_cost: row.get(15)?,
                input_tokens: row.get(16)?,
                output_tokens: row.get(17)?,
                cache_write_tokens: row.get(18)?,
                cache_read_tokens: row.get(19)?,
                total_tokens: row.get(20)?,
                input_cost: row.get(21)?,
                output_cost: row.get(22)?,
                cache_write_cost: row.get(23)?,
                cache_read_cost: row.get(24)?,
                has_thinking: row.get::<_, i32>(25)? != 0,
                subagent_count: row.get(26)?,
                is_active: row.get::<_, i32>(27)? != 0,
                used_tokens: row.get(28)?,
                window_tokens: row.get(29)?,
            })
        })
        .ok();

    let mut sessions: Vec<HistoricalSession> = rows
        .map(|r| r.filter_map(|x| x.ok()).collect::<Vec<_>>())
        .unwrap_or_default();
    apply_api_equivalent_estimates(&mut sessions);
    sessions
}

fn query_daily_stats(conn: &Connection, provider: &str, cutoff: &str) -> Vec<DailyStat> {
    let history_timestamp = history_timestamp_expr();
    let sql = format!(
        "SELECT date({history_timestamp}) AS session_date,
                project,
                model,
                COUNT(*),
                COALESCE(SUM(known_cost), 0),
                COALESCE(SUM(total_tokens), 0),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(cache_write_tokens), 0),
                COALESCE(SUM(cache_read_tokens), 0),
                {COST_COVERAGE_SQL}
         FROM sessions
         WHERE (?1 = 'all' OR provider = ?1)
           AND date({history_timestamp}) >= ?2
         GROUP BY session_date, project, model
         ORDER BY session_date DESC, project, model"
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let rows = stmt
        .query_map(params![provider, cutoff], |row| {
            let session_count = row.get::<_, i64>(3)?;
            let coverage = coverage_from_sql(
                session_count,
                row.get(10)?,
                row.get(11)?,
                row.get(12)?,
                row.get(13)?,
                row.get(14)?,
            );
            Ok(DailyStat {
                date: row.get(0)?,
                project: row.get(1)?,
                model: row.get(2)?,
                session_count,
                priced_sessions: coverage.priced_sessions as i64,
                cost_basis: coverage.cost_basis,
                cost_sources: coverage.cost_sources,
                total_cost: row.get(4)?,
                total_tokens: row.get(5)?,
                input_tokens: row.get(6)?,
                output_tokens: row.get(7)?,
                cache_write_tokens: row.get(8)?,
                cache_read_tokens: row.get(9)?,
            })
        })
        .ok();

    rows.map(|r| r.filter_map(|x| x.ok()).collect())
        .unwrap_or_default()
}

pub fn get_daily_stats(days: Option<i64>) -> Vec<DailyStat> {
    get_daily_stats_scoped(None, days)
}

pub fn get_daily_stats_scoped(provider: Option<&str>, days: Option<i64>) -> Vec<DailyStat> {
    let Ok(conn) = db().lock() else {
        return vec![];
    };
    let provider = analytics_provider_scope(provider);
    let cutoff = (Utc::now() - chrono::Duration::days(days.unwrap_or(30)))
        .format("%Y-%m-%d")
        .to_string();
    query_daily_stats(&conn, &provider, &cutoff)
}

pub fn get_analytics_summary() -> AnalyticsSummary {
    get_analytics_summary_scoped(None)
}

pub fn get_analytics_summary_scoped(provider: Option<&str>) -> AnalyticsSummary {
    let Ok(conn) = db().lock() else {
        return AnalyticsSummary::default();
    };
    let provider = analytics_provider_scope(provider);
    analytics_summary_from_connection(&conn, &provider)
}

fn analytics_summary_from_connection(conn: &Connection, provider: &str) -> AnalyticsSummary {
    let sessions = query_sessions(
        conn, provider, None, None, None, None, None, None, None, None, None, None,
    );
    let total_sessions = sessions.len() as i64;
    let total_cost = sessions
        .iter()
        .filter_map(|session| session.known_cost)
        .filter(|cost| cost.is_finite() && *cost >= 0.0)
        .sum();
    let coverage = summarize_cost_provenance(sessions.iter().map(|session| {
        (
            session.cost_basis,
            session.cost_source.as_str(),
            session.known_cost,
        )
    }));

    let total_tokens: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(total_tokens), 0) FROM sessions WHERE (?1 = 'all' OR provider = ?1)",
            params![provider],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let total_cache_read: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(cache_read_tokens), 0) FROM sessions WHERE (?1 = 'all' OR provider = ?1)",
            params![provider],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let total_cache_write: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(cache_write_tokens), 0) FROM sessions WHERE (?1 = 'all' OR provider = ?1)",
            params![provider],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let avg_duration_secs: f64 = conn
        .query_row(
            "SELECT COALESCE(AVG(duration_secs), 0) FROM sessions WHERE (?1 = 'all' OR provider = ?1) AND duration_secs > 0",
            params![provider],
            |r| r.get(0),
        )
        .unwrap_or(0.0);

    let avg_tokens_per_session: f64 = if total_sessions > 0 {
        total_tokens as f64 / total_sessions as f64
    } else {
        0.0
    };

    let avg_cost_per_session: f64 = if coverage.priced_sessions > 0 {
        total_cost / coverage.priced_sessions as f64
    } else {
        0.0
    };

    let mut project_totals = BTreeMap::<&str, (f64, usize)>::new();
    for session in &sessions {
        let entry = project_totals
            .entry(session.project.as_str())
            .or_insert((0.0, 0));
        entry.0 += session.known_cost.unwrap_or(0.0);
        entry.1 += 1;
    }
    let top_project = project_totals
        .into_iter()
        .max_by(
            |(project_a, (cost_a, count_a)), (project_b, (cost_b, count_b))| {
                cost_a
                    .total_cmp(cost_b)
                    .then_with(|| count_a.cmp(count_b))
                    .then_with(|| project_b.cmp(project_a))
            },
        )
        .map(|(project, _)| project.to_string())
        .unwrap_or_else(|| "—".to_string());

    let top_model: String = conn
        .query_row(
            "SELECT model FROM sessions WHERE (?1 = 'all' OR provider = ?1) GROUP BY model ORDER BY COUNT(*) DESC LIMIT 1",
            params![provider],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "—".to_string());

    let days_tracked: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(DISTINCT date({})) FROM sessions WHERE (?1 = 'all' OR provider = ?1)",
                history_timestamp_expr()
            ),
            params![provider],
            |r| r.get(0),
        )
        .unwrap_or(0);

    AnalyticsSummary {
        total_sessions,
        priced_sessions: coverage.priced_sessions as i64,
        cost_basis: coverage.cost_basis,
        cost_sources: coverage.cost_sources,
        total_cost,
        total_tokens,
        total_cache_read,
        total_cache_write,
        avg_duration_secs,
        avg_tokens_per_session,
        avg_cost_per_session,
        top_project,
        top_model,
        days_tracked,
    }
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct AnalyticsSummary {
    pub total_sessions: i64,
    pub priced_sessions: i64,
    pub cost_basis: CostBasis,
    pub cost_sources: Vec<String>,
    pub total_cost: f64,
    pub total_tokens: i64,
    pub total_cache_read: i64,
    pub total_cache_write: i64,
    pub avg_duration_secs: f64,
    pub avg_tokens_per_session: f64,
    pub avg_cost_per_session: f64,
    pub top_project: String,
    pub top_model: String,
    pub days_tracked: i64,
}

pub fn get_project_stats(days: Option<i64>) -> Vec<ProjectStat> {
    get_project_stats_scoped(None, days)
}

pub fn get_project_stats_scoped(provider: Option<&str>, days: Option<i64>) -> Vec<ProjectStat> {
    let Ok(conn) = db().lock() else { return vec![] };
    let provider = analytics_provider_scope(provider);
    let d = days.unwrap_or(30);
    let cutoff = (Utc::now() - chrono::Duration::days(d)).to_rfc3339();
    query_project_stats(&conn, &provider, &cutoff)
}

fn query_project_stats(conn: &Connection, provider: &str, cutoff: &str) -> Vec<ProjectStat> {
    let sql = format!(
        "SELECT project,
            COUNT(*) as cnt,
            COALESCE(SUM(known_cost), 0),
            COALESCE(SUM(total_tokens), 0),
            CASE
                WHEN COUNT(known_cost) > 0
                THEN COALESCE(SUM(known_cost), 0) / COUNT(known_cost)
                ELSE 0
            END,
            COALESCE(AVG(duration_secs), 0),
            COALESCE(SUM(cache_read_tokens), 0),
            COALESCE(SUM(cache_write_tokens), 0),
            (SELECT model FROM sessions s2
             WHERE (?1 = 'all' OR s2.provider = ?1) AND s2.project = sessions.project
             GROUP BY model ORDER BY COUNT(*) DESC LIMIT 1),
            {COST_COVERAGE_SQL}
        FROM sessions
        WHERE (?1 = 'all' OR provider = ?1) AND COALESCE({}, datetime('now')) >= ?2
        GROUP BY project ORDER BY COALESCE(SUM(known_cost), 0) DESC, project ASC",
        history_timestamp_expr()
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map(params![provider, cutoff], |row| {
        let session_count = row.get::<_, i64>(1)?;
        let coverage = coverage_from_sql(
            session_count,
            row.get(9)?,
            row.get(10)?,
            row.get(11)?,
            row.get(12)?,
            row.get(13)?,
        );
        Ok(ProjectStat {
            project: row.get(0)?,
            session_count,
            priced_sessions: coverage.priced_sessions as i64,
            cost_basis: coverage.cost_basis,
            cost_sources: coverage.cost_sources,
            total_cost: row.get(2)?,
            total_tokens: row.get(3)?,
            avg_session_cost: row.get(4)?,
            avg_duration_secs: row.get(5)?,
            cache_read_tokens: row.get(6)?,
            cache_write_tokens: row.get(7)?,
            top_model: row.get::<_, String>(8).unwrap_or_default(),
        })
    })
    .ok()
    .map(|r| r.filter_map(|x| x.ok()).collect())
    .unwrap_or_default()
}

pub fn get_hourly_activity(days: Option<i64>) -> Vec<HourlyActivity> {
    get_hourly_activity_scoped(None, days)
}

pub fn get_hourly_activity_scoped(
    provider: Option<&str>,
    days: Option<i64>,
) -> Vec<HourlyActivity> {
    let Ok(conn) = db().lock() else { return vec![] };
    let provider = analytics_provider_scope(provider);
    let d = days.unwrap_or(30);
    let cutoff = (Utc::now() - chrono::Duration::days(d)).to_rfc3339();
    query_hourly_activity(&conn, &provider, &cutoff)
}

fn query_hourly_activity(conn: &Connection, provider: &str, cutoff: &str) -> Vec<HourlyActivity> {
    let sql = format!(
        "SELECT CAST(strftime('%H', COALESCE({}, ''), 'localtime') AS INTEGER) as hour,
            COUNT(*), COALESCE(SUM(known_cost), 0), {COST_COVERAGE_SQL}
        FROM sessions
        WHERE (?1 = 'all' OR provider = ?1) AND COALESCE({}, datetime('now')) >= ?2
        GROUP BY hour ORDER BY hour",
        history_timestamp_expr(),
        history_timestamp_expr()
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map(params![provider, cutoff], |row| {
        let session_count = row.get::<_, i64>(1)?;
        let coverage = coverage_from_sql(
            session_count,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
        );
        Ok(HourlyActivity {
            hour: row.get(0)?,
            session_count,
            priced_sessions: coverage.priced_sessions as i64,
            cost_basis: coverage.cost_basis,
            cost_sources: coverage.cost_sources,
            total_cost: row.get(2)?,
        })
    })
    .ok()
    .map(|r| r.filter_map(|x| x.ok()).collect())
    .unwrap_or_default()
}

pub fn get_top_sessions(limit: Option<i64>, days: Option<i64>) -> Vec<HistoricalSession> {
    get_top_sessions_scoped(None, limit, days)
}

pub fn get_top_sessions_scoped(
    provider: Option<&str>,
    limit: Option<i64>,
    days: Option<i64>,
) -> Vec<HistoricalSession> {
    get_session_history_scoped(provider, days, None, limit)
        .into_iter()
        .collect::<Vec<_>>()
        .into_iter()
        .take(limit.unwrap_or(25) as usize)
        .collect()
}

fn cost_forecast_from_connection(
    conn: &Connection,
    provider: &str,
    now: chrono::DateTime<Utc>,
) -> CostForecast {
    let month_start = now.format("%Y-%m-01T00:00:00+00:00").to_string();
    let days_elapsed = now.day() as i64;
    let days_in_month = {
        let (y, m) = (now.year(), now.month());
        if m == 12 {
            chrono::NaiveDate::from_ymd_opt(y + 1, 1, 1)
        } else {
            chrono::NaiveDate::from_ymd_opt(y, m + 1, 1)
        }
        .and_then(|d| d.pred_opt())
        .map(|d| d.day() as i64)
        .unwrap_or(30)
    };
    let sql = format!(
        "SELECT cost_status, cost_source, known_cost, provider, model, model_id,
                input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, speed
         FROM sessions
         WHERE (?1 = 'all' OR provider = ?1) AND COALESCE({}, datetime('now')) >= ?2",
        history_timestamp_expr()
    );
    let observations = conn
        .prepare(&sql)
        .and_then(|mut stmt| {
            stmt.query_map(params![provider, month_start], |row| {
                let status: String = row.get(0)?;
                let mut source: String = row.get(1)?;
                let stored_known: Option<f64> = row.get(2)?;
                let mut basis = CostBasis::from_storage(&status, &source, stored_known);
                let mut known_cost = stored_known;
                // Reconstruct API-equivalent spend for unpriced subscription
                // sessions so month-to-date spend and the projection are not
                // permanently blank.
                if basis == CostBasis::Unavailable && stored_known.is_none() {
                    let row_provider: String = row.get(3)?;
                    let model_id: String = row.get(5)?;
                    let model = if model_id.trim().is_empty() {
                        row.get::<_, String>(4)?
                    } else {
                        model_id
                    };
                    if let Some(est) = estimate_api_equivalent_cost_with_speed(
                        &row_provider,
                        &model,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get::<_, String>(10)?.as_str(),
                    ) {
                        basis = CostBasis::Estimated;
                        source = "api_equivalent".to_string();
                        known_cost = Some(est.total);
                    }
                }
                Ok((basis, source, known_cost))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default();
    let coverage = summarize_cost_provenance(
        observations
            .iter()
            .map(|(basis, source, known_cost)| (*basis, source.as_str(), *known_cost)),
    );
    let mut billed_sessions = 0usize;
    let mut api_equivalent_sessions = 0usize;
    let mut billed_spend = 0.0;
    let mut api_equivalent = 0.0;
    for (_, source, known_cost) in &observations {
        let Some(cost) = *known_cost else {
            continue;
        };
        if !cost.is_finite() || cost < 0.0 {
            continue;
        }
        match monetary_provenance(source) {
            MonetaryProvenance::ProviderBilled => {
                billed_sessions += 1;
                billed_spend += cost;
            }
            MonetaryProvenance::ApiEquivalent => {
                api_equivalent_sessions += 1;
                api_equivalent += cost;
            }
            MonetaryProvenance::Other => {}
        }
    }
    let billed_spend_usd = (billed_sessions > 0).then_some(billed_spend);
    let api_equivalent_usd = (api_equivalent_sessions > 0).then_some(api_equivalent);
    let daily_billed_spend_usd = billed_spend_usd
        .filter(|_| days_elapsed > 0)
        .map(|value| value / days_elapsed as f64);
    let daily_api_equivalent_usd = api_equivalent_usd
        .filter(|_| days_elapsed > 0)
        .map(|value| value / days_elapsed as f64);
    CostForecast {
        billed_spend_usd,
        daily_billed_spend_usd,
        projected_billed_spend_usd: daily_billed_spend_usd
            .map(|value| value * days_in_month as f64),
        api_equivalent_usd,
        daily_api_equivalent_usd,
        projected_api_equivalent_usd: daily_api_equivalent_usd
            .map(|value| value * days_in_month as f64),
        days_elapsed,
        days_in_month,
        cost_basis: coverage.cost_basis,
        cost_sources: coverage.cost_sources,
        sessions: coverage.sessions,
        priced_sessions: coverage.priced_sessions,
        billed_sessions,
        api_equivalent_sessions,
        refreshed_at: now.to_rfc3339(),
    }
}

fn empty_cost_forecast(now: chrono::DateTime<Utc>) -> CostForecast {
    CostForecast {
        billed_spend_usd: None,
        daily_billed_spend_usd: None,
        projected_billed_spend_usd: None,
        api_equivalent_usd: None,
        daily_api_equivalent_usd: None,
        projected_api_equivalent_usd: None,
        days_elapsed: 0,
        days_in_month: 30,
        cost_basis: CostBasis::Unavailable,
        cost_sources: Vec::new(),
        sessions: 0,
        priced_sessions: 0,
        billed_sessions: 0,
        api_equivalent_sessions: 0,
        refreshed_at: now.to_rfc3339(),
    }
}

pub fn get_cost_forecast() -> CostForecast {
    get_cost_forecast_scoped(None)
}

pub fn get_cost_forecast_scoped(provider: Option<&str>) -> CostForecast {
    let now = Utc::now();
    let Ok(conn) = db().lock() else {
        return empty_cost_forecast(now);
    };
    let provider = analytics_provider_scope(provider);
    cost_forecast_from_connection(&conn, &provider, now)
}

pub fn get_budget_status() -> BudgetStatus {
    get_budget_status_scoped(None)
}

pub fn get_budget_status_scoped(provider: Option<&str>) -> BudgetStatus {
    let now = Utc::now();
    let Ok(conn) = db().lock() else {
        let forecast = empty_cost_forecast(now);
        return BudgetStatus {
            monthly_budget: 0.0,
            alert_threshold_pct: 80.0,
            billed_spend_usd: forecast.billed_spend_usd,
            projected_billed_spend_usd: forecast.projected_billed_spend_usd,
            api_equivalent_usd: forecast.api_equivalent_usd,
            projected_api_equivalent_usd: forecast.projected_api_equivalent_usd,
            pct_used: None,
            over_budget: false,
            cost_basis: forecast.cost_basis,
            cost_sources: forecast.cost_sources,
            sessions: forecast.sessions,
            priced_sessions: forecast.priced_sessions,
            billed_sessions: forecast.billed_sessions,
            api_equivalent_sessions: forecast.api_equivalent_sessions,
            refreshed_at: forecast.refreshed_at,
        };
    };
    let provider = analytics_provider_scope(provider);
    budget_status_from_connection(&conn, &provider, now)
}

fn budget_status_from_connection(
    conn: &Connection,
    provider: &str,
    now: chrono::DateTime<Utc>,
) -> BudgetStatus {
    let forecast = cost_forecast_from_connection(conn, provider, now);
    budget_status_from_forecast(conn, forecast)
}

fn budget_status_from_forecast(conn: &Connection, forecast: CostForecast) -> BudgetStatus {
    let (budget, threshold): (f64, f64) = conn
        .query_row(
            "SELECT monthly_budget, alert_threshold_pct FROM budget_config WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or((0.0, 80.0));
    let pct = forecast
        .billed_spend_usd
        .filter(|_| budget > 0.0)
        .map(|spent| (spent / budget) * 100.0);
    BudgetStatus {
        monthly_budget: budget,
        alert_threshold_pct: threshold,
        billed_spend_usd: forecast.billed_spend_usd,
        projected_billed_spend_usd: forecast.projected_billed_spend_usd,
        api_equivalent_usd: forecast.api_equivalent_usd,
        projected_api_equivalent_usd: forecast.projected_api_equivalent_usd,
        pct_used: pct,
        over_budget: budget > 0.0
            && forecast
                .projected_billed_spend_usd
                .is_some_and(|projection| projection > budget),
        cost_basis: forecast.cost_basis,
        cost_sources: forecast.cost_sources,
        sessions: forecast.sessions,
        priced_sessions: forecast.priced_sessions,
        billed_sessions: forecast.billed_sessions,
        api_equivalent_sessions: forecast.api_equivalent_sessions,
        refreshed_at: forecast.refreshed_at,
    }
}

/// Dashboard history, forecast, and hourly activity under one SQLite mutex
/// acquisition. The individual queries remain independently testable, while
/// the UI receives a coherent snapshot without queueing four IPC commands.
pub fn get_dashboard_data_scoped(
    provider: Option<&str>,
    days: i64,
    history_limit: i64,
) -> DashboardDataSnapshot {
    let now = Utc::now();
    let provider = analytics_provider_scope(provider);
    let Ok(conn) = db().lock() else {
        return DashboardDataSnapshot {
            summary: AnalyticsSummary::default(),
            sessions: Vec::new(),
            forecast: empty_cost_forecast(now),
            hourly_activity: Vec::new(),
        };
    };
    let cutoff = (now - chrono::Duration::days(days)).to_rfc3339();
    DashboardDataSnapshot {
        summary: analytics_summary_for_window(&conn, &provider, days),
        sessions: query_sessions(
            &conn,
            &provider,
            Some(days),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(history_limit),
        ),
        forecast: cost_forecast_from_connection(&conn, &provider, now),
        hourly_activity: query_hourly_activity(&conn, &provider, &cutoff),
    }
}

/// Costs view inputs under one SQLite mutex acquisition. `history` preserves
/// the unfiltered table population; `aggregate_sessions` follows the selected
/// project and feeds the reconciled KPI total.
pub fn get_costs_data_scoped(
    provider: Option<&str>,
    days: i64,
    project: Option<&str>,
    history_limit: i64,
) -> CostsDataSnapshot {
    let now = Utc::now();
    let provider = analytics_provider_scope(provider);
    let Ok(conn) = db().lock() else {
        let forecast = empty_cost_forecast(now);
        return CostsDataSnapshot {
            history: Vec::new(),
            aggregate_sessions: Vec::new(),
            budget: BudgetStatus {
                monthly_budget: 0.0,
                alert_threshold_pct: 80.0,
                billed_spend_usd: None,
                projected_billed_spend_usd: None,
                api_equivalent_usd: None,
                projected_api_equivalent_usd: None,
                pct_used: None,
                over_budget: false,
                cost_basis: CostBasis::Unavailable,
                cost_sources: Vec::new(),
                sessions: 0,
                priced_sessions: 0,
                billed_sessions: 0,
                api_equivalent_sessions: 0,
                refreshed_at: forecast.refreshed_at.clone(),
            },
            forecast,
            daily_usage: Vec::new(),
        };
    };
    costs_data_from_connection(&conn, &provider, days, project, history_limit, now)
}

fn costs_data_from_connection(
    conn: &Connection,
    provider: &str,
    days: i64,
    project: Option<&str>,
    history_limit: i64,
    now: chrono::DateTime<Utc>,
) -> CostsDataSnapshot {
    let history = query_sessions(
        conn,
        provider,
        Some(days),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(history_limit),
    );
    let aggregate_sessions = query_sessions(
        conn,
        provider,
        Some(days),
        None,
        None,
        project,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    let cutoff = (now - chrono::Duration::days(days))
        .format("%Y-%m-%d")
        .to_string();
    let forecast = cost_forecast_from_connection(conn, provider, now);
    let budget = budget_status_from_forecast(conn, forecast.clone());
    CostsDataSnapshot {
        history,
        aggregate_sessions,
        forecast,
        budget,
        daily_usage: query_daily_stats(conn, provider, &cutoff),
    }
}

pub fn set_budget(monthly_budget: f64, alert_threshold_pct: Option<f64>) -> Result<(), String> {
    let conn = db()
        .lock()
        .map_err(|_| "analytics database lock poisoned".to_string())?;
    let threshold = alert_threshold_pct.unwrap_or(80.0);
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO budget_config (id, monthly_budget, alert_threshold_pct, updated_at)
         VALUES (1, ?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET monthly_budget=?1, alert_threshold_pct=?2, updated_at=?3",
        params![monthly_budget, threshold, now],
    )
    .map_err(|error| format!("failed to persist budget: {error}"))?;
    Ok(())
}

pub fn get_model_distribution(days: Option<i64>) -> Vec<ModelStat> {
    get_model_distribution_scoped(None, days)
}

pub fn get_model_distribution_scoped(provider: Option<&str>, days: Option<i64>) -> Vec<ModelStat> {
    let Ok(conn) = db().lock() else { return vec![] };
    let provider = analytics_provider_scope(provider);
    let d = days.unwrap_or(30);
    let cutoff = (Utc::now() - chrono::Duration::days(d)).to_rfc3339();
    query_model_distribution(&conn, &provider, &cutoff)
}

fn query_model_distribution(conn: &Connection, provider: &str, cutoff: &str) -> Vec<ModelStat> {
    let sql = format!(
        "SELECT model, COUNT(*), COALESCE(SUM(known_cost), 0), {COST_COVERAGE_SQL}
        FROM sessions
        WHERE (?1 = 'all' OR provider = ?1) AND COALESCE({}, datetime('now')) >= ?2
        GROUP BY model ORDER BY COUNT(*) DESC",
        history_timestamp_expr()
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map(params![provider, cutoff], |row| {
        let session_count = row.get::<_, i64>(1)?;
        let coverage = coverage_from_sql(
            session_count,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
        );
        Ok(ModelStat {
            model: row.get(0)?,
            session_count,
            priced_sessions: coverage.priced_sessions as i64,
            cost_basis: coverage.cost_basis,
            cost_sources: coverage.cost_sources,
            total_cost: row.get(2)?,
        })
    })
    .ok()
    .map(|r| r.filter_map(|x| x.ok()).collect())
    .unwrap_or_default()
}

/// One calendar day of spend, as aggregated by SQLite.
#[derive(Debug, Clone, PartialEq)]
pub struct DailyCostRow {
    pub date: String,
    pub cost: f64,
    pub sessions: i64,
    pub priced_sessions: i64,
    pub cost_basis: CostBasis,
    pub cost_sources: Vec<String>,
}

/// Window-wide daily spend, aggregated in SQL.
///
/// The Reports timeline must describe the same window the analyzers see, so
/// this deliberately does not reuse the capped session page: with more than
/// the analyzer row cap in a window, summing the returned rows would zero-fill
/// days whose sessions were discarded and understate the curve.
///
/// `from_date` is an inclusive calendar date (`YYYY-MM-DD`), matching the first
/// day the caller intends to plot. Comparing on the date prefix keeps the
/// boundary day whole instead of cutting it at the rolling `now - days`
/// instant, which is what made the timeline disagree with its own totals.
pub fn get_daily_costs(from_date: &str, project: Option<&str>) -> Vec<DailyCostRow> {
    get_daily_costs_scoped(None, from_date, project)
}

pub fn get_daily_costs_scoped(
    provider: Option<&str>,
    from_date: &str,
    project: Option<&str>,
) -> Vec<DailyCostRow> {
    let Ok(conn) = db().lock() else { return vec![] };
    let provider = analytics_provider_scope(provider);
    query_daily_costs(&conn, &provider, from_date, project)
}

fn query_daily_costs(
    conn: &Connection,
    provider: &str,
    from_date: &str,
    project: Option<&str>,
) -> Vec<DailyCostRow> {
    // Aggregate from the estimation-aware session reader instead of a raw
    // SUM(known_cost). A pure-SQL sum only sees provider-billed rows, so the
    // cost timeline went blank for subscription usage; grouping the same
    // API-equivalent sessions the rest of the app uses keeps one source of
    // truth for cost.
    let history_ts = history_timestamp_expr();
    let day_expr = format!("substr({history_ts}, 1, 10)");
    let mut sql = format!(
        "SELECT id, provider, project, model, model_id, {day_expr} AS day, COALESCE(known_cost, 0),
                cost_status, cost_source, known_cost,
                input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, speed
         FROM sessions
         WHERE (?1 = 'all' OR provider = ?1) AND {history_ts} IS NOT NULL AND {day_expr} >= ?2"
    );
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
        Box::new(provider.to_string()),
        Box::new(from_date.to_string()),
    ];
    if let Some(project) = project {
        sql.push_str(" AND project = ?3");
        params_vec.push(Box::new(project.to_string()));
    }

    let refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
    let mut stmt = match conn.prepare(&sql) {
        Ok(stmt) => stmt,
        Err(err) => {
            warn!("Failed to prepare daily cost query: {err}");
            return vec![];
        }
    };

    struct DailyRowObs {
        day: String,
        basis: CostBasis,
        source: String,
        known_cost: Option<f64>,
    }

    let observations = stmt
        .query_map(refs.as_slice(), |row| {
            let day: String = row.get(5)?;
            let stored_known: Option<f64> = row.get(9)?;
            let mut basis = CostBasis::from_storage(
                row.get::<_, String>(7)?.as_str(),
                row.get::<_, String>(8)?.as_str(),
                stored_known,
            );
            let mut source: String = row.get(8)?;
            let mut known_cost = stored_known;
            if basis == CostBasis::Unavailable && stored_known.is_none() {
                let provider: String = row.get(1)?;
                let model_id: String = row.get(4)?;
                let model = if model_id.trim().is_empty() {
                    row.get::<_, String>(3)?
                } else {
                    model_id
                };
                if let Some(est) = estimate_api_equivalent_cost_with_speed(
                    &provider,
                    &model,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                    row.get::<_, String>(14)?.as_str(),
                ) {
                    basis = CostBasis::Estimated;
                    source = "api_equivalent".to_string();
                    known_cost = Some(est.total);
                }
            }
            Ok(DailyRowObs {
                day,
                basis,
                source,
                known_cost,
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|row| row.ok()).collect::<Vec<_>>())
        .unwrap_or_default();

    let mut by_day: std::collections::BTreeMap<String, Vec<DailyRowObs>> =
        std::collections::BTreeMap::new();
    for obs in observations {
        by_day.entry(obs.day.clone()).or_default().push(obs);
    }

    by_day
        .into_iter()
        .map(|(day, rows)| {
            let cost: f64 = rows
                .iter()
                .filter_map(|r| r.known_cost)
                .filter(|c| c.is_finite() && *c >= 0.0)
                .sum();
            let coverage = summarize_cost_provenance(
                rows.iter()
                    .map(|r| (r.basis, r.source.as_str(), r.known_cost)),
            );
            DailyCostRow {
                date: day,
                cost,
                sessions: rows.len() as i64,
                priced_sessions: coverage.priced_sessions as i64,
                cost_basis: coverage.cost_basis,
                cost_sources: coverage.cost_sources,
            }
        })
        .collect()
}

pub fn export_all_data() -> serde_json::Value {
    export_all_data_scoped(None)
}

pub fn export_all_data_scoped(provider: Option<&str>) -> serde_json::Value {
    let provider = analytics_provider_scope(provider);
    let now = Utc::now();
    let Ok(conn) = db().lock() else {
        return serde_json::json!({
            "exported_at": now.to_rfc3339(),
            "summary": AnalyticsSummary::default(),
            "sessions": [],
            "daily_stats": [],
        });
    };
    export_all_data_from_connection(&conn, &provider, now)
}

fn export_all_data_from_connection(
    conn: &Connection,
    provider: &str,
    now: chrono::DateTime<Utc>,
) -> serde_json::Value {
    let sessions = query_sessions(
        conn,
        provider,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(10_000),
    );
    let cutoff = (now - chrono::Duration::days(365))
        .format("%Y-%m-%d")
        .to_string();
    let daily = query_daily_stats(conn, provider, &cutoff);
    let summary = analytics_summary_from_connection(conn, provider);
    serde_json::json!({
        "exported_at": now.to_rfc3339(),
        "summary": summary,
        "sessions": sessions,
        "daily_stats": daily,
    })
}

fn clear_history_from_connection(conn: &mut Connection, provider: &str) -> Result<i64> {
    let transaction = conn.transaction()?;
    let deleted: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM sessions WHERE (?1 = 'all' OR provider = ?1)",
        params![provider],
        |row| row.get(0),
    )?;
    transaction.execute(
        "DELETE FROM sessions_fts
         WHERE rowid IN (
             SELECT rowid FROM sessions WHERE (?1 = 'all' OR provider = ?1)
         )",
        params![provider],
    )?;
    transaction.execute(
        "DELETE FROM sessions WHERE (?1 = 'all' OR provider = ?1)",
        params![provider],
    )?;
    transaction.commit()?;
    Ok(deleted)
}

pub fn clear_history_scoped(provider: Option<&str>) -> Result<i64> {
    let provider = analytics_provider_scope(provider);
    let mut conn = db()
        .lock()
        .map_err(|_| anyhow::anyhow!("analytics database lock is poisoned"))?;
    clear_history_from_connection(&mut conn, &provider)
}

pub fn get_db_size_bytes() -> u64 {
    std::fs::metadata(db_path()).map(|m| m.len()).unwrap_or(0)
}

pub fn upsert_opencode_session(session: &super::commands::SessionInfo, updated: i64) -> bool {
    upsert_external_session(session, updated)
}

pub fn upsert_external_session(session: &super::commands::SessionInfo, updated: i64) -> bool {
    let Ok(conn) = db().lock() else {
        return false;
    };
    let Some(updated) = chrono::DateTime::from_timestamp_millis(updated) else {
        return false;
    };
    match upsert_external_session_into(&conn, session, &updated.to_rfc3339()) {
        Ok(_) => true,
        Err(error) => {
            tracing::warn!(provider=%session.provider,"Provider analytics write failed: {error}");
            false
        }
    }
}

/// Writes a session read from another program's store unless the stored row
/// already has the same source timestamp: re-reading an unchanged session (on
/// every start, or every poll) then costs no write. Returns whether it wrote.
fn upsert_external_session_into(
    conn: &Connection,
    session: &super::commands::SessionInfo,
    updated_at: &str,
) -> rusqlite::Result<bool> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT updated_at FROM sessions WHERE id = ?1",
            [storage_session_id(&session.provider, &session.session_id)],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    if stored.as_deref() == Some(updated_at) {
        return Ok(false);
    }
    upsert_session_into(conn, session, updated_at)?;
    Ok(true)
}

fn analytics_summary_for_window(conn: &Connection, provider: &str, days: i64) -> AnalyticsSummary {
    let rows = query_sessions(
        conn,
        provider,
        Some(days),
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
    let count = rows.len() as i64;
    let coverage = summarize_cost_provenance(
        rows.iter()
            .map(|row| (row.cost_basis, row.cost_source.as_str(), row.known_cost)),
    );
    let total_cost: f64 = rows.iter().filter_map(|row| row.known_cost).sum();
    let total_tokens: i64 = rows.iter().map(|row| row.total_tokens).sum();
    let mut projects: BTreeMap<String, i64> = BTreeMap::new();
    let mut models: BTreeMap<String, i64> = BTreeMap::new();
    for row in &rows {
        *projects.entry(row.project.clone()).or_default() += row.total_tokens;
        *models.entry(row.model.clone()).or_default() += row.total_tokens;
    }
    let top = |map: BTreeMap<String, i64>| {
        map.into_iter()
            .max_by_key(|(_, tokens)| *tokens)
            .map(|(name, _)| name)
            .unwrap_or_default()
    };
    AnalyticsSummary {
        total_sessions: count,
        priced_sessions: coverage.priced_sessions as i64,
        cost_basis: coverage.cost_basis,
        cost_sources: coverage.cost_sources,
        total_cost,
        total_tokens,
        total_cache_read: rows.iter().map(|row| row.cache_read_tokens).sum(),
        total_cache_write: rows.iter().map(|row| row.cache_write_tokens).sum(),
        avg_duration_secs: if count > 0 {
            rows.iter().map(|row| row.duration_secs).sum::<i64>() as f64 / count as f64
        } else {
            0.0
        },
        avg_tokens_per_session: if count > 0 {
            total_tokens as f64 / count as f64
        } else {
            0.0
        },
        avg_cost_per_session: if coverage.priced_sessions > 0 {
            total_cost / coverage.priced_sessions as f64
        } else {
            0.0
        },
        top_project: top(projects),
        top_model: top(models),
        days_tracked: rows
            .iter()
            .filter_map(|row| row.started_at.as_deref())
            .map(|value| value.chars().take(10).collect::<String>())
            .collect::<BTreeSet<_>>()
            .len() as i64,
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod opencode_tests;
