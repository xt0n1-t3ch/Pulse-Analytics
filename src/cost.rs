//! Claude model pricing: model classification, per-category rates, cache-write lifetimes and
//! cost arithmetic.

use serde::Serialize;

/// Model pricing per million tokens.
/// Update these when new models are released:
/// https://platform.claude.com/docs/en/about-claude/pricing
///
/// Prompt caching pricing:
///   5-minute cache write = 1.25x base input price (`cache_write_per_million`)
///   1-hour cache write   = 2x base input price (`cache_write_1h_per_million`)
///   cache read           = 0.10x base input price (Opus 5.5 and Sonnet 5.5: 0.05x;
///                          Fable/Mythos 5.1 and 5.2: 0.025x)
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct ModelPricing {
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cache_write_per_million: f64,
    pub cache_read_per_million: f64,
}

/// A 1-hour cache write costs this multiple of base input on every Claude
/// model (<https://platform.claude.com/docs/en/about-claude/pricing>, verified
/// 2026-11-07).
const CACHE_WRITE_1H_INPUT_MULTIPLIER: f64 = 2.0;

impl ModelPricing {
    /// Rate for a 1-hour cache write, per million tokens.
    pub fn cache_write_1h_per_million(&self) -> f64 {
        self.input_per_million * CACHE_WRITE_1H_INPUT_MULTIPLIER
    }
}

/// Cache-write tokens of one request, split by cache lifetime.
///
/// Claude Code transcripts report the split in
/// `message.usage.cache_creation.ephemeral_{5m,1h}_input_tokens`. A request
/// without the split is all five-minute writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheWrites {
    pub five_minute: u64,
    pub one_hour: u64,
}

impl CacheWrites {
    /// Every write is a five-minute write (no split available).
    pub const fn all_five_minute(tokens: u64) -> Self {
        Self {
            five_minute: tokens,
            one_hour: 0,
        }
    }

    pub const fn split(five_minute: u64, one_hour: u64) -> Self {
        Self {
            five_minute,
            one_hour,
        }
    }

    /// Builds the split from the request's total writes and the reported
    /// one-hour count. The one-hour count is clamped to the total and the rest
    /// is five-minute, so token totals always match the total Pulse displays.
    pub fn from_total_and_one_hour(total: u64, one_hour: Option<u64>) -> Self {
        let one_hour = one_hour.unwrap_or(0).min(total);
        Self {
            five_minute: total - one_hour,
            one_hour,
        }
    }

    pub const fn total(self) -> u64 {
        self.five_minute.saturating_add(self.one_hour)
    }
}

/// Fast-mode (priority speed) rate multiplier. When a turn runs at Fast speed on
/// a fast-capable model, every token category is billed at this multiple of the
/// model's standard rate. Fast mode is exactly 2x standard on every
/// fast-capable model: Opus 4.8 and Opus 5 ($10 / $50), Opus 5.5 ($8 / $40),
/// and Fable 5.2 and Mythos 5.2 ($16 / $80).
pub const FAST_RATE_MULTIPLIER: f64 = 2.0;

/// Strips context window suffixes like `[1m]` from model IDs.
/// e.g. "claude-opus-4-6[1m]" → "claude-opus-4-6"
fn strip_context_suffix(model_id: &str) -> &str {
    model_id.split('[').next().unwrap_or(model_id)
}

fn is_mythos_class(id: &str) -> bool {
    id.contains("fable") || id.contains("mythos")
}

/// Claude Fable/Mythos 5.1 and later ("claude-fable-5-1", dated variants).
/// Expects an id with any `[1m]` context suffix already stripped.
fn is_mythos_5_1_or_later(id: &str) -> bool {
    ["fable", "mythos"]
        .into_iter()
        .any(|family| is_version_at_least(id, family, 5, 1))
}

/// Matches exactly Claude Fable 5.2 and Claude Mythos 5.2 ids
/// ("claude-fable-5-2", "claude-mythos-5-2", dated and Bedrock variants).
/// Expects an id with any `[1m]` context suffix already stripped and lowercased.
fn is_mythos_5_2_class(id: &str) -> bool {
    ["fable", "mythos"]
        .into_iter()
        .any(|family| family_version(id, family) == Some((5, 2)))
}

/// Claude Fable 5.2 and Claude Mythos 5.2 official API rates: $8 input, $40
/// output, $10 5-minute cache write (1.25x) and $0.20 cache read (0.025x).
/// Released 2026-11-09 at 20% below version 5.1 ($10 / $50). The 1M context
/// window is billed at the standard rate with no long-context surcharge.
/// <https://platform.claude.com/docs/en/about-claude/pricing>, verified 2026-11-10.
const FABLE_5_2_PRICING: ModelPricing = ModelPricing {
    input_per_million: 8.0,
    output_per_million: 40.0,
    cache_write_per_million: 10.0,
    cache_read_per_million: 0.20,
};

/// Matches Claude Opus 5 ids ("claude-opus-5" and dated variants) without
/// absorbing Claude Opus 5.5 ("claude-opus-5-5") or a hypothetical "Opus 50".
/// Expects an id with any `[1m]` context suffix already stripped. The version
/// parser ignores eight-digit date suffixes, so a dated Opus 5 id still reads
/// as exactly 5.0.
fn is_opus_5_class(id: &str) -> bool {
    family_version(id, "opus") == Some((5, 0))
}

/// Matches Claude Opus 5.5 ids ("claude-opus-5-5" and dated variants).
/// Expects an id with any `[1m]` context suffix already stripped.
fn is_opus_5_5_class(id: &str) -> bool {
    family_version(id, "opus") == Some((5, 5))
}

/// Claude Opus 5.5 official API rates: $4 input, $20 output, $5 5-minute cache
/// write (1.25x) and $0.20 cache read. Its cache-read multiplier is 0.05x, not
/// the standard 0.10x.
/// <https://platform.claude.com/docs/en/about-claude/pricing>, verified 2026-11-07.
const OPUS_5_5_PRICING: ModelPricing = ModelPricing {
    input_per_million: 4.0,
    output_per_million: 20.0,
    cache_write_per_million: 5.0,
    cache_read_per_million: 0.20,
};

/// Matches exactly Claude Sonnet 5 ids ("claude-sonnet-5" and dated variants)
/// without absorbing Claude Sonnet 5.5 ("claude-sonnet-5-5"), a hypothetical
/// "Sonnet 50" or an id such as "claude-sonnet-5x" that merely starts with the
/// same digits. Expects an id with any `[1m]` context suffix already stripped.
/// The version parser ignores eight-digit date suffixes, so a dated Sonnet 5 id
/// still reads as exactly 5.0.
fn is_sonnet_5_class(id: &str) -> bool {
    family_version(id, "sonnet") == Some((5, 0))
}

/// Matches Claude Sonnet 5.5 ids ("claude-sonnet-5-5" and dated variants).
/// Expects an id with any `[1m]` context suffix already stripped.
fn is_sonnet_5_5_class(id: &str) -> bool {
    family_version(id, "sonnet") == Some((5, 5))
}

/// Claude Sonnet 5 official API rates: $2 input, $10 output, $2.50 5-minute
/// cache write and $0.20 cache read. Anthropic announced these as
/// introductory pricing through August 31, 2026, then cancelled the scheduled
/// increase to $3/$15, so they are the standard price.
/// <https://platform.claude.com/docs/en/about-claude/pricing>, verified 2026-09-22.
const SONNET_5_PRICING: ModelPricing = ModelPricing {
    input_per_million: 2.0,
    output_per_million: 10.0,
    cache_write_per_million: 2.50,
    cache_read_per_million: 0.20,
};

/// Claude Sonnet 5.5 official API rates: $2 input, $10 output, $2.50 5-minute
/// cache write (1.25x) and $0.10 cache read (0.05x). Only the cache read
/// differs from Sonnet 5 ($0.20). The 1M context window is billed at the
/// standard rate with no long-context surcharge.
/// <https://platform.claude.com/docs/en/about-claude/pricing>, verified 2026-11-07.
const SONNET_5_5_PRICING: ModelPricing = ModelPricing {
    input_per_million: 2.0,
    output_per_million: 10.0,
    cache_write_per_million: 2.50,
    cache_read_per_million: 0.10,
};

/// Matches Claude Haiku 5.5 ids ("claude-haiku-5-5" and dated variants).
/// Expects an id with any `[1m]` context suffix already stripped and lowercased.
fn is_haiku_5_5_class(id: &str) -> bool {
    family_version(id, "haiku") == Some((5, 5))
}

/// Claude Haiku 5.5 prices a request by its prompt length: input + cache read +
/// cache write. Above this many tokens every category uses
/// the high row (`HAIKU_5_5_LONG_PROMPT_PRICING`); exactly this many is still the low row.
pub const HAIKU_5_5_LONG_PROMPT_THRESHOLD: u64 = 100_000;

/// Claude Haiku 5.5 rates for a prompt of at most 100,000 tokens: $0.10 input,
/// $0.50 output, $0.125 5-minute cache write and $0.01 cache read. The 1M
/// window has no other surcharge.
/// <https://platform.claude.com/docs/en/about-claude/pricing>, verified 2026-11-07.
const HAIKU_5_5_PRICING: ModelPricing = ModelPricing {
    input_per_million: 0.10,
    output_per_million: 0.50,
    cache_write_per_million: 0.125,
    cache_read_per_million: 0.01,
};

/// Claude Haiku 5.5 rates for a prompt above 100,000 tokens: $0.50 input,
/// $2.50 output, $0.625 5-minute cache write and $0.05 cache read.
/// <https://platform.claude.com/docs/en/about-claude/pricing>, verified 2026-11-07.
const HAIKU_5_5_LONG_PROMPT_PRICING: ModelPricing = ModelPricing {
    input_per_million: 0.50,
    output_per_million: 2.50,
    cache_write_per_million: 0.625,
    cache_read_per_million: 0.05,
};

/// True when the model's rates depend on the prompt length of each request
/// (Claude Haiku 5.5). Session totals cannot say which requests crossed the
/// threshold, so callers that only have totals must not price such a model
/// above [`HAIKU_5_5_LONG_PROMPT_THRESHOLD`].
pub fn has_prompt_length_tiers(model_id: &str) -> bool {
    is_haiku_5_5_class(&strip_context_suffix(model_id).to_lowercase())
}

/// Resolves a model's per-million-token pricing. A prompt-tiered model
/// (Haiku 5.5) resolves to its low row; use [`model_pricing_for_prompt`] when
/// the request's prompt length is known.
pub fn model_pricing(model_id: &str) -> ModelPricing {
    let id = strip_context_suffix(model_id).to_lowercase();
    if is_haiku_5_5_class(&id) {
        return HAIKU_5_5_PRICING;
    }
    if is_sonnet_5_class(&id) {
        return SONNET_5_PRICING;
    }
    if is_sonnet_5_5_class(&id) {
        return SONNET_5_5_PRICING;
    }
    if is_opus_5_5_class(&id) {
        return OPUS_5_5_PRICING;
    }
    if is_mythos_5_2_class(&id) {
        return FABLE_5_2_PRICING;
    }
    if id.contains("opus") {
        if is_new_opus(&id) {
            ModelPricing {
                input_per_million: 5.0,
                output_per_million: 25.0,
                cache_write_per_million: 6.25,
                cache_read_per_million: 0.50,
            }
        } else {
            ModelPricing {
                input_per_million: 15.0,
                output_per_million: 75.0,
                cache_write_per_million: 18.75,
                cache_read_per_million: 1.50,
            }
        }
    } else if id.contains("haiku") {
        if id.contains("3-5") || id.contains("3.5") {
            ModelPricing {
                input_per_million: 0.80,
                output_per_million: 4.0,
                cache_write_per_million: 1.0,
                cache_read_per_million: 0.08,
            }
        } else if id.contains("haiku-3") && !id.contains("3-5") {
            ModelPricing {
                input_per_million: 0.25,
                output_per_million: 1.25,
                cache_write_per_million: 0.30,
                cache_read_per_million: 0.03,
            }
        } else {
            ModelPricing {
                input_per_million: 1.0,
                output_per_million: 5.0,
                cache_write_per_million: 1.25,
                cache_read_per_million: 0.10,
            }
        }
    } else if is_mythos_class(&id) {
        ModelPricing {
            input_per_million: 10.0,
            output_per_million: 50.0,
            cache_write_per_million: 12.5,
            // Fable/Mythos 5.1 cache hits cost 0.025x input; version 5 keeps 0.1x.
            // Version 5.2 has its own row (`FABLE_5_2_PRICING`).
            cache_read_per_million: if is_mythos_5_1_or_later(&id) {
                0.25
            } else {
                1.0
            },
        }
    } else {
        ModelPricing {
            input_per_million: 3.0,
            output_per_million: 15.0,
            cache_write_per_million: 3.75,
            cache_read_per_million: 0.30,
        }
    }
}

/// Per-million rates for one request whose prompt (input + cache read + cache
/// write) is `prompt_tokens` long. Equal to [`model_pricing`] for every model
/// except Claude Haiku 5.5, which bills a prompt above
/// [`HAIKU_5_5_LONG_PROMPT_THRESHOLD`] at its high row.
pub fn model_pricing_for_prompt(model_id: &str, prompt_tokens: u64) -> ModelPricing {
    if prompt_tokens > HAIKU_5_5_LONG_PROMPT_THRESHOLD && has_prompt_length_tiers(model_id) {
        HAIKU_5_5_LONG_PROMPT_PRICING
    } else {
        model_pricing(model_id)
    }
}

/// Opus 4.5 and later use new pricing ($5/$25). Thin wrapper over the shared
/// version comparison so there is exactly one place that knows how to read a
/// version out of a model ID.
///
/// Don't match: opus-4, opus-4-0, opus-4-1, opus-3, opus (bare).
fn is_new_opus(id: &str) -> bool {
    is_version_at_least(id, "opus", 4, 5)
}

/// Cost at the model's flat rates, with no long-context surcharge. Every cache
/// write is a five-minute write. A prompt-tiered model uses the row that
/// matches the request's prompt length.
pub fn calculate_cost(
    model_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
) -> f64 {
    let prompt_tokens = input_tokens
        .saturating_add(cache_creation_tokens)
        .saturating_add(cache_read_tokens);
    let pricing = model_pricing_for_prompt(model_id, prompt_tokens);
    let input_cost = (input_tokens as f64 / 1_000_000.0) * pricing.input_per_million;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * pricing.output_per_million;
    let cache_write_cost =
        (cache_creation_tokens as f64 / 1_000_000.0) * pricing.cache_write_per_million;
    let cache_read_cost = (cache_read_tokens as f64 / 1_000_000.0) * pricing.cache_read_per_million;
    input_cost + output_cost + cache_write_cost + cache_read_cost
}

/// Extract a display name like "Claude Opus 4.6" from any model ID format.
/// Handles both dated IDs ("claude-opus-4-6-20260213") and short IDs ("claude-opus-4-6").
pub fn model_display_name(model_id: &str) -> String {
    let id = strip_context_suffix(model_id).to_lowercase();
    let trimmed = id.trim();

    if trimmed.is_empty() || trimmed == "<synthetic>" {
        return "Claude".to_string();
    }

    let family = if id.contains("opus") {
        "Opus"
    } else if id.contains("haiku") {
        "Haiku"
    } else if id.contains("sonnet") {
        "Sonnet"
    } else if id.contains("fable") {
        "Fable"
    } else if id.contains("mythos") {
        "Mythos"
    } else {
        let cleaned = id
            .trim_end_matches(|c: char| c == '-' || c.is_ascii_digit())
            .trim_end_matches('-');
        let display = if cleaned.is_empty() {
            model_id
        } else {
            cleaned
        };
        return format!("Claude ({})", display);
    };

    let family_lower = family.to_lowercase();
    let after_family = id
        .find(&family_lower)
        .map(|pos| &id[pos + family_lower.len()..])
        .unwrap_or("");

    let segments: Vec<&str> = after_family
        .split('-')
        .filter(|s| !s.is_empty())
        .take_while(|s| s.len() <= 3 && s.chars().all(|c| c.is_ascii_digit()))
        .collect();

    if segments.is_empty() {
        format!("Claude {family}")
    } else {
        format!("Claude {} {}", family, segments.join("."))
    }
}

/// Returns true if the model supports the 1M token context window.
/// Supported: Opus 4.5+, Sonnet 4.6/4.5/4, Sonnet 5, Sonnet 5.5 and Haiku 5.5.
/// Not supported: older Haiku, legacy Opus, Sonnet 3.x.
/// Also returns true if the model ID contains a `[1m]` suffix (explicit 1M context indicator).
pub fn supports_1m_context(model_id: &str) -> bool {
    if model_id.contains("[1m]") {
        return true;
    }
    let id = strip_context_suffix(model_id).to_lowercase();
    if is_mythos_class(&id)
        || is_sonnet_5_class(&id)
        || is_sonnet_5_5_class(&id)
        || is_haiku_5_5_class(&id)
    {
        return true;
    }
    if id.contains("haiku") {
        return false;
    }
    if id.contains("opus") {
        return is_new_opus(&id);
    }
    if id.contains("sonnet") {
        let sonnet_pos = id.find("sonnet").unwrap_or(id.len());
        let before = &id[..sonnet_pos];
        let prefix_version: Option<u32> = before
            .split('-')
            .filter(|s| !s.is_empty() && s.len() <= 3 && s.chars().all(|c| c.is_ascii_digit()))
            .filter_map(|s| s.parse::<u32>().ok())
            .next();
        if let Some(v) = prefix_version
            && v < 4
        {
            return false;
        }
        let after = &id[sonnet_pos + 6..];
        let first_seg = after
            .split('-')
            .find(|s| !s.is_empty() && s.len() <= 3 && s.chars().all(|c| c.is_ascii_digit()));
        return match first_seg {
            Some(seg) => seg.parse::<u32>().map(|v| v >= 4).unwrap_or(true),
            None => true,
        };
    }
    false
}

/// Returns true if the model has 1M context GA for **API token-rate pricing**
/// (no long-context surcharge on input / output / cache).
///
/// Per Anthropic's official pricing docs
/// (<https://console.anthropic.com/docs/en/about-claude/pricing>, effective
/// 2026-03-13):
/// > *"Opus 4.7, Opus 4.6, and Sonnet 4.6 include the full 1M token context
/// > window at standard pricing."*
///
/// GA models (flat per-token rate at any context length, no 2×/1.5× premium):
///   • Opus 4.6+ · Opus 4.7 · **Sonnet 4.6** · Sonnet 5 · Sonnet 5.5 · Haiku 5.5
///     (Sonnet 5.5 confirmed 2026-09-29; Haiku 5.5 1M window confirmed 2026-11-07,
///     its only price change is the prompt-length tier)
///
/// Beta models (2× input, 1.5× output, 2× cache when total API input > 200K):
///   • Sonnet 4 / 4.5 · Opus 4 / 4.5
///
/// NOTE: **Plan-level "Extra Usage" accounting** (claude.ai Pro / Max / Teams)
/// is a *separate* concept and is NOT represented here. When a subscriber
/// exceeds their baseline plan quota, overage is billed pay-as-you-go against
/// their Extra Usage cap. Pulse tracks that independently via `crate::usage`.
/// This function only concerns token-rate surcharges applied at the API layer.
///
/// Also returns true for any model with an explicit `[1m]` suffix (an
/// explicit GA indicator emitted by tooling that has already reconciled
/// pricing).
pub fn is_ga_1m_context(model_id: &str) -> bool {
    if model_id.contains("[1m]") {
        return true;
    }
    let id = strip_context_suffix(model_id).to_lowercase();
    if is_mythos_class(&id)
        || is_sonnet_5_class(&id)
        || is_sonnet_5_5_class(&id)
        || is_haiku_5_5_class(&id)
    {
        return true;
    }
    if id.contains("opus") {
        return is_version_at_least(&id, "opus", 4, 6);
    }
    if id.contains("sonnet") {
        return is_version_at_least(&id, "sonnet", 4, 6);
    }
    false
}

/// Checks if a model ID has a version >= major.minor after the family name.
///
/// Accepts both the two-segment shape used through the 4.x line
/// ("claude-opus-4-8" -> 4.8) and the single-segment shape introduced by
/// Claude Opus 5 ("claude-opus-5" -> 5.0), where a missing minor reads as 0.
/// A dated id ("claude-opus-5-20260724") still parses as 5.0 because the date
/// is too long to be mistaken for a version segment.
///
/// The single-segment relaxation deliberately requires the family to be
/// followed by a version at all, so a bare "opus" stays unversioned and keeps
/// falling through to legacy handling. Future single-segment families (Opus 6,
/// Sonnet 6, ...) are handled here rather than by another special case.
fn is_version_at_least(id: &str, family: &str, min_major: u32, min_minor: u32) -> bool {
    let Some((major, minor)) = family_version(id, family) else {
        return false;
    };
    major > min_major || (major == min_major && minor >= min_minor)
}

/// Parses the `(major, minor)` version that follows `family` in a model ID.
/// Returns `None` when the family is absent or carries no version segment.
///
/// Version segments are at most three digits, which is what keeps an eight
/// digit date suffix from being read as a version.
fn family_version(id: &str, family: &str) -> Option<(u32, u32)> {
    let after = &id[id.find(family)? + family.len()..];
    let segments: Vec<u32> = after
        .split('-')
        .filter(|s| !s.is_empty() && s.len() <= 3 && s.chars().all(|c| c.is_ascii_digit()))
        .filter_map(|s| s.parse::<u32>().ok())
        .collect();
    match segments.as_slice() {
        [major, minor, ..] => Some((*major, *minor)),
        // Single-segment ids (Claude Opus 5 onward) have an implicit .0 minor.
        [major] => Some((*major, 0)),
        [] => None,
    }
}

/// Like `calculate_cost` but applies the 1M context surcharge for **beta** models only.
///
/// GA models (Opus 4.6+, Opus 4.7, Sonnet 4.6, Haiku 5.5): standard pricing at any context length.
/// Per Anthropic pricing docs (2026-03-13 update): standard per-token rate across the full 1M window.
///
/// Beta models (Sonnet 4 / 4.5, Opus 4 / 4.5): 2× input, 1.5× output, 2× cache when >200K total API input.
///
/// Plan-level "Extra Usage" (claude.ai Pro / Max / Teams) is a separate
/// concept tracked via `crate::usage`: not a token-rate multiplier.
pub fn calculate_cost_with_context(
    model_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
) -> f64 {
    calculate_category_costs(
        model_id,
        input_tokens,
        output_tokens,
        cache_creation_tokens,
        cache_read_tokens,
        false,
    )
    .total()
}

/// Returns the model display name with a 1M context indicator when applicable.
///
/// - GA models (Opus 4.6+, Sonnet 4.6+, Sonnet 5, Sonnet 5.5, Haiku 5.5): always shows "(1M)"
///   since 1M is their native context.
/// - Beta models: shows "(1M Context)" only when `max_turn_api_input > 200K` (single turn indicator).
/// - Unsupported models: no suffix.
pub fn model_display_with_context(
    model_id: &str,
    base_display: &str,
    max_turn_api_input: u64,
) -> String {
    if is_ga_1m_context(model_id) {
        format!("{} (1M)", base_display)
    } else if supports_1m_context(model_id) && max_turn_api_input > 200_000 {
        format!("{} (1M Context)", base_display)
    } else {
        base_display.to_string()
    }
}

/// Strips the "Claude " prefix from a display name for compact Discord display.
/// "Claude Opus 4.6" → "Opus 4.6", "Claude Sonnet 4.5" → "Sonnet 4.5".
/// The Discord app title is already "Claude Code", so repeating "Claude" is redundant.
pub fn strip_claude_prefix(display_name: &str) -> &str {
    display_name.strip_prefix("Claude ").unwrap_or(display_name)
}

/// True when the model uses a newer tokenizer that produces more tokens per prompt
/// for the same input text than its immediate predecessor, inflating billing even
/// though per-token rates are unchanged. Opus 4.7+ can produce up to ~35% more
/// tokens than 4.6 for identical text; Claude Sonnet 5 ships the same class of
/// tokenizer change versus Sonnet 4.6 (~1.0-1.35x, per Anthropic's Sonnet 5 launch
/// post). This is a tokenizer property and feeds no cost math.
///
/// Claude Opus 5.5 and Claude Sonnet 5.5 are excluded by product decision
/// (2026-09-29): no 5.5-generation model carries this flag, and newer families
/// (Fable and Mythos 5.2 included) must not get it by default. Adding a family here needs an explicit,
/// per-model product decision backed by a published figure.
///
/// UI surfaces this as a tooltip/warning so users understand cost deltas vs the
/// previous generation.
pub fn has_inflated_tokenizer(model_id: &str) -> bool {
    let id = strip_context_suffix(model_id).to_lowercase();
    // Sonnet 5.5 is checked before anything else so it can never fall into a
    // broader Sonnet rule. Only exactly Sonnet 5.0 keeps the flag.
    if is_sonnet_5_5_class(&id) {
        return false;
    }
    if is_sonnet_5_class(&id) {
        return true;
    }
    if !id.contains("opus") {
        return false;
    }
    // Claude Opus 5 is deliberately excluded. Anthropic's pricing page
    // attributes the newer tokenizer to "Claude 4.7 and later models" but
    // publishes no Opus 5 figure, so Pulse reports only what is documented
    // per-model rather than inferring a multiplier. This flag is display-only
    // and feeds no cost math, so the choice cannot skew billing either way.
    // Claude Opus 5.5 is excluded by product decision (see above), whatever
    // tokenizer it uses.
    if is_opus_5_class(&id) || is_opus_5_5_class(&id) {
        return false;
    }
    is_version_at_least(&id, "opus", 4, 7)
}

/// True when the model supports fast mode (priority speed) billing.
/// Fast mode launched with Opus 4.8 and covers Opus 4.8 and later (currently
/// Opus 4.8, Opus 5 and Opus 5.5), plus exactly Fable 5.2 and Mythos 5.2.
/// Fable/Mythos 5.1 and 5, every Sonnet and every Haiku have no fast mode.
/// <https://platform.claude.com/docs/en/build-with-claude/fast-mode>, verified 2026-11-10.
pub fn is_fast_capable(model_id: &str) -> bool {
    let id = strip_context_suffix(model_id).to_lowercase();
    if is_mythos_5_2_class(&id) {
        return true;
    }
    if !id.contains("opus") {
        return false;
    }
    is_version_at_least(&id, "opus", 4, 8)
}

/// The effective rate multiplier for a turn: `FAST_RATE_MULTIPLIER` when the turn
/// ran at fast speed on a fast-capable model, otherwise `1.0`.
pub fn speed_multiplier(model_id: &str, fast: bool) -> f64 {
    if fast && is_fast_capable(model_id) {
        FAST_RATE_MULTIPLIER
    } else {
        1.0
    }
}

/// Speed- and context-aware cost split into the four billable token categories.
/// The four components always sum to `calculate_cost_with_context_and_speed` for
/// the same inputs, so per-turn accumulation of these components reconciles with
/// the accumulated total cost.
#[derive(Debug, Clone, Copy, Default)]
pub struct TurnCostBreakdown {
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_write_cost: f64,
    pub cache_read_cost: f64,
}

impl TurnCostBreakdown {
    pub fn total(&self) -> f64 {
        self.input_cost + self.output_cost + self.cache_write_cost + self.cache_read_cost
    }
}

/// Per-category form of `calculate_cost_with_context_and_speed`: returns the four
/// billable components for a turn, each already scaled by the beta 1M-context
/// surcharge (beta models above the threshold) and the fast-mode multiplier.
///
/// `pure_input_tokens` is the non-cached input (input minus cache write/read);
/// the beta threshold and the Haiku 5.5 prompt tier are evaluated on total API
/// input (`pure_input + cache_write + cache_read`). Every cache write is a
/// five-minute write; use [`calculate_category_costs_with_cache_ttl`] when the
/// transcript reports the split.
pub fn calculate_category_costs(
    model_id: &str,
    pure_input_tokens: u64,
    output_tokens: u64,
    cache_write_tokens: u64,
    cache_read_tokens: u64,
    fast: bool,
) -> TurnCostBreakdown {
    calculate_category_costs_with_cache_ttl(
        model_id,
        pure_input_tokens,
        output_tokens,
        CacheWrites::all_five_minute(cache_write_tokens),
        cache_read_tokens,
        fast,
    )
}

/// [`calculate_category_costs`] with cache writes split by lifetime: five-minute
/// writes bill at the model's write rate and one-hour writes at 2x base input.
/// The request's prompt tier, the beta surcharge and the fast multiplier apply
/// to both kinds of write.
pub fn calculate_category_costs_with_cache_ttl(
    model_id: &str,
    pure_input_tokens: u64,
    output_tokens: u64,
    cache_writes: CacheWrites,
    cache_read_tokens: u64,
    fast: bool,
) -> TurnCostBreakdown {
    let total_api_input = pure_input_tokens
        .saturating_add(cache_writes.total())
        .saturating_add(cache_read_tokens);
    let pricing = model_pricing_for_prompt(model_id, total_api_input);
    let beta_surcharge =
        supports_1m_context(model_id) && !is_ga_1m_context(model_id) && total_api_input > 200_000;
    let (input_factor, output_factor, cache_factor) = if beta_surcharge {
        (2.0, 1.5, 2.0)
    } else {
        (1.0, 1.0, 1.0)
    };
    let speed = speed_multiplier(model_id, fast);
    let per_million = |tokens: u64, rate: f64| (tokens as f64 / 1_000_000.0) * rate;

    TurnCostBreakdown {
        input_cost: per_million(pure_input_tokens, pricing.input_per_million)
            * input_factor
            * speed,
        output_cost: per_million(output_tokens, pricing.output_per_million) * output_factor * speed,
        cache_write_cost: (per_million(cache_writes.five_minute, pricing.cache_write_per_million)
            + per_million(cache_writes.one_hour, pricing.cache_write_1h_per_million()))
            * cache_factor
            * speed,
        cache_read_cost: per_million(cache_read_tokens, pricing.cache_read_per_million)
            * cache_factor
            * speed,
    }
}

/// Like `calculate_cost_with_context` but applies the fast-mode surcharge when the
/// turn ran at fast speed on a fast-capable model.
///
/// Fast mode bills every token category at `FAST_RATE_MULTIPLIER`x the standard
/// rate on top of the beta 1M-context surcharge, so a fast turn always costs
/// exactly `FAST_RATE_MULTIPLIER`x its standard-speed equivalent.
pub fn calculate_cost_with_context_and_speed(
    model_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    fast: bool,
) -> f64 {
    calculate_category_costs(
        model_id,
        input_tokens,
        output_tokens,
        cache_creation_tokens,
        cache_read_tokens,
        fast,
    )
    .total()
}

#[cfg(test)]
mod tests;
