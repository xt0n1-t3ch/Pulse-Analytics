//! Detects background workflows of a Claude session from its run directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const WORKFLOW_RUN_PREFIX: &str = "wf_";
const WORKFLOWS_DIR: &str = "workflows";
const SUBAGENT_WORKFLOWS_DIR: &[&str] = &["subagents", "workflows"];
const WORKFLOW_JOURNAL_FILE: &str = "journal.jsonl";
const SUBAGENT_FILE_PREFIX: &str = "agent-";
const DEFAULT_INFLIGHT_STALE_SECS: u64 = 180;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct BackgroundWorkInfo {
    pub workflow_active: bool,
    pub active_agent_count: usize,
    pub latest_signal_at: Option<SystemTime>,
}

impl BackgroundWorkInfo {
    pub fn is_active(self) -> bool {
        self.workflow_active || self.active_agent_count > 0
    }

    fn merge(&mut self, other: BackgroundWorkInfo) {
        self.workflow_active |= other.workflow_active;
        self.active_agent_count = self
            .active_agent_count
            .saturating_add(other.active_agent_count);
        self.latest_signal_at = max_system_time(self.latest_signal_at, other.latest_signal_at);
    }
}

pub fn detect_background_work(transcript_path: &Path) -> BackgroundWorkInfo {
    let Some(base) = session_run_dir(transcript_path) else {
        return BackgroundWorkInfo::default();
    };

    let stale_after = Duration::from_secs(DEFAULT_INFLIGHT_STALE_SECS);
    let mut info = detect_workflow_records(&base.join(WORKFLOWS_DIR), stale_after);
    let subagent_dir = SUBAGENT_WORKFLOWS_DIR
        .iter()
        .fold(base.clone(), |path, segment| path.join(segment));
    info.merge(detect_live_journals(&subagent_dir, stale_after));
    info
}

fn session_run_dir(transcript_path: &Path) -> Option<PathBuf> {
    if transcript_path.extension().and_then(|item| item.to_str()) != Some("jsonl") {
        return None;
    }
    Some(transcript_path.with_extension(""))
}

fn detect_workflow_records(workflows_dir: &Path, stale_after: Duration) -> BackgroundWorkInfo {
    let mut info = BackgroundWorkInfo::default();
    let Ok(entries) = fs::read_dir(workflows_dir) else {
        return info;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|item| item.to_str()) else {
            continue;
        };
        if !name.starts_with(WORKFLOW_RUN_PREFIX) || !name.ends_with(".json") {
            continue;
        }
        let Some(modified) = entry.metadata().ok().and_then(|meta| meta.modified().ok()) else {
            continue;
        };
        if SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default()
            >= stale_after
        {
            continue;
        }
        let Ok(data) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        let active_status = record
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(is_active_workflow_status);
        let active_agents = record
            .get("workflowProgress")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|item| {
                        item.get("state")
                            .and_then(Value::as_str)
                            .is_some_and(is_active_agent_state)
                    })
                    .count()
            })
            .unwrap_or(0);

        if active_status || active_agents > 0 {
            info.workflow_active = true;
            info.active_agent_count = info.active_agent_count.saturating_add(active_agents);
            info.latest_signal_at = max_system_time(info.latest_signal_at, Some(modified));
        }
    }

    info
}

fn detect_live_journals(subagents_dir: &Path, stale_after: Duration) -> BackgroundWorkInfo {
    let mut info = BackgroundWorkInfo::default();
    let Ok(entries) = fs::read_dir(subagents_dir) else {
        return info;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|item| item.to_str()) else {
            continue;
        };
        if !name.starts_with(WORKFLOW_RUN_PREFIX) {
            continue;
        }
        let Some((active_agents, modified)) = live_workflow_agents(&path, stale_after) else {
            continue;
        };
        if active_agents > 0 {
            info.workflow_active = true;
            info.active_agent_count = info.active_agent_count.saturating_add(active_agents);
            info.latest_signal_at = max_system_time(info.latest_signal_at, Some(modified));
        }
    }

    info
}

fn live_workflow_agents(workflow_dir: &Path, stale_after: Duration) -> Option<(usize, SystemTime)> {
    let journal = live_journal_agents(&workflow_dir.join(WORKFLOW_JOURNAL_FILE), stale_after);
    let agent_files = live_agent_files(workflow_dir, stale_after);

    match (journal, agent_files) {
        (Some((journal_count, journal_modified)), Some((file_count, file_modified))) => Some((
            journal_count.max(file_count),
            journal_modified.max(file_modified),
        )),
        (Some(journal), None) => Some(journal),
        (None, Some(agent_files)) => Some(agent_files),
        (None, None) => None,
    }
}

fn live_journal_agents(journal_path: &Path, stale_after: Duration) -> Option<(usize, SystemTime)> {
    let metadata = fs::metadata(journal_path).ok()?;
    let modified = metadata.modified().ok()?;
    if SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default()
        >= stale_after
    {
        return None;
    }

    let mut started = 0usize;
    let mut finished = 0usize;
    let data = fs::read_to_string(journal_path).ok()?;
    for line in data.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("started") => started = started.saturating_add(1),
            Some("result") => finished = finished.saturating_add(1),
            _ => {}
        }
    }

    Some((started.saturating_sub(finished), modified))
}

fn live_agent_files(workflow_dir: &Path, stale_after: Duration) -> Option<(usize, SystemTime)> {
    let entries = fs::read_dir(workflow_dir).ok()?;
    let mut active_agents = 0usize;
    let mut latest_signal_at = None;

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|item| item.to_str()) else {
            continue;
        };
        if !name.starts_with(SUBAGENT_FILE_PREFIX)
            || path.extension().and_then(|item| item.to_str()) != Some("jsonl")
        {
            continue;
        }
        let Some(modified) = entry.metadata().ok().and_then(|meta| meta.modified().ok()) else {
            continue;
        };
        if SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default()
            >= stale_after
        {
            continue;
        }
        active_agents = active_agents.saturating_add(1);
        latest_signal_at = max_system_time(latest_signal_at, Some(modified));
    }

    (active_agents > 0).then_some((active_agents, latest_signal_at?))
}

fn is_active_workflow_status(status: &str) -> bool {
    let normalized = status.to_ascii_lowercase();
    !normalized.is_empty()
        && !matches!(
            normalized.as_str(),
            "completed" | "failed" | "error" | "cancelled" | "canceled" | "aborted"
        )
}

fn is_active_agent_state(state: &str) -> bool {
    let normalized = state.to_ascii_lowercase();
    !normalized.is_empty()
        && !matches!(
            normalized.as_str(),
            "done" | "failed" | "error" | "cancelled" | "canceled" | "skipped"
        )
}

fn max_system_time(left: Option<SystemTime>, right: Option<SystemTime>) -> Option<SystemTime> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_session() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().expect("temp dir");
        let transcript = temp.path().join("session.jsonl");
        fs::write(&transcript, "{}\n").expect("transcript");
        let workflows = temp.path().join("session").join("workflows");
        let subagents = temp
            .path()
            .join("session")
            .join("subagents")
            .join("workflows");
        fs::create_dir_all(&workflows).expect("workflows");
        fs::create_dir_all(&subagents).expect("subagents");
        (temp, transcript, workflows, subagents)
    }

    #[test]
    fn detects_active_workflow_record_and_agent_count() {
        let (_temp, transcript, workflows, _subagents) = new_session();
        fs::write(
            workflows.join("wf_active.json"),
            r#"{"status":"running","workflowProgress":[{"state":"done"},{"state":"retrying"}]}"#,
        )
        .expect("workflow");

        let info = detect_background_work(&transcript);

        assert!(info.workflow_active);
        assert_eq!(info.active_agent_count, 1);
        assert!(info.latest_signal_at.is_some());
    }

    #[test]
    fn ignores_terminal_workflow_record() {
        let (_temp, transcript, workflows, _subagents) = new_session();
        fs::write(
            workflows.join("wf_done.json"),
            r#"{"status":"completed","workflowProgress":[{"state":"done"}]}"#,
        )
        .expect("workflow");

        let info = detect_background_work(&transcript);

        assert!(!info.is_active());
    }

    #[test]
    fn ignores_stale_active_workflow_record() {
        let (_temp, _transcript, workflows, _subagents) = new_session();
        fs::write(
            workflows.join("wf_stale.json"),
            r#"{"status":"running","workflowProgress":[{"state":"retrying"}]}"#,
        )
        .expect("workflow");

        let info = detect_workflow_records(&workflows, Duration::ZERO);

        assert!(!info.is_active());
    }

    #[test]
    fn detects_live_subagent_journal_count() {
        let (_temp, transcript, _workflows, subagents) = new_session();
        let run_dir = subagents.join("wf_live");
        fs::create_dir_all(&run_dir).expect("run dir");
        fs::write(
            run_dir.join("journal.jsonl"),
            "{\"type\":\"started\"}\n{\"type\":\"started\"}\n{\"type\":\"result\"}\n",
        )
        .expect("journal");

        let info = detect_background_work(&transcript);

        assert!(info.workflow_active);
        assert_eq!(info.active_agent_count, 1);
    }

    #[test]
    fn detects_live_subagent_agent_files_when_journal_is_stale_or_missing() {
        let (_temp, transcript, _workflows, subagents) = new_session();
        let run_dir = subagents.join("wf_live_files");
        fs::create_dir_all(&run_dir).expect("run dir");
        fs::write(run_dir.join("agent-a.jsonl"), "{}\n").expect("agent a");
        fs::write(run_dir.join("agent-b.jsonl"), "{}\n").expect("agent b");

        let info = detect_background_work(&transcript);

        assert!(info.workflow_active);
        assert_eq!(info.active_agent_count, 2);
        assert!(info.latest_signal_at.is_some());
    }
}
