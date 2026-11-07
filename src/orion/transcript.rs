//! Streams Orion JSONL transcripts into the few facts Pulse needs.
//!
//! A transcript is append-only (`docs/specs/session-storage.md` in the Orion
//! repository): one record per line, and for each `kind` + `id` the last `put`
//! wins. A streamed assistant message is written many times and its first
//! write carries 0 tokens, so only the final line of each id counts. Large
//! lines are never held: records are filtered by substring first and only the
//! few that matter are parsed, into typed structs that skip everything else.
//!
//! The cache keeps one parsed state per session and, for a plain file, parses
//! only the bytes appended since the previous poll. A gzip file is cold (Orion
//! compresses a session after 7 days without writes) and is parsed whole when it
//! changes. The state is reset when a file shrinks, because Orion truncates a
//! torn tail on recovery.

use super::records::{Count, EntryData, MessageData, PartData, Record};
use super::{Estimate, Usage, estimate_cost};
use flate2::read::MultiGzDecoder;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Orion spills payloads over 64 KiB, so the longest line seen is about 65 KiB.
/// A line past this cap is skipped instead of buffered.
const MAX_LINE_BYTES: u64 = 1024 * 1024;
/// Tool parts that stay "running" after a crash are never closed; keep few.
const MAX_RUNNING_TOOLS: usize = 64;

const KIND_MESSAGE: &str = "\"kind\":\"message\"";
const KIND_ENTRY: &str = "\"kind\":\"entry\"";
const KIND_PART: &str = "\"kind\":\"part\"";
const ASSISTANT: &str = "\"role\":\"assistant\"";
const OP_REMOVE: &str = "\"op\":\"remove\"";
const TOOL_PART: &str = "\"type\":\"tool\"";
const MODEL_SELECTION_ENTRY: &str = "\"type\":\"runtime/model_selection\"";
const CONTEXT_USAGE_ENTRY: &str = "\"type\":\"runtime/context_usage\"";

/// Final state of one assistant message, which is one model request.
#[derive(Clone, Debug)]
pub(super) struct Message {
    pub model: String,
    pub usage: Usage,
    /// Priced once when the line is read, so a poll never re-prices history.
    pub estimate: Option<Estimate>,
    pub created: i64,
    pub completed: bool,
    pub ended_on_tool_calls: bool,
    /// `tokens.total`, else input plus output: the prompt size of the request.
    pub fill: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Selection {
    pub provider: String,
    pub model: String,
    pub effort: Option<String>,
}

/// Orion's own `runtime/context_usage` entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ContextUsage {
    pub used: Option<u64>,
    pub max: Option<u64>,
    pub auto_compact_threshold: Option<u64>,
    pub cache_hit_rate: Option<f64>,
}

#[derive(Clone, Debug)]
pub(super) struct ToolMark {
    pub tool: String,
    /// Raw `file_path`; the caller sanitizes it before display.
    pub target: Option<String>,
    pub at: i64,
}

#[derive(Debug, Default)]
pub(super) struct Transcript {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
    /// Bytes of complete lines already applied (plain files only).
    offset: u64,
    pub messages: HashMap<String, Message>,
    pub selection: Option<Selection>,
    pub context: Option<ContextUsage>,
    pub running_tools: HashMap<String, ToolMark>,
    /// Provider of the newest assistant message, for sessions with no stored selection.
    pub provider: String,
    newest: i64,
}

impl Transcript {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            ..Self::default()
        }
    }

    fn reset(&mut self) {
        *self = Self::new(&self.path.clone());
    }

    /// The newest assistant message by creation time, id breaking ties.
    pub fn last_message(&self) -> Option<&Message> {
        self.messages
            .iter()
            .max_by(|a, b| (a.1.created, a.0).cmp(&(b.1.created, b.0)))
            .map(|(_, message)| message)
    }

    fn apply(&mut self, line: &[u8]) {
        let Ok(text) = std::str::from_utf8(line) else {
            return;
        };
        if text.contains(KIND_MESSAGE) {
            if text.contains(ASSISTANT) || text.contains(OP_REMOVE) {
                self.apply_message(text);
            }
        } else if text.contains(KIND_ENTRY) {
            if text.contains(MODEL_SELECTION_ENTRY) {
                self.apply_selection(text);
            } else if text.contains(CONTEXT_USAGE_ENTRY) {
                self.apply_context(text);
            }
        } else if text.contains(KIND_PART) && text.contains(TOOL_PART) {
            self.apply_tool(text);
        }
    }

    fn apply_message(&mut self, text: &str) {
        let Ok(record) = serde_json::from_str::<Record<MessageData>>(text) else {
            return;
        };
        if record.kind != "message" {
            return;
        }
        if record.op == "remove" {
            self.messages.remove(&record.id);
            return;
        }
        let Some(data) = record
            .data
            .filter(|data| data.role.as_deref() == Some("assistant"))
        else {
            return;
        };
        let tokens = data.tokens.unwrap_or_default();
        let cache = tokens.cache.unwrap_or_default();
        let count = |value: Option<Count>| value.map_or(0, |count| count.0);
        let usage = Usage {
            input: count(tokens.input),
            output: count(tokens.output),
            reasoning: count(tokens.reasoning),
            cache_read: count(cache.read),
            cache_write: count(cache.write),
        };
        let model = data.model_id.unwrap_or_default();
        let time = data.time.unwrap_or_default();
        let created = time.created.unwrap_or(record.at);
        if created >= self.newest {
            self.newest = created;
            self.provider = data.provider_id.unwrap_or_default();
        }
        self.messages.insert(
            record.id,
            Message {
                estimate: (usage.total() > 0)
                    .then(|| estimate_cost(&model, &usage))
                    .flatten(),
                fill: tokens.total.map_or_else(|| usage.total(), |total| total.0),
                model,
                usage,
                created,
                completed: time.completed.is_some(),
                ended_on_tool_calls: data.finish.as_deref() == Some("tool-calls"),
            },
        );
    }

    fn apply_selection(&mut self, text: &str) {
        let Ok(record) = serde_json::from_str::<Record<EntryData>>(text) else {
            return;
        };
        let selection = record
            .data
            .filter(|_| record.kind == "entry" && record.op != "remove")
            .and_then(|data| data.model_selection);
        if let Some(selection) = selection {
            self.selection = Some(Selection {
                provider: selection.provider_id.unwrap_or_default(),
                model: selection.model_id.unwrap_or_default(),
                effort: selection
                    .options
                    .and_then(|options| options.reasoning_level),
            });
        }
    }

    fn apply_context(&mut self, text: &str) {
        let Ok(record) = serde_json::from_str::<Record<EntryData>>(text) else {
            return;
        };
        let window = record
            .data
            .filter(|_| record.kind == "entry" && record.op != "remove")
            .and_then(|data| data.window);
        if let Some(window) = window {
            let positive = |value: Option<Count>| value.map(|count| count.0).filter(|n| *n > 0);
            self.context = Some(ContextUsage {
                used: window.used_tokens.map(|count| count.0),
                max: positive(window.max_tokens),
                auto_compact_threshold: positive(window.auto_compact_threshold_tokens),
                cache_hit_rate: window
                    .cache
                    .and_then(|cache| cache.hit_rate)
                    .filter(|rate| rate.is_finite() && (0.0..=1.0).contains(rate)),
            });
        }
    }

    fn apply_tool(&mut self, text: &str) {
        let Ok(record) = serde_json::from_str::<Record<PartData>>(text) else {
            return;
        };
        let Some(part) = record
            .data
            .filter(|part| record.kind == "part" && part.kind.as_deref() == Some("tool"))
        else {
            return;
        };
        let state = part.state.unwrap_or_default();
        if record.op == "remove" || state.status.as_deref() != Some("running") {
            self.running_tools.remove(&record.id);
            return;
        }
        let input = state.input.unwrap_or_default();
        self.running_tools.insert(
            record.id,
            ToolMark {
                tool: part.tool.unwrap_or_default(),
                target: input.file_path.or(input.file_path_camel),
                at: record.at,
            },
        );
        if self.running_tools.len() > MAX_RUNNING_TOOLS
            && let Some(oldest) = self
                .running_tools
                .iter()
                .min_by_key(|(_, mark)| mark.at)
                .map(|(id, _)| id.clone())
        {
            self.running_tools.remove(&oldest);
        }
    }
}

/// Parsed transcripts by session id.
#[derive(Debug, Default)]
pub(super) struct TranscriptCache {
    entries: HashMap<String, Transcript>,
}

impl TranscriptCache {
    pub fn get(&self, session_id: &str) -> Option<&Transcript> {
        self.entries.get(session_id)
    }

    /// Drops every session the caller no longer follows, so a long history does
    /// not stay in memory after it has been summarized.
    pub fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.entries.retain(|id, _| keep(id));
    }

    /// Brings one session up to date with the first existing file among
    /// `candidates`. A session with no file yet has no entry.
    pub fn refresh(&mut self, session_id: &str, candidates: &[PathBuf]) -> io::Result<()> {
        let known = self
            .entries
            .get(session_id)
            .map(|transcript| transcript.path.clone())
            .filter(|path| path.is_file());
        let Some(path) = known.or_else(|| candidates.iter().find(|path| path.is_file()).cloned())
        else {
            self.entries.remove(session_id);
            return Ok(());
        };
        let metadata = std::fs::metadata(&path)?;
        let (len, modified) = (metadata.len(), metadata.modified().ok());
        let entry = self
            .entries
            .entry(session_id.to_owned())
            .or_insert_with(|| Transcript::new(&path));
        if entry.path != path {
            *entry = Transcript::new(&path);
        }
        if entry.len == len && entry.modified == modified && entry.modified.is_some() {
            return Ok(());
        }
        entry.len = len;
        entry.modified = modified;
        let gzip = path.extension().is_some_and(|extension| extension == "gz");
        if gzip || len < entry.offset {
            entry.reset();
            entry.len = len;
            entry.modified = modified;
        }
        let mut file = File::open(&path)?;
        if gzip {
            let mut reader = BufReader::new(MultiGzDecoder::new(file));
            read_lines(&mut reader, |line| entry.apply(line))?;
        } else {
            file.seek(SeekFrom::Start(entry.offset))?;
            let mut reader = BufReader::new(file);
            let used = read_lines(&mut reader, |line| entry.apply(line))?;
            entry.offset += used;
        }
        Ok(())
    }
}

/// Calls `on_line` for each newline-terminated line (without the newline) and
/// returns the bytes those lines used. A last line with no newline is still
/// being written and is left for the next read; a line over the cap is skipped.
fn read_lines<R: BufRead>(reader: &mut R, mut on_line: impl FnMut(&[u8])) -> io::Result<u64> {
    let mut buffer = Vec::new();
    let mut used = 0u64;
    loop {
        buffer.clear();
        let read = Read::by_ref(reader)
            .take(MAX_LINE_BYTES)
            .read_until(b'\n', &mut buffer)? as u64;
        if read == 0 {
            return Ok(used);
        }
        if buffer.last() == Some(&b'\n') {
            used += read;
            buffer.pop();
            on_line(&buffer);
        } else if read < MAX_LINE_BYTES {
            return Ok(used);
        } else {
            used += read + skip_line(reader)?;
        }
    }
}

fn skip_line<R: BufRead>(reader: &mut R) -> io::Result<u64> {
    let mut skipped = 0u64;
    loop {
        let (newline, available) = {
            let chunk = reader.fill_buf()?;
            (chunk.iter().position(|byte| *byte == b'\n'), chunk.len())
        };
        match newline {
            Some(index) => {
                reader.consume(index + 1);
                return Ok(skipped + index as u64 + 1);
            }
            None if available == 0 => return Ok(skipped),
            None => {
                reader.consume(available);
                skipped += available as u64;
            }
        }
    }
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
