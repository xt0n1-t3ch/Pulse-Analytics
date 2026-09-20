use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
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
use serde_json::Value;

use crate::codex::util::silent_command;

const CACHE_TTL: Duration = Duration::from_secs(30);
const RETRY_BACKOFF: Duration = Duration::from_secs(15);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(8);

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

/// Short-lived cache around the authenticated read. Pulse polls every five
/// seconds; spawning a new app-server for every UI tick would be wasteful and
/// would not make provider quota more accurate than this 30-second cadence.
#[derive(Default)]
pub struct AccountUsageManager {
    read_only: bool,
    home: Option<PathBuf>,
    auth_modified: Option<std::time::SystemTime>,
    cached: Option<AccountUsageReading>,
    cached_at: Option<Instant>,
    last_attempt: Option<Instant>,
}

impl AccountUsageManager {
    pub fn for_home(home: PathBuf, read_only: bool) -> Self {
        Self {
            home: Some(home),
            read_only,
            ..Self::default()
        }
    }

    pub fn read_only() -> Self {
        Self {
            read_only: true,
            ..Self::default()
        }
    }

    pub fn get_usage(&mut self, force: bool) -> Result<AccountUsageReading> {
        let home = self
            .home
            .clone()
            .unwrap_or_else(crate::codex::config::codex_home);
        let modified = std::fs::metadata(home.join("auth.json"))
            .and_then(|meta| meta.modified())
            .ok();
        if modified != self.auth_modified {
            self.cached = None;
            self.cached_at = None;
            self.last_attempt = None;
            self.auth_modified = modified;
        }
        let now = Instant::now();
        if !force
            && let (Some(cached), Some(cached_at)) = (&self.cached, self.cached_at)
            && now.duration_since(cached_at) < CACHE_TTL
        {
            return Ok(cached.clone());
        }

        if !force
            && let Some(last_attempt) = self.last_attempt
            && now.duration_since(last_attempt) < RETRY_BACKOFF
        {
            bail!("Codex account quota retry is cooling down");
        }

        self.last_attempt = Some(now);
        let reading = query_account_usage(RESPONSE_TIMEOUT, self.read_only, &home)?;
        self.store_reading(reading.clone(), now);
        Ok(reading)
    }

    /// A provider response is a complete snapshot for its account scope. Do
    /// not merge windows from its predecessor: a removed window is absent,
    /// rather than a zero-valued counter that may leak into the next broadcast.
    fn store_reading(&mut self, reading: AccountUsageReading, observed_at: Instant) {
        self.cached = Some(reading);
        self.cached_at = Some(observed_at);
    }
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

fn query_account_usage(
    timeout: Duration,
    read_only: bool,
    home: &Path,
) -> Result<AccountUsageReading> {
    let isolated_home = read_only.then(tempfile::tempdir).transpose()?;
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
    let mut command = codex_app_server_command();
    if let Some(home) = &isolated_home {
        command.env("CODEX_HOME", home.path());
        command.args(["-c", "cli_auth_credentials_store=\"file\""]);
    } else {
        command.env("CODEX_HOME", home);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start Codex app-server quota reader")?;

    let result = (|| {
        let mut stdin = child
            .stdin
            .take()
            .context("Codex app-server stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
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

    stop_owned_child(&mut child);
    result
}

fn codex_app_server_command() -> Command {
    #[cfg(windows)]
    {
        if let Some(path) = windows_codex_cli_path() {
            let mut command = silent_command(path.to_string_lossy().as_ref());
            command.args(["app-server", "--stdio"]);
            return command;
        }

        // `codex` is commonly an npm .cmd shim on Windows. CreateProcess does
        // not execute batch files directly, so this is the final PATH fallback.
        let mut command = silent_command("cmd.exe");
        command.args(["/d", "/s", "/c", "codex app-server --stdio"]);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = silent_command("codex");
        command.args(["app-server", "--stdio"]);
        command
    }
}

pub fn isolated_app_server_command(home: &Path) -> Command {
    let mut command = codex_app_server_command();
    command.env("CODEX_HOME", home);
    command.args(["-c", "cli_auth_credentials_store=\"file\""]);
    command
}

#[cfg(windows)]
fn windows_codex_cli_path() -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("CODEX_CLI_PATH") {
        let path = PathBuf::from(configured);
        if is_codex_executable(&path) {
            return Some(path);
        }
    }

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

fn stop_owned_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
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
        let command = codex_app_server_command();
        let debug = format!("{command:?}");
        assert!(debug.contains("app-server"));
        assert!(debug.contains("stdio"));
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
