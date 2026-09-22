use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

use crate::config;
use crate::cost;

const SESSION_SCAN_MAX_DEPTH: usize = 16;
const SESSION_SCAN_MAX_ENTRIES: usize = 100_000;
const SESSION_TAIL_ANCHOR_BYTES: u64 = 64;
const SESSION_CHECKPOINT_SCHEMA_VERSION: u32 = 1;
/// Checkpoints store accumulated costs, so they are only reusable by the build
/// that priced them. A release that changes model rates must not restore costs
/// that an older build computed at the old rates.
const SESSION_CHECKPOINT_APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const SESSION_CHECKPOINT_MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    #[default]
    Idle,
    Thinking,
    ReadingFile,
    EditingFile,
    RunningCommand,
    WaitingInput,
}

/// Per-turn API speed tier reported in each assistant message's `usage.speed`.
/// Fast mode (Opus 4.8+) bills at `cost::FAST_RATE_MULTIPLIER`x standard rates.
/// Defaults to Standard when absent. Orthogonal to `service_tier` (priority),
/// which affects routing/display but not price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Speed {
    #[default]
    Standard,
    Fast,
}

impl Speed {
    /// Case-insensitive parse from the JSONL `usage.speed` label. Any value other
    /// than "fast" (including absent/unknown) resolves to Standard.
    pub fn from_label(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "fast" => Self::Fast,
            _ => Self::Standard,
        }
    }

    /// Build a `Speed` from a fast-mode boolean so the wire label has a single
    /// source of truth (`as_str`) rather than inline `if fast { "fast" } …`.
    pub fn from_fast(fast: bool) -> Self {
        if fast { Self::Fast } else { Self::Standard }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Fast => "fast",
        }
    }

    pub fn is_fast(&self) -> bool {
        matches!(self, Self::Fast)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Low,
    #[default]
    Medium,
    High,
    /// "Extra High" — Opus 4.7+ exclusive (API `effort: xhigh`).
    /// Serialized as `extra_high` for config/JSON; API value is `xhigh`.
    ExtraHigh,
    Max,
}

impl ReasoningEffort {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::ExtraHigh => "Extra High",
            Self::Max => "Max",
        }
    }

    /// Anthropic API value for the `effort` parameter.
    /// Low → "low", Medium → "medium", High → "high", ExtraHigh → "xhigh", Max → "max".
    pub fn api_value(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::ExtraHigh => "xhigh",
            Self::Max => "max",
        }
    }

    /// Parse from API string (accepts "xhigh", "extra_high", "extrahigh" for ExtraHigh).
    pub fn from_api(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" | "med" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" | "x-high" | "extra_high" | "extrahigh" | "extra high" => Some(Self::ExtraHigh),
            "max" | "maximum" => Some(Self::Max),
            _ => None,
        }
    }

    /// True when this effort level enables extended thinking (High, ExtraHigh, or Max).
    pub fn is_high(&self) -> bool {
        matches!(self, Self::High | Self::ExtraHigh | Self::Max)
    }

    /// Short display for tight contexts (Discord state line, badges).
    /// Extra High → "X-High", others use `label()`.
    pub fn short_label(&self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Med",
            Self::High => "High",
            Self::ExtraHigh => "X-High",
            Self::Max => "Max",
        }
    }
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Fallback: read default effort from `settings.json`.
/// The primary source is JSONL extraction + thinking-block inference; this is only
/// used as the initial default when a session starts parsing.
pub fn read_claude_effort_level() -> ReasoningEffort {
    let path = crate::config::claude_home().join("settings.json");
    let Ok(data) = std::fs::read_to_string(&path) else {
        return ReasoningEffort::default();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) else {
        return ReasoningEffort::default();
    };
    match json.get("effortLevel").and_then(|v| v.as_str()) {
        Some(s) => ReasoningEffort::from_api(s).unwrap_or_default(),
        None => ReasoningEffort::Medium,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActivitySnapshot {
    pub kind: ActivityKind,
    pub target: Option<String>,
    pub observed_at: Option<DateTime<Utc>>,
    pub last_active_at: Option<DateTime<Utc>>,
    pub last_effective_signal_at: Option<DateTime<Utc>>,
    pub idle_candidate_at: Option<DateTime<Utc>>,
    pub pending_calls: usize,
}

impl ActivitySnapshot {
    pub fn action_text(&self) -> &'static str {
        match self.kind {
            ActivityKind::Thinking => "Thinking",
            ActivityKind::ReadingFile => "Reading",
            ActivityKind::EditingFile => "Editing",
            ActivityKind::RunningCommand => "Running command",
            ActivityKind::WaitingInput => "Waiting for input",
            ActivityKind::Idle => "Idle",
        }
    }

    pub fn to_text(&self, show_target: bool) -> String {
        if show_target && let Some(ref target) = self.target {
            let trimmed = target.trim();
            if !trimmed.is_empty()
                && let Some(short) = shorten_activity_target(&self.kind, trimmed)
                && !short.trim().is_empty()
            {
                return format!("{} {}", self.action_text(), short);
            }
        }
        self.action_text().to_string()
    }
}

/// Condenses an activity target to its most readable short form for Discord/UI display.
/// Files → filename only. Commands → clean command summary only.
fn shorten_activity_target(kind: &ActivityKind, target: &str) -> Option<String> {
    match kind {
        ActivityKind::ReadingFile | ActivityKind::EditingFile => {
            crate::activity_target::readable_file_target(target, 50).into()
        }
        ActivityKind::RunningCommand => summarize_command_for_presence(target, 50),
        _ => target.to_string().into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataSource {
    Statusline,
    Jsonl,
}

/// Information about a subagent spawned by a parent session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentInfo {
    pub agent_type: String,
    pub model: Option<String>,
    pub model_display: Option<String>,
    pub activity: Option<ActivitySnapshot>,
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RateLimits {
    pub primary: Option<UsageWindowLimits>,
    pub secondary: Option<UsageWindowLimits>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageWindowLimits {
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub window_minutes: u64,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeSessionSnapshot {
    pub session_id: String,
    pub cwd: PathBuf,
    pub project_name: String,
    pub git_branch: Option<String>,
    pub model: Option<String>,
    pub model_display: Option<String>,
    pub session_total_tokens: Option<u64>,
    pub last_turn_tokens: Option<u64>,
    pub session_delta_tokens: Option<u64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    /// Maximum per-turn total API input (input + cache_creation + cache_read) ever seen across
    /// the session's lifetime, never decreasing -- including across compactions. Only a single
    /// turn exceeding 200K reliably indicates 1M extended context usage. Do not use this for
    /// "how full is the context right now"; see `current_context_tokens`.
    pub max_turn_api_input: u64,
    /// Total API input (input + cache_creation + cache_read) as of the most recent turn since
    /// the session's last compaction (or session start, if none) -- the field that answers
    /// "how full is the context right now". Resets at each `compact_boundary` event to the
    /// real post-compaction size Claude Code itself reports.
    pub current_context_tokens: u64,
    pub reasoning_effort: ReasoningEffort,
    /// True when the effort was read from an explicit JSONL system-reminder
    /// injection (reasoning effort level: X / <reasoning_effort>NN</…>).
    /// False when we only have the settings.json default — in that case the
    /// effort might not match what the user has selected in Claude Desktop's
    /// composer (which lives in Electron memory only).
    #[serde(default)]
    pub reasoning_effort_explicit: bool,
    pub has_thinking_blocks: bool,
    /// Speed of the most recent assistant turn (for display). Cost accumulation
    /// is speed-correct per turn even when speed varies mid-session.
    #[serde(default)]
    pub speed: Speed,
    /// Service tier of the most recent assistant turn ("priority"/"standard").
    /// Tracked for display only — orthogonal to speed and does not change price.
    #[serde(default)]
    pub service_tier: Option<String>,
    pub total_cost: f64,
    /// Speed-aware per-turn accumulation of each billable category. Mirrors how
    /// `total_cost` is accumulated, so `input_cost + output_cost +
    /// cache_write_cost + cache_read_cost == total_cost` to f64 tolerance.
    #[serde(default)]
    pub input_cost: f64,
    #[serde(default)]
    pub output_cost: f64,
    #[serde(default)]
    pub cache_write_cost: f64,
    #[serde(default)]
    pub cache_read_cost: f64,
    pub total_api_duration_ms: u64,
    pub limits: RateLimits,
    pub activity: Option<ActivitySnapshot>,
    pub started_at: Option<DateTime<Utc>>,
    pub last_token_event_at: Option<DateTime<Utc>>,
    pub last_activity: SystemTime,
    pub source: DataSource,
    pub source_file: PathBuf,
    #[serde(default)]
    pub background_work: crate::workflow_state::BackgroundWorkInfo,
    /// Subagents spawned by this session (populated after collection).
    #[serde(default)]
    pub subagents: Vec<SubagentInfo>,
    /// True if this session IS a subagent (not a parent session).
    #[serde(default)]
    pub is_subagent: bool,
    /// Parent session ID for subagent sessions.
    #[serde(default)]
    pub parent_session_id: Option<String>,
}

impl ClaudeSessionSnapshot {
    /// True when extended thinking is active with High reasoning effort (ULTRATHINK mode).
    pub fn is_ultrathinking(&self) -> bool {
        self.has_thinking_blocks && matches!(self.reasoning_effort, ReasoningEffort::Max)
    }
}

#[derive(Debug, Default)]
pub struct GitBranchCache {
    ttl: Duration,
    entries: HashMap<PathBuf, CachedBranch>,
}

#[derive(Debug, Clone)]
struct CachedBranch {
    value: Option<String>,
    expires_at: Instant,
}

impl GitBranchCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: HashMap::new(),
        }
    }

    pub fn get(&mut self, project_path: &Path) -> Option<String> {
        if project_path.as_os_str().is_empty() || !project_path.exists() {
            return None;
        }

        let key = project_path.to_path_buf();
        if let Some(cached) = self.entries.get(&key)
            && Instant::now() < cached.expires_at
        {
            return cached.value.clone();
        }

        let value = fetch_git_branch(project_path);
        self.entries.insert(
            key,
            CachedBranch {
                value: value.clone(),
                expires_at: Instant::now() + self.ttl,
            },
        );
        value
    }
}

fn fetch_git_branch(project_path: &Path) -> Option<String> {
    let output = crate::util::silent_command("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() || branch == "HEAD" {
        let sha_output = crate::util::silent_command("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(project_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;

        if sha_output.status.success() {
            let sha = String::from_utf8_lossy(&sha_output.stdout)
                .trim()
                .to_string();
            if !sha.is_empty() {
                return Some(sha);
            }
        }
        return None;
    }

    Some(branch)
}

#[derive(Debug, Default)]
pub struct SessionParseCache {
    entries: HashMap<PathBuf, CachedSessionEntry>,
}

#[derive(Debug)]
struct CachedSessionEntry {
    cursor: u64,
    file_len: u64,
    modified: SystemTime,
    tail_anchor: Vec<u8>,
    body_bytes_read_last_poll: u64,
    anchor_bytes_read_last_poll: u64,
    persisted_cursor: u64,
    persisted_file_len: u64,
    accumulator: SessionAccumulator,
    snapshot: Option<ClaudeSessionSnapshot>,
}

impl CachedSessionEntry {
    fn new(modified: SystemTime, default_effort: ReasoningEffort) -> Self {
        Self {
            cursor: 0,
            file_len: 0,
            modified,
            tail_anchor: Vec::new(),
            body_bytes_read_last_poll: 0,
            anchor_bytes_read_last_poll: 0,
            persisted_cursor: 0,
            persisted_file_len: 0,
            accumulator: SessionAccumulator::with_default_effort(default_effort),
            snapshot: None,
        }
    }

    fn reset(&mut self, modified: SystemTime, default_effort: ReasoningEffort) {
        self.cursor = 0;
        self.file_len = 0;
        self.modified = modified;
        self.tail_anchor.clear();
        self.body_bytes_read_last_poll = 0;
        self.anchor_bytes_read_last_poll = 0;
        self.persisted_cursor = 0;
        self.persisted_file_len = 0;
        self.accumulator = SessionAccumulator::with_default_effort(default_effort);
        self.snapshot = None;
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistentSessionCheckpoint {
    schema_version: u32,
    /// Build that computed `accumulator` costs. Missing in checkpoints written
    /// before 1.9.4, which therefore never match.
    #[serde(default)]
    app_version: String,
    source_path: PathBuf,
    cursor: u64,
    file_len: u64,
    modified: SystemTime,
    tail_anchor: Vec<u8>,
    accumulator: SessionAccumulator,
    snapshot: Option<ClaudeSessionSnapshot>,
}

fn session_checkpoint_root(path: &Path) -> Option<PathBuf> {
    config::projects_paths()
        .into_iter()
        .find_map(|projects_root| {
            path.starts_with(&projects_root).then(|| {
                projects_root
                    .parent()
                    .unwrap_or(&projects_root)
                    .join("pulse-session-checkpoints")
                    .join(format!("v{SESSION_CHECKPOINT_SCHEMA_VERSION}"))
            })
        })
}

fn session_checkpoint_path(checkpoint_root: &Path, source_path: &Path) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source_path.hash(&mut hasher);
    checkpoint_root.join(format!("{:016x}.json", hasher.finish()))
}

fn persist_session_checkpoint_to(
    checkpoint_root: &Path,
    source_path: &Path,
    entry: &CachedSessionEntry,
) -> Result<()> {
    fs::create_dir_all(checkpoint_root).with_context(|| {
        format!(
            "cannot create session checkpoint directory {}",
            checkpoint_root.display()
        )
    })?;
    let checkpoint = PersistentSessionCheckpoint {
        schema_version: SESSION_CHECKPOINT_SCHEMA_VERSION,
        app_version: SESSION_CHECKPOINT_APP_VERSION.to_string(),
        source_path: source_path.to_path_buf(),
        cursor: entry.cursor,
        file_len: entry.file_len,
        modified: entry.modified,
        tail_anchor: entry.tail_anchor.clone(),
        accumulator: entry.accumulator.clone(),
        snapshot: entry.snapshot.clone(),
    };
    let destination = session_checkpoint_path(checkpoint_root, source_path);
    let mut temporary = tempfile::NamedTempFile::new_in(checkpoint_root).with_context(|| {
        format!(
            "cannot create temporary session checkpoint in {}",
            checkpoint_root.display()
        )
    })?;
    serde_json::to_writer(&mut temporary, &checkpoint)
        .context("cannot serialize session checkpoint")?;
    temporary
        .flush()
        .context("cannot flush session checkpoint")?;
    temporary
        .as_file_mut()
        .sync_all()
        .context("cannot sync session checkpoint")?;
    temporary.persist(&destination).map_err(|error| {
        anyhow::Error::new(error.error).context(format!(
            "cannot atomically replace session checkpoint {}",
            destination.display()
        ))
    })?;
    Ok(())
}

fn load_session_checkpoint_from(
    checkpoint_root: &Path,
    source_path: &Path,
    metadata: &std::fs::Metadata,
    modified: SystemTime,
) -> Option<CachedSessionEntry> {
    let checkpoint_path = session_checkpoint_path(checkpoint_root, source_path);
    let checkpoint_metadata = fs::metadata(&checkpoint_path).ok()?;
    if checkpoint_metadata.len() > SESSION_CHECKPOINT_MAX_BYTES {
        return None;
    }
    let bytes = fs::read(&checkpoint_path).ok()?;
    let checkpoint: PersistentSessionCheckpoint = serde_json::from_slice(&bytes).ok()?;
    if checkpoint.schema_version != SESSION_CHECKPOINT_SCHEMA_VERSION
        || checkpoint.app_version != SESSION_CHECKPOINT_APP_VERSION
        || checkpoint.source_path != source_path
        || checkpoint.cursor > checkpoint.file_len
        || checkpoint.file_len > metadata.len()
        || checkpoint.modified > modified
        || checkpoint.tail_anchor.len() != checkpoint.cursor.min(SESSION_TAIL_ANCHOR_BYTES) as usize
        || checkpoint
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.source_file != source_path)
    {
        return None;
    }

    let mut source = std::fs::File::open(source_path).ok()?;
    if read_tail_anchor(&mut source, checkpoint.cursor).ok()? != checkpoint.tail_anchor {
        return None;
    }

    Some(CachedSessionEntry {
        cursor: checkpoint.cursor,
        file_len: checkpoint.file_len,
        modified: checkpoint.modified,
        tail_anchor: checkpoint.tail_anchor,
        body_bytes_read_last_poll: 0,
        anchor_bytes_read_last_poll: 0,
        persisted_cursor: checkpoint.cursor,
        persisted_file_len: checkpoint.file_len,
        accumulator: checkpoint.accumulator,
        snapshot: checkpoint.snapshot,
    })
}

fn load_session_checkpoint(
    source_path: &Path,
    metadata: &std::fs::Metadata,
    modified: SystemTime,
) -> Option<CachedSessionEntry> {
    let checkpoint_root = session_checkpoint_root(source_path)?;
    load_session_checkpoint_from(&checkpoint_root, source_path, metadata, modified)
}

fn persist_session_checkpoint(source_path: &Path, entry: &mut CachedSessionEntry) {
    if entry.persisted_cursor == entry.cursor && entry.persisted_file_len == entry.file_len {
        return;
    }
    let Some(checkpoint_root) = session_checkpoint_root(source_path) else {
        return;
    };
    match persist_session_checkpoint_to(&checkpoint_root, source_path, entry) {
        Ok(()) => {
            entry.persisted_cursor = entry.cursor;
            entry.persisted_file_len = entry.file_len;
        }
        Err(error) => tracing::warn!(
            path = %source_path.display(),
            %error,
            "failed to persist derived session checkpoint"
        ),
    }
}

fn read_tail_anchor(file: &mut std::fs::File, cursor: u64) -> Result<Vec<u8>> {
    let anchor_len = cursor.min(SESSION_TAIL_ANCHOR_BYTES) as usize;
    if anchor_len == 0 {
        return Ok(Vec::new());
    }
    file.seek(SeekFrom::Start(cursor - anchor_len as u64))?;
    let mut anchor = vec![0; anchor_len];
    file.read_exact(&mut anchor)?;
    Ok(anchor)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SessionAccumulator {
    #[allow(dead_code)]
    session_id: Option<String>,
    cwd: Option<PathBuf>,
    started_at: Option<DateTime<Utc>>,
    model: Option<String>,
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_cache_creation_tokens: u64,
    total_cache_read_tokens: u64,
    /// Maximum per-turn API input (input + cache_creation + cache_read) ever seen in this
    /// session. Never decreases, including across compactions -- this answers "has this
    /// session ever required the 1M context tier", a lifetime question. Do not use this for
    /// "how full is the context right now"; see `current_context_tokens`.
    max_turn_api_input: u64,
    /// Total API input (input + cache_creation + cache_read) as of the most recent turn since
    /// the last compaction, or since session start if none. Resets to the real post-compaction
    /// size reported in a `compact_boundary` event's `compactMetadata.postTokens`, then tracks
    /// each subsequent turn's own total -- this is the field that answers "how full is the
    /// context right now".
    current_context_tokens: u64,
    total_cost: f64,
    input_cost: f64,
    output_cost: f64,
    cache_write_cost: f64,
    cache_read_cost: f64,
    session_total_tokens: Option<u64>,
    previous_session_total_tokens: Option<u64>,
    last_turn_tokens: Option<u64>,
    limits: RateLimits,
    last_token_event_at: Option<DateTime<Utc>>,
    activity_tracker: ActivityTracker,
    reasoning_effort: ReasoningEffort,
    reasoning_effort_explicitly_set: bool,
    /// True once a main-chain (non-sidechain) line supplied an effort value.
    /// Locks out subagent effort overrides from rewriting the session's tier.
    effort_from_main_chain: bool,
    has_thinking_blocks: bool,
    /// Speed of the most recent assistant turn (for display).
    speed: Speed,
    /// Service tier of the most recent assistant turn (for display).
    service_tier: Option<String>,
    /// Most recent full file path from Read/Write/Edit tool calls.
    /// Used to determine active workspace in multi-workspace VS Code setups.
    last_file_target: Option<PathBuf>,
    /// Usage already counted for each API response, keyed by `message.id`.
    /// Claude Code writes one JSONL line per content block (thinking, text,
    /// tool_use), and every line repeats the response's usage; `output_tokens`
    /// can grow while the response streams. Each response is counted once,
    /// at its latest reported usage.
    #[serde(default)]
    counted_usage: HashMap<String, CountedUsage>,
}

/// Tokens and category costs one API response contributed to the totals.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct CountedUsage {
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    input_cost: f64,
    output_cost: f64,
    cache_write_cost: f64,
    cache_read_cost: f64,
}

impl SessionAccumulator {
    fn add_usage(&mut self, usage: CountedUsage, sign: f64) {
        if sign >= 0.0 {
            self.total_input_tokens +=
                usage.input_tokens + usage.cache_creation_tokens + usage.cache_read_tokens;
            self.total_output_tokens += usage.output_tokens;
            self.total_cache_creation_tokens += usage.cache_creation_tokens;
            self.total_cache_read_tokens += usage.cache_read_tokens;
        } else {
            self.total_input_tokens = self.total_input_tokens.saturating_sub(
                usage.input_tokens + usage.cache_creation_tokens + usage.cache_read_tokens,
            );
            self.total_output_tokens = self.total_output_tokens.saturating_sub(usage.output_tokens);
            self.total_cache_creation_tokens = self
                .total_cache_creation_tokens
                .saturating_sub(usage.cache_creation_tokens);
            self.total_cache_read_tokens = self
                .total_cache_read_tokens
                .saturating_sub(usage.cache_read_tokens);
        }
        self.input_cost = (self.input_cost + sign * usage.input_cost).max(0.0);
        self.output_cost = (self.output_cost + sign * usage.output_cost).max(0.0);
        self.cache_write_cost = (self.cache_write_cost + sign * usage.cache_write_cost).max(0.0);
        self.cache_read_cost = (self.cache_read_cost + sign * usage.cache_read_cost).max(0.0);
        self.total_cost = (self.total_cost
            + sign
                * (usage.input_cost
                    + usage.output_cost
                    + usage.cache_write_cost
                    + usage.cache_read_cost))
            .max(0.0);
    }
}

impl SessionAccumulator {
    fn with_default_effort(effort: ReasoningEffort) -> Self {
        Self {
            reasoning_effort: effort,
            ..Default::default()
        }
    }
}

const IDLE_DEBOUNCE_SECS: i64 = 45;
const ORPHANED_PENDING_CALL_SECS: i64 = 600;
const LIVE_FILE_ACTIVITY_SECS: i64 = 30;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ActivityTracker {
    snapshot: Option<ActivitySnapshot>,
    pending_calls: HashMap<String, PendingActivity>,
    last_event_at: Option<DateTime<Utc>>,
    last_effective_signal_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingActivity {
    #[allow(dead_code)]
    kind: ActivityKind,
    #[allow(dead_code)]
    target: Option<String>,
}

impl ActivityTracker {
    fn observe_timestamp(&mut self, observed_at: Option<DateTime<Utc>>) {
        if let Some(ts) = observed_at {
            self.last_event_at = max_datetime(self.last_event_at, Some(ts));
        }
    }

    fn observe_effective_signal(&mut self, observed_at: Option<DateTime<Utc>>) {
        self.observe_timestamp(observed_at);
        self.last_effective_signal_at = max_datetime(self.last_effective_signal_at, observed_at);
        if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.last_effective_signal_at =
                max_datetime(snapshot.last_effective_signal_at, observed_at);
        }
    }

    fn mark_activity(
        &mut self,
        kind: ActivityKind,
        target: Option<String>,
        observed_at: Option<DateTime<Utc>>,
    ) {
        self.observe_effective_signal(observed_at);
        let previous_active = self.snapshot.as_ref().and_then(|item| item.last_active_at);
        let last_active_at = max_datetime(previous_active, observed_at);
        let idle_candidate_at = if self.pending_calls.is_empty()
            && !matches!(kind, ActivityKind::Idle | ActivityKind::WaitingInput)
        {
            last_active_at
        } else {
            None
        };

        self.snapshot = Some(ActivitySnapshot {
            kind,
            target,
            observed_at,
            last_active_at,
            last_effective_signal_at: self.last_effective_signal_at,
            idle_candidate_at,
            pending_calls: self.pending_calls.len(),
        });
    }

    fn note_commentary(&mut self, observed_at: Option<DateTime<Utc>>) {
        self.observe_effective_signal(observed_at);
        let should_promote = self.snapshot.as_ref().is_none_or(|snapshot| {
            matches!(
                snapshot.kind,
                ActivityKind::Idle | ActivityKind::WaitingInput
            )
        });
        if should_promote {
            self.mark_activity(ActivityKind::Thinking, None, observed_at);
        }
    }

    fn register_call(
        &mut self,
        call_id: &str,
        kind: ActivityKind,
        target: Option<String>,
        observed_at: Option<DateTime<Utc>>,
    ) {
        self.pending_calls.insert(
            call_id.to_string(),
            PendingActivity {
                kind: kind.clone(),
                target: target.clone(),
            },
        );
        self.mark_activity(kind, target, observed_at);
    }

    fn resolve_call(&mut self, call_id: &str, observed_at: Option<DateTime<Utc>>) {
        self.pending_calls.remove(call_id);
        self.observe_effective_signal(observed_at);

        if self.pending_calls.is_empty() {
            if let Some(snapshot) = self.snapshot.as_mut() {
                snapshot.pending_calls = 0;
                snapshot.idle_candidate_at = observed_at;
            }
        } else if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.pending_calls = self.pending_calls.len();
        }
    }

    fn apply_idle_debounce(&mut self, now: DateTime<Utc>) {
        let Some(ref snapshot) = self.snapshot else {
            return;
        };
        if matches!(
            snapshot.kind,
            ActivityKind::Idle | ActivityKind::WaitingInput
        ) {
            return;
        }
        if !self.pending_calls.is_empty() {
            return;
        }
        let Some(idle_candidate) = snapshot.idle_candidate_at else {
            return;
        };
        let elapsed = now.signed_duration_since(idle_candidate).num_seconds();
        if elapsed >= IDLE_DEBOUNCE_SECS
            && let Some(snapshot) = self.snapshot.as_mut()
        {
            snapshot.kind = ActivityKind::Idle;
            snapshot.target = None;
            snapshot.observed_at = snapshot.last_active_at.or(snapshot.observed_at);
            snapshot.idle_candidate_at = None;
            snapshot.pending_calls = 0;
        }
    }

    fn note_live_file_activity(&mut self, modified_at: DateTime<Utc>, now: DateTime<Utc>) {
        if now.signed_duration_since(modified_at).num_seconds() > LIVE_FILE_ACTIVITY_SECS {
            return;
        }

        self.observe_effective_signal(Some(modified_at));
        if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.last_active_at = max_datetime(snapshot.last_active_at, Some(modified_at));
            snapshot.last_effective_signal_at =
                max_datetime(snapshot.last_effective_signal_at, Some(modified_at));
            if matches!(snapshot.kind, ActivityKind::Idle) {
                snapshot.kind = ActivityKind::Thinking;
                snapshot.target = None;
            }
            if self.pending_calls.is_empty() && is_working_activity_kind(&snapshot.kind) {
                snapshot.idle_candidate_at = Some(modified_at);
            }
        }
    }

    fn finalize(&self, now: DateTime<Utc>) -> Option<ActivitySnapshot> {
        let mut snapshot = self.snapshot.clone()?;
        snapshot.pending_calls = self.pending_calls.len();

        let last_signal = snapshot
            .last_effective_signal_at
            .or(self.last_effective_signal_at)
            .or(snapshot.last_active_at)
            .or(snapshot.observed_at)
            .or(self.last_event_at);

        if snapshot.pending_calls > 0 {
            if let Some(last_signal) = last_signal
                && now.signed_duration_since(last_signal).num_seconds() < ORPHANED_PENDING_CALL_SECS
            {
                snapshot.idle_candidate_at = None;
                return Some(snapshot);
            }
            snapshot.pending_calls = 0;
            snapshot.idle_candidate_at = last_signal;
        }

        if !matches!(
            snapshot.kind,
            ActivityKind::Idle | ActivityKind::WaitingInput
        ) {
            let idle_candidate = snapshot
                .idle_candidate_at
                .or(snapshot.last_active_at)
                .or(snapshot.observed_at)
                .or(last_signal);
            if let Some(idle_candidate) = idle_candidate
                && now.signed_duration_since(idle_candidate).num_seconds() >= IDLE_DEBOUNCE_SECS
            {
                snapshot.kind = ActivityKind::Idle;
                snapshot.target = None;
            }
        }

        Some(snapshot)
    }
}

fn max_datetime(a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (a, b) {
        (Some(va), Some(vb)) => Some(va.max(vb)),
        (Some(va), None) => Some(va),
        (None, Some(vb)) => Some(vb),
        (None, None) => None,
    }
}

#[derive(Debug, Deserialize)]
struct StatusLineData {
    session_id: String,
    cwd: String,
    model: StatusLineModel,
    workspace: StatusLineWorkspace,
    cost: StatusLineCost,
    context_window: StatusLineContextWindow,
}

#[derive(Debug, Deserialize)]
struct StatusLineModel {
    id: String,
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct StatusLineWorkspace {
    #[serde(default)]
    #[allow(dead_code)]
    current_dir: String,
    #[serde(default)]
    project_dir: String,
}

#[derive(Debug, Deserialize)]
struct StatusLineCost {
    total_cost_usd: f64,
    #[serde(default)]
    total_duration_ms: i64,
    #[serde(default)]
    total_api_duration_ms: i64,
}

#[derive(Debug, Deserialize)]
struct StatusLineContextWindow {
    #[serde(default)]
    total_input_tokens: u64,
    #[serde(default)]
    total_output_tokens: u64,
}

pub fn read_statusline_data(git_cache: &mut GitBranchCache) -> Option<ClaudeSessionSnapshot> {
    let data_path = config::statusline_data_path();

    let file_info = fs::metadata(&data_path).ok()?;
    let modified = file_info.modified().ok()?;

    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    if age > Duration::from_secs(60) {
        return None;
    }

    let data = fs::read_to_string(&data_path).ok()?;
    let status: StatusLineData = serde_json::from_str(&data).ok()?;

    if status.session_id.is_empty()
        || status.session_id.len() < 10
        || !status.session_id.contains('-')
    {
        return None;
    }

    let project_path = if !status.workspace.project_dir.is_empty() {
        status.workspace.project_dir.clone()
    } else if !status.cwd.is_empty() {
        status.cwd.clone()
    } else {
        return None;
    };

    let sanitized_model_id = non_synthetic_model(&status.model.id);
    let sanitized_display_name = non_synthetic_display_name(&status.model.display_name);

    if sanitized_model_id.is_none() && sanitized_display_name.is_none() {
        return None;
    }
    if status.model.display_name == "Test" {
        return None;
    }

    let project_name = derive_project_name(Path::new(&project_path));

    let git_branch = git_cache.get(Path::new(&project_path));
    let total_tokens =
        status.context_window.total_input_tokens + status.context_window.total_output_tokens;

    let started_at = if status.cost.total_duration_ms > 0 {
        Some(Utc::now() - chrono::Duration::milliseconds(status.cost.total_duration_ms))
    } else {
        None
    };

    Some(ClaudeSessionSnapshot {
        session_id: status.session_id,
        cwd: PathBuf::from(&project_path),
        project_name,
        git_branch,
        model: sanitized_model_id.clone(),
        model_display: sanitized_display_name.or_else(|| {
            sanitized_model_id
                .as_ref()
                .map(|model_id| cost::model_display_name(model_id).to_string())
        }),
        session_total_tokens: Some(total_tokens),
        last_turn_tokens: None,
        session_delta_tokens: None,
        input_tokens: status.context_window.total_input_tokens,
        output_tokens: status.context_window.total_output_tokens,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        max_turn_api_input: 0,
        current_context_tokens: 0,
        reasoning_effort: read_claude_effort_level(),
        reasoning_effort_explicit: false,
        has_thinking_blocks: false,
        speed: Speed::default(),
        service_tier: None,
        total_cost: status.cost.total_cost_usd,
        input_cost: 0.0,
        output_cost: 0.0,
        cache_write_cost: 0.0,
        cache_read_cost: 0.0,
        total_api_duration_ms: status.cost.total_api_duration_ms.max(0) as u64,
        limits: RateLimits::default(),
        activity: None,
        started_at,
        last_token_event_at: None,
        last_activity: modified,
        source: DataSource::Statusline,
        source_file: data_path,
        background_work: crate::workflow_state::BackgroundWorkInfo::default(),
        subagents: Vec::new(),
        is_subagent: false,
        parent_session_id: None,
    })
}

/// Apportion an authoritative session total across the JSONL-derived per-category cost
/// proportions. Returns the multiplier to apply to each JSONL category so the four
/// categories sum to `authoritative_total`; `0.0` when there is no JSONL cost basis.
fn category_scale(authoritative_total: f64, jsonl_total: f64) -> f64 {
    if jsonl_total.abs() <= f64::EPSILON {
        0.0
    } else {
        authoritative_total / jsonl_total
    }
}

/// Merge a statusline-sourced session into JSONL-parsed sessions.
/// The statusline `total_cost_usd` is Claude Code's own authoritative billed total (it
/// already reflects fast mode, priority tier, and anything the CLI knows), so it wins the
/// headline total + model. JSONL supplies the speed-aware per-category cost proportions —
/// scaled to reconcile with that authoritative total — plus granular tokens + activity.
pub fn merge_statusline_into_sessions(
    sessions: &mut Vec<ClaudeSessionSnapshot>,
    statusline: ClaudeSessionSnapshot,
) {
    let existing_idx = sessions
        .iter()
        .position(|s| s.session_id == statusline.session_id);
    if let Some(idx) = existing_idx {
        let jsonl = &sessions[idx];
        let merged_model = statusline.model.clone().or_else(|| jsonl.model.clone());
        let merged_model_display = statusline
            .model_display
            .clone()
            .or_else(|| jsonl.model_display.clone());
        let scale = category_scale(statusline.total_cost, jsonl.total_cost);
        let merged = ClaudeSessionSnapshot {
            last_turn_tokens: statusline.last_turn_tokens.or(jsonl.last_turn_tokens),
            session_delta_tokens: statusline
                .session_delta_tokens
                .or(jsonl.session_delta_tokens),
            max_turn_api_input: jsonl.max_turn_api_input.max(statusline.max_turn_api_input),
            current_context_tokens: jsonl.current_context_tokens,
            activity: statusline.activity.clone().or(jsonl.activity.clone()),
            last_token_event_at: statusline.last_token_event_at.or(jsonl.last_token_event_at),
            model: merged_model,
            model_display: merged_model_display,
            reasoning_effort: jsonl.reasoning_effort,
            reasoning_effort_explicit: jsonl.reasoning_effort_explicit,
            has_thinking_blocks: jsonl.has_thinking_blocks,
            background_work: jsonl.background_work,
            speed: jsonl.speed,
            service_tier: jsonl.service_tier.clone(),
            input_cost: jsonl.input_cost * scale,
            output_cost: jsonl.output_cost * scale,
            cache_write_cost: jsonl.cache_write_cost * scale,
            cache_read_cost: jsonl.cache_read_cost * scale,
            ..statusline
        };
        sessions[idx] = merged;
    } else {
        sessions.insert(0, statusline);
    }
}

/// Claude Code JSONL message format
#[derive(Debug, Deserialize)]
struct JsonlMessage {
    #[serde(rename = "type")]
    msg_type: Option<String>,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    message: Option<JsonlMessageContent>,
    #[serde(default)]
    is_api_error_message: bool,
    /// Reasoning-effort tier the turn actually ran at, written by Claude Code as a
    /// **top-level** field on every `assistant` line (`"effort": "high"`), sibling to
    /// `type`/`message`. Values match `ReasoningEffort::from_api`
    /// (`low|medium|high|xhigh|max`). This is the only filesystem signal for the
    /// composer's effort selector — `~/.claude/settings.json` has no `effortLevel`
    /// key in current builds, so without this field every session reads as Medium.
    #[serde(default)]
    effort: Option<String>,
    /// `true` on subagent turns. Subagents can run at their own effort override, so
    /// their `effort` must not overwrite the parent session's selector.
    #[serde(rename = "isSidechain")]
    #[serde(default)]
    is_sidechain: bool,
    #[serde(rename = "compactMetadata")]
    #[serde(default)]
    compact_metadata: Option<CompactMetadata>,
}

/// Metadata Claude Code attaches to a `type: "system", subtype: "compact_boundary"` line when
/// it auto- or manually compacts a session's context. `post_tokens` is the authoritative size
/// of the context immediately after compaction -- the correct baseline for "how full is this
/// session's context right now", as opposed to a running maximum that never decreases.
#[derive(Debug, Deserialize)]
struct CompactMetadata {
    #[serde(rename = "postTokens")]
    #[serde(default)]
    post_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct JsonlMessageContent {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<JsonlUsage>,
    #[serde(default)]
    content: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct JsonlUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    speed: Option<String>,
    #[serde(default)]
    service_tier: Option<String>,
}

pub fn collect_active_sessions(
    git_cache: &mut GitBranchCache,
    parse_cache: &mut SessionParseCache,
    stale_threshold: Duration,
    active_sticky_window: Duration,
) -> Result<Vec<ClaudeSessionSnapshot>> {
    let projects_roots = config::projects_paths();
    let ide_workspaces = config::read_ide_workspace_folders();
    collect_active_sessions_multi(
        &projects_roots,
        stale_threshold,
        active_sticky_window,
        git_cache,
        parse_cache,
        &ide_workspaces,
    )
}

struct SessionFileCandidate {
    path: PathBuf,
    metadata: std::fs::Metadata,
    modified: SystemTime,
    is_subagent: bool,
}

fn is_subagent_session_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str().eq_ignore_ascii_case("subagents"))
}

fn subagent_parent_transcript_path(path: &Path) -> Option<PathBuf> {
    let subagents_dir = path.parent()?;
    if !subagents_dir
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("subagents"))
    {
        return None;
    }
    let session_dir = subagents_dir.parent()?;
    let session_id = session_dir.file_name()?.to_str()?;
    Some(session_dir.parent()?.join(format!("{session_id}.jsonl")))
}

fn selected_session_paths(
    candidates: &[(PathBuf, SystemTime, bool)],
    parse_cutoff: SystemTime,
) -> HashSet<PathBuf> {
    let available = candidates
        .iter()
        .map(|(path, _, _)| path.clone())
        .collect::<HashSet<_>>();
    let mut selected = candidates
        .iter()
        .filter(|(_, modified, _)| *modified >= parse_cutoff)
        .map(|(path, _, _)| path.clone())
        .collect::<HashSet<_>>();

    // Claude stores subagents under `<session-id>/subagents/` while the
    // parent transcript is the sibling `<session-id>.jsonl`. Keep that parent
    // even if its own mtime is older so merge semantics remain intact.
    for (path, modified, is_subagent) in candidates {
        if *is_subagent
            && *modified >= parse_cutoff
            && let Some(parent) = subagent_parent_transcript_path(path)
            && available.contains(&parent)
        {
            selected.insert(parent);
        }
    }
    selected
}

pub fn collect_active_sessions_multi(
    projects_roots: &[PathBuf],
    stale_threshold: Duration,
    active_sticky_window: Duration,
    git_cache: &mut GitBranchCache,
    parse_cache: &mut SessionParseCache,
    ide_workspaces: &[PathBuf],
) -> Result<Vec<ClaudeSessionSnapshot>> {
    let now = SystemTime::now();
    let stale_cutoff = now
        .checked_sub(stale_threshold)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let sticky_cutoff = now
        .checked_sub(active_sticky_window)
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let default_effort = read_claude_effort_level();
    let mut sessions = Vec::new();
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();
    let mut candidates = Vec::new();

    for projects_root in projects_roots {
        if !projects_root.exists() {
            continue;
        }

        for entry in walk_session_root(projects_root) {
            let path = entry.path();
            if !entry.file_type().is_file() {
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }

            let metadata = match entry.metadata() {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            let modified = match metadata.modified() {
                Ok(m) => m,
                Err(_) => continue,
            };

            candidates.push(SessionFileCandidate {
                path: path.to_path_buf(),
                metadata,
                modified,
                is_subagent: is_subagent_session_path(path),
            });
        }
    }

    let parse_cutoff = stale_cutoff.min(sticky_cutoff);
    let selection_input = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.path.clone(),
                candidate.modified,
                candidate.is_subagent,
            )
        })
        .collect::<Vec<_>>();
    let selected_paths = selected_session_paths(&selection_input, parse_cutoff);

    for candidate in candidates {
        if !selected_paths.contains(&candidate.path) {
            continue;
        }
        let path = candidate.path.as_path();
        let modified = candidate.modified;
        let is_subagent_file = candidate.is_subagent;
        seen_paths.insert(candidate.path.clone());

        let encoded_dir = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        if let Some(mut snapshot) = parse_session_file_cached(
            path,
            &candidate.metadata,
            modified,
            &encoded_dir,
            git_cache,
            parse_cache,
            default_effort,
            ide_workspaces,
        )? {
            if !is_subagent_file {
                snapshot.background_work = crate::workflow_state::detect_background_work(path);
            }
            if is_subagent_file {
                snapshot.is_subagent = true;
                snapshot.parent_session_id = subagent_parent_session_id(path);
                if snapshot
                    .parent_session_id
                    .as_ref()
                    .is_some_and(|parent_id| parent_id == &snapshot.session_id)
                    && let Some(agent_id) = path.file_stem().and_then(|item| item.to_str())
                {
                    snapshot.session_id = agent_id.to_string();
                }
                let meta_path = path.with_extension("meta.json");
                if let Ok(meta_data) = fs::read_to_string(&meta_path)
                    && let Ok(meta_json) = serde_json::from_str::<serde_json::Value>(&meta_data)
                    && let Some(agent_type) = meta_json.get("agentType").and_then(|v| v.as_str())
                {
                    snapshot.project_name = format!("subagent:{}", agent_type);
                }
            }

            let recency = session_recency(&snapshot, modified);
            snapshot.last_activity = recency;
            if should_include_session(&snapshot, recency, stale_cutoff, sticky_cutoff) {
                sessions.push(snapshot);
            }
        }
    }

    parse_cache
        .entries
        .retain(|path, _| seen_paths.contains(path));
    sessions = dedupe_sessions_by_id(sessions);

    merge_subagents_into_parents(&mut sessions);

    sessions.sort_by_key(|session| Reverse(session_rank_key(session)));
    Ok(sessions)
}

fn subagent_parent_session_id(path: &Path) -> Option<String> {
    let mut dir = path.parent();
    while let Some(current) = dir {
        if current
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("subagents"))
        {
            return current
                .parent()
                .and_then(|parent| parent.file_name())
                .map(|name| name.to_string_lossy().to_string())
                .filter(|id| is_valid_session_id(id));
        }
        dir = current.parent();
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn parse_session_file_cached(
    path: &Path,
    metadata: &std::fs::Metadata,
    modified: SystemTime,
    encoded_dir: &str,
    git_cache: &mut GitBranchCache,
    parse_cache: &mut SessionParseCache,
    default_effort: ReasoningEffort,
    ide_workspaces: &[PathBuf],
) -> Result<Option<ClaudeSessionSnapshot>> {
    let file_len = metadata.len();
    let path_buf = path.to_path_buf();

    if !parse_cache.entries.contains_key(&path_buf) {
        let restored = load_session_checkpoint(path, metadata, modified)
            .unwrap_or_else(|| CachedSessionEntry::new(modified, default_effort));
        parse_cache.entries.insert(path_buf.clone(), restored);
    }
    let entry = parse_cache
        .entries
        .get_mut(&path_buf)
        .expect("session cache entry was inserted above");

    entry.body_bytes_read_last_poll = 0;
    entry.anchor_bytes_read_last_poll = 0;

    if file_len < entry.file_len || file_len < entry.cursor || modified < entry.modified {
        entry.reset(modified, default_effort);
    }

    if file_len == entry.file_len
        && modified == entry.modified
        && file_len == entry.cursor
        && entry.snapshot.is_some()
    {
        return Ok(entry.snapshot.clone());
    }

    let mut file =
        std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;

    if entry.cursor > 0 && !entry.tail_anchor.is_empty() {
        let observed_anchor = read_tail_anchor(&mut file, entry.cursor).with_context(|| {
            format!(
                "failed to validate incremental cursor in {}",
                path.display()
            )
        })?;
        entry.anchor_bytes_read_last_poll = observed_anchor.len() as u64;
        if observed_anchor != entry.tail_anchor {
            entry.reset(modified, default_effort);
            // Preserve proof that this poll performed the bounded validation
            // which rejected the stale cursor.
            entry.anchor_bytes_read_last_poll = observed_anchor.len() as u64;
        }
    }

    if file_len == entry.cursor && entry.snapshot.is_some() {
        entry.file_len = file_len;
        entry.modified = modified;
        return Ok(entry.snapshot.clone());
    }

    file.seek(SeekFrom::Start(entry.cursor)).with_context(|| {
        format!(
            "failed to seek to cursor {} in {}",
            entry.cursor,
            path.display()
        )
    })?;

    let mut reader = BufReader::new(file);
    let mut new_cursor = entry.cursor;
    let mut line = Vec::new();

    loop {
        line.clear();
        let bytes_read = match reader.read_until(b'\n', &mut line) {
            Ok(bytes_read) => bytes_read,
            Err(_) => break,
        };
        if bytes_read == 0 {
            break;
        }
        entry.body_bytes_read_last_poll = entry
            .body_bytes_read_last_poll
            .saturating_add(bytes_read as u64);

        // A final JSONL record does not require a trailing newline. Parse a
        // syntactically complete record at EOF, but keep the cursor at the
        // fragment start when a writer was interrupted mid-object.
        let terminated = line.last() == Some(&b'\n');
        if terminated {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }

        let line = match std::str::from_utf8(&line) {
            Ok(line) => line,
            Err(_) if terminated => {
                new_cursor = new_cursor.saturating_add(bytes_read as u64);
                continue;
            }
            Err(_) => break,
        };
        if line.trim().is_empty() {
            new_cursor = new_cursor.saturating_add(bytes_read as u64);
            continue;
        }

        let msg: JsonlMessage = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(_) if terminated => {
                new_cursor = new_cursor.saturating_add(bytes_read as u64);
                continue;
            }
            Err(_) => break,
        };

        new_cursor = new_cursor.saturating_add(bytes_read as u64);
        process_jsonl_message(&mut entry.accumulator, &msg);
    }

    entry.cursor = new_cursor;
    entry.file_len = file_len;
    entry.modified = modified;
    let mut file = reader.into_inner();
    entry.tail_anchor = read_tail_anchor(&mut file, entry.cursor)
        .with_context(|| format!("failed to checkpoint cursor in {}", path.display()))?;

    let model_id = match entry.accumulator.model.as_deref() {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => {
            entry.snapshot = None;
            persist_session_checkpoint(path, entry);
            return Ok(None);
        }
    };

    let cwd = entry
        .accumulator
        .cwd
        .clone()
        .unwrap_or_else(|| decode_project_path(encoded_dir));

    let effective_project_root = if config::is_drive_root(&cwd) {
        entry
            .accumulator
            .last_file_target
            .as_deref()
            .and_then(|target| config::find_best_workspace(target, ide_workspaces))
    } else {
        None
    };
    let project_name = derive_project_name(effective_project_root.as_deref().unwrap_or(&cwd));

    let git_branch_path = effective_project_root.as_deref().unwrap_or(&cwd);
    let git_branch = git_cache.get(git_branch_path);
    let total_tokens = entry.accumulator.total_input_tokens + entry.accumulator.total_output_tokens;

    let session_delta = entry.accumulator.session_total_tokens.and_then(|current| {
        entry
            .accumulator
            .previous_session_total_tokens
            .map(|prev| current.saturating_sub(prev))
    });

    let session_id = entry
        .accumulator
        .session_id
        .clone()
        .filter(|session_id| is_valid_session_id(session_id))
        .unwrap_or_else(|| {
            if encoded_dir.is_empty() {
                cwd.to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "_")
            } else {
                format!("encoded_{encoded_dir}")
            }
        });

    let last_turn_tokens = entry.accumulator.last_turn_tokens;
    let input_tokens = entry.accumulator.total_input_tokens;
    let output_tokens = entry.accumulator.total_output_tokens;
    let cache_creation_tokens = entry.accumulator.total_cache_creation_tokens;
    let cache_read_tokens = entry.accumulator.total_cache_read_tokens;
    let max_turn_api_input = entry.accumulator.max_turn_api_input;
    let current_context_tokens = entry.accumulator.current_context_tokens;
    let total_cost = entry.accumulator.total_cost;
    let input_cost = entry.accumulator.input_cost;
    let output_cost = entry.accumulator.output_cost;
    let cache_write_cost = entry.accumulator.cache_write_cost;
    let cache_read_cost = entry.accumulator.cache_read_cost;
    let speed = entry.accumulator.speed;
    let service_tier = entry.accumulator.service_tier.clone();
    let limits = entry.accumulator.limits.clone();
    let started_at = entry.accumulator.started_at;
    let last_token_event_at = entry.accumulator.last_token_event_at;

    let now = Utc::now();
    entry
        .accumulator
        .activity_tracker
        .note_live_file_activity(DateTime::<Utc>::from(modified), now);
    entry.accumulator.activity_tracker.apply_idle_debounce(now);

    let snapshot = ClaudeSessionSnapshot {
        session_id,
        cwd: cwd.clone(),
        project_name,
        git_branch,
        model: Some(model_id.clone()),
        model_display: Some(cost::model_display_name(&model_id).to_string()),
        session_total_tokens: Some(total_tokens),
        last_turn_tokens,
        session_delta_tokens: session_delta,
        input_tokens,
        output_tokens,
        cache_creation_tokens,
        cache_read_tokens,
        max_turn_api_input,
        current_context_tokens,
        reasoning_effort: infer_effort(&entry.accumulator),
        reasoning_effort_explicit: entry.accumulator.reasoning_effort_explicitly_set,
        has_thinking_blocks: entry.accumulator.has_thinking_blocks,
        speed,
        service_tier,
        total_cost,
        input_cost,
        output_cost,
        cache_write_cost,
        cache_read_cost,
        total_api_duration_ms: 0,
        limits,
        activity: entry.accumulator.activity_tracker.finalize(now),
        started_at,
        last_token_event_at,
        last_activity: modified,
        source: DataSource::Jsonl,
        source_file: path_buf,
        background_work: crate::workflow_state::BackgroundWorkInfo::default(),
        subagents: Vec::new(),
        is_subagent: false,
        parent_session_id: None,
    };

    entry.snapshot = Some(snapshot.clone());
    persist_session_checkpoint(path, entry);
    Ok(Some(snapshot))
}

fn process_jsonl_message(acc: &mut SessionAccumulator, msg: &JsonlMessage) {
    let observed_at = msg
        .timestamp
        .as_deref()
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&Utc));

    if acc.started_at.is_none() {
        acc.started_at = observed_at;
    }

    if acc.cwd.is_none()
        && let Some(ref cwd) = msg.cwd
        && !cwd.is_empty()
    {
        acc.cwd = Some(PathBuf::from(cwd));
    }
    if acc.session_id.is_none()
        && let Some(session_id) = msg.session_id.as_deref()
        && is_valid_session_id(session_id)
    {
        acc.session_id = Some(session_id.to_string());
    }

    // Read before any early return: `effort` is a top-level field and the line
    // carrying it (compact boundaries, lines without `message`) may bail below.
    apply_effort_field(acc, msg);

    let msg_type = msg.msg_type.as_deref().unwrap_or("");

    if msg_type == "system" && msg.subtype.as_deref() == Some("compact_boundary") {
        if let Some(post_tokens) = msg.compact_metadata.as_ref().and_then(|m| m.post_tokens) {
            acc.current_context_tokens = post_tokens;
        }
        acc.activity_tracker.observe_timestamp(observed_at);
        return;
    }

    let Some(ref message) = msg.message else {
        acc.activity_tracker.observe_timestamp(observed_at);
        return;
    };

    match msg_type {
        "assistant" => {
            if let Some(ref model) = message.model
                && !model.is_empty()
                && !msg.is_api_error_message
                && !is_synthetic_model_id(model)
            {
                acc.model = Some(model.clone());
            }

            if let Some(ref usage) = message.usage {
                let input = usage.input_tokens;
                let output = usage.output_tokens;
                let cache_creation = usage.cache_creation_input_tokens;
                let cache_read = usage.cache_read_input_tokens;

                let turn_speed = usage
                    .speed
                    .as_deref()
                    .map(Speed::from_label)
                    .unwrap_or_default();
                acc.speed = turn_speed;
                acc.service_tier = usage.service_tier.clone();

                let all_input = input + cache_creation + cache_read;

                acc.max_turn_api_input = acc.max_turn_api_input.max(all_input);
                acc.current_context_tokens = all_input;

                let model_id = acc.model.as_deref().unwrap_or("claude-sonnet-4-20250514");
                let turn_costs = cost::calculate_category_costs(
                    model_id,
                    input,
                    output,
                    cache_creation,
                    cache_read,
                    turn_speed.is_fast(),
                );
                let counted = CountedUsage {
                    input_tokens: input,
                    output_tokens: output,
                    cache_creation_tokens: cache_creation,
                    cache_read_tokens: cache_read,
                    input_cost: turn_costs.input_cost,
                    output_cost: turn_costs.output_cost,
                    cache_write_cost: turn_costs.cache_write_cost,
                    cache_read_cost: turn_costs.cache_read_cost,
                };
                // A repeated response replaces its earlier contribution
                // instead of adding to it.
                if let Some(id) = message.id.as_deref().filter(|id| !id.is_empty())
                    && let Some(previous) = acc.counted_usage.insert(id.to_string(), counted)
                {
                    acc.add_usage(previous, -1.0);
                }
                acc.add_usage(counted, 1.0);

                acc.last_turn_tokens = Some(all_input + output);
                acc.last_token_event_at = observed_at;

                acc.previous_session_total_tokens = acc.session_total_tokens;
                acc.session_total_tokens = Some(acc.total_input_tokens + acc.total_output_tokens);
            }

            if let Some(ref content_val) = message.content {
                process_content_for_activity(acc, content_val, observed_at);
            } else {
                acc.activity_tracker.note_commentary(observed_at);
            }
        }
        "user" => {
            acc.has_thinking_blocks = false;
            // The legacy system-reminder scrape is a fallback for transcripts
            // that predate the top-level `effort` field. Once a main-chain
            // assistant line has supplied the authoritative value, a later
            // reminder — often carried by an interleaved sidechain user event
            // describing a subagent — must not overwrite it.
            if !acc.effort_from_main_chain {
                extract_reasoning_effort(acc, message);
            }
            if let Some(ref content_val) = message.content {
                process_content_for_activity(acc, content_val, observed_at);
            } else {
                acc.activity_tracker.observe_timestamp(observed_at);
            }
        }
        _ => {
            acc.activity_tracker.observe_timestamp(observed_at);
        }
    }
}

fn walk_session_root(root: &Path) -> impl Iterator<Item = walkdir::DirEntry> + '_ {
    walk_session_root_with_limits(root, SESSION_SCAN_MAX_DEPTH, SESSION_SCAN_MAX_ENTRIES)
}

fn walk_session_root_with_limits(
    root: &Path,
    max_depth: usize,
    max_entries: usize,
) -> impl Iterator<Item = walkdir::DirEntry> + '_ {
    WalkDir::new(root)
        .max_depth(max_depth)
        .into_iter()
        .take(max_entries)
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(err) => {
                tracing::warn!(
                    root = %root.display(),
                    error = %err,
                    "failed to inspect Claude session path"
                );
                None
            }
        })
}

/// Scan a `message.content` value (array or single object) for tool_use / tool_result entries.
fn process_content_for_activity(
    acc: &mut SessionAccumulator,
    content_val: &Value,
    observed_at: Option<DateTime<Utc>>,
) {
    let items: Vec<&Value> = if let Some(arr) = content_val.as_array() {
        arr.iter().collect()
    } else if content_val.is_object() {
        vec![content_val]
    } else {
        acc.activity_tracker.note_commentary(observed_at);
        return;
    };

    let mut found_activity = false;
    for item in items {
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match item_type {
            "tool_use" => {
                let (kind, target) = classify_tool_call(item);
                if matches!(kind, ActivityKind::ReadingFile | ActivityKind::EditingFile)
                    && let Some(full_path) = extract_full_file_path(item.get("input"))
                {
                    acc.last_file_target = Some(PathBuf::from(full_path));
                }
                let call_id = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                acc.activity_tracker
                    .register_call(&call_id, kind, target, observed_at);
                found_activity = true;
            }
            "thinking" => {
                acc.has_thinking_blocks = true;
            }
            "tool_result" => {
                let call_id = item
                    .get("tool_use_id")
                    .or_else(|| item.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                acc.activity_tracker.resolve_call(&call_id, observed_at);
                found_activity = true;
            }
            _ => {}
        }
    }

    if !found_activity {
        acc.activity_tracker.note_commentary(observed_at);
    }
}

/// Return the effort we are actually confident about.
///
/// Current Claude Code builds DO record the composer's effort selector: every
/// `assistant` transcript line carries a top-level `"effort"` field
/// (`low|medium|high|xhigh|max`), which `apply_effort_field` reads. That is the
/// authoritative source. The older `<reasoning_effort>` / "reasoning effort
/// level: X" system-reminder scraping is kept as a fallback for legacy
/// transcripts, and `settings.json`'s `effortLevel` seeds the initial default.
///
/// No inference: this function never upgrades `Medium → High` just because
/// thinking blocks appeared. Confidently wrong is worse than honest uncertainty.
fn infer_effort(acc: &SessionAccumulator) -> ReasoningEffort {
    acc.reasoning_effort
}

/// Apply the top-level `effort` field Claude Code writes on transcript lines.
///
/// Latest main-chain value wins, so switching the selector mid-session is
/// reflected on the next turn.
///
/// Sidechain lines are ignored outright. A subagent may run at its own effort
/// override, and those lines are interleaved into the parent transcript — a
/// subagent that answers before the parent's first turn would otherwise define
/// the parent's tier for the whole session. Nothing displays a subagent's own
/// effort today (`SubagentInfo` carries model, activity, tokens and cost), so
/// dropping it costs no information and removes an ordering-dependent lie.
fn apply_effort_field(acc: &mut SessionAccumulator, msg: &JsonlMessage) {
    if msg.is_sidechain {
        return;
    }
    let Some(raw) = msg.effort.as_deref() else {
        return;
    };
    let Some(effort) = ReasoningEffort::from_api(raw) else {
        return;
    };

    acc.effort_from_main_chain = true;
    acc.reasoning_effort = effort;
    acc.reasoning_effort_explicitly_set = true;
}

/// Extract reasoning effort level from system-reminder text injected into user messages.
/// Searches for both `"reasoning effort level: X"` and `"antml:reasoning_effort"` XML tags.
fn extract_reasoning_effort(acc: &mut SessionAccumulator, message: &JsonlMessageContent) {
    let Some(ref content_val) = message.content else {
        return;
    };

    let mut set_effort = |text: &str| {
        if let Some(pos) = text.find("reasoning effort level: ") {
            let after = &text[pos + 24..];
            let level = after
                .split(|c: char| !c.is_alphanumeric())
                .next()
                .unwrap_or("");
            if let Some(effort) = parse_effort_level(level) {
                acc.reasoning_effort = effort;
                acc.reasoning_effort_explicitly_set = true;
            }
        }
        if let Some(pos) = text.find("antml:reasoning_effort>") {
            let after = &text[pos + 23..];
            let value = after.split('<').next().unwrap_or("").trim();
            if let Ok(n) = value.parse::<u32>() {
                let effort = match n {
                    0..=25 => ReasoningEffort::Low,
                    26..=55 => ReasoningEffort::Medium,
                    56..=80 => ReasoningEffort::High,
                    81..=99 => ReasoningEffort::ExtraHigh,
                    _ => ReasoningEffort::Max,
                };
                acc.reasoning_effort = effort;
                acc.reasoning_effort_explicitly_set = true;
            } else if let Some(effort) = parse_effort_level(value) {
                acc.reasoning_effort = effort;
                acc.reasoning_effort_explicitly_set = true;
            }
        }
    };

    if let Some(s) = content_val.as_str() {
        set_effort(s);
    } else if let Some(arr) = content_val.as_array() {
        for item in arr {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                set_effort(text);
            }
            if let Some(content) = item.get("content").and_then(|v| v.as_str()) {
                set_effort(content);
            }
        }
    }
}

fn parse_effort_level(s: &str) -> Option<ReasoningEffort> {
    ReasoningEffort::from_api(s)
}

fn classify_tool_call(content: &Value) -> (ActivityKind, Option<String>) {
    let name = content.get("name").and_then(|v| v.as_str()).unwrap_or("");

    let input = content.get("input");

    match name {
        "Read" | "read" | "file_read" | "NotebookRead" => {
            let target = extract_filename(input);
            (ActivityKind::ReadingFile, target)
        }
        "Write" | "write" | "Edit" | "edit" | "file_write" | "NotebookEdit" => {
            let target = extract_filename(input);
            (ActivityKind::EditingFile, target)
        }
        "Glob" | "glob" | "Grep" | "grep" | "Search" | "search" => {
            let target = input
                .and_then(|v| v.get("pattern"))
                .and_then(|v| v.as_str())
                .map(|s| truncate_target(s, 40));
            (ActivityKind::ReadingFile, target)
        }
        "Bash" | "bash" | "command_execute" | "terminal" => {
            let command = input
                .and_then(|v| v.get("command"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            classify_shell_activity(command)
        }
        "AskUserQuestion" | "ask_user" => (ActivityKind::WaitingInput, None),
        "WebSearch" | "WebFetch" | "web_search" | "web_fetch" => (ActivityKind::Thinking, None),
        _ => (ActivityKind::Thinking, None),
    }
}

/// Extract the full file path from a tool_use input field (for workspace detection).
fn extract_full_file_path(input: Option<&Value>) -> Option<String> {
    input
        .and_then(|v| {
            v.get("file_path")
                .or(v.get("path"))
                .or(v.get("notebook_path"))
        })
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Extract just the filename from a file_path/path input field.
fn extract_filename(input: Option<&Value>) -> Option<String> {
    input
        .and_then(|v| {
            v.get("file_path")
                .or(v.get("path"))
                .or(v.get("notebook_path"))
        })
        .and_then(|v| v.as_str())
        .map(|s| {
            Path::new(s)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| truncate_target(s, 40))
        })
}

/// Truncate a target string, respecting UTF-8 char boundaries.
fn truncate_target(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let suffix_len = 3;
        let mut end = max_len.saturating_sub(suffix_len);
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

fn summarize_command_for_presence(command: &str, max_len: usize) -> Option<String> {
    let mut tokens = command
        .split_whitespace()
        .map(clean_command_token)
        .filter(|token| !token.is_empty())
        .skip_while(|token| is_noisy_command_prefix(token));

    let first = tokens.next()?;
    if is_noisy_command_token(&first) {
        return None;
    }

    let second = tokens.next().filter(|token| !is_noisy_command_token(token));
    if is_decorative_command(&first, second.as_deref()) {
        return None;
    }
    let summary = match (first.as_str(), second.as_deref()) {
        ("rg", Some("--files")) => "rg --files".to_string(),
        ("cargo", Some(sub)) => format!("cargo {sub}"),
        ("npm", Some("run")) => {
            match tokens.next().filter(|token| !is_noisy_command_token(token)) {
                Some(script) => format!("npm run {script}"),
                None => "npm run".to_string(),
            }
        }
        ("pnpm", Some(sub)) | ("bun", Some(sub)) | ("yarn", Some(sub)) => {
            format!("{first} {sub}")
        }
        ("git", Some(sub)) => format!("git {sub}"),
        ("cmd", Some("/c")) => "cmd /c".to_string(),
        ("powershell", Some(sub)) | ("pwsh", Some(sub)) => format!("{first} {sub}"),
        (_, Some(sub)) if !sub.starts_with('-') && sub.len() <= 18 => format!("{first} {sub}"),
        _ => first,
    };

    let summary = truncate_target(&summary, max_len);
    (!summary.trim().is_empty()).then_some(summary)
}

fn classify_shell_activity(command: &str) -> (ActivityKind, Option<String>) {
    match summarize_command_for_presence(command, 50) {
        Some(target) => (ActivityKind::RunningCommand, Some(target)),
        None => (ActivityKind::Thinking, None),
    }
}

fn clean_command_token(token: &str) -> String {
    token
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('`')
        .trim_end_matches(';')
        .to_string()
}

fn is_noisy_command_prefix(token: &str) -> bool {
    if token == "&&" || token == ";" {
        return true;
    }

    let Some((name, value)) = token.split_once('=') else {
        return false;
    };

    !name.is_empty()
        && !value.is_empty()
        && name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_noisy_command_token(token: &str) -> bool {
    token == "-"
        || token.contains(":/")
        || token.contains(":\\")
        || token.starts_with("\\\\")
        || token.starts_with('/')
        || token.starts_with(".\\")
        || token.starts_with("./")
}

fn is_decorative_command(command: &str, next: Option<&str>) -> bool {
    let command = command.trim().to_ascii_lowercase();
    if matches!(command.as_str(), "echo" | "printf" | "write-host") {
        return true;
    }

    next.is_some_and(|token| {
        let trimmed = token.trim();
        !trimmed.is_empty()
            && trimmed
                .chars()
                .all(|ch| matches!(ch, '=' | '-' | '_' | '*' | '#' | '.' | '·'))
    })
}

fn is_valid_session_id(session_id: &str) -> bool {
    let trimmed = session_id.trim();
    if trimmed.len() != 36 {
        return false;
    }
    let bytes = trimmed.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        let is_hyphen_slot = matches!(index, 8 | 13 | 18 | 23);
        if is_hyphen_slot {
            if *byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn is_synthetic_model_id(model_id: &str) -> bool {
    let trimmed = model_id.trim();
    trimmed.is_empty() || trimmed.eq_ignore_ascii_case("<synthetic>")
}

fn non_synthetic_model(model_id: &str) -> Option<String> {
    (!is_synthetic_model_id(model_id)).then(|| model_id.trim().to_string())
}

fn non_synthetic_display_name(display_name: &str) -> Option<String> {
    let trimmed = display_name.trim();
    (!trimmed.is_empty() && !trimmed.starts_with('<') && trimmed != "Test")
        .then(|| trimmed.to_string())
}

/// Smart project name derivation with fallback chain:
/// 1. Path::file_name() — works for normal paths
/// 2. Nearest project metadata (package.json, Cargo.toml, pyproject.toml, go.mod)
/// 3. Git repo name — parent folder of nearest .git
/// 4. Drive label — "C: Drive" for drive roots
/// 5. "Unknown Project" fallback
fn derive_project_name(cwd: &Path) -> String {
    if let Some(name) = cwd.file_name() {
        let s = name.to_string_lossy();
        if !s.is_empty() {
            return s.to_string();
        }
    }

    if let Some(name) = read_project_name_from_ancestors(cwd, 3) {
        return name;
    }

    if let Some(name) = find_git_repo_name(cwd, 3) {
        return name;
    }

    let path_str = cwd.to_string_lossy();
    if path_str.len() <= 3 {
        let drive = path_str
            .chars()
            .next()
            .filter(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_ascii_uppercase());
        if let Some(letter) = drive {
            return format!("{letter}: Drive");
        }
    }

    "Unknown Project".to_string()
}

/// Walk up from `dir` (inclusive) looking for project metadata files.
/// Returns the `name` field from the first match found.
fn read_project_name_from_ancestors(dir: &Path, max_levels: usize) -> Option<String> {
    let mut current = Some(dir);
    for _ in 0..=max_levels {
        let d = current?;
        if let Some(name) = read_project_name_from_metadata(d) {
            return Some(name);
        }
        current = d.parent();
    }
    None
}

/// Try to extract a project name from metadata files in a single directory.
fn read_project_name_from_metadata(dir: &Path) -> Option<String> {
    let pkg = dir.join("package.json");
    if let Some(name) = read_json_name_field(&pkg) {
        return Some(name);
    }

    if let Some(name) = read_toml_name_field(&dir.join("Cargo.toml")) {
        return Some(name);
    }

    if let Some(name) = read_toml_name_field(&dir.join("pyproject.toml")) {
        return Some(name);
    }

    // go.mod — module xxx
    let gomod = dir.join("go.mod");
    if gomod.is_file()
        && let Ok(data) = fs::read_to_string(&gomod)
        && let Some(first_line) = data.lines().next()
        && let Some(module) = first_line.strip_prefix("module ")
    {
        let name = module.trim();
        let short = name.rsplit('/').next().unwrap_or(name);
        if !short.is_empty() {
            return Some(short.to_string());
        }
    }

    None
}

fn read_json_name_field(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let data = fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&data).ok()?;
    let name = json.get("name")?.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn read_toml_name_field(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let data = fs::read_to_string(path).ok()?;
    for line in data.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("name")
            && let Some(val) = trimmed.split('=').nth(1)
        {
            let name = val.trim().trim_matches('"').trim_matches('\'');
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Walk up from `dir` looking for a `.git` directory, return its parent folder name.
fn find_git_repo_name(dir: &Path, max_levels: usize) -> Option<String> {
    let mut current = Some(dir);
    for _ in 0..=max_levels {
        let d = current?;
        if d.join(".git").exists() {
            return d
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|s| !s.is_empty());
        }
        current = d.parent();
    }
    None
}

/// Decode encoded project path from Claude Code's directory encoding.
/// Claude Code encodes paths: `/` becomes `-`, and literal `-` becomes `--`.
fn decode_project_path(encoded: &str) -> PathBuf {
    if encoded.is_empty() {
        return PathBuf::from(".");
    }

    let decoded = encoded.replace("--", "\x00");
    let decoded = decoded.replace('-', std::path::MAIN_SEPARATOR_STR);
    let decoded = decoded.replace('\x00', "-");

    PathBuf::from(decoded)
}

pub fn preferred_active_session(
    sessions: &[ClaudeSessionSnapshot],
) -> Option<&ClaudeSessionSnapshot> {
    sessions
        .iter()
        .max_by_key(|session| session_rank_key(session))
}

pub fn latest_limits_source(sessions: &[ClaudeSessionSnapshot]) -> Option<&ClaudeSessionSnapshot> {
    sessions
        .iter()
        .filter(|session| limits_present(&session.limits))
        .max_by_key(|session| {
            let observed = session
                .last_token_event_at
                .map(|ts| ts.timestamp())
                .unwrap_or(i64::MIN);
            let activity = session
                .last_activity
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            (observed, activity)
        })
}

pub fn limits_present(limits: &RateLimits) -> bool {
    limits.primary.is_some() || limits.secondary.is_some()
}

/// Merge subagent token/cost data into parent sessions, and populate parent's `subagents` vec.
/// Subagent sessions remain in the list (for Recent Sessions display) but inherit parent's project name.
#[allow(clippy::too_many_arguments)]
fn merge_subagents_into_parents(sessions: &mut [ClaudeSessionSnapshot]) {
    let mut subagent_data: HashMap<String, Vec<(usize, SubagentInfo)>> = HashMap::new();
    for (idx, session) in sessions.iter().enumerate() {
        if !session.is_subagent {
            continue;
        }
        let Some(ref parent_id) = session.parent_session_id else {
            continue;
        };
        let agent_type = session
            .project_name
            .strip_prefix("subagent:")
            .unwrap_or("unknown")
            .to_string();
        let info = SubagentInfo {
            agent_type,
            model: session.model.clone(),
            model_display: session.model_display.clone(),
            activity: session.activity.clone(),
            tokens: session.input_tokens + session.output_tokens,
            cost: session.total_cost,
        };
        subagent_data
            .entry(parent_id.clone())
            .or_default()
            .push((idx, info));
    }

    for session in sessions.iter_mut() {
        if session.is_subagent {
            continue;
        }
        let Some(subagents) = subagent_data.remove(&session.session_id) else {
            continue;
        };
        for (_, info) in &subagents {
            session.input_tokens += info.tokens.saturating_sub(info.cost as u64);
            session.total_cost += info.cost;
            session.subagents.push(info.clone());
        }
    }

    let parent_names: HashMap<String, String> = sessions
        .iter()
        .filter(|s| !s.is_subagent)
        .map(|s| (s.session_id.clone(), s.project_name.clone()))
        .collect();
    for session in sessions.iter_mut() {
        if session.is_subagent
            && let Some(ref parent_id) = session.parent_session_id
            && let Some(parent_name) = parent_names.get(parent_id)
        {
            let agent_type = session
                .project_name
                .strip_prefix("subagent:")
                .unwrap_or("agent")
                .to_string();
            session.project_name = format!("↳ {} ({})", agent_type, parent_name);
        }
    }
}

fn dedupe_sessions_by_id(sessions: Vec<ClaudeSessionSnapshot>) -> Vec<ClaudeSessionSnapshot> {
    let mut deduped: Vec<ClaudeSessionSnapshot> = Vec::new();
    let mut index_by_id: HashMap<String, usize> = HashMap::new();

    for session in sessions {
        let session_id = session.session_id.clone();
        if let Some(existing_index) = index_by_id.get(&session_id).copied() {
            if session_rank_key(&session) > session_rank_key(&deduped[existing_index]) {
                deduped[existing_index] = session;
            }
            continue;
        }

        let next_index = deduped.len();
        index_by_id.insert(session_id, next_index);
        deduped.push(session);
    }

    deduped
}

fn should_include_session(
    snapshot: &ClaudeSessionSnapshot,
    recency: SystemTime,
    stale_cutoff: SystemTime,
    sticky_cutoff: SystemTime,
) -> bool {
    if recency >= stale_cutoff {
        return true;
    }
    if recency < sticky_cutoff {
        return false;
    }
    snapshot
        .activity
        .as_ref()
        .is_some_and(session_activity_is_sticky_active)
}

fn session_activity_is_sticky_active(activity: &ActivitySnapshot) -> bool {
    if activity.pending_calls > 0 {
        return true;
    }
    matches!(activity.kind, ActivityKind::WaitingInput) || is_working_activity_kind(&activity.kind)
}

fn session_rank_key(snapshot: &ClaudeSessionSnapshot) -> (SystemTime, usize, u8, String) {
    let (pending_calls, activity_priority) =
        snapshot
            .activity
            .as_ref()
            .map_or((0usize, 0u8), |activity| {
                (
                    activity.pending_calls,
                    session_activity_priority(&activity.kind),
                )
            });
    (
        snapshot.last_activity,
        pending_calls,
        activity_priority,
        snapshot.session_id.clone(),
    )
}

fn session_activity_priority(kind: &ActivityKind) -> u8 {
    match kind {
        ActivityKind::Thinking
        | ActivityKind::ReadingFile
        | ActivityKind::EditingFile
        | ActivityKind::RunningCommand => 3,
        ActivityKind::WaitingInput => 2,
        ActivityKind::Idle => 1,
    }
}

fn is_working_activity_kind(kind: &ActivityKind) -> bool {
    matches!(
        kind,
        ActivityKind::Thinking
            | ActivityKind::ReadingFile
            | ActivityKind::EditingFile
            | ActivityKind::RunningCommand
    )
}

fn session_recency(snapshot: &ClaudeSessionSnapshot, file_modified: SystemTime) -> SystemTime {
    let mut newest = SystemTime::UNIX_EPOCH;

    if let Some(activity) = &snapshot.activity {
        for candidate in [
            activity.last_effective_signal_at,
            activity.last_active_at,
            activity.observed_at,
        ] {
            if let Some(ts) = candidate.and_then(datetime_to_system_time)
                && ts > newest
            {
                newest = ts;
            }
        }
    }

    if let Some(ts) = snapshot
        .last_token_event_at
        .and_then(datetime_to_system_time)
        && ts > newest
    {
        newest = ts;
    }

    if let Some(ts) = snapshot.background_work.latest_signal_at
        && ts > newest
    {
        newest = ts;
    }

    if newest > SystemTime::UNIX_EPOCH {
        newest
    } else {
        file_modified
    }
}

fn datetime_to_system_time(ts: DateTime<Utc>) -> Option<SystemTime> {
    if ts.timestamp() < 0 {
        return None;
    }
    let secs = ts.timestamp() as u64;
    let nanos = ts.timestamp_subsec_nanos() as u64;
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(secs))?
        .checked_add(Duration::from_nanos(nanos))
}

#[cfg(test)]
mod tests {
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
        fs::write(session_checkpoint_path(&checkpoint_root, &path), b"{")
            .expect("corrupt checkpoint");

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

        // A later user event carrying the old system-reminder text — typically
        // describing a subagent — must not rewrite the authoritative value.
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
        let old = Utc::now()
            - chrono::Duration::seconds(ORPHANED_PENDING_CALL_SECS + IDLE_DEBOUNCE_SECS + 1);
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

        let fast_msg =
            assistant_usage_message(model, 100_000, 20_000, Some("fast"), Some("priority"));
        let standard_msg =
            assistant_usage_message(model, 50_000, 10_000, Some("standard"), Some("standard"));
        let absent_speed_msg = assistant_usage_message(model, 30_000, 5_000, None, None);

        process_jsonl_message(&mut acc, &fast_msg);
        process_jsonl_message(&mut acc, &standard_msg);
        process_jsonl_message(&mut acc, &absent_speed_msg);

        let expected =
            cost::calculate_cost_with_context_and_speed(model, 100_000, 20_000, 0, 0, true)
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
            &assistant_usage_message_with_cache(
                model,
                100_000,
                20_000,
                8_000,
                60_000,
                Some("fast"),
            ),
        );
        process_jsonl_message(
            &mut acc,
            &assistant_usage_message_with_cache(
                model,
                50_000,
                10_000,
                4_000,
                30_000,
                Some("standard"),
            ),
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
            (acc.cache_read_cost - (fast_turn.cache_read_cost + std_turn.cache_read_cost)).abs()
                < 1e-7
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
}
