use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use codex_presence_core::{
    ACCOUNT_RATE_LIMITS_METHOD, AccountRateLimitsRead, EffectiveLimitSelection,
    IndividualSpendLimit, RateLimitEnvelope, RateLimitResetCreditsSummary, UsageSignal,
    UsageSnapshot, UsageSource, UsageStream, parse_account_rate_limits_response,
    select_session_envelope_global_first, snapshot_from_stream_with_provenance,
};
use fs2::FileExt;
use serde_json::Value;

use crate::codex::util::silent_command;
use crate::owned_child::OwnedChild;

const CACHE_TTL: Duration = Duration::from_secs(180);
const RETRY_BACKOFF: Duration = Duration::from_secs(120);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(8);
const CLI_NEGATIVE_TTL: Duration = Duration::from_secs(600);
const PROBE_HOME_CAP_BYTES: u64 = 32 * 1024 * 1024;
const MANAGED_LOG_CAP_BYTES: u64 = 64 * 1024 * 1024;

/// Age after which a reading is stale in access routes. One cache period plus
/// one minute of scheduling slack, so a reading is stale only when a refresh
/// that should have happened did not.
pub fn reading_max_age() -> chrono::Duration {
    chrono::Duration::seconds(CACHE_TTL.as_secs() as i64 + 60)
}

/// One coherent account quota snapshot from Codex's authenticated app-server.
/// The observation time belongs to the read itself because the protocol does
/// not attach timestamps to `account/rateLimits/read` responses.
#[derive(Clone, Debug)]
pub struct AccountUsageReading {
    pub account_email: Option<String>,
    pub account_plan_type: Option<String>,
    pub envelopes: Vec<RateLimitEnvelope>,
    pub individual_limits: Vec<IndividualSpendLimit>,
    pub rate_limit_reset_credits: Option<RateLimitResetCreditsSummary>,
    pub observed_at: DateTime<Utc>,
}

impl AccountUsageReading {
    pub fn plan_envelopes(&self) -> Vec<RateLimitEnvelope> {
        let mut envelopes = self.envelopes.clone();
        if let Some(plan) = self.account_plan_type.as_ref() {
            envelopes.push(RateLimitEnvelope {
                plan_type: Some(plan.clone()),
                scope: codex_presence_core::RateLimitScope::GlobalAccount,
                observed_at: Some(self.observed_at),
                ..Default::default()
            });
        }
        envelopes
    }

    pub fn usage_snapshot(&self) -> UsageSnapshot {
        // `rateLimitsByLimitId` can contain model-scoped windows alongside the
        // account-wide bucket. Pulse's account route and broadcaster consume
        // the effective account envelope; retaining every scope here would
        // flatten a model-only 5h window into a misleading global allowance.
        // The raw envelopes stay on this reading for diagnostics and plan
        // resolution, while the chosen envelope keeps every window the
        // provider actually reported for that account scope.
        let envelopes = select_session_envelope_global_first(&self.envelopes)
            .into_iter()
            .collect();
        let stream = UsageStream::new(
            UsageSource::new(
                "codex-subscription:default",
                [UsageSignal::CodexSubscriptionUsage],
            ),
            envelopes,
        );
        snapshot_from_stream_with_provenance(&stream, "Codex account API")
    }

    pub fn effective_limits(&self) -> Option<EffectiveLimitSelection> {
        effective_limits_from_envelopes(&self.envelopes)
    }
}

type ProbeFn = fn(&Path, bool) -> Result<AccountUsageReading>;

/// Cache around the authenticated read for one Codex home. Each read starts a
/// Codex app-server, so readings are reused for `CACHE_TTL` and failures back
/// off for `RETRY_BACKOFF`. Use [`read_shared_usage`] so every caller of the
/// same home shares one manager.
#[derive(Default)]
pub struct AccountUsageManager {
    read_only: bool,
    home: Option<PathBuf>,
    auth_modified: Option<std::time::SystemTime>,
    cached: Option<AccountUsageReading>,
    cached_at: Option<Instant>,
    last_attempt: Option<Instant>,
    last_error: Option<String>,
    probe: Option<ProbeFn>,
}

impl AccountUsageManager {
    pub fn for_home(home: PathBuf, read_only: bool) -> Self {
        Self {
            home: Some(home),
            read_only,
            ..Self::default()
        }
    }

    pub fn get_usage(&mut self, force: bool) -> Result<AccountUsageReading> {
        self.get_usage_requested_at(force, Instant::now())
    }

    /// `requested_at` lets a forced read that waited behind another caller's
    /// probe reuse that newer result instead of starting a second app-server.
    fn get_usage_requested_at(
        &mut self,
        force: bool,
        requested_at: Instant,
    ) -> Result<AccountUsageReading> {
        let home = self
            .home
            .clone()
            .unwrap_or_else(crate::codex::config::codex_home);
        let modified = fs::metadata(home.join("auth.json"))
            .and_then(|meta| meta.modified())
            .ok();
        if modified != self.auth_modified {
            self.cached = None;
            self.cached_at = None;
            self.last_attempt = None;
            self.last_error = None;
            self.auth_modified = modified;
        }
        let now = Instant::now();
        if let (Some(cached), Some(cached_at)) = (&self.cached, self.cached_at)
            && (if force {
                cached_at >= requested_at
            } else {
                now.duration_since(cached_at) < CACHE_TTL
            })
        {
            return Ok(cached.clone());
        }

        if !force
            && let Some(last_attempt) = self.last_attempt
            && now.duration_since(last_attempt) < RETRY_BACKOFF
        {
            bail!(
                "{}",
                self.last_error
                    .as_deref()
                    .unwrap_or("Codex account quota retry is cooling down")
            );
        }

        self.last_attempt = Some(now);
        let probe = self.probe.unwrap_or(probe_account_usage);
        match probe(&home, self.read_only) {
            Ok(reading) => {
                self.last_error = None;
                self.store_reading(reading.clone(), Instant::now());
                Ok(reading)
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    /// A provider response is a complete snapshot for its account scope. Do
    /// not merge windows from its predecessor: a removed window is absent,
    /// rather than a zero-valued counter that may leak into the next broadcast.
    fn store_reading(&mut self, reading: AccountUsageReading, observed_at: Instant) {
        self.cached = Some(reading);
        self.cached_at = Some(observed_at);
    }
}

type SharedManager = Arc<Mutex<AccountUsageManager>>;

fn shared_managers() -> &'static Mutex<HashMap<(PathBuf, bool), SharedManager>> {
    static MANAGERS: OnceLock<Mutex<HashMap<(PathBuf, bool), SharedManager>>> = OnceLock::new();
    MANAGERS.get_or_init(Default::default)
}

fn shared_manager(home: &Path, read_only: bool) -> SharedManager {
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let mut managers = shared_managers()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    Arc::clone(
        managers
            .entry((home.clone(), read_only))
            .or_insert_with(|| {
                Arc::new(Mutex::new(AccountUsageManager::for_home(home, read_only)))
            }),
    )
}

/// Reads account usage through the one manager that owns `home` (the default
/// Codex home when `None`). Concurrent callers for the same home wait for the
/// in-flight probe and reuse its reading.
pub fn read_shared_usage(
    home: Option<&Path>,
    read_only: bool,
    force: bool,
) -> Result<AccountUsageReading> {
    let requested_at = Instant::now();
    let home = home
        .map(Path::to_path_buf)
        .unwrap_or_else(crate::codex::config::codex_home);
    let manager = shared_manager(&home, read_only);
    let mut manager = manager.lock().unwrap_or_else(PoisonError::into_inner);
    manager.get_usage_requested_at(force, requested_at)
}

pub fn effective_limits_from_envelopes(
    envelopes: &[RateLimitEnvelope],
) -> Option<EffectiveLimitSelection> {
    let selected = select_session_envelope_global_first(envelopes)?;
    Some(EffectiveLimitSelection {
        source_session_id: "account/rateLimits/read".to_string(),
        source_limit_id: selected.limit_id.clone(),
        source_scope: selected.scope,
        observed_at: selected.observed_at,
        limits: selected.limits,
        credits: selected.credits,
    })
}

/// JSONL is a fallback only while its quota event is genuinely recent. This
/// prevents an old global bucket from looking live after Codex starts emitting
/// `rate_limits: null` in newer token events.
pub fn fresh_envelopes(
    envelopes: impl IntoIterator<Item = RateLimitEnvelope>,
    now: DateTime<Utc>,
    max_age: chrono::Duration,
) -> Vec<RateLimitEnvelope> {
    envelopes
        .into_iter()
        .filter(|item| {
            item.observed_at
                .is_some_and(|observed| observed <= now && now - observed <= max_age)
        })
        .collect()
}

fn probe_account_usage(home: &Path, read_only: bool) -> Result<AccountUsageReading> {
    query_account_usage(RESPONSE_TIMEOUT, read_only, home)
}

fn query_account_usage(
    timeout: Duration,
    read_only: bool,
    home: &Path,
) -> Result<AccountUsageReading> {
    let cli = resolve_codex_cli().context("Codex CLI is not installed")?;
    let external_auth = if read_only {
        let bytes = std::fs::read(home.join("auth.json"))
            .context("Codex local credentials are unavailable")?;
        let auth: Value = serde_json::from_slice(&bytes)?;
        let access = auth
            .pointer("/tokens/access_token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .context("Codex access token is unavailable")?;
        let account = auth
            .pointer("/tokens/account_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .context("Codex account identity is unavailable")?;
        Some(
            serde_json::json!({"type": "chatgptAuthTokens", "accessToken": access, "chatgptAccountId": account}),
        )
    } else {
        None
    };
    let probe_home = read_only.then(ProbeHomeLease::acquire).transpose()?;
    let mut command = app_server_command(&cli, probe_home.is_some());
    command.env(
        "CODEX_HOME",
        probe_home
            .as_ref()
            .map_or(home, |lease| lease.path.as_path()),
    );
    let mut child = OwnedChild::spawn(
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()),
    )
    .context("failed to start Codex app-server quota reader")?;

    let result = (|| {
        let mut stdin = child
            .take_stdin()
            .context("Codex app-server stdin unavailable")?;
        let stdout = child
            .take_stdout()
            .context("Codex app-server stdout unavailable")?;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        write_json_line(
            &mut stdin,
            &serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": { "clientInfo": { "name": "pulse", "version": env!("CARGO_PKG_VERSION") }, "capabilities": { "experimentalApi": true } }
            }),
        )?;
        let initialized = read_response(&rx, 1, timeout)?;
        if initialized.get("error").is_some() {
            bail!("Codex app-server rejected initialization");
        }

        write_json_line(
            &mut stdin,
            &serde_json::json!({ "method": "initialized", "params": {} }),
        )?;
        if let Some(auth) = external_auth {
            write_json_line(
                &mut stdin,
                &serde_json::json!({"id": 5, "method": "account/login/start", "params": auth}),
            )?;
            let response = read_response(&rx, 5, timeout)?;
            if response.get("error").is_some() {
                bail!("Codex read-only account connection is unavailable");
            }
        }
        write_json_line(
            &mut stdin,
            &serde_json::json!({ "id": 2, "method": ACCOUNT_RATE_LIMITS_METHOD, "params": null }),
        )?;
        let response = read_response(&rx, 2, timeout)?;
        let observed_at = Utc::now();
        let usage = parse_rate_limits_response(&response.to_string(), observed_at);
        write_json_line(
            &mut stdin,
            &serde_json::json!({"id": 3, "method": "account/read", "params": {"refreshToken": false}}),
        )?;
        let account = read_response(&rx, 3, timeout).ok();
        let account_plan_type = account.as_ref().and_then(account_plan_type);
        let account_email = account
            .as_ref()
            .and_then(|value| value.pointer("/result/account/email"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        match usage {
            Ok(mut reading) => {
                reading.account_email = account_email;
                reading.account_plan_type = account_plan_type;
                Ok(reading)
            }
            Err(_) if account_plan_type.is_some() => Ok(AccountUsageReading {
                account_email,
                account_plan_type,
                envelopes: Vec::new(),
                individual_limits: Vec::new(),
                rate_limit_reset_credits: None,
                observed_at,
            }),
            Err(error) => Err(error),
        }
    })();

    // The probe home is cleaned only after every process that could hold its
    // files has exited.
    child.terminate();
    drop(probe_home);
    if !read_only {
        prune_managed_logs(home, MANAGED_LOG_CAP_BYTES);
    }
    result
}

/// Exclusive use of the shared probe home, across threads and Pulse processes.
/// No credential outlives the lease, and the home is reset when Codex's own
/// logs push it past its size cap.
struct ProbeHomeLease {
    path: PathBuf,
    _lock: File,
}

impl ProbeHomeLease {
    fn acquire() -> Result<Self> {
        Self::acquire_at(crate::storage::codex_probe_home())
    }

    fn acquire_at(path: PathBuf) -> Result<Self> {
        let parent = path.parent().context("Codex probe home has no parent")?;
        fs::create_dir_all(parent)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))
            .context("Codex probe home lock is unavailable")?;
        lock.lock_exclusive()
            .context("Codex probe home lock is unavailable")?;
        fs::create_dir_all(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        // A crash during an earlier probe may have left a credential behind.
        remove_probe_credentials(&path);
        Ok(Self { path, _lock: lock })
    }
}

impl Drop for ProbeHomeLease {
    fn drop(&mut self) {
        remove_probe_credentials(&self.path);
        if directory_size(&self.path) > PROBE_HOME_CAP_BYTES
            && let Err(error) = fs::remove_dir_all(&self.path)
        {
            tracing::warn!(%error, "failed to reset the Codex probe home");
        }
    }
}

fn remove_probe_credentials(home: &Path) {
    match fs::remove_file(home.join("auth.json")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(%error, "failed to remove the Codex probe credential"),
    }
}

fn directory_size(path: &Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.metadata().ok())
        .filter(fs::Metadata::is_file)
        .map(|metadata| metadata.len())
        .sum()
}

/// Pulse-managed account homes keep their credential, so only Codex's log
/// databases are removed once they pass `cap`. Homes outside Pulse storage
/// belong to the user and are never touched.
fn prune_managed_logs(home: &Path, cap: u64) {
    let root = crate::storage::home();
    let root = root.canonicalize().unwrap_or(root);
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    if !home.starts_with(&root) {
        return;
    }
    let home = home.as_path();
    prune_codex_logs(home, cap);
}

fn prune_codex_logs(home: &Path, cap: u64) {
    let Ok(entries) = fs::read_dir(home) else {
        return;
    };
    let logs: Vec<(PathBuf, u64)> = entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with("logs_") && name.contains(".sqlite")
        })
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| (entry.path(), metadata.len()))
        })
        .collect();
    if logs.iter().map(|(_, len)| len).sum::<u64>() <= cap {
        return;
    }
    for (path, _) in logs {
        if let Err(error) = fs::remove_file(&path) {
            tracing::warn!(path = %path.display(), %error, "failed to prune Codex log database");
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CodexCli {
    Executable(PathBuf),
    /// An npm `codex.cmd` shim on PATH whose native binary was not found.
    #[cfg(windows)]
    BatchShim(PathBuf),
}

impl CodexCli {
    fn path(&self) -> &Path {
        match self {
            Self::Executable(path) => path,
            #[cfg(windows)]
            Self::BatchShim(path) => path,
        }
    }
}

fn app_server_command(cli: &CodexCli, file_credentials: bool) -> Command {
    match cli {
        CodexCli::Executable(path) => {
            let mut command = silent_command(path.to_string_lossy().as_ref());
            command.args(["app-server", "--stdio"]);
            if file_credentials {
                command.args(["-c", "cli_auth_credentials_store=\"file\""]);
            }
            command
        }
        #[cfg(windows)]
        CodexCli::BatchShim(_) => {
            // CreateProcess does not run batch files directly. cmd.exe is the
            // direct child, so the job object owns the Node grandchild. Codex
            // reads an unparseable `-c` value as a plain string, which avoids
            // nesting quotes through cmd.exe and the shim.
            let mut command = silent_command("cmd.exe");
            command.args([
                "/d",
                "/s",
                "/c",
                if file_credentials {
                    "codex app-server --stdio -c cli_auth_credentials_store=file"
                } else {
                    "codex app-server --stdio"
                },
            ]);
            command
        }
    }
}

/// Command for an app-server that owns `home`'s credentials (managed sign-in).
pub fn isolated_app_server_command(home: &Path) -> Result<Command> {
    let cli = resolve_codex_cli().context("Codex CLI is not installed")?;
    let mut command = app_server_command(&cli, true);
    command.env("CODEX_HOME", home);
    Ok(command)
}

/// Whether a Codex CLI can be started, from the cached resolution.
pub fn codex_cli_available() -> bool {
    resolve_codex_cli().is_some()
}

struct CliCache {
    resolved: Option<CodexCli>,
    checked_at: Instant,
}

static CODEX_CLI: Mutex<Option<CliCache>> = Mutex::new(None);

fn resolve_codex_cli() -> Option<CodexCli> {
    let mut cache = CODEX_CLI.lock().unwrap_or_else(PoisonError::into_inner);
    cached_codex_cli(&mut cache, Instant::now(), discover_codex_cli)
}

/// A found CLI is reused until its file disappears; a miss is retried after
/// `CLI_NEGATIVE_TTL`, so an install is picked up without a restart.
fn cached_codex_cli(
    cache: &mut Option<CliCache>,
    now: Instant,
    discover: impl FnOnce() -> Option<CodexCli>,
) -> Option<CodexCli> {
    if let Some(entry) = cache.as_ref() {
        match &entry.resolved {
            Some(cli) if cli.path().is_file() => return Some(cli.clone()),
            None if now.duration_since(entry.checked_at) < CLI_NEGATIVE_TTL => return None,
            _ => {}
        }
    }
    let resolved = discover();
    *cache = Some(CliCache {
        resolved: resolved.clone(),
        checked_at: now,
    });
    resolved
}

fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default()
}

#[cfg(not(windows))]
fn discover_codex_cli() -> Option<CodexCli> {
    std::env::var_os("CODEX_CLI_PATH")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            path_dirs()
                .into_iter()
                .map(|dir| dir.join("codex"))
                .find(|path| path.is_file())
        })
        .map(CodexCli::Executable)
}

#[cfg(windows)]
fn discover_codex_cli() -> Option<CodexCli> {
    if let Some(path) = std::env::var_os("CODEX_CLI_PATH").map(PathBuf::from)
        && is_codex_executable(&path)
    {
        return Some(CodexCli::Executable(path));
    }
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let (executable, shim) = discover_codex_cli_in(&path_dirs(), local_app_data.as_deref());
    executable
        .or_else(windows_codex_cli_path_from_powershell)
        .map(CodexCli::Executable)
        .or_else(|| shim.map(CodexCli::BatchShim))
}

/// Filesystem-only lookup: a native `codex.exe` on PATH, the user-local Codex
/// installs, then the binary behind an npm `codex.cmd` shim. Returns the shim
/// separately as the last resort.
#[cfg(windows)]
fn discover_codex_cli_in(
    path_dirs: &[PathBuf],
    local_app_data: Option<&Path>,
) -> (Option<PathBuf>, Option<PathBuf>) {
    // App execution aliases there are reparse points, not the CLI.
    let path_dirs: Vec<&PathBuf> = path_dirs
        .iter()
        .filter(|dir| {
            !dir.ends_with(Path::new("Microsoft").join("WindowsApps"))
                && !dir
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .ends_with("\\microsoft\\windowsapps")
        })
        .collect();
    if let Some(path) = path_dirs
        .iter()
        .map(|dir| dir.join("codex.exe"))
        .find(|path| is_codex_executable(path))
    {
        return (Some(path), None);
    }
    if let Some(local) = local_app_data {
        let programs = local.join("Programs").join("OpenAI");
        if let Some(path) = [
            programs.join("CodexCLI").join("bin").join("codex.exe"),
            programs.join("Codex").join("bin").join("codex.exe"),
        ]
        .into_iter()
        .find(|path| is_codex_executable(path))
        {
            return (Some(path), None);
        }
        if let Some(path) =
            find_codex_executable_under(&local.join("OpenAI").join("Codex").join("bin"), 4)
        {
            return (Some(path), None);
        }
    }
    let shim = path_dirs
        .iter()
        .map(|dir| dir.join("codex.cmd"))
        .find(|path| path.is_file());
    let shim_target = shim.as_ref().and_then(|shim| {
        let package = shim
            .parent()?
            .join("node_modules")
            .join("@openai")
            .join("codex");
        find_codex_executable_under(&package, 8)
    });
    match shim_target {
        Some(path) => (Some(path), None),
        None => (None, shim),
    }
}

#[cfg(windows)]
fn find_codex_executable_under(root: &Path, max_depth: usize) -> Option<PathBuf> {
    if !root.is_dir() {
        return None;
    }
    walkdir::WalkDir::new(root)
        .max_depth(max_depth)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .map(walkdir::DirEntry::into_path)
        .find(|path| is_codex_executable(path))
}

#[cfg(windows)]
fn windows_codex_cli_path_from_powershell() -> Option<PathBuf> {
    let script = r#"
$paths = [System.Collections.Generic.List[string]]::new()
$localRoot = Join-Path $env:LOCALAPPDATA 'OpenAI\Codex\bin'
if (Test-Path -LiteralPath $localRoot) {
  Get-ChildItem -LiteralPath $localRoot -Recurse -Filter 'codex.exe' -File -ErrorAction SilentlyContinue |
    ForEach-Object { [void]$paths.Add($_.FullName) }
}
$package = Get-AppxPackage -Name OpenAI.Codex -ErrorAction SilentlyContinue | Select-Object -First 1
if ($null -ne $package) {
  [void]$paths.Add((Join-Path $package.InstallLocation 'app\resources\codex.exe'))
}
Get-CimInstance Win32_Process -Filter "Name = 'codex.exe'" -ErrorAction SilentlyContinue |
  ForEach-Object { if ($_.ExecutablePath) { [void]$paths.Add($_.ExecutablePath) } }
$paths | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -Unique
"#;
    let output = silent_command("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    select_existing_codex_path(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(windows)]
fn select_existing_codex_path(output: &str) -> Option<PathBuf> {
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    select_existing_codex_path_for_local_root(output, local_app_data.as_deref())
}

#[cfg(windows)]
fn select_existing_codex_path_for_local_root(
    output: &str,
    local_app_data: Option<&Path>,
) -> Option<PathBuf> {
    let candidates = output
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .collect::<Vec<_>>();

    // Pulse is unpackaged on Windows. Prefer the user-local CLI that Codex
    // exposes to sibling processes before falling back to the package path,
    // whose ACL can deny CreateProcess outside the AppX container.
    candidates
        .iter()
        .find(|path| local_app_data.is_some_and(|root| is_user_local_codex_cli(path, root)))
        .cloned()
        .or_else(|| {
            candidates
                .into_iter()
                .find(|path| is_bundled_codex_cli(path))
        })
}

#[cfg(windows)]
fn is_user_local_codex_cli(path: &Path, local_app_data: &Path) -> bool {
    if !is_codex_executable(path) {
        return false;
    }
    let expected_root = local_app_data.join("OpenAI").join("Codex").join("bin");
    path.parent()
        .is_some_and(|parent| path_starts_with_case_insensitive(parent, &expected_root))
}

#[cfg(windows)]
fn path_starts_with_case_insensitive(path: &Path, root: &Path) -> bool {
    let mut path_components = path.components();
    root.components().all(|expected| {
        path_components.next().is_some_and(|actual| {
            actual
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy())
        })
    })
}

#[cfg(windows)]
fn is_bundled_codex_cli(path: &Path) -> bool {
    is_codex_executable(path)
        && path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name.eq_ignore_ascii_case("resources"))
}

#[cfg(windows)]
fn is_codex_executable(path: &Path) -> bool {
    path.is_file()
        && path
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("codex.exe"))
}

fn write_json_line(stdin: &mut impl Write, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *stdin, value)
        .context("failed to encode Codex app-server request")?;
    stdin
        .write_all(b"\n")
        .context("failed to write Codex app-server request")?;
    stdin
        .flush()
        .context("failed to flush Codex app-server request")
}

fn read_response(rx: &Receiver<String>, id: i64, timeout: Duration) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("Codex app-server response timed out");
        }
        let line = rx
            .recv_timeout(remaining)
            .context("Codex app-server response stream closed")?;
        let value: Value =
            serde_json::from_str(&line).context("Codex app-server emitted invalid JSON")?;
        if value.get("id").and_then(Value::as_i64) == Some(id) {
            return Ok(value);
        }
    }
}

pub fn parse_rate_limits_response(
    response: &str,
    observed_at: DateTime<Utc>,
) -> Result<AccountUsageReading> {
    let AccountRateLimitsRead {
        envelopes,
        individual_limits,
        rate_limit_reset_credits,
        observed_at,
    } = parse_account_rate_limits_response(response, observed_at)
        .map_err(|error| anyhow::anyhow!(error))?;
    Ok(AccountUsageReading {
        account_email: None,
        account_plan_type: None,
        envelopes,
        individual_limits,
        rate_limit_reset_credits,
        observed_at,
    })
}

fn account_plan_type(response: &Value) -> Option<String> {
    let account = response.pointer("/result/account")?;
    if account.get("type").and_then(Value::as_str) != Some("chatgpt") {
        return None;
    }
    let plan = account.get("planType").and_then(Value::as_str)?;
    (crate::codex::telemetry::plan::parse_plan_type(Some(plan))
        != crate::codex::telemetry::plan::DetectedPlanTier::Unknown)
        .then(|| plan.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn account_reading_with_global_windows(
        windows: Vec<codex_presence_core::UsageWindow>,
    ) -> AccountUsageReading {
        AccountUsageReading {
            account_email: None,
            account_plan_type: None,
            envelopes: vec![RateLimitEnvelope {
                limit_id: Some("codex".to_string()),
                scope: codex_presence_core::RateLimitScope::GlobalAccount,
                limits: codex_presence_core::RateLimits::new(windows),
                ..RateLimitEnvelope::default()
            }],
            individual_limits: Vec::new(),
            rate_limit_reset_credits: None,
            observed_at: Utc.timestamp_opt(2_000, 0).single().unwrap(),
        }
    }

    #[test]
    fn freshness_filter_drops_yesterdays_jsonl_quota() {
        let now = Utc.timestamp_opt(2_000, 0).single().unwrap();
        let stale = RateLimitEnvelope {
            observed_at: Utc.timestamp_opt(1_000, 0).single(),
            ..RateLimitEnvelope::default()
        };
        let fresh = RateLimitEnvelope {
            observed_at: Utc.timestamp_opt(1_950, 0).single(),
            ..RateLimitEnvelope::default()
        };

        let filtered = fresh_envelopes(vec![stale, fresh], now, chrono::Duration::minutes(15));
        assert_eq!(filtered.len(), 1);
        assert_eq!(
            filtered[0].observed_at,
            Utc.timestamp_opt(1_950, 0).single()
        );
    }

    #[test]
    fn owned_query_command_is_non_interactive() {
        let command = app_server_command(&CodexCli::Executable(PathBuf::from("codex")), true);
        let debug = format!("{command:?}");
        assert!(debug.contains("app-server"));
        assert!(debug.contains("stdio"));
        assert!(debug.contains("cli_auth_credentials_store"));
    }

    fn reading() -> AccountUsageReading {
        account_reading_with_global_windows(Vec::new())
    }

    fn manager_with_probe(probe: ProbeFn) -> AccountUsageManager {
        AccountUsageManager {
            probe: Some(probe),
            ..AccountUsageManager::for_home(PathBuf::from("pulse-test-missing-codex-home"), true)
        }
    }

    #[test]
    fn readings_are_reused_for_the_cache_period_and_refreshed_after_it() {
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        fn probe(_: &Path, _: bool) -> Result<AccountUsageReading> {
            CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(reading())
        }
        let mut manager = manager_with_probe(probe);
        for _ in 0..36 {
            manager.get_usage(false).expect("reading");
        }
        assert_eq!(CALLS.load(std::sync::atomic::Ordering::SeqCst), 1);

        let expired = Instant::now() - CACHE_TTL - Duration::from_secs(1);
        manager.cached_at = Some(expired);
        manager.last_attempt = Some(expired);
        manager.get_usage(false).expect("refreshed reading");
        assert_eq!(CALLS.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn failures_back_off_unless_the_user_forces_a_refresh() {
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        fn probe(_: &Path, _: bool) -> Result<AccountUsageReading> {
            CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            bail!("Codex app-server response timed out")
        }
        let mut manager = manager_with_probe(probe);
        assert!(manager.get_usage(false).is_err());
        let cooling = manager.get_usage(false).expect_err("cooling down");
        assert_eq!(cooling.to_string(), "Codex app-server response timed out");
        assert_eq!(CALLS.load(std::sync::atomic::Ordering::SeqCst), 1);

        manager.last_attempt = Some(Instant::now() - RETRY_BACKOFF - Duration::from_secs(1));
        assert!(manager.get_usage(false).is_err());
        assert!(manager.get_usage(true).is_err());
        assert_eq!(CALLS.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[test]
    fn a_forced_read_reuses_a_probe_that_finished_while_it_waited() {
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        fn probe(_: &Path, _: bool) -> Result<AccountUsageReading> {
            CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(reading())
        }
        let mut manager = manager_with_probe(probe);
        let requested_at = Instant::now();
        manager.store_reading(reading(), requested_at);
        manager
            .get_usage_requested_at(true, requested_at)
            .expect("reused");
        thread::sleep(Duration::from_millis(2));
        assert_eq!(CALLS.load(std::sync::atomic::Ordering::SeqCst), 0);
        manager.get_usage(true).expect("forced");
        assert_eq!(CALLS.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn every_lane_for_one_home_shares_a_single_probe() {
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        fn slow_probe(_: &Path, _: bool) -> Result<AccountUsageReading> {
            CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            thread::sleep(Duration::from_millis(200));
            Ok(reading())
        }
        let home = tempfile::tempdir().expect("home");
        shared_manager(home.path(), true)
            .lock()
            .expect("manager")
            .probe = Some(slow_probe);

        // The main lane and the Accounts lane read concurrently, and the
        // Accounts lane names the same home through a different spelling.
        let alias = home.path().join(".");
        let lanes: Vec<_> = [home.path().to_path_buf(), alias]
            .into_iter()
            .map(|path| thread::spawn(move || read_shared_usage(Some(&path), true, false)))
            .collect();
        for lane in lanes {
            lane.join().expect("lane").expect("reading");
        }
        read_shared_usage(Some(home.path()), true, false).expect("cached");
        assert_eq!(CALLS.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn probe_home_is_reused_and_never_keeps_a_credential() {
        let root = tempfile::tempdir().expect("root");
        let path = root.path().join("codex").join("probe-home");
        {
            let lease = ProbeHomeLease::acquire_at(path.clone()).expect("lease");
            fs::write(lease.path.join("auth.json"), b"{\"tokens\":{}}").expect("auth");
            fs::write(lease.path.join("logs_2.sqlite"), b"log").expect("log");
            let contender = File::open(path.with_extension("lock")).expect("lock file");
            assert!(
                contender.try_lock_exclusive().is_err(),
                "a second probe must wait for the shared home"
            );
        }
        assert!(!path.join("auth.json").exists());
        assert!(
            path.join("logs_2.sqlite").exists(),
            "small homes are reused"
        );

        fs::write(path.join("auth.json"), b"left by a crash").expect("stale auth");
        let lease = ProbeHomeLease::acquire_at(path.clone()).expect("lease");
        assert!(!lease.path.join("auth.json").exists());
        drop(lease);
    }

    #[test]
    fn probe_home_is_reset_once_it_passes_its_size_cap() {
        let root = tempfile::tempdir().expect("root");
        let path = root.path().join("probe-home");
        {
            let lease = ProbeHomeLease::acquire_at(path.clone()).expect("lease");
            let log = File::create(lease.path.join("logs_2.sqlite")).expect("log");
            log.set_len(PROBE_HOME_CAP_BYTES + 1).expect("grow log");
        }
        assert!(!path.exists());
        ProbeHomeLease::acquire_at(path.clone()).expect("recreated");
        assert!(path.is_dir());
    }

    #[test]
    fn managed_log_pruning_keeps_the_credential() {
        let home = tempfile::tempdir().expect("home");
        fs::write(home.path().join("auth.json"), b"credential").expect("auth");
        fs::write(home.path().join("logs_2.sqlite"), vec![0; 64]).expect("log");
        fs::write(home.path().join("logs_2.sqlite-wal"), vec![0; 64]).expect("wal");
        prune_codex_logs(home.path(), 1024);
        assert!(home.path().join("logs_2.sqlite").exists());
        prune_codex_logs(home.path(), 100);
        assert!(!home.path().join("logs_2.sqlite").exists());
        assert!(!home.path().join("logs_2.sqlite-wal").exists());
        assert_eq!(
            fs::read(home.path().join("auth.json")).expect("auth"),
            b"credential"
        );
    }

    #[test]
    fn resolved_cli_is_cached_until_its_file_disappears() {
        let dir = tempfile::tempdir().expect("dir");
        let exe = dir.path().join("codex.exe");
        fs::write(&exe, b"fixture").expect("fixture");
        let mut cache = None;
        let now = Instant::now();
        let found = CodexCli::Executable(exe.clone());
        assert_eq!(
            cached_codex_cli(&mut cache, now, || Some(found.clone())),
            Some(found.clone())
        );
        assert_eq!(
            cached_codex_cli(&mut cache, now, || panic!("must reuse the cached path")),
            Some(found)
        );
        fs::remove_file(&exe).expect("remove");
        assert_eq!(cached_codex_cli(&mut cache, now, || None), None);
        assert_eq!(
            cached_codex_cli(&mut cache, now + Duration::from_secs(1), || panic!(
                "a miss is cached"
            )),
            None
        );
        let later = now + CLI_NEGATIVE_TTL + Duration::from_secs(1);
        assert_eq!(
            cached_codex_cli(&mut cache, later, || Some(CodexCli::Executable(
                PathBuf::from("x")
            ))),
            Some(CodexCli::Executable(PathBuf::from("x")))
        );
    }

    #[cfg(windows)]
    #[test]
    fn cli_discovery_prefers_native_binaries_over_the_npm_shim() {
        let root = tempfile::tempdir().expect("root");
        let aliases = root.path().join("Microsoft").join("WindowsApps");
        let npm = root.path().join("npm");
        fs::create_dir_all(&aliases).expect("aliases");
        fs::create_dir_all(&npm).expect("npm");
        fs::write(aliases.join("codex.exe"), b"alias").expect("alias");
        fs::write(npm.join("codex.cmd"), b"@echo off").expect("shim");

        let dirs = vec![aliases.clone(), npm.clone()];
        assert_eq!(
            discover_codex_cli_in(&dirs, None),
            (None, Some(npm.join("codex.cmd"))),
            "an execution alias is not the CLI"
        );

        let vendored = npm
            .join("node_modules")
            .join("@openai")
            .join("codex")
            .join("vendor")
            .join("x86_64-pc-windows-msvc")
            .join("codex")
            .join("codex.exe");
        fs::create_dir_all(vendored.parent().expect("parent")).expect("vendor");
        fs::write(&vendored, b"native").expect("native");
        assert_eq!(discover_codex_cli_in(&dirs, None), (Some(vendored), None));

        let local = root.path().join("Local");
        let installed = local
            .join("Programs")
            .join("OpenAI")
            .join("CodexCLI")
            .join("bin")
            .join("codex.exe");
        fs::create_dir_all(installed.parent().expect("parent")).expect("install");
        fs::write(&installed, b"installed").expect("installed");
        assert_eq!(
            discover_codex_cli_in(&dirs, Some(&local)),
            (Some(installed), None)
        );
    }

    #[test]
    fn windows_provider_probes_use_the_hidden_launcher() {
        let source = include_str!("account_usage.rs");
        assert!(
            source.contains("silent_command(\"powershell\")"),
            "the Windows subscription probe must inherit CREATE_NO_WINDOW"
        );
        assert!(
            source.contains("silent_command(\"cmd.exe\")"),
            "the Windows app-server fallback must inherit CREATE_NO_WINDOW"
        );
        assert!(
            !source.contains("Command::new(\"powershell\")"),
            "provider probes must not bypass the shared hidden launcher"
        );
    }

    #[cfg(windows)]
    #[test]
    fn bundled_cli_resolution_rejects_the_desktop_gui_executable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gui = dir.path().join("Codex.exe");
        let resources = dir.path().join("app").join("resources");
        std::fs::create_dir_all(&resources).expect("resources directory");
        let expected = resources.join("codex.exe");
        std::fs::write(&gui, b"gui fixture").expect("write gui fixture");
        std::fs::write(&expected, b"fixture").expect("write fixture");
        let output = format!("{}\n{}\n", gui.display(), expected.display());

        assert_eq!(select_existing_codex_path(&output), Some(expected));
    }

    #[cfg(windows)]
    #[test]
    fn bundled_cli_resolution_prefers_the_user_local_cli_over_packaged_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let packaged = dir
            .path()
            .join("WindowsApps")
            .join("OpenAI.Codex_26.727.6591.0_x64__fixture")
            .join("app")
            .join("resources")
            .join("codex.exe");
        let user_local = dir
            .path()
            .join("AppData")
            .join("Local")
            .join("OpenAI")
            .join("Codex")
            .join("bin")
            .join("fixture")
            .join("codex.exe");
        std::fs::create_dir_all(packaged.parent().expect("packaged parent"))
            .expect("packaged directory");
        std::fs::create_dir_all(user_local.parent().expect("user-local parent"))
            .expect("user-local directory");
        std::fs::write(&packaged, b"packaged fixture").expect("packaged fixture");
        std::fs::write(&user_local, b"user-local fixture").expect("user-local fixture");
        let output = format!("{}\n{}\n", packaged.display(), user_local.display());
        let local_app_data = dir.path().join("AppData").join("Local");

        assert_eq!(
            select_existing_codex_path_for_local_root(&output, Some(&local_app_data)),
            Some(user_local),
            "an unpackaged Pulse process must not select the inaccessible WindowsApps CLI"
        );
    }

    #[test]
    fn fresh_account_response_removes_a_window_from_the_cached_snapshot() {
        let mut manager = AccountUsageManager::default();
        let cached_at = Instant::now();
        manager.store_reading(
            account_reading_with_global_windows(vec![
                codex_presence_core::UsageWindow {
                    used_percent: 0.0,
                    remaining_percent: 100.0,
                    window_minutes: 300,
                    resets_at: None,
                },
                codex_presence_core::UsageWindow {
                    used_percent: 4.0,
                    remaining_percent: 96.0,
                    window_minutes: 10_080,
                    resets_at: None,
                },
            ]),
            cached_at,
        );
        manager.store_reading(
            account_reading_with_global_windows(vec![codex_presence_core::UsageWindow {
                used_percent: 5.0,
                remaining_percent: 95.0,
                window_minutes: 10_080,
                resets_at: None,
            }]),
            cached_at + Duration::from_secs(1),
        );

        let snapshot = manager
            .cached
            .as_ref()
            .expect("fresh response cached")
            .usage_snapshot();
        assert_eq!(snapshot.scopes.len(), 1);
        assert_eq!(snapshot.scopes[0].windows.len(), 1);
        assert_eq!(snapshot.scopes[0].windows[0].window_minutes, 10_080);
        assert_eq!(snapshot.scopes[0].windows[0].remaining_percent, 95.0);
    }
    #[test]
    fn account_plan_is_independent_from_quota_windows() {
        for raw in [
            "free",
            "go",
            "plus",
            "pro",
            "prolite",
            "pro_5x",
            "pro_20x",
            "business",
            "enterprise",
            "edu",
        ] {
            let response =
                serde_json::json!({"result":{"account":{"type":"chatgpt","planType":raw}}});
            assert_eq!(account_plan_type(&response).as_deref(), Some(raw));
        }
        assert!(
            account_plan_type(&serde_json::json!({"result":{"account":{"type":"apiKey"}}}))
                .is_none()
        );
        let reading = AccountUsageReading {
            account_email: None,
            account_plan_type: Some("prolite".into()),
            envelopes: Vec::new(),
            individual_limits: Vec::new(),
            rate_limit_reset_credits: None,
            observed_at: Utc::now(),
        };
        assert_eq!(
            reading.plan_envelopes()[0].plan_type.as_deref(),
            Some("prolite")
        );
        assert!(reading.envelopes.is_empty());
        assert!(reading.effective_limits().is_none());
    }
}
