//! Claude session discovery and parsing: transcript JSONL, statusline data, context, effort,
//! activity and cost per session.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fs;
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
const SESSION_CHECKPOINT_TEMP_GRACE: Duration = Duration::from_secs(600);

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
/// Fast mode (Opus 4.8+, Fable 5.2 and Mythos 5.2) bills at
/// `cost::FAST_RATE_MULTIPLIER`x standard rates.
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
    /// "Extra High" (API `effort: xhigh`): Opus 4.7 and later, every Fable and
    /// Mythos model, Sonnet 5 and later, and Haiku 5.5.
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
    /// False when we only have the settings.json default, in that case the
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
    /// Tracked for display only: orthogonal to speed and does not change price.
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
    crate::git_head::branch(project_path)
}

#[derive(Debug, Default)]
pub struct SessionParseCache {
    entries: HashMap<PathBuf, CachedSessionEntry>,
    /// When set, parsing may read checkpoints but never writes them. Used by
    /// one-off scans of the whole history, which would otherwise leave a
    /// checkpoint behind for every old transcript.
    read_only_checkpoints: bool,
}

impl SessionParseCache {
    /// A cache for a one-off full scan that writes no checkpoints.
    pub fn without_checkpoint_writes() -> Self {
        Self {
            read_only_checkpoints: true,
            ..Self::default()
        }
    }
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
    /// Build that computed `accumulator` costs. Absent in checkpoints from
    /// older builds, which therefore never match.
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

/// Pulse-owned checkpoint directory under `PULSE_HOME`.
fn session_checkpoint_home() -> PathBuf {
    crate::storage::cache_path("claude-checkpoints")
        .join(format!("v{SESSION_CHECKPOINT_SCHEMA_VERSION}"))
}

/// Checkpoints are only kept for transcripts under a configured Claude
/// projects root, so parsing an arbitrary file never writes Pulse state.
fn session_checkpoint_root(path: &Path) -> Option<PathBuf> {
    config::projects_paths()
        .into_iter()
        .any(|projects_root| path.starts_with(&projects_root))
        .then(session_checkpoint_home)
}

/// Directory where earlier builds wrote checkpoints, inside Claude's own
/// home. Pulse removes its files from there during pruning.
fn legacy_session_checkpoint_roots() -> Vec<PathBuf> {
    config::projects_paths()
        .into_iter()
        .filter_map(|projects_root| {
            projects_root
                .parent()
                .map(|parent| parent.join("pulse-session-checkpoints"))
        })
        .collect()
}

/// FNV-1a over the path text. Unlike `DefaultHasher`, the result is stable
/// across Rust releases, so a toolchain update never orphans checkpoints.
fn stable_path_hash(path: &Path) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn session_checkpoint_path(checkpoint_root: &Path, source_path: &Path) -> PathBuf {
    checkpoint_root.join(format!("{:016x}.json", stable_path_hash(source_path)))
}

/// Header fields read when pruning; the accumulator itself is ignored.
#[derive(Deserialize)]
struct CheckpointHeader {
    schema_version: u32,
    #[serde(default)]
    app_version: String,
    source_path: PathBuf,
}

/// Removes checkpoints another build wrote, checkpoints whose transcript is
/// gone, unreadable files and leftover temporary files. Returns the number of
/// files removed.
fn prune_session_checkpoints_in(checkpoint_root: &Path) -> usize {
    let Ok(entries) = fs::read_dir(checkpoint_root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_json = path.extension().is_some_and(|ext| ext == "json");
        // A non-JSON file is a temporary write; a fresh one may still be in
        // progress on the polling thread.
        let recently_written =
            entry
                .metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| {
                    modified.elapsed().unwrap_or_default() < SESSION_CHECKPOINT_TEMP_GRACE
                });
        if !is_json && recently_written {
            continue;
        }
        let keep = is_json
            && entry
                .metadata()
                .is_ok_and(|meta| meta.len() <= SESSION_CHECKPOINT_MAX_BYTES)
            && fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<CheckpointHeader>(&bytes).ok())
                .is_some_and(|header| {
                    header.schema_version == SESSION_CHECKPOINT_SCHEMA_VERSION
                        && header.app_version == SESSION_CHECKPOINT_APP_VERSION
                        && header.source_path.is_file()
                        && session_checkpoint_path(checkpoint_root, &header.source_path) == path
                });
        if !keep && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Removes every file in a legacy checkpoint directory, then the empty
/// directories. Only Pulse ever wrote there.
fn remove_legacy_session_checkpoints(legacy_root: &Path) -> usize {
    let mut removed = 0;
    for version_dir in fs::read_dir(legacy_root).into_iter().flatten().flatten() {
        let dir = version_dir.path();
        if !dir.is_dir() {
            continue;
        }
        for file in fs::read_dir(&dir).into_iter().flatten().flatten() {
            if file.path().is_file() && fs::remove_file(file.path()).is_ok() {
                removed += 1;
            }
        }
        let _ = fs::remove_dir(&dir);
    }
    let _ = fs::remove_dir(legacy_root);
    removed
}

/// Prunes checkpoints once per process, on a background thread so the first
/// poll is not delayed.
pub fn prune_session_checkpoints_once() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let root = session_checkpoint_home();
        let legacy = legacy_session_checkpoint_roots();
        let spawned = std::thread::Builder::new()
            .name("pulse-checkpoint-prune".to_string())
            .spawn(move || {
                let mut removed = prune_session_checkpoints_in(&root);
                for legacy_root in legacy {
                    removed += remove_legacy_session_checkpoints(&legacy_root);
                }
                if removed > 0 {
                    tracing::info!(removed, "pruned stale session checkpoints");
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "failed to start session checkpoint pruning");
        }
    });
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
/// headline total + model. JSONL supplies the speed-aware per-category cost proportions,
/// scaled to reconcile with that authoritative total, plus granular tokens + activity.
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
    /// composer's effort selector: `~/.claude/settings.json` has no `effortLevel`
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
    /// Cache-write split by lifetime; absent in older transcripts.
    #[serde(default)]
    cache_creation: Option<JsonlCacheCreation>,
    #[serde(default)]
    speed: Option<String>,
    #[serde(default)]
    service_tier: Option<String>,
}

/// `message.usage.cache_creation`. Only the one-hour count is read: whatever
/// `cache_creation_input_tokens` holds beyond it is a five-minute write, which
/// is exactly `ephemeral_5m_input_tokens` when the object is consistent.
#[derive(Debug, Deserialize)]
struct JsonlCacheCreation {
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

pub fn collect_active_sessions(
    git_cache: &mut GitBranchCache,
    parse_cache: &mut SessionParseCache,
    stale_threshold: Duration,
    active_sticky_window: Duration,
) -> Result<Vec<ClaudeSessionSnapshot>> {
    prune_session_checkpoints_once();
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

#[expect(
    clippy::too_many_arguments,
    reason = "the parse needs the file metadata plus both caches"
)]
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
    let write_checkpoints = !parse_cache.read_only_checkpoints;

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
            if write_checkpoints {
                persist_session_checkpoint(path, entry);
            }
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
    if write_checkpoints {
        persist_session_checkpoint(path, entry);
    }
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

                // A turn with no known model has no published rate: it keeps its
                // tokens but adds no cost, so a session of such turns reads as
                // unpriced instead of borrowing another model's price.
                let cache_writes = cost::CacheWrites::from_total_and_one_hour(
                    cache_creation,
                    usage
                        .cache_creation
                        .as_ref()
                        .map(|split| split.ephemeral_1h_input_tokens),
                );
                let turn_costs = acc
                    .model
                    .as_deref()
                    .map(|model_id| {
                        cost::calculate_category_costs_with_cache_ttl(
                            model_id,
                            input,
                            output,
                            cache_writes,
                            cache_read,
                            turn_speed.is_fast(),
                        )
                    })
                    .unwrap_or_default();
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
            // reminder, often carried by an interleaved sidechain user event
            // describing a subagent, must not overwrite it.
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
/// override, and those lines are interleaved into the parent transcript. A
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
/// 1. Path::file_name(): works for normal paths
/// 2. Nearest project metadata (package.json, Cargo.toml, pyproject.toml, go.mod)
/// 3. Git repo name: parent folder of nearest .git
/// 4. Drive label: "C: Drive" for drive roots
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

    // go.mod: `module <path>`
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
mod tests;
