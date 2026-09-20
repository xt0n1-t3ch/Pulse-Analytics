use super::{Config, Session};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

#[derive(Default)]
pub struct Collector {
    cache: HashMap<PathBuf, Cached>,
    desktop: HashMap<String, (String, String)>,
    desktop_checked: Option<Instant>,
}
struct Cached {
    length: u64,
    modified: Option<SystemTime>,
    session: Session,
    dirty: bool,
}
pub struct Batch {
    pub changed: Vec<Session>,
    pub live: Vec<Session>,
    pub diagnostics: Vec<String>,
}

impl Collector {
    pub fn poll(&mut self, config: &Config) -> Batch {
        if self
            .desktop_checked
            .is_none_or(|last| last.elapsed() >= Duration::from_secs(30))
        {
            self.desktop = desktop_index(&super::desktop_home().join("thread-index.db"));
            self.desktop_checked = Some(Instant::now());
        }
        let mut diagnostics = Vec::new();
        let mut present = HashSet::new();
        for profile in config.roots() {
            for entry in walkdir::WalkDir::new(profile.join("projects"))
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default();
                if !name.ends_with(".jsonl")
                    || name.ends_with(".checkpoints.jsonl")
                    || name.starts_with("hooks-audit-")
                {
                    continue;
                }
                present.insert(path.to_owned());
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                let modified = metadata.modified().ok();
                if let Some(cached) = self.cache.get_mut(path)
                    && cached.length == metadata.len()
                    && cached.modified == modified
                {
                    cached.dirty |= apply_desktop_metadata(&mut cached.session, &self.desktop);
                    continue;
                }
                match read_session(path, &profile) {
                    Ok(Some(mut session)) => {
                        session.byok = custom_provider_model(&profile, &session.model);
                        apply_desktop_metadata(&mut session, &self.desktop);
                        self.cache.insert(
                            path.to_owned(),
                            Cached {
                                length: metadata.len(),
                                modified,
                                session,
                                dirty: true,
                            },
                        );
                    }
                    Ok(None) => {}
                    Err(_) => diagnostics.push(
                        "A Command Code transcript could not be read; it will be retried.".into(),
                    ),
                }
            }
        }
        self.cache.retain(|path, _| present.contains(path));
        let mut unique: HashMap<String, Session> = HashMap::new();
        let mut dirty = HashSet::new();
        for cached in self.cache.values() {
            if cached.dirty {
                dirty.insert(cached.session.id.clone());
            }
            let entry = unique
                .entry(cached.session.id.clone())
                .or_insert_with(|| cached.session.clone());
            if cached.session.updated > entry.updated {
                *entry = cached.session.clone();
            }
        }
        let now = chrono::Utc::now().timestamp_millis();
        let mut changed = Vec::new();
        let mut live = Vec::new();
        for (id, session) in unique {
            if session.is_recent(now) {
                live.push(session.clone());
            }
            if dirty.contains(&id) {
                changed.push(session);
            }
        }
        live.sort_by_key(|session| std::cmp::Reverse(session.updated));
        Batch {
            changed,
            live,
            diagnostics,
        }
    }
    pub fn acknowledge(&mut self, batch: &Batch) {
        let ids: HashSet<_> = batch
            .changed
            .iter()
            .map(|session| session.id.as_str())
            .collect();
        for cached in self.cache.values_mut() {
            if ids.contains(cached.session.id.as_str()) {
                cached.dirty = false;
            }
        }
    }
}

fn apply_desktop_metadata(
    session: &mut Session,
    index: &HashMap<String, (String, String)>,
) -> bool {
    let Some((title, branch)) = index.get(&session.id) else {
        return false;
    };
    let title = (!title.is_empty()).then(|| title.clone());
    let branch = (!branch.is_empty()).then(|| branch.clone());
    let changed = !session.desktop || session.title != title || session.branch != branch;
    session.desktop = true;
    session.title = title;
    session.branch = branch;
    changed
}

fn timestamp(value: &Value) -> Option<i64> {
    value
        .as_str()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis())
        .or_else(|| value.as_i64().filter(|value| *value > 0))
}
fn numeric(value: &Value, name: &str) -> u64 {
    value.get(name).and_then(Value::as_u64).unwrap_or(0)
}

fn read_session(path: &Path, profile: &Path) -> std::io::Result<Option<Session>> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut session = Session {
        profile: profile.to_owned(),
        activity: "Idle".into(),
        ..Default::default()
    };
    let mut ids = HashSet::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader
            .by_ref()
            .take(8 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut line)?
            == 0
        {
            break;
        }
        if line.len() > 8 * 1024 * 1024 {
            while !line.ends_with(b"\n") {
                line.clear();
                if reader
                    .by_ref()
                    .take(8 * 1024 * 1024)
                    .read_until(b'\n', &mut line)?
                    == 0
                {
                    break;
                }
            }
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        let observed = timestamp(&value["timestamp"]).unwrap_or(session.updated);
        match value["type"].as_str() {
            Some("session") => {
                let Some(id) = value["id"].as_str() else {
                    continue;
                };
                if !session.id.is_empty() && session.id != id {
                    continue;
                }
                session.id = id.into();
                session.created = observed;
                session.directory = value["cwd"].as_str().map(PathBuf::from).unwrap_or_default();
                session.project = session
                    .directory
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Unknown project")
                    .into();
                session.updated = session.updated.max(observed);
            }
            Some("model_change") => {
                if let Some(model) = value["model"]
                    .as_str()
                    .or_else(|| value["modelId"].as_str())
                {
                    session.model = model.into();
                }
            }
            Some("message") => {
                let Some(id) = value["id"].as_str() else {
                    continue;
                };
                if !ids.insert(id.to_owned()) {
                    continue;
                }
                if let Some(model) = value["model"].as_str() {
                    session.model = model.into();
                }
                if let Some(effort) = value["effort"].as_str() {
                    session.effort = Some(effort.into());
                }
                let message = &value["message"];
                if let Some(usage) = value.get("usage").filter(|value| value.is_object()) {
                    session.usage_messages += 1;
                    session.usage.input = session
                        .usage
                        .input
                        .saturating_add(numeric(usage, "inputTokens"));
                    session.usage.output = session
                        .usage
                        .output
                        .saturating_add(numeric(usage, "outputTokens"));
                    session.usage.cache_read = session
                        .usage
                        .cache_read
                        .saturating_add(numeric(usage, "cacheReadTokens"));
                    session.usage.cache_write = session
                        .usage
                        .cache_write
                        .saturating_add(numeric(usage, "cacheWriteTokens"));
                    if let Some(cost) = usage
                        .get("costUsd")
                        .and_then(Value::as_f64)
                        .filter(|value| value.is_finite() && *value >= 0.0)
                    {
                        let total = session.known_cost.unwrap_or(0.0) + cost;
                        if total.is_finite() {
                            session.known_cost = Some(total);
                            session.priced_messages += 1;
                        }
                    }
                    session.context_used = Some(
                        numeric(usage, "inputTokens")
                            .saturating_add(numeric(usage, "cacheReadTokens"))
                            .saturating_add(numeric(usage, "cacheWriteTokens")),
                    );
                    session.context_window = value
                        .get("contextWindowTokens")
                        .and_then(Value::as_u64)
                        .or_else(|| usage.get("contextWindowTokens").and_then(Value::as_u64))
                        .filter(|value| *value > 0);
                }
                if observed >= session.updated {
                    session.updated = observed;
                    session.activity = activity(message);
                }
            }
            _ => {}
        }
    }
    session.cost_complete =
        session.usage_messages > 0 && session.usage_messages == session.priced_messages;
    Ok((!session.id.is_empty()).then_some(session))
}
fn activity(message: &Value) -> String {
    if message["role"] == "user" {
        if message["content"]
            .as_array()
            .is_some_and(|parts| parts.iter().any(|part| part["type"] == "tool_result"))
        {
            return "Thinking".into();
        }
        return "Thinking".into();
    }
    let parts = message["content"].as_array();
    if let Some(tool) =
        parts.and_then(|parts| parts.iter().rev().find(|part| part["type"] == "tool_use"))
    {
        let name = tool["name"].as_str().unwrap_or("").to_ascii_lowercase();
        return if name.contains("edit") || name.contains("write") || name.contains("patch") {
            "Editing files"
        } else if name.contains("read") || name.contains("search") || name.contains("glob") {
            "Reading files"
        } else {
            "Running command"
        }
        .into();
    }
    if parts.is_some_and(|parts| parts.iter().any(|part| part["type"] == "text")) {
        return "Waiting for input".into();
    }
    if parts.is_some_and(|parts| parts.iter().any(|part| part["type"] == "thinking")) {
        return "Thinking".into();
    }
    "Waiting for input".into()
}
fn desktop_index(path: &Path) -> HashMap<String, (String, String)> {
    let mut result = HashMap::new();
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return result;
    };
    let _ = conn.busy_timeout(Duration::from_millis(100));
    let Ok(mut statement) = conn.prepare("SELECT id,title,git_branch FROM threads") else {
        return result;
    };
    if let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            (
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            ),
        ))
    }) {
        result.extend(rows.filter_map(Result::ok));
    }
    result
}

fn custom_provider_model(profile: &Path, model: &str) -> bool {
    let Ok(bytes) = std::fs::read(profile.join("providers.json")) else {
        return false;
    };
    let Ok(config) = serde_json::from_slice::<Value>(&bytes) else {
        return true;
    };
    let providers = config
        .get("provider")
        .or_else(|| config.get("providers"))
        .unwrap_or(&config);
    providers.as_object().is_some_and(|providers| {
        providers
            .keys()
            .any(|name| model.starts_with(&format!("{name}/")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn transcript() -> String {
        let now = chrono::Utc::now().to_rfc3339();
        format!(
            r#"{{"type":"session","id":"shared-session","cwd":"C:/repo","timestamp":"{now}"}}
{{"type":"message","id":"a","timestamp":"{now}","model":"provider/model","effort":"high","usage":{{"inputTokens":10,"outputTokens":20,"cacheReadTokens":30,"cacheWriteTokens":40,"costUsd":0.5}},"message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Read","input":{{}}}}]}}}}
{{"type":"message","id":"a","timestamp":"{now}","usage":{{"inputTokens":10,"outputTokens":20,"costUsd":0.5}},"message":{{"role":"assistant","content":[]}}}}
{{"type":"message","id":"b","timestamp":"{now}","model":"other/model","usage":{{"inputTokens":7,"outputTokens":3}},"message":{{"role":"assistant","content":[{{"type":"text","text":"Done"}}]}}}}
"#
        )
    }
    #[test]
    fn transcript_deduplicates_messages_preserves_partial_cost_and_completed_history() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("session.jsonl");
        std::fs::write(&file, transcript()).unwrap();
        let session = read_session(&file, dir.path()).unwrap().unwrap();
        assert_eq!(session.usage.total(), 110);
        assert_eq!(session.known_cost, Some(0.5));
        assert!(!session.cost_complete);
        assert_eq!(session.model, "other/model");
        assert!(!session.is_active(chrono::Utc::now().timestamp_millis()));
        assert!(session.context_window.is_none());
    }
    #[test]
    fn collector_ignores_checkpoints_and_overlapping_roots_do_not_double_count() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        std::fs::create_dir(&projects).unwrap();
        std::fs::write(projects.join("session.jsonl"), transcript()).unwrap();
        std::fs::write(projects.join("session.checkpoints.jsonl"), transcript()).unwrap();
        let config = Config {
            profile_paths: vec![dir.path().into(), dir.path().into()],
            ..Default::default()
        };
        let mut collector = Collector::default();
        let batch = collector.poll(&config);
        assert_eq!(batch.changed.len(), 1);
        collector.acknowledge(&batch);
        assert!(collector.poll(&config).changed.is_empty());
    }
    #[test]
    fn desktop_metadata_changes_refresh_without_recounting_usage() {
        let mut session = Session {
            id: "desktop".into(),
            usage: super::super::Usage {
                input: 42,
                ..Default::default()
            },
            ..Default::default()
        };
        let index = HashMap::from([(
            "desktop".into(),
            ("Renamed session".into(), "feature".into()),
        )]);
        assert!(apply_desktop_metadata(&mut session, &index));
        assert_eq!(session.title.as_deref(), Some("Renamed session"));
        assert!(session.desktop);
        assert_eq!(session.usage.input, 42);
        assert!(!apply_desktop_metadata(&mut session, &index));
    }
    #[test]
    fn completed_text_with_thinking_is_not_live_work() {
        let message =
            serde_json::json!({"role":"assistant","content":[{"type":"thinking"},{"type":"text"}]});
        assert_eq!(activity(&message), "Waiting for input");
    }
}
