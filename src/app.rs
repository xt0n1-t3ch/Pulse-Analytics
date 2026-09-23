use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::debug;

use crate::config::{self, PresenceConfig, RuntimeSettings};
use crate::discord::DiscordPresence;
use crate::metrics::MetricsTracker;
use crate::process_guard::{self, RunningState};
use crate::session::{
    ClaudeSessionSnapshot, GitBranchCache, RateLimits, SessionParseCache,
    collect_active_sessions_multi, latest_limits_source, merge_statusline_into_sessions,
    preferred_active_session, read_statusline_data,
};
use crate::usage::UsageManager;
use crate::util::{format_time_until, format_token_triplet};

#[derive(Debug, Clone)]
pub enum AppMode {
    /// Headless daemon — runs Discord Rich Presence in the background. Primary
    /// entry for users who launch the GUI (Pulse) separately, or for CLI users
    /// who want the daemon only without the analytics window.
    SmartForeground,
    /// Child-wrapper mode: spawns `claude <args>` and mirrors its lifetime,
    /// keeping Rich Presence updated for the duration of the CC session.
    ClaudeChild { args: Vec<String> },
}

pub fn run(config: PresenceConfig, mode: AppMode, runtime: RuntimeSettings) -> Result<()> {
    match mode {
        AppMode::SmartForeground => run_daemon(config, runtime),
        AppMode::ClaudeChild { args } => run_claude_wrapper(config, runtime, args),
    }
}

pub fn print_status(config: &PresenceConfig) -> Result<()> {
    let runtime = config::runtime_settings();
    let projects_roots = config::projects_paths();
    let mut cache = GitBranchCache::new(Duration::from_secs(30));
    let mut parse_cache = SessionParseCache::default();
    let ide_workspaces = config::read_ide_workspace_folders();
    let sessions = collect_active_sessions_multi(
        &projects_roots,
        runtime.stale_threshold,
        runtime.active_sticky_window,
        &mut cache,
        &mut parse_cache,
        &ide_workspaces,
    )?;
    let running = process_guard::inspect_running_instance()?;
    let (is_running, running_pid) = match running {
        RunningState::NotRunning => (false, None),
        RunningState::Running { pid } => (true, pid),
    };

    let mut usage_mgr = UsageManager::new();
    let usage = usage_mgr.get_usage();

    println!("cc-discord-presence status");
    println!("running: {is_running}");
    if let Some(pid) = running_pid {
        println!("pid: {pid}");
    }
    println!("config: {}", config::config_path().display());
    print_path_list("projects_dirs", &projects_roots);
    println!(
        "discord_client_id: {}",
        if config.effective_client_id().is_some() {
            "configured"
        } else {
            "missing"
        }
    );
    if let Some(plan_name) = config.plan_display_name() {
        println!("plan: {plan_name}");
    }
    println!("active_sessions: {}", sessions.len());

    let statusline = read_statusline_data(&mut cache);
    if let Some(ref sl) = statusline {
        println!("statusline_source: active (session {})", sl.session_id);
    }

    if let Some(ref usage) = usage {
        println!(
            "api_usage: 5h {:.0}% used | 7d {:.0}% used",
            usage.five_hour.utilization, usage.seven_day.utilization
        );
    }

    if let Some(active) = preferred_active_session(&sessions).or(statusline.as_ref()) {
        let limits_source = latest_limits_source(&sessions);
        if let Some(source) = limits_source {
            println!("limits_source_session: {}", source.session_id);
        }
        print_active_summary(
            active,
            limits_source.map(|source| &source.limits),
            config.privacy.show_activity,
            config.privacy.show_activity_target,
        );
    }
    Ok(())
}

pub fn doctor(config: &PresenceConfig) -> Result<u8> {
    let mut issues = 0u8;
    let projects_roots = config::projects_paths();
    let existing_roots: Vec<&PathBuf> =
        projects_roots.iter().filter(|path| path.exists()).collect();

    println!("cc-discord-presence doctor");
    println!("config_path: {}", config::config_path().display());
    println!(
        "statusline_path: {}",
        config::statusline_data_path().display()
    );
    println!("credentials_path: {}", config::credentials_path().display());
    print_path_list("projects_paths", &projects_roots);

    if existing_roots.is_empty() {
        issues += 1;
        println!("[WARN] No discovered Claude Code sessions directory is currently accessible.");
    } else {
        println!(
            "[OK] Discovered {} accessible projects root(s).",
            existing_roots.len()
        );
    }

    if config.effective_client_id().is_none() {
        issues += 1;
        println!("[WARN] Discord client id not configured.");
    } else {
        println!("[OK] Discord client id configured.");
    }

    if config::statusline_data_path().exists() {
        println!("[OK] Statusline data file exists.");
    } else {
        println!("[INFO] Statusline data file not found (will fall back to JSONL parsing).");
    }

    if config::credentials_path().exists() {
        println!("[OK] Anthropic credentials file exists.");
    } else {
        println!("[INFO] Credentials file not found (API usage tracking unavailable).");
    }

    if command_available("claude") {
        println!("[OK] claude command available.");
    } else if !existing_roots.is_empty() {
        println!(
            "[INFO] claude command not found in PATH (session-file monitoring can still work)."
        );
    } else {
        issues += 1;
        println!("[WARN] claude command not found in PATH.");
    }

    if command_available("git") {
        println!("[OK] git command available.");
    } else {
        issues += 1;
        println!("[WARN] git command not found in PATH.");
    }

    if let Some(plan) = config.plan_display_name() {
        println!("[OK] Plan: {plan}");
    } else {
        println!("[INFO] No plan configured (run setup wizard to set one).");
    }

    if issues == 0 {
        println!("Doctor: healthy");
        Ok(0)
    } else {
        println!("Doctor: {issues} issue(s) found");
        Ok(1)
    }
}

/// Headless daemon loop — polls session state, updates Discord Rich Presence,
/// tracks metrics, and sleeps until the next poll interval. Runs until the
/// stop signal (SIGINT / Ctrl+C) is raised.
///
/// This is the GUI-era replacement for the old `run_foreground_tui` — all
/// visual analytics live in the Pulse GUI now; the CLI only pumps Discord.
fn run_daemon(mut config: PresenceConfig, runtime: RuntimeSettings) -> Result<()> {
    let stop = install_stop_signal()?;
    let mut git_cache = GitBranchCache::new(Duration::from_secs(30));
    let mut parse_cache = SessionParseCache::default();
    let mut discord = DiscordPresence::new(config.effective_client_id());
    let mut usage_mgr = UsageManager::new();
    let mut metrics_tracker = MetricsTracker::new();
    let projects_roots = config::projects_paths();
    let mut last_extra_spent: Option<f64> = None;

    println!("cc-discord-presence daemon — Discord Rich Presence is active.");
    println!("Launch the Pulse GUI (`pulse`) for analytics. Press Ctrl+C to stop.");

    while !stop.load(Ordering::Relaxed) {
        let (_, cached_usage) = tick_session_cycle(
            &projects_roots,
            &runtime,
            &mut config,
            &mut git_cache,
            &mut parse_cache,
            &mut metrics_tracker,
            &mut usage_mgr,
            &mut discord,
        )?;

        if let Some(ref usage) = cached_usage
            && let Some(ref extra) = usage.extra_usage
            && let Some(spent) = extra.used_credits
        {
            if let Some(prev) = last_extra_spent
                && (spent - prev).abs() > 0.001
            {
                crate::sound::play_extra_usage_alert();
                if let Some(token) = usage_mgr.get_access_token() {
                    crate::usage::spawn_extra_usage_toggle_cycle(
                        token,
                        crate::chrome_session::read_claude_session_key(),
                    );
                }
            }
            last_extra_spent = Some(spent);
        }

        thread::sleep(runtime.poll_interval);
    }

    discord.shutdown();
    Ok(())
}

/// Collect sessions, merge statusline, update metrics, fetch usage, and update Discord.
/// Returns `(sessions, cached_usage)` for callers that need them.
#[allow(clippy::too_many_arguments)]
fn tick_session_cycle(
    projects_roots: &[PathBuf],
    runtime: &RuntimeSettings,
    config: &mut PresenceConfig,
    git_cache: &mut GitBranchCache,
    parse_cache: &mut SessionParseCache,
    metrics_tracker: &mut MetricsTracker,
    usage_mgr: &mut UsageManager,
    discord: &mut DiscordPresence,
) -> Result<(Vec<ClaudeSessionSnapshot>, Option<crate::usage::UsageData>)> {
    // Re-read the config every cycle. The Pulse GUI writes presence toggles to
    // this same file, and a long-lived publisher that kept its startup copy
    // would keep broadcasting after the operator paused Rich Presence.
    if let Ok(fresh) = PresenceConfig::load_or_init() {
        *config = fresh;
    }
    let config = &*config;

    let ide_workspaces = config::read_ide_workspace_folders();
    crate::session::prune_session_checkpoints_once();
    let mut sessions = collect_active_sessions_multi(
        projects_roots,
        runtime.stale_threshold,
        runtime.active_sticky_window,
        git_cache,
        parse_cache,
        &ide_workspaces,
    )?;

    if let Some(statusline_session) = read_statusline_data(git_cache) {
        merge_statusline_into_sessions(&mut sessions, statusline_session);
    }

    metrics_tracker.update(&sessions);
    metrics_tracker.persist_if_due();

    let active = preferred_active_session(&sessions);
    let effective_limits = latest_limits_source(&sessions).map(|source| &source.limits);
    let usage = usage_mgr.get_usage();
    if let Err(err) = discord.update(active, effective_limits, usage.as_ref(), config) {
        debug!(error = %err, "discord presence update failed");
    }

    Ok((sessions, usage))
}

fn run_claude_wrapper(
    mut config: PresenceConfig,
    runtime: RuntimeSettings,
    args: Vec<String>,
) -> Result<()> {
    let stop = install_stop_signal()?;
    let mut child = spawn_claude_child(args)?;
    let mut git_cache = GitBranchCache::new(Duration::from_secs(30));
    let mut parse_cache = SessionParseCache::default();
    let mut discord = DiscordPresence::new(config.effective_client_id());
    let mut usage_mgr = UsageManager::new();
    let mut metrics_tracker = MetricsTracker::new();
    let projects_roots = config::projects_paths();

    println!("claude child started; Discord presence tracking is active.");

    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = child.kill();
            break;
        }

        tick_session_cycle(
            &projects_roots,
            &runtime,
            &mut config,
            &mut git_cache,
            &mut parse_cache,
            &mut metrics_tracker,
            &mut usage_mgr,
            &mut discord,
        )?;

        if let Some(status) = child
            .try_wait()
            .context("failed to query claude child status")?
        {
            println!("claude exited with status: {status}");
            break;
        }

        thread::sleep(runtime.poll_interval);
    }

    discord.shutdown();
    Ok(())
}

fn spawn_claude_child(args: Vec<String>) -> Result<Child> {
    let mut command = Command::new("claude");
    command
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    command
        .spawn()
        .context("failed to spawn `claude` child process")
}

fn print_active_summary(
    active: &ClaudeSessionSnapshot,
    effective_limits: Option<&RateLimits>,
    show_activity: bool,
    show_activity_target: bool,
) {
    println!("active_session:");
    println!("  project: {}", active.project_name);
    println!("  path: {}", active.cwd.display());
    println!("  model: {}", active.model.as_deref().unwrap_or("unknown"));
    if let Some(display) = &active.model_display {
        println!("  model_display: {display}");
    }
    println!(
        "  branch: {}",
        active.git_branch.as_deref().unwrap_or("n/a")
    );
    if show_activity && let Some(activity) = &active.activity {
        println!("  activity: {}", activity.to_text(show_activity_target));
    }
    if active.total_cost > 0.0 {
        println!("  cost: ${:.4}", active.total_cost);
    }
    println!(
        "  {}",
        format_token_triplet(
            active.session_delta_tokens,
            active.last_turn_tokens,
            active.session_total_tokens,
        )
    );

    let limits = effective_limits.unwrap_or(&active.limits);
    if let Some(primary) = &limits.primary {
        println!(
            "  5h remaining: {:.0}% (reset {})",
            primary.remaining_percent,
            format_time_until(primary.resets_at)
        );
    }
    if let Some(secondary) = &limits.secondary {
        println!(
            "  7d remaining: {:.0}% (reset {})",
            secondary.remaining_percent,
            format_time_until(secondary.resets_at)
        );
    }
}

fn command_available(program: &str) -> bool {
    // `doctor` is also used by Windows-hosted automation; a version probe
    // must never flash a console while checking an optional provider binary.
    crate::util::silent_command(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn print_path_list(label: &str, paths: &[PathBuf]) {
    println!("{label}:");
    for path in paths {
        println!("  - {}", path.display());
    }
}

fn install_stop_signal() -> Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || {
        flag.store(true, Ordering::Relaxed);
    })
    .context("failed to install Ctrl+C handler")?;
    Ok(stop)
}
