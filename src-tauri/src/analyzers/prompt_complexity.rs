//! Prompt complexity analyzer: scores how complex and specific prompts were, with a short
//! preview of each.

use std::collections::HashMap;

use serde::Serialize;

use crate::db::HistoricalSession;

use super::session_trace::SessionTrace;

const MAX_PROMPT_PREVIEW_CHARS: usize = 240;

#[derive(Debug, Clone, Serialize)]
pub struct PromptComplexitySession {
    pub session_id: String,
    pub project: String,
    pub complexity_score: u8,
    pub specificity_score: u8,
    pub label: &'static str,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptComplexityReport {
    pub available: bool,
    pub sessions_analyzed: usize,
    pub prompts_analyzed: usize,
    pub avg_complexity_score: f64,
    pub avg_specificity_score: f64,
    pub high_complexity_sessions: usize,
    pub low_specificity_sessions: usize,
    pub diagnosis: String,
    pub top_sessions: Vec<PromptComplexitySession>,
}

pub fn analyze(
    sessions: &[HistoricalSession],
    traces: &HashMap<String, SessionTrace>,
) -> PromptComplexityReport {
    let mut scored = Vec::new();

    for session in sessions {
        let Some(trace) = traces.get(&session.id) else {
            continue;
        };
        let Some(prompt) = trace.first_prompt.as_deref() else {
            continue;
        };
        let complexity = complexity_score(prompt);
        let specificity = specificity_score(prompt);
        scored.push(PromptComplexitySession {
            session_id: session.id.clone(),
            project: session.project.clone(),
            complexity_score: complexity,
            specificity_score: specificity,
            label: complexity_label(complexity),
            preview: bounded_prompt_preview(prompt),
        });
    }

    if scored.is_empty() {
        return PromptComplexityReport {
            available: false,
            sessions_analyzed: sessions.len(),
            prompts_analyzed: 0,
            avg_complexity_score: 0.0,
            avg_specificity_score: 0.0,
            high_complexity_sessions: 0,
            low_specificity_sessions: 0,
            diagnosis: "No prompt previews available yet.".to_string(),
            top_sessions: Vec::new(),
        };
    }

    let prompts_analyzed = scored.len();
    let avg_complexity_score = scored
        .iter()
        .map(|s| s.complexity_score as f64)
        .sum::<f64>()
        / prompts_analyzed as f64;
    let avg_specificity_score = scored
        .iter()
        .map(|s| s.specificity_score as f64)
        .sum::<f64>()
        / prompts_analyzed as f64;
    let high_complexity_sessions = scored.iter().filter(|s| s.complexity_score >= 70).count();
    let low_specificity_sessions = scored.iter().filter(|s| s.specificity_score < 45).count();

    scored.sort_by(|a, b| {
        b.complexity_score
            .cmp(&a.complexity_score)
            .then(a.specificity_score.cmp(&b.specificity_score))
    });

    let diagnosis = if avg_specificity_score < 45.0 {
        "Prompts skew broad. Add file paths, line numbers, concrete errors, or exact commands to cut search overhead."
            .to_string()
    } else if avg_complexity_score >= 70.0 {
        "Prompts are detailed and multi-constraint. Good for hard tasks, but split giant asks into smaller turns when possible."
            .to_string()
    } else {
        "Prompt complexity looks balanced. Specific enough for routing, not wildly over-scoped."
            .to_string()
    };

    PromptComplexityReport {
        available: true,
        sessions_analyzed: sessions.len(),
        prompts_analyzed,
        avg_complexity_score,
        avg_specificity_score,
        high_complexity_sessions,
        low_specificity_sessions,
        diagnosis,
        top_sessions: scored.into_iter().take(8).collect(),
    }
}

fn bounded_prompt_preview(prompt: &str) -> String {
    let normalized = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= MAX_PROMPT_PREVIEW_CHARS {
        return normalized;
    }

    let mut preview = normalized
        .chars()
        .take(MAX_PROMPT_PREVIEW_CHARS - 1)
        .collect::<String>();
    preview.push('…');
    preview
}

pub(crate) fn complexity_score(prompt: &str) -> u8 {
    let lower = prompt.to_ascii_lowercase();
    let word_count = prompt.split_whitespace().count();
    let newline_count = prompt.lines().count();
    let file_refs = count_file_refs(prompt);
    let mut score = ((word_count as f64 / 45.0) * 35.0).round() as i32;

    if newline_count >= 3 {
        score += 10;
    }
    if prompt.contains("```") || prompt.contains('`') {
        score += 10;
    }
    if file_refs > 0 {
        score += 12;
    }
    if has_line_reference(&lower) {
        score += 10;
    }
    if has_error_signal(&lower) {
        score += 8;
    }
    if has_command_signal(&lower) {
        score += 8;
    }
    if has_multi_constraint_signal(&lower) {
        score += 12;
    }

    score.clamp(0, 100) as u8
}

pub(crate) fn specificity_score(prompt: &str) -> u8 {
    let lower = prompt.to_ascii_lowercase();
    let mut score = 20i32;

    let file_refs = count_file_refs(prompt);
    score += (file_refs.min(3) as i32) * 15;
    if has_line_reference(&lower) {
        score += 15;
    }
    if has_command_signal(&lower) {
        score += 10;
    }
    if has_error_signal(&lower) {
        score += 10;
    }
    if lower.contains("root cause") || lower.contains("exact") || lower.contains("specific") {
        score += 10;
    }
    if lower.contains("something")
        || lower.contains("whatever")
        || lower.contains("take a look")
        || lower.contains("help me")
    {
        score -= 15;
    }

    score.clamp(0, 100) as u8
}

fn count_file_refs(prompt: &str) -> usize {
    prompt
        .split_whitespace()
        .filter(|token| {
            token.contains('/')
                || token.contains('\\')
                || token.ends_with(".rs")
                || token.ends_with(".ts")
                || token.ends_with(".tsx")
                || token.ends_with(".js")
                || token.ends_with(".svelte")
                || token.ends_with(".json")
                || token.ends_with(".toml")
        })
        .count()
}

fn has_line_reference(lower: &str) -> bool {
    lower.contains(" line ")
        || lower.contains("line:")
        || lower.contains("#l")
        || lower.contains("lines ")
}

fn has_error_signal(lower: &str) -> bool {
    lower.contains("error")
        || lower.contains("exception")
        || lower.contains("stack trace")
        || lower.contains("failed")
        || lower.contains("panic")
}

fn has_command_signal(lower: &str) -> bool {
    lower.contains("cargo ")
        || lower.contains("npm ")
        || lower.contains("pnpm ")
        || lower.contains("bun ")
        || lower.contains("pytest")
        || lower.contains("clippy")
}

fn has_multi_constraint_signal(lower: &str) -> bool {
    lower.contains("1.")
        || lower.contains("2.")
        || lower.contains("- ")
        || lower.contains("must")
        || lower.contains("also")
        || lower.contains("while")
}

fn complexity_label(score: u8) -> &'static str {
    match score {
        0..=34 => "Low",
        35..=59 => "Moderate",
        60..=79 => "High",
        _ => "Extreme",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_with_paths_and_errors_scores_high() {
        let prompt = "Fix JWT validation in src/auth/validate.ts line 42. cargo test fails with panic: invalid signature";
        assert!(complexity_score(prompt) >= 60);
        assert!(specificity_score(prompt) >= 60);
    }

    #[test]
    fn vague_prompt_scores_low_specificity() {
        let prompt = "help me with this weird bug";
        assert!(specificity_score(prompt) < 45);
    }

    #[test]
    fn prompt_preview_is_single_line_bounded_and_omits_the_tail() {
        let prompt = format!("Start\nwith\tcontext {} SECRET_TAIL", "x".repeat(400));
        let preview = bounded_prompt_preview(&prompt);

        assert!(preview.chars().count() <= 240);
        assert!(!preview.contains('\n'));
        assert!(!preview.contains('\t'));
        assert!(!preview.contains("SECRET_TAIL"));
        assert!(preview.ends_with('…'));
    }
}
