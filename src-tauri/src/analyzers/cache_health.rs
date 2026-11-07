//! Cache Health analyzer: gives the user an A–F letter grade based on their
//! prompt cache hit ratio. Recent days are weighted more heavily so a good
//! week lifts the grade even if historical cache was poor (and vice versa).

use serde::Serialize;

use cc_discord_presence::provider::Provider;

use crate::db::HistoricalSession;

#[derive(Debug, Clone, Serialize)]
pub struct CacheHealthReport {
    pub grade: char,
    pub grade_label: &'static str,
    pub color: &'static str,
    pub hit_ratio: f64,
    pub trend_weighted_ratio: f64,
    pub total_cache_read: i64,
    pub total_cache_write: i64,
    pub total_input: i64,
    pub sessions_analyzed: usize,
    pub diagnosis: String,
}

/// Map a cache-hit ratio (0–100) to an A–F grade.
pub fn grade_for_ratio(ratio: f64) -> (char, &'static str, &'static str) {
    match ratio as u32 {
        80..=100 => ('A', "Excellent", "#57F287"),
        65..=79 => ('B', "Healthy", "#A8D08D"),
        50..=64 => ('C', "Fair", "#F5A524"),
        30..=49 => ('D', "Poor", "#E87638"),
        _ => ('F', "Broken", "#ED4245"),
    }
}

/// Compute overall cache hit ratio across the provided sessions.
pub fn overall_ratio(sessions: &[HistoricalSession]) -> f64 {
    let mut total_cache_read = 0i64;
    let mut total_input = 0i64;
    for s in sessions {
        let pure_input = (s.input_tokens - s.cache_write_tokens - s.cache_read_tokens).max(0);
        total_cache_read += s.cache_read_tokens;
        total_input += pure_input;
    }
    let denom = total_cache_read + total_input;
    if denom == 0 {
        0.0
    } else {
        (total_cache_read as f64 / denom as f64) * 100.0
    }
}

/// Trend-weighted ratio: sessions whose `started_at` falls in the most-recent
/// 7-day window count 2× toward the final ratio, so recent improvements (or
/// regressions) move the grade faster than a flat average would allow.
pub fn trend_weighted_ratio(sessions: &[HistoricalSession]) -> f64 {
    use chrono::{DateTime, Duration, Utc};

    let now = Utc::now();
    let recent_cutoff = now - Duration::days(7);

    let mut weighted_read: f64 = 0.0;
    let mut weighted_input: f64 = 0.0;

    for s in sessions {
        let started = s
            .started_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc));
        let weight: f64 = match started {
            Some(dt) if dt >= recent_cutoff => 2.0,
            _ => 1.0,
        };

        let pure_input = (s.input_tokens - s.cache_write_tokens - s.cache_read_tokens).max(0);
        weighted_read += s.cache_read_tokens as f64 * weight;
        weighted_input += pure_input as f64 * weight;
    }

    let denom = weighted_read + weighted_input;
    if denom == 0.0 {
        0.0
    } else {
        (weighted_read / denom) * 100.0
    }
}

pub fn analyze(sessions: &[HistoricalSession]) -> CacheHealthReport {
    analyze_for_provider(Provider::Claude, sessions)
}

pub fn analyze_for_provider(
    provider: Provider,
    sessions: &[HistoricalSession],
) -> CacheHealthReport {
    let overall = overall_ratio(sessions);
    let weighted = trend_weighted_ratio(sessions);
    // Grade off the weighted score so recent behavior dominates.
    let (grade, grade_label, color) = grade_for_ratio(weighted);

    let total_cache_read: i64 = sessions.iter().map(|s| s.cache_read_tokens).sum();
    let total_cache_write: i64 = sessions.iter().map(|s| s.cache_write_tokens).sum();
    let total_input: i64 = sessions
        .iter()
        .map(|s| (s.input_tokens - s.cache_write_tokens - s.cache_read_tokens).max(0))
        .sum();

    let diagnosis = diagnose(
        provider,
        grade,
        weighted,
        overall,
        total_cache_read + total_input,
    );

    CacheHealthReport {
        grade,
        grade_label,
        color,
        hit_ratio: overall,
        trend_weighted_ratio: weighted,
        total_cache_read,
        total_cache_write,
        total_input,
        sessions_analyzed: sessions.len(),
        diagnosis,
    }
}

fn diagnose(provider: Provider, grade: char, weighted: f64, overall: f64, denom: i64) -> String {
    let instruction_file = provider.instruction_file_name();
    let product_name = provider.display_name();
    if denom == 0 {
        return format!(
            "Not enough usage data yet to grade your cache health. Keep using {product_name} and check back after a few sessions."
        );
    }
    match grade {
        'A' => format!(
            "Cache is working hard for you — {weighted:.0}% of input tokens are served from \
            cache. Keep your {instruction_file} and system prompts stable to preserve this."
        ),
        'B' => format!(
            "Solid cache hit ratio ({weighted:.0}%). Small wins available: reorder prompts so \
            stable context sits at the top and volatile bits go last."
        ),
        'C' => format!(
            "Cache is helping but leaking ({weighted:.0}%). Something in your {instruction_file} or \
            tooling is invalidating the prefix more often than it should."
        ),
        'D' => format!(
            "Cache is barely working ({weighted:.0}% recent, {overall:.0}% all-time). Big cost \
            upside if you can stabilize the prompt prefix — every turn re-pays for context."
        ),
        _ => format!(
            "Cache is broken ({weighted:.0}%). Almost every turn re-bills the full prefix. \
            Check for recent {instruction_file} edits or a {product_name} version that regressed caching."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grade_a_excellent() {
        assert_eq!(grade_for_ratio(95.0).0, 'A');
        assert_eq!(grade_for_ratio(80.0).0, 'A');
    }

    #[test]
    fn grade_b_healthy() {
        assert_eq!(grade_for_ratio(79.0).0, 'B');
        assert_eq!(grade_for_ratio(65.0).0, 'B');
    }

    #[test]
    fn grade_f_broken() {
        assert_eq!(grade_for_ratio(0.0).0, 'F');
        assert_eq!(grade_for_ratio(10.0).0, 'F');
        assert_eq!(grade_for_ratio(29.9).0, 'F');
    }

    #[test]
    fn overall_ratio_empty_returns_zero() {
        assert_eq!(overall_ratio(&[]), 0.0);
    }

    /// Diagnosis copy must name the provider's own instruction file. Telling a
    /// Codex user to edit CLAUDE.md sends them to a file that does not exist in
    /// their setup.
    #[test]
    fn diagnosis_names_the_providers_own_instruction_file() {
        for grade in ['A', 'C', 'F'] {
            let claude = diagnose(Provider::Claude, grade, 90.0, 88.0, 1_000);
            let codex = diagnose(Provider::Codex, grade, 90.0, 88.0, 1_000);

            if claude.contains(".md") {
                assert!(
                    claude.contains("CLAUDE.md"),
                    "claude grade {grade}: {claude}"
                );
                assert!(!claude.contains("AGENTS.md"), "claude grade {grade}");
                assert!(codex.contains("AGENTS.md"), "codex grade {grade}: {codex}");
                assert!(!codex.contains("CLAUDE.md"), "codex grade {grade}");
            }
        }
    }

    /// The same applies to the product name in the empty and broken states.
    #[test]
    fn diagnosis_names_the_providers_own_product() {
        let claude = diagnose(Provider::Claude, 'A', 0.0, 0.0, 0);
        let codex = diagnose(Provider::Codex, 'A', 0.0, 0.0, 0);
        assert!(claude.contains("Claude Code"), "{claude}");
        assert!(!codex.contains("Claude Code"), "{codex}");
    }
}
