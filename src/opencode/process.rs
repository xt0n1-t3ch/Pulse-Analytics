//! Detection of running OpenCode and OpenChamber processes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub parent_pid: u32,
    pub executable: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagedProcess {
    pid: u32,
    owner_pid: u32,
    binary: String,
    runtime: String,
}

/// OpenCode and OpenChamber processes with their executable paths, read in-process.
pub fn running_processes() -> Vec<ProcessIdentity> {
    #[cfg(windows)]
    {
        crate::process_list::snapshot()
            .into_iter()
            .filter(|process| {
                ["opencode.exe", "opencode-cli.exe", "OpenChamber.exe"]
                    .iter()
                    .any(|name| process.name.eq_ignore_ascii_case(name))
            })
            .filter_map(|process| {
                Some(ProcessIdentity {
                    pid: process.pid,
                    parent_pid: process.parent_pid,
                    executable: crate::process_list::executable_path(process.pid)?,
                })
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

pub fn managed_openchamber_is_live(root: &Path, processes: &[ProcessIdentity]) -> bool {
    let Ok(entries) = std::fs::read_dir(root.join("managed-opencode")) else {
        return false;
    };
    entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<ManagedProcess>(&bytes).ok())
        .any(|record| valid_record(&record, processes))
}

fn valid_record(record: &ManagedProcess, processes: &[ProcessIdentity]) -> bool {
    record.runtime == "desktop"
        && processes.iter().any(|process| {
            process.pid == record.pid
                && process.parent_pid == record.owner_pid
                && process.executable.eq_ignore_ascii_case(&record.binary)
        })
        && processes.iter().any(|process| {
            process.pid == record.owner_pid
                && PathBuf::from(&process.executable)
                    .file_name()
                    .is_some_and(|name| name.eq_ignore_ascii_case("OpenChamber.exe"))
        })
}

#[derive(Default)]
pub struct RuntimeDetector {
    checked_at: Option<Instant>,
    openchamber_sessions: HashSet<String>,
}

impl RuntimeDetector {
    pub fn enrich(&mut self, session: &mut super::Session) {
        if self
            .checked_at
            .is_none_or(|last| last.elapsed() >= Duration::from_secs(30))
        {
            self.checked_at = Some(Instant::now());
            self.openchamber_sessions.clear();
            let root = dirs::home_dir()
                .unwrap_or_default()
                .join(".config/openchamber");
            let processes = running_processes();
            let backends = processes
                .iter()
                .filter(|process| {
                    Path::new(&process.executable)
                        .file_name()
                        .is_some_and(|name| {
                            name.eq_ignore_ascii_case("opencode.exe")
                                || name.eq_ignore_ascii_case("opencode-cli.exe")
                        })
                })
                .count();
            if backends == 1
                && managed_openchamber_is_live(&root, &processes)
                && let Ok(bytes) = std::fs::read(root.join("sessions-directories.json"))
                && bytes.len() <= 4 * 1024 * 1024
                && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
                && let Some(folders) = value
                    .get("foldersMap")
                    .and_then(serde_json::Value::as_object)
            {
                for groups in folders.values().filter_map(serde_json::Value::as_array) {
                    for group in groups {
                        if let Some(ids) = group
                            .get("sessionIds")
                            .and_then(serde_json::Value::as_array)
                        {
                            self.openchamber_sessions.extend(
                                ids.iter()
                                    .filter_map(serde_json::Value::as_str)
                                    .map(str::to_string),
                            );
                        }
                    }
                }
            }
        }
        session.metadata.surface = if self.openchamber_sessions.contains(&session.id) {
            "OpenChamber"
        } else {
            "OpenCode"
        }
        .into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_or_reused_managed_pid_does_not_prove_openchamber() {
        let record = ManagedProcess {
            pid: 1,
            owner_pid: 2,
            binary: Path::new("app")
                .join("opencode.exe")
                .to_string_lossy()
                .into_owned(),
            runtime: "desktop".into(),
        };
        assert!(!valid_record(&record, &[]));
        let processes = vec![
            ProcessIdentity {
                pid: 1,
                parent_pid: 2,
                executable: record.binary.clone(),
            },
            ProcessIdentity {
                pid: 2,
                parent_pid: 0,
                executable: Path::new("app")
                    .join("OpenChamber.exe")
                    .to_string_lossy()
                    .into_owned(),
            },
        ];
        assert!(valid_record(&record, &processes));
        let wrong = ManagedProcess {
            binary: Path::new("different")
                .join("opencode.exe")
                .to_string_lossy()
                .into_owned(),
            ..record
        };
        assert!(!valid_record(&wrong, &processes));
    }
}
