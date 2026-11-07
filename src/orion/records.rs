//! Typed views of the transcript records Pulse reads. Every field is optional
//! and unknown fields are skipped, so a record shape Orion extends later still
//! parses and a large payload is lexed, never allocated.

use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};
use std::fmt;

#[derive(Deserialize)]
pub(super) struct Record<D> {
    #[serde(default)]
    pub(super) kind: String,
    #[serde(default)]
    pub(super) op: String,
    #[serde(default)]
    pub(super) id: String,
    #[serde(default)]
    pub(super) at: i64,
    pub(super) data: Option<D>,
}

#[derive(Deserialize)]
pub(super) struct MessageData {
    pub(super) role: Option<String>,
    #[serde(rename = "modelId")]
    pub(super) model_id: Option<String>,
    #[serde(rename = "providerId")]
    pub(super) provider_id: Option<String>,
    pub(super) time: Option<MessageTime>,
    pub(super) finish: Option<String>,
    pub(super) tokens: Option<Tokens>,
}

#[derive(Default, Deserialize)]
pub(super) struct MessageTime {
    pub(super) created: Option<i64>,
    pub(super) completed: Option<i64>,
}

#[derive(Default, Deserialize)]
pub(super) struct Tokens {
    pub(super) total: Option<Count>,
    pub(super) input: Option<Count>,
    pub(super) output: Option<Count>,
    pub(super) reasoning: Option<Count>,
    pub(super) cache: Option<CacheTokens>,
}

#[derive(Default, Deserialize)]
pub(super) struct CacheTokens {
    pub(super) read: Option<Count>,
    pub(super) write: Option<Count>,
}

#[derive(Deserialize)]
pub(super) struct EntryData {
    #[serde(rename = "modelSelection")]
    pub(super) model_selection: Option<ModelSelection>,
    pub(super) window: Option<ContextWindow>,
}

#[derive(Deserialize)]
pub(super) struct ModelSelection {
    #[serde(rename = "providerId")]
    pub(super) provider_id: Option<String>,
    #[serde(rename = "modelId")]
    pub(super) model_id: Option<String>,
    pub(super) options: Option<SelectionOptions>,
}

#[derive(Deserialize)]
pub(super) struct SelectionOptions {
    #[serde(rename = "reasoningLevel")]
    pub(super) reasoning_level: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ContextWindow {
    #[serde(rename = "usedTokens")]
    pub(super) used_tokens: Option<Count>,
    #[serde(rename = "maxTokens")]
    pub(super) max_tokens: Option<Count>,
    #[serde(rename = "autoCompactThresholdTokens")]
    pub(super) auto_compact_threshold_tokens: Option<Count>,
    pub(super) cache: Option<CacheWindow>,
}

#[derive(Deserialize)]
pub(super) struct CacheWindow {
    #[serde(rename = "hitRate")]
    pub(super) hit_rate: Option<f64>,
}

#[derive(Deserialize)]
pub(super) struct PartData {
    #[serde(rename = "type")]
    pub(super) kind: Option<String>,
    pub(super) tool: Option<String>,
    pub(super) state: Option<ToolState>,
}

#[derive(Default, Deserialize)]
pub(super) struct ToolState {
    pub(super) status: Option<String>,
    pub(super) input: Option<ToolInput>,
}

#[derive(Default, Deserialize)]
pub(super) struct ToolInput {
    pub(super) file_path: Option<String>,
    #[serde(rename = "filePath")]
    pub(super) file_path_camel: Option<String>,
}

/// A token count from a JSON number. A negative, fractional or non-finite value
/// is clamped, and any other type counts as zero, so one odd record cannot
/// drop the whole line.
#[derive(Clone, Copy, Debug)]
pub(super) struct Count(pub(super) u64);

impl<'de> Deserialize<'de> for Count {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CountVisitor;
        impl Visitor<'_> for CountVisitor {
            type Value = Count;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a token count")
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Count, E> {
                Ok(Count(value))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Count, E> {
                Ok(Count(value.max(0) as u64))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Count, E> {
                Ok(Count(if value.is_finite() {
                    value.max(0.0) as u64
                } else {
                    0
                }))
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Count, E> {
                Ok(Count(0))
            }
        }
        deserializer.deserialize_any(CountVisitor)
    }
}
