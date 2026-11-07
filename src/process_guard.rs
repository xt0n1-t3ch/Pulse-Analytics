//! Single-instance guard for the Claude daemon: lock file, owner metadata and takeover of stray
//! pollers.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use crate::config;
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

const TAKEOVER_SOFT_TIMEOUT: Duration = Duration::from_secs(4);
const TAKEOVER_HARD_TIMEOUT: Duration = Duration::from_secs(3);
const TAKEOVER_RETRY_INTERVAL: Duration = Duration::from_millis(120);

pub enum AcquireState {
    Acquired(InstanceGuard),
    AlreadyRunning { pid: Option<u32> },
}

pub struct AcquireWithTakeover {
    pub guard: InstanceGuard,
    pub takeover_pid: Option<u32>,
}

pub enum RunningState {
    NotRunning,
    Running { pid: Option<u32> },
}

pub struct InstanceGuard {
    file: File,
    meta_path: PathBuf,
    pid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstanceMetadata {
    pid: u32,
    exe_path: Option<String>,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        remove_instance_metadata_if_owned(self.pid, &self.meta_path);
    }
}

pub fn acquire_or_takeover_single_instance() -> Result<AcquireWithTakeover> {
    match acquire_single_instance()? {
        AcquireState::Acquired(guard) => Ok(AcquireWithTakeover {
            guard,
            takeover_pid: None,
        }),
        AcquireState::AlreadyRunning { pid } => {
            let Some(existing_pid) = pid else {
                bail!(
                    "cc-discord-presence is already running, but PID metadata is unavailable for takeover"
                );
            };
            if existing_pid == std::process::id() {
                bail!(
                    "cc-discord-presence lock appears to be owned by current process; close the existing process and retry"
                );
            }

            terminate_process(existing_pid, false)?;
            if let Some(guard) = wait_for_lock(TAKEOVER_SOFT_TIMEOUT)? {
                return Ok(AcquireWithTakeover {
                    guard,
                    takeover_pid: Some(existing_pid),
                });
            }

            terminate_process(existing_pid, true)?;
            if let Some(guard) = wait_for_lock(TAKEOVER_HARD_TIMEOUT)? {
                return Ok(AcquireWithTakeover {
                    guard,
                    takeover_pid: Some(existing_pid),
                });
            }

            bail!(
                "failed to acquire single-instance lock after takeover attempt (PID {existing_pid})"
            );
        }
    }
}

pub fn acquire_single_instance() -> Result<AcquireState> {
    let lock_path = config::lock_path();
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create lock directory {}", parent.display()))?;
    }

    let meta_path = config::instance_meta_path();
    let mut file = open_lock_file(&lock_path)?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            write_pid_compat(&mut file)?;
            let metadata = InstanceMetadata {
                pid: std::process::id(),
                exe_path: env::current_exe()
                    .ok()
                    .map(|path| path.display().to_string()),
            };
            write_instance_metadata(&meta_path, &metadata)?;
            Ok(AcquireState::Acquired(InstanceGuard {
                file,
                meta_path,
                pid: metadata.pid,
            }))
        }
        Err(_) => {
            let pid = read_instance_metadata(&meta_path)
                .ok()
                .flatten()
                .map(|m| m.pid);
            Ok(AcquireState::AlreadyRunning { pid })
        }
    }
}

pub fn inspect_running_instance() -> Result<RunningState> {
    let lock_path = config::lock_path();
    if !lock_path.exists() {
        return Ok(RunningState::NotRunning);
    }

    let meta_path = config::instance_meta_path();
    let file = open_lock_file(&lock_path)?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            let _ = file.unlock();
            let _ = fs::remove_file(&meta_path);
            Ok(RunningState::NotRunning)
        }
        Err(_) => {
            let pid = read_instance_metadata(&meta_path)
                .ok()
                .flatten()
                .map(|m| m.pid);
            Ok(RunningState::Running { pid })
        }
    }
}

/// Terminate stray background analytics pollers that would double-poll
/// alongside a running Pulse GUI: the standalone `cc-discord-presence` CLI and
/// the debug-only `pulse-dev-bridge`. Pulse is the single source of truth for
/// the analytics database and Discord presence, so a second producer writing
/// the same store is the primary cause of "duplicate background instances".
///
/// Best-effort and by exact executable name; the Pulse GUI (`Pulse.exe`) is
/// never a target, so this cannot terminate the caller. Failures are ignored.
#[cfg(windows)]
pub fn reap_stray_pollers() {
    for process in crate::process_list::snapshot() {
        if ["cc-discord-presence.exe", "pulse-dev-bridge.exe"]
            .iter()
            .any(|name| process.name.eq_ignore_ascii_case(name))
        {
            crate::process_list::terminate(process.pid);
        }
    }
}

/// No-op off Windows: matching pollers by name there risks hitting the
/// caller's own command line, because the repository itself is named
/// `cc-discord-presence`.
#[cfg(not(windows))]
pub fn reap_stray_pollers() {}

fn wait_for_lock(timeout: Duration) -> Result<Option<InstanceGuard>> {
    let deadline = Instant::now() + timeout;
    loop {
        match acquire_single_instance()? {
            AcquireState::Acquired(guard) => return Ok(Some(guard)),
            AcquireState::AlreadyRunning { .. } => {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                thread::sleep(TAKEOVER_RETRY_INTERVAL);
            }
        }
    }
}

fn terminate_process(pid: u32, force: bool) -> Result<()> {
    if !process_exists(pid) {
        return Ok(());
    }

    // Windows has no polite stop for a windowless process; both modes end it.
    #[cfg(windows)]
    {
        let _ = force;
        if !crate::process_list::terminate(pid) {
            bail!("failed to terminate running instance PID {pid}");
        }
    }

    #[cfg(unix)]
    {
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        // `kill` gives 0 and negative ids a process-group meaning; the lock
        // metadata must never be able to address one.
        let target = libc::pid_t::try_from(pid)
            .ok()
            .filter(|target| *target > 0)
            .with_context(|| format!("PID {pid} is not a valid process id"))?;
        // SAFETY: `kill` has no memory-safety preconditions and `target` is a
        // single positive process id.
        let rc = unsafe { libc::kill(target, signal) };
        let error = std::io::Error::last_os_error();
        if rc != 0 && process_exists(pid) {
            bail!("failed to terminate running instance PID {pid} with signal {signal}: {error}");
        }
    }

    Ok(())
}

fn process_exists(pid: u32) -> bool {
    crate::process_list::is_alive(pid)
}

fn open_lock_file(path: &PathBuf) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("failed to open lock file {}", path.display()))
}

fn write_pid_compat(file: &mut File) -> Result<()> {
    let pid = std::process::id();
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    file.write_all(pid.to_string().as_bytes())?;
    file.flush()?;
    Ok(())
}

fn write_instance_metadata(path: &PathBuf, metadata: &InstanceMetadata) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create instance metadata directory {}",
                parent.display()
            )
        })?;
    }
    let payload = serde_json::to_string_pretty(metadata)?;
    fs::write(path, payload)
        .with_context(|| format!("failed to write instance metadata {}", path.display()))?;
    Ok(())
}

fn read_instance_metadata(path: &PathBuf) -> Result<Option<InstanceMetadata>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read instance metadata {}", path.display()))?;
    let parsed: InstanceMetadata = serde_json::from_str(&raw)
        .with_context(|| format!("invalid JSON in {}", path.display()))?;
    Ok(Some(parsed))
}

fn remove_instance_metadata_if_owned(expected_pid: u32, path: &PathBuf) {
    let Ok(Some(metadata)) = read_instance_metadata(path) else {
        return;
    };
    if metadata.pid == expected_pid {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Shared with every other module that redirects `CLAUDE_HOME`; see
    /// `config::home_env_lock`.
    fn env_lock() -> &'static Mutex<()> {
        crate::config::home_env_lock()
    }

    #[test]
    fn roundtrip_instance_metadata() {
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join("instance.json");

        let original = InstanceMetadata {
            pid: 1234,
            exe_path: Some("cc-discord-presence".to_string()),
        };

        write_instance_metadata(&path, &original).expect("write metadata");
        let loaded = read_instance_metadata(&path)
            .expect("read metadata")
            .expect("metadata value");

        assert_eq!(loaded.pid, 1234);
        assert_eq!(loaded.exe_path.as_deref(), Some("cc-discord-presence"));
    }

    #[cfg(unix)]
    #[test]
    fn terminate_process_sends_sigterm_then_sigkill() {
        use std::os::unix::process::ExitStatusExt;
        for (force, expected) in [(false, libc::SIGTERM), (true, libc::SIGKILL)] {
            let mut child = std::process::Command::new("sleep")
                .arg("60")
                .spawn()
                .expect("spawn sleep");
            terminate_process(child.id(), force).expect("terminate");
            let status = child.wait().expect("reap child");
            assert_eq!(status.signal(), Some(expected));
        }
    }

    #[cfg(unix)]
    #[test]
    fn terminate_process_ignores_an_id_that_is_already_gone() {
        let mut child = std::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn short-lived child");
        let pid = child.id();
        child.wait().expect("reap child");
        terminate_process(pid, false).expect("a dead pid is not an error");
    }

    #[test]
    fn inspect_reports_running_while_lock_is_held() {
        let _mutex = env_lock().lock().expect("env lock");
        let tmp = TempDir::new().expect("temp dir");
        unsafe {
            env::set_var("PULSE_HOME", tmp.path());
        }

        let state = acquire_single_instance().expect("acquire instance");
        let guard = match state {
            AcquireState::Acquired(guard) => guard,
            AcquireState::AlreadyRunning { .. } => panic!("expected acquired lock"),
        };

        match inspect_running_instance().expect("inspect running") {
            RunningState::Running { .. } => {}
            RunningState::NotRunning => panic!("expected running state while lock is held"),
        }

        drop(guard);

        match inspect_running_instance().expect("inspect stopped") {
            RunningState::NotRunning => {}
            RunningState::Running { .. } => panic!("expected not running after lock release"),
        }

        unsafe {
            env::remove_var("PULSE_HOME");
        }
    }
}
