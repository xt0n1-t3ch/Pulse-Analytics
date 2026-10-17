mod connections;
mod models;
mod providers;
mod store;

pub use models::{
    AccountBalance, AccountIdentity, AccountSnapshot, AccountWindow, ConnectAccountRequest,
};

use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Default)]
struct PollState {
    running: HashSet<String>,
    due: HashMap<String, Instant>,
}

static POLLS: OnceLock<Mutex<PollState>> = OnceLock::new();

fn polls() -> &'static Mutex<PollState> {
    POLLS.get_or_init(|| Mutex::new(PollState::default()))
}

/// The Accounts view lists accounts every two seconds while it is open.
const ACCOUNTS_VIEW_WINDOW: Duration = Duration::from_secs(30);

static ACCOUNTS_VIEW_SEEN: Mutex<Option<Instant>> = Mutex::new(None);

fn note_accounts_view() {
    if let Ok(mut seen) = ACCOUNTS_VIEW_SEEN.lock() {
        *seen = Some(Instant::now());
    }
}

fn accounts_view_open(now: Instant) -> bool {
    ACCOUNTS_VIEW_SEEN.lock().is_ok_and(|seen| {
        seen.is_some_and(|at| now.saturating_duration_since(at) < ACCOUNTS_VIEW_WINDOW)
    })
}

/// A Codex read starts a Codex app-server, so background polling of Codex
/// accounts runs only while the Accounts view is open or on an explicit
/// refresh or connection.
fn background_read_allowed(provider: &str, requested: bool, view_open: bool) -> bool {
    provider != "codex" || requested || view_open
}

fn provider_label(provider: &str) -> &'static str {
    match provider {
        "claude" => "Claude",
        "codex" => "Codex",
        "opencode" => "OpenCode Go",
        "commandcode" => "Command Code",
        _ => "Account",
    }
}

fn validate_provider(provider: &str) -> Result<()> {
    if !matches!(provider, "claude" | "codex" | "opencode" | "commandcode") {
        bail!("Unsupported account provider");
    }
    Ok(())
}

pub fn discover() -> Result<Vec<AccountSnapshot>> {
    let home = cc_discord_presence::config::claude_home();
    let candidates = [
        ("claude", home),
        ("codex", cc_discord_presence::codex::config::codex_home()),
        ("opencode", cc_discord_presence::opencode::data_dir()),
        (
            "commandcode",
            dirs::home_dir().unwrap_or_default().join(".commandcode"),
        ),
    ];
    for (provider, path) in candidates {
        let file = if provider == "claude" {
            ".credentials.json"
        } else {
            "auth.json"
        };
        if path.join(file).is_file() {
            add_local(provider, path, false, None)?;
        }
    }
    start();
    for record in store::records()? {
        schedule(&record.id, false)?;
    }
    list()
}

fn add_local(
    provider: &str,
    path: std::path::PathBuf,
    explicit: bool,
    label: Option<String>,
) -> Result<String> {
    validate_provider(provider)?;
    if !path.is_absolute() {
        bail!("Choose an absolute profile folder");
    }
    let path = path
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("Profile folder is unavailable"))?;
    let file = if provider == "claude" {
        ".credentials.json"
    } else {
        "auth.json"
    };
    if !path.join(file).is_file() {
        bail!("This folder does not contain the provider credentials");
    }
    let source_key = format!("local:{provider}:{}", path.to_string_lossy());
    let record = store::AccountRecord {
        id: store::new_id()?,
        provider: provider.into(),
        origin: "local".into(),
        source_path: path,
        label: label
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| provider_label(provider).into()),
        removed: false,
    };
    store::add(&record, &source_key, explicit)
}

pub fn list() -> Result<Vec<AccountSnapshot>> {
    let mut snapshots = store::snapshots()?;
    let state = polls()
        .lock()
        .map_err(|_| anyhow::anyhow!("Account refresh state is unavailable"))?;
    for snapshot in &mut snapshots {
        snapshot.refreshing = state.running.contains(&snapshot.id);
    }
    drop(state);
    connections::overlay(&mut snapshots);
    Ok(snapshots)
}

pub(crate) fn command_account(profile: &std::path::Path) -> Option<AccountSnapshot> {
    let profile = profile.canonicalize().ok()?;
    let records = store::records().ok()?;
    let record = records.iter().find(|record| {
        record.provider == "commandcode"
            && record.origin == "local"
            && record.source_path.canonicalize().ok().as_ref() == Some(&profile)
    })?;
    list()
        .ok()?
        .into_iter()
        .find(|snapshot| snapshot.id == record.id)
}

pub(crate) fn command_presence(profile: &std::path::Path) -> (Option<String>, Option<String>) {
    let Some(account) = command_account(profile).filter(|account| account.status == "connected")
    else {
        return (None, None);
    };
    let parts: Vec<String> = account
        .windows
        .iter()
        .filter_map(|window| {
            let percent = window.used_percent?;
            let label = match window.duration_minutes {
                Some(300) => "5h",
                Some(10080) => "7d",
                None if window.id == "monthly" => "mo",
                _ => return None,
            };
            Some(format!("{label} {percent:.0}% used"))
        })
        .collect();
    let total = account
        .balances
        .iter()
        .filter(|balance| balance.unit == "credits")
        .try_fold(0.0, |sum, balance| {
            balance.value.parse::<f64>().ok().map(|value| sum + value)
        });
    (
        (!parts.is_empty()).then(|| parts.join(" · ")),
        total
            .filter(|value| value.is_finite())
            .map(|value| format!("Credits {value:.2}")),
    )
}

pub fn connect(request: ConnectAccountRequest) -> Result<String> {
    validate_provider(&request.provider)?;
    if request
        .label
        .as_ref()
        .is_some_and(|value| value.chars().count() > 100)
    {
        bail!("Account label is too long");
    }
    if request.method == "local" {
        let id = add_local(
            &request.provider,
            request
                .path
                .map(std::path::PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("Select a profile folder"))?,
            true,
            request.label,
        )?;
        schedule(&id, true)?;
        return Ok(id);
    }
    connections::connect(request)
}

pub fn remove(id: &str) -> Result<()> {
    let record = store::record(id)?;
    connections::cancel(id)?;
    store::remove(id)?;
    if record.origin == "managed" {
        connections::forget_secret(&record)?;
    }
    Ok(())
}

pub fn cancel(id: &str) -> Result<()> {
    connections::cancel(id)
}

pub fn refresh(id: &str) -> Result<()> {
    schedule(id, true)
}

fn wait_for_idle(id: &str, cancelled: &std::sync::atomic::AtomicBool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(35);
    while polls().lock().is_ok_and(|state| state.running.contains(id)) {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) || Instant::now() >= deadline {
            bail!("The previous usage read is still finishing. Try connecting again.");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn invalidate(id: &str) {
    if let Ok(mut state) = polls().lock() {
        state.due.remove(id);
    }
}

fn schedule(id: &str, requested: bool) -> Result<()> {
    let record = store::record(id)?;
    if record.removed
        || connections::is_pending(id)
        || !background_read_allowed(
            &record.provider,
            requested,
            accounts_view_open(Instant::now()),
        )
    {
        return Ok(());
    }
    let mut state = polls()
        .lock()
        .map_err(|_| anyhow::anyhow!("Account refresh state is unavailable"))?;
    if state.running.contains(id) || state.running.len() >= 4 {
        return Ok(());
    }
    if state.due.get(id).is_some_and(|due| *due > Instant::now()) {
        return Ok(());
    }
    state.running.insert(id.to_owned());
    drop(state);
    std::thread::spawn(move || {
        let snapshot = providers::read(&record);
        let delay = snapshot
            .retry_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|retry| {
                retry
                    .signed_duration_since(chrono::Utc::now())
                    .num_seconds()
                    .max(30) as u64
            })
            .unwrap_or(if snapshot.status == "connected" {
                if record.provider == "claude" { 300 } else { 60 }
            } else {
                120
            });
        if store::record(&record.id).is_ok() {
            let _ = store::save(&snapshot);
        } else {
            let _ = connections::forget_secret(&record);
        }
        if let Ok(mut state) = polls().lock() {
            state.running.remove(&record.id);
            state
                .due
                .insert(record.id, Instant::now() + Duration::from_secs(delay));
        }
    });
    Ok(())
}

pub fn start() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::spawn(|| {
            loop {
                if let Ok(records) = store::records() {
                    for record in records {
                        let _ = schedule(&record.id, false);
                    }
                }
                std::thread::sleep(Duration::from_secs(5));
            }
        });
    });
}

#[tauri::command]
pub fn list_accounts() -> Result<Vec<AccountSnapshot>, String> {
    note_accounts_view();
    list().map_err(|error| error.to_string())
}
#[tauri::command]
pub fn discover_accounts() -> Result<Vec<AccountSnapshot>, String> {
    note_accounts_view();
    discover().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_accounts_are_read_only_on_view_or_request() {
        assert!(!background_read_allowed("codex", false, false));
        assert!(background_read_allowed("codex", true, false));
        assert!(background_read_allowed("codex", false, true));
        for provider in ["claude", "opencode", "commandcode"] {
            assert!(background_read_allowed(provider, false, false));
        }
    }

    #[test]
    fn accounts_view_demand_lapses_after_the_view_stops_polling() {
        note_accounts_view();
        let now = Instant::now();
        assert!(accounts_view_open(now));
        assert!(!accounts_view_open(
            now + ACCOUNTS_VIEW_WINDOW + Duration::from_secs(1)
        ));
    }
}
#[tauri::command]
pub fn connect_account(request: ConnectAccountRequest) -> Result<String, String> {
    connect(request).map_err(|error| error.to_string())
}
#[tauri::command]
pub fn cancel_account_connection(account_id: String) -> Result<(), String> {
    cancel(&account_id).map_err(|error| error.to_string())
}
#[tauri::command]
pub fn remove_account(account_id: String) -> Result<(), String> {
    remove(&account_id).map_err(|error| error.to_string())
}
#[tauri::command]
pub fn refresh_account(account_id: String) -> Result<(), String> {
    refresh(&account_id).map_err(|error| error.to_string())
}
