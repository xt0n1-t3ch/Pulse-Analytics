use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const DEFAULT_STALE_SECONDS: u64 = 90;
const DEFAULT_POLL_SECONDS: u64 = 2;
const DEFAULT_ACTIVE_STICKY_SECONDS: u64 = 300;
const MIN_ACTIVE_STICKY_SECONDS: u64 = 30;
const CONFIG_SCHEMA_VERSION: u32 = 6;
pub const DEFAULT_DISCORD_CLIENT_ID: &str = "1466664856261230716";

/// Default large_image asset key — must be uploaded to the Developer Portal at
/// https://discord.com/developers/applications/{client_id}/rich-presence/assets
/// Discord Rich Presence ONLY reliably renders asset keys; external URLs are
/// unreliable (silently dropped by Discord on many client versions).
pub const DEFAULT_LARGE_IMAGE_KEY: &str = "large";
const PREVIOUS_DEFAULT_LARGE_IMAGE_KEY: &str = "claude-code";

/// Default large image of configs written before schema 3. The schema-3
/// migration compares stored configs against this exact string, so it keeps
/// the repository's former name even though GitHub now serves it as
/// `Pulse-Analytics`.
pub const DEFAULT_MASCOT_ASSET_URL: &str = "https://raw.githubusercontent.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/main/assets/branding/claude-mascot.jpg";

/// Default small activity asset keys — each must exist in the Developer Portal.
/// Users who host custom assets can override via `activity_small_image_keys` in
/// the config file or via the Settings UI.
pub const DEFAULT_ACTIVITY_THINKING_KEY: &str = "thinking";
pub const DEFAULT_ACTIVITY_READING_KEY: &str = "reading";
pub const DEFAULT_ACTIVITY_EDITING_KEY: &str = "editing";
pub const DEFAULT_ACTIVITY_RUNNING_KEY: &str = "running";
pub const DEFAULT_ACTIVITY_WAITING_KEY: &str = "waiting";
pub const DEFAULT_ACTIVITY_IDLE_KEY: &str = "idle";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PresenceConfig {
    pub schema_version: u32,
    pub discord_client_id: Option<String>,
    pub plan: Option<String>,
    pub initialized: bool,
    /// Master Rich Presence switch. Persisted since schema v6 — before that the
    /// Pulse GUI kept it in process memory only, so turning presence off was
    /// silently forgotten on the next launch. Mirrors Codex's `presence_enabled`.
    pub presence_enabled: bool,
    pub privacy: PrivacyConfig,
    pub display: DisplayConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacyConfig {
    pub enabled: bool,
    pub show_project_name: bool,
    pub show_git_branch: bool,
    pub show_model: bool,
    pub show_tokens: bool,
    pub show_cost: bool,
    pub show_plan: bool,
    pub show_limits: bool,
    pub show_context: bool,
    pub show_activity: bool,
    pub show_activity_target: bool,
    pub show_systems: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TerminalLogoMode {
    #[default]
    Auto,
    Ascii,
    Image,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreset {
    #[default]
    PremiumTerminal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HeroAnimationMode {
    Off,
    #[default]
    Subtle,
    Expressive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    pub large_image_key: String,
    pub large_text: String,
    pub small_image_key: String,
    pub small_text: String,
    pub activity_small_image_keys: ActivitySmallImageKeys,
    pub terminal_logo_mode: TerminalLogoMode,
    pub terminal_logo_path: Option<String>,
    pub theme_preset: ThemePreset,
    pub hero_animation: HeroAnimationMode,
    pub mascot_image_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ActivitySmallImageKeys {
    pub thinking: Option<String>,
    pub reading: Option<String>,
    pub editing: Option<String>,
    pub running: Option<String>,
    pub waiting: Option<String>,
    pub idle: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeSettings {
    pub stale_threshold: Duration,
    pub active_sticky_window: Duration,
    pub poll_interval: Duration,
}

impl Default for PresenceConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            discord_client_id: Some(DEFAULT_DISCORD_CLIENT_ID.to_string()),
            plan: None,
            initialized: false,
            presence_enabled: true,
            privacy: PrivacyConfig::default(),
            display: DisplayConfig::default(),
        }
    }
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            show_project_name: true,
            show_git_branch: true,
            show_model: true,
            show_tokens: true,
            show_cost: true,
            show_plan: true,
            show_limits: true,
            show_context: true,
            show_activity: true,
            show_activity_target: true,
            show_systems: true,
        }
    }
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            large_image_key: DEFAULT_LARGE_IMAGE_KEY.to_string(),
            large_text: "Claude Code".to_string(),
            small_image_key: DEFAULT_ACTIVITY_IDLE_KEY.to_string(),
            small_text: "Idle".to_string(),
            activity_small_image_keys: ActivitySmallImageKeys::defaults(),
            terminal_logo_mode: TerminalLogoMode::Auto,
            terminal_logo_path: None,
            theme_preset: ThemePreset::default(),
            hero_animation: HeroAnimationMode::default(),
            mascot_image_path: None,
        }
    }
}

impl ActivitySmallImageKeys {
    /// Defaults that match the standard Developer Portal asset naming convention.
    /// Users can override any entry via config to point at custom uploaded assets.
    pub fn defaults() -> Self {
        Self {
            thinking: Some(DEFAULT_ACTIVITY_THINKING_KEY.to_string()),
            reading: Some(DEFAULT_ACTIVITY_READING_KEY.to_string()),
            editing: Some(DEFAULT_ACTIVITY_EDITING_KEY.to_string()),
            running: Some(DEFAULT_ACTIVITY_RUNNING_KEY.to_string()),
            waiting: Some(DEFAULT_ACTIVITY_WAITING_KEY.to_string()),
            idle: Some(DEFAULT_ACTIVITY_IDLE_KEY.to_string()),
        }
    }
}

impl PresenceConfig {
    pub fn load_or_init() -> Result<Self> {
        let cfg_path = config_path();
        if let Some(parent) = cfg_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create config directory {}", parent.display())
            })?;
        }

        if cfg_path.exists() {
            let raw = fs::read_to_string(&cfg_path)
                .with_context(|| format!("failed to read {}", cfg_path.display()))?;
            let mut parsed: PresenceConfig = serde_json::from_str(&raw)
                .with_context(|| format!("invalid JSON in {}", cfg_path.display()))?;
            if parsed.normalize_and_migrate() {
                parsed.save()?;
            }
            Ok(parsed)
        } else {
            let cfg = PresenceConfig::default();
            cfg.save()?;
            Ok(cfg)
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create config directory {}", parent.display())
            })?;
        }

        // Atomic (temp file + fsync + rename), matching the Codex config writer.
        // Pulse's poller re-reads this file every few seconds while the GUI
        // rewrites it on every toggle; a truncating `fs::write` lets a reader
        // land on invalid JSON, and callers swallow that as `unwrap_or_default()`
        // — i.e. every privacy switch silently snaps back to its default.
        crate::codex::util::write_json_pretty_atomic(&path, self)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    pub fn effective_client_id(&self) -> Option<String> {
        let from_env = env::var("CC_DISCORD_CLIENT_ID")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        if from_env.is_some() {
            return from_env;
        }

        self.discord_client_id
            .as_ref()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    pub fn plan_display_name(&self) -> Option<&str> {
        crate::plan::display_name_from_key(self.plan.as_deref()?)
    }

    pub fn plan_badge_name(&self) -> Option<&str> {
        crate::plan::badge_name_from_key(self.plan.as_deref()?)
    }

    pub fn toggle_privacy(&mut self) -> bool {
        self.privacy.enabled = !self.privacy.enabled;
        self.privacy.enabled
    }

    pub fn resolved_mascot_image_path(&self) -> Option<String> {
        let from_config = self
            .display
            .mascot_image_path
            .as_ref()
            .or(self.display.terminal_logo_path.as_ref())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .filter(|value| Path::new(value).exists());

        from_config.or_else(|| {
            bundled_asset_path("assets/branding/claude-mascot.jpg")
                .filter(|path| path.exists())
                .map(|path| path.display().to_string())
        })
    }

    fn normalize_and_migrate(&mut self) -> bool {
        let mut changed = false;
        let previous_version = self.schema_version;

        if self.schema_version < CONFIG_SCHEMA_VERSION {
            self.schema_version = CONFIG_SCHEMA_VERSION;
            changed = true;
        }

        if previous_version < 3 && self.display.large_image_key.trim() == DEFAULT_MASCOT_ASSET_URL {
            self.display.large_image_key = DEFAULT_LARGE_IMAGE_KEY.to_string();
            changed = true;
        }

        if previous_version < 4
            && self.display.large_image_key.trim() == PREVIOUS_DEFAULT_LARGE_IMAGE_KEY
        {
            self.display.large_image_key = DEFAULT_LARGE_IMAGE_KEY.to_string();
            changed = true;
        }

        if previous_version < 5 {
            self.privacy.show_systems = true;
            changed = true;
        }

        if previous_version < 3 {
            let defaults = ActivitySmallImageKeys::defaults();
            let slots = [
                (
                    &mut self.display.activity_small_image_keys.thinking,
                    &defaults.thinking,
                ),
                (
                    &mut self.display.activity_small_image_keys.reading,
                    &defaults.reading,
                ),
                (
                    &mut self.display.activity_small_image_keys.editing,
                    &defaults.editing,
                ),
                (
                    &mut self.display.activity_small_image_keys.running,
                    &defaults.running,
                ),
                (
                    &mut self.display.activity_small_image_keys.waiting,
                    &defaults.waiting,
                ),
                (
                    &mut self.display.activity_small_image_keys.idle,
                    &defaults.idle,
                ),
            ];
            for (slot, default) in slots {
                if slot.as_ref().is_none_or(|s| s.trim().is_empty()) && default.is_some() {
                    *slot = default.clone();
                    changed = true;
                }
            }
            if self.display.small_image_key.trim().is_empty() {
                self.display.small_image_key = DEFAULT_ACTIVITY_IDLE_KEY.to_string();
                changed = true;
            }
        }

        if is_missing(&self.discord_client_id) {
            self.discord_client_id = Some(DEFAULT_DISCORD_CLIENT_ID.to_string());
            changed = true;
        }

        if self.display.large_image_key.trim().is_empty() {
            self.display.large_image_key = DisplayConfig::default().large_image_key;
            changed = true;
        }
        if self.display.large_text.trim().is_empty() {
            self.display.large_text = DisplayConfig::default().large_text;
            changed = true;
        }
        for item in [
            &mut self.display.activity_small_image_keys.thinking,
            &mut self.display.activity_small_image_keys.reading,
            &mut self.display.activity_small_image_keys.editing,
            &mut self.display.activity_small_image_keys.running,
            &mut self.display.activity_small_image_keys.waiting,
            &mut self.display.activity_small_image_keys.idle,
        ] {
            if normalize_optional_string(item) {
                changed = true;
            }
        }
        if self
            .display
            .terminal_logo_path
            .as_deref()
            .is_some_and(|path| path.trim().is_empty())
        {
            self.display.terminal_logo_path = None;
            changed = true;
        }
        if self.display.mascot_image_path.is_none() && self.display.terminal_logo_path.is_some() {
            self.display.mascot_image_path = self.display.terminal_logo_path.clone();
            changed = true;
        }
        if normalize_optional_string(&mut self.display.mascot_image_path) {
            changed = true;
        }

        changed
    }
}

pub fn runtime_settings() -> RuntimeSettings {
    let sticky_seconds = env_u64(
        "CC_PRESENCE_ACTIVE_STICKY_SECONDS",
        DEFAULT_ACTIVE_STICKY_SECONDS,
    )
    .max(MIN_ACTIVE_STICKY_SECONDS);
    RuntimeSettings {
        stale_threshold: Duration::from_secs(env_u64(
            "CC_PRESENCE_STALE_SECONDS",
            DEFAULT_STALE_SECONDS,
        )),
        active_sticky_window: Duration::from_secs(sticky_seconds),
        poll_interval: Duration::from_secs(env_u64(
            "CC_PRESENCE_POLL_SECONDS",
            DEFAULT_POLL_SECONDS,
        )),
    }
}

pub fn claude_home() -> PathBuf {
    if let Ok(custom) = env::var("CLAUDE_HOME") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
}

/// Serializes tests that redirect `CLAUDE_HOME`.
///
/// The variable is process-global, so two tests swapping it concurrently make
/// each other read the wrong tree. It lives beside `claude_home()` — the reader
/// they are redirecting — so every module shares one lock instead of each
/// growing its own.
#[cfg(test)]
pub fn home_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

pub fn projects_path() -> PathBuf {
    claude_home().join("projects")
}

pub fn projects_paths() -> Vec<PathBuf> {
    let mut ordered: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    push_unique_path(&mut ordered, &mut seen, projects_path());

    #[cfg(windows)]
    {
        if include_wsl_session_roots() {
            for candidate in windows_wsl_projects_candidates() {
                push_unique_path(&mut ordered, &mut seen, candidate);
            }
        }
    }

    #[cfg(all(unix, not(windows)))]
    {
        for candidate in wsl_windows_projects_candidates() {
            push_unique_path(&mut ordered, &mut seen, candidate);
        }
    }

    ordered
}

pub fn statusline_data_path() -> PathBuf {
    claude_home().join("discord-presence-data.json")
}

pub fn credentials_path() -> PathBuf {
    claude_home().join(".credentials.json")
}

pub fn usage_cache_path() -> PathBuf {
    crate::storage::home()
        .join("claude")
        .join("discord-presence-usage-cache.json")
}

pub fn config_path() -> PathBuf {
    crate::storage::home()
        .join("claude")
        .join("discord-presence-config.json")
}

pub fn lock_path() -> PathBuf {
    claude_home().join("cc-discord-presence.lock")
}

pub fn instance_meta_path() -> PathBuf {
    claude_home().join("cc-discord-presence.instance.json")
}

fn bundled_asset_path(relative: &str) -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let candidates = [
        exe.parent().map(|dir| dir.join(relative)),
        exe.parent()
            .and_then(|dir| dir.parent())
            .map(|dir| dir.join(relative)),
        env::current_dir().ok().map(|dir| dir.join(relative)),
    ];

    candidates.into_iter().flatten().find(|path| path.exists())
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

fn is_missing(value: &Option<String>) -> bool {
    value.as_ref().map(|v| v.trim().is_empty()).unwrap_or(true)
}

fn normalize_optional_string(value: &mut Option<String>) -> bool {
    if let Some(item) = value.as_mut() {
        let trimmed = item.trim().to_string();
        if trimmed.is_empty() {
            *value = None;
            return true;
        }
        if *item != trimmed {
            *item = trimmed;
            return true;
        }
    }
    false
}

fn push_unique_path(paths: &mut Vec<PathBuf>, seen: &mut HashSet<String>, candidate: PathBuf) {
    if candidate.as_os_str().is_empty() {
        return;
    }
    let key = path_key(&candidate);
    if seen.insert(key) {
        paths.push(candidate);
    }
}

fn path_key(path: &Path) -> String {
    #[cfg(windows)]
    {
        path.to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase()
    }

    #[cfg(not(windows))]
    {
        path.to_string_lossy().to_string()
    }
}

#[cfg(all(unix, not(windows)))]
fn wsl_windows_projects_candidates() -> Vec<PathBuf> {
    if !running_in_wsl() {
        return Vec::new();
    }

    let mut candidates = Vec::new();

    if let Ok(profile) = env::var("USERPROFILE") {
        let profile = profile.trim();
        if !profile.is_empty() {
            candidates.push(PathBuf::from(profile).join(".claude").join("projects"));
        }
    }

    if let Ok(username) = env::var("USERNAME").or_else(|_| env::var("USER")) {
        let username = username.trim();
        if !username.is_empty() {
            candidates.push(
                PathBuf::from("/mnt/c/Users")
                    .join(username)
                    .join(".claude")
                    .join("projects"),
            );
        }
    }

    candidates
}

#[cfg(windows)]
fn parse_bool_flag(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
}

#[cfg(windows)]
fn include_wsl_session_roots() -> bool {
    parse_bool_flag(env::var("CC_PRESENCE_INCLUDE_WSL").ok().as_deref())
}

#[cfg(all(unix, not(windows)))]
fn running_in_wsl() -> bool {
    if env::var_os("WSL_DISTRO_NAME").is_some() {
        return true;
    }
    fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|value| value.to_ascii_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

#[cfg(windows)]
fn windows_wsl_projects_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let distros = windows_wsl_distro_names();
    for distro in distros {
        if let Some(home) = wsl_home_for_distro(&distro) {
            candidates.push(wsl_home_to_unc_projects_path(&distro, &home));
            continue;
        }
        if let Some(username) = env::var("USERNAME")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            let fallback = format!(
                r"\\wsl.localhost\{}\home\{}\.claude\projects",
                distro, username
            );
            candidates.push(PathBuf::from(fallback));
        }
    }
    candidates
}

#[cfg(windows)]
fn windows_wsl_distro_names() -> Vec<String> {
    let output = crate::util::silent_command("wsl.exe")
        .args(["-l", "-q"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    decode_windows_text_output(&output.stdout)
        .lines()
        .map(|line| line.trim().trim_start_matches('*').trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

#[cfg(windows)]
fn wsl_home_for_distro(distro: &str) -> Option<String> {
    let output = crate::util::silent_command("wsl.exe")
        .args(["-d", distro, "--", "sh", "-lc", "printf %s \"$HOME\""])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let home = decode_windows_text_output(&output.stdout)
        .trim()
        .to_string();
    if home.is_empty() { None } else { Some(home) }
}

#[cfg(windows)]
fn wsl_home_to_unc_projects_path(distro: &str, home: &str) -> PathBuf {
    let mut unc = format!(r"\\wsl.localhost\{}", distro);
    for part in home.trim().trim_start_matches('/').split('/') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        unc.push('\\');
        unc.push_str(part);
    }
    unc.push_str(r"\.claude\projects");
    PathBuf::from(unc)
}

#[cfg(windows)]
fn decode_windows_text_output(bytes: &[u8]) -> String {
    let has_interleaved_nuls = bytes
        .iter()
        .skip(1)
        .step_by(2)
        .take(64)
        .any(|byte| *byte == 0);

    if bytes.starts_with(&[0xFF, 0xFE]) || has_interleaved_nuls {
        let mut utf16: Vec<u16> = Vec::with_capacity(bytes.len() / 2);
        let mut chunks = bytes.chunks_exact(2);
        for chunk in &mut chunks {
            utf16.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }
        return String::from_utf16_lossy(&utf16);
    }

    String::from_utf8_lossy(bytes).to_string()
}

/// Read workspace folders from VS Code IDE lock files (`~/.claude/ide/*.lock`).
/// Returns non-root workspace paths sorted longest-first (most specific first).
/// Filters out drive roots (e.g., `C:\`) and validates the IDE process is alive.
pub fn read_ide_workspace_folders() -> Vec<PathBuf> {
    let ide_dir = claude_home().join("ide");
    let Ok(entries) = fs::read_dir(&ide_dir) else {
        return Vec::new();
    };

    let mut workspaces: Vec<PathBuf> = Vec::new();
    let mut seen = HashSet::new();

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lock") {
            continue;
        }
        let Ok(data) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(lock) = serde_json::from_str::<serde_json::Value>(&data) else {
            continue;
        };

        if let Some(pid) = lock.get("pid").and_then(|v| v.as_u64())
            && !is_process_alive(pid as u32)
        {
            continue;
        }

        let Some(folders) = lock.get("workspaceFolders").and_then(|v| v.as_array()) else {
            continue;
        };

        for folder in folders {
            let Some(folder_str) = folder.as_str() else {
                continue;
            };
            let folder_path = PathBuf::from(folder_str);
            if is_drive_root(&folder_path) {
                continue;
            }
            let key = path_key(&folder_path);
            if seen.insert(key) {
                workspaces.push(folder_path);
            }
        }
    }

    workspaces.sort_by_key(|path| std::cmp::Reverse(path.as_os_str().len()));
    workspaces
}

/// Find the deepest (most specific) workspace folder that is an ancestor of `file_path`.
/// Returns `None` if no workspace matches.
pub fn find_best_workspace(file_path: &Path, workspaces: &[PathBuf]) -> Option<PathBuf> {
    let file_key = path_key(file_path);
    workspaces
        .iter()
        .find(|ws| {
            let ws_key = path_key(ws);
            file_key.starts_with(&ws_key)
        })
        .cloned()
}

/// Returns true if a path looks like a drive root (e.g., `C:\`, `D:/`, `/`).
pub fn is_drive_root(path: &Path) -> bool {
    let s = path.to_string_lossy();
    let trimmed = s.trim_end_matches(['/', '\\']);
    trimmed.len() <= 2
        && trimmed
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        || s == "/"
        || s == "\\"
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        crate::util::silent_command("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map(|out| {
                let text = String::from_utf8_lossy(&out.stdout);
                text.contains(&pid.to_string())
            })
            .unwrap_or(false)
    }

    #[cfg(not(windows))]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_client_id_is_returned() {
        let cfg = PresenceConfig {
            discord_client_id: Some("from-config".to_string()),
            ..PresenceConfig::default()
        };
        assert_eq!(cfg.effective_client_id().as_deref(), Some("from-config"));
    }

    #[test]
    fn plan_display_names() {
        let mut cfg = PresenceConfig {
            plan: Some("pro".to_string()),
            ..Default::default()
        };
        assert_eq!(cfg.plan_display_name(), Some("Pro ($20/mo)"));
        assert_eq!(cfg.plan_badge_name(), Some("PRO"));

        cfg.plan = Some("max_20x".to_string());
        assert_eq!(cfg.plan_display_name(), Some("Max ($200/mo)"));
        assert_eq!(cfg.plan_badge_name(), Some("MAX 20x"));

        cfg.plan = Some("max_5x".to_string());
        assert_eq!(cfg.plan_badge_name(), Some("MAX 5x"));

        cfg.plan = Some("team".to_string());
        assert_eq!(cfg.plan_display_name(), Some("Teams"));
        assert_eq!(cfg.plan_badge_name(), Some("TEAMS"));

        cfg.plan = None;
        assert_eq!(cfg.plan_display_name(), None);
        assert_eq!(cfg.plan_badge_name(), None);
    }

    #[test]
    fn migration_sets_default_client_id_when_missing() {
        let mut cfg = PresenceConfig {
            schema_version: 0,
            discord_client_id: None,
            plan: None,
            initialized: false,
            presence_enabled: true,
            privacy: PrivacyConfig::default(),
            display: DisplayConfig::default(),
        };

        let changed = cfg.normalize_and_migrate();

        assert!(changed);
        assert_eq!(cfg.schema_version, CONFIG_SCHEMA_VERSION);
        assert_eq!(
            cfg.discord_client_id.as_deref(),
            Some(DEFAULT_DISCORD_CLIENT_ID)
        );
    }

    #[test]
    fn display_defaults_to_auto_logo_mode() {
        let cfg = PresenceConfig::default();
        assert_eq!(cfg.display.terminal_logo_mode, TerminalLogoMode::Auto);
        assert_eq!(cfg.display.terminal_logo_path, None);
        assert_eq!(cfg.display.theme_preset, ThemePreset::PremiumTerminal);
        assert_eq!(cfg.display.hero_animation, HeroAnimationMode::Subtle);
        assert_eq!(cfg.display.large_image_key, DEFAULT_LARGE_IMAGE_KEY);
    }

    #[test]
    fn migration_v2_url_to_current_asset_key() {
        let mut cfg = PresenceConfig {
            schema_version: 2,
            discord_client_id: Some(DEFAULT_DISCORD_CLIENT_ID.to_string()),
            plan: None,
            initialized: true,
            presence_enabled: true,
            privacy: PrivacyConfig::default(),
            display: DisplayConfig {
                large_image_key: DEFAULT_MASCOT_ASSET_URL.to_string(),
                ..DisplayConfig::default()
            },
        };
        let changed = cfg.normalize_and_migrate();
        assert!(changed);
        assert_eq!(cfg.schema_version, CONFIG_SCHEMA_VERSION);
        assert_eq!(cfg.display.large_image_key, DEFAULT_LARGE_IMAGE_KEY);
    }

    #[test]
    fn migration_v3_claude_code_asset_key_to_square_large_key() {
        let mut cfg = PresenceConfig {
            schema_version: 3,
            discord_client_id: Some(DEFAULT_DISCORD_CLIENT_ID.to_string()),
            plan: None,
            initialized: true,
            presence_enabled: true,
            privacy: PrivacyConfig::default(),
            display: DisplayConfig {
                large_image_key: PREVIOUS_DEFAULT_LARGE_IMAGE_KEY.to_string(),
                ..DisplayConfig::default()
            },
        };
        let changed = cfg.normalize_and_migrate();
        assert!(changed);
        assert_eq!(cfg.schema_version, CONFIG_SCHEMA_VERSION);
        assert_eq!(cfg.display.large_image_key, DEFAULT_LARGE_IMAGE_KEY);
    }

    #[test]
    fn migration_preserves_custom_large_image_key() {
        let mut cfg = PresenceConfig {
            schema_version: 2,
            discord_client_id: Some(DEFAULT_DISCORD_CLIENT_ID.to_string()),
            plan: None,
            initialized: true,
            presence_enabled: true,
            privacy: PrivacyConfig::default(),
            display: DisplayConfig {
                large_image_key: "my-custom-key".to_string(),
                ..DisplayConfig::default()
            },
        };
        cfg.normalize_and_migrate();
        assert_eq!(cfg.display.large_image_key, "my-custom-key");
    }

    #[test]
    fn migration_fills_missing_activity_keys() {
        let mut cfg = PresenceConfig {
            schema_version: 2,
            discord_client_id: Some(DEFAULT_DISCORD_CLIENT_ID.to_string()),
            plan: None,
            initialized: true,
            presence_enabled: true,
            privacy: PrivacyConfig::default(),
            display: DisplayConfig {
                activity_small_image_keys: ActivitySmallImageKeys::default(),
                ..DisplayConfig::default()
            },
        };
        cfg.normalize_and_migrate();
        assert_eq!(
            cfg.display.activity_small_image_keys.thinking.as_deref(),
            Some(DEFAULT_ACTIVITY_THINKING_KEY)
        );
        assert_eq!(
            cfg.display.activity_small_image_keys.idle.as_deref(),
            Some(DEFAULT_ACTIVITY_IDLE_KEY)
        );
    }

    #[test]
    fn privacy_toggle() {
        let mut cfg = PresenceConfig::default();
        assert!(!cfg.privacy.enabled);
        assert!(cfg.toggle_privacy());
        assert!(cfg.privacy.enabled);
        assert!(!cfg.toggle_privacy());
        assert!(!cfg.privacy.enabled);
    }

    #[test]
    fn migration_promotes_terminal_logo_to_mascot_path() {
        let mut cfg = PresenceConfig::default();
        cfg.display.terminal_logo_path = Some("C:/tmp/claude-mascot.jpg".to_string());
        cfg.display.mascot_image_path = None;

        let changed = cfg.normalize_and_migrate();

        assert!(changed);
        assert_eq!(
            cfg.display.mascot_image_path.as_deref(),
            Some("C:/tmp/claude-mascot.jpg")
        );
    }

    #[test]
    fn is_drive_root_detects_roots() {
        assert!(is_drive_root(Path::new("C:\\")));
        assert!(is_drive_root(Path::new("D:\\")));
        assert!(is_drive_root(Path::new("c:\\")));
        assert!(is_drive_root(Path::new("C:")));
        assert!(is_drive_root(Path::new("/")));
        assert!(!is_drive_root(Path::new("C:\\Users")));
        assert!(!is_drive_root(Path::new("D:\\X\\Work")));
        assert!(!is_drive_root(Path::new("/home/user")));
    }

    #[test]
    fn find_best_workspace_matches_deepest() {
        let workspaces = vec![
            PathBuf::from("D:\\X\\Work\\Property Alpha"),
            PathBuf::from("D:\\X\\Web Development\\MCP Servers"),
            PathBuf::from("D:\\X"),
        ];
        let result = find_best_workspace(
            Path::new("D:\\X\\Work\\Property Alpha\\src\\index.ts"),
            &workspaces,
        );
        assert_eq!(result, Some(PathBuf::from("D:\\X\\Work\\Property Alpha")));

        let result = find_best_workspace(
            Path::new("D:\\X\\Web Development\\MCP Servers\\cc-discord-presence\\src\\main.rs"),
            &workspaces,
        );
        assert_eq!(
            result,
            Some(PathBuf::from("D:\\X\\Web Development\\MCP Servers"))
        );

        let result = find_best_workspace(Path::new("D:\\X\\random\\file.txt"), &workspaces);
        assert_eq!(result, Some(PathBuf::from("D:\\X")));

        let result = find_best_workspace(Path::new("E:\\other\\file.txt"), &workspaces);
        assert_eq!(result, None);
    }

    #[cfg(windows)]
    #[test]
    fn wsl_roots_are_explicit_opt_in() {
        assert!(!parse_bool_flag(None));
        assert!(!parse_bool_flag(Some("")));
        assert!(!parse_bool_flag(Some("0")));
        assert!(!parse_bool_flag(Some("false")));
        assert!(parse_bool_flag(Some("1")));
        assert!(parse_bool_flag(Some("true")));
        assert!(parse_bool_flag(Some("YES")));
        assert!(parse_bool_flag(Some("on")));
    }
}
