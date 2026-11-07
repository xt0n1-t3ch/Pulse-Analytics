//! Turns raw tool targets (paths and commands) into short readable labels for presence text.

use std::path::Path;

pub fn readable_file_target(raw: &str, max_len: usize) -> String {
    let cleaned = trim_shell_quotes(raw);
    if cleaned.is_empty() {
        return String::new();
    }

    let cleaned = strip_session_context(cleaned);
    let file_label = Path::new(cleaned)
        .file_name()
        .and_then(|item| item.to_str())
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .unwrap_or(cleaned);
    truncate_activity_target(strip_session_context(file_label).to_string(), max_len)
}

pub fn truncate_activity_target(input: String, max_len: usize) -> String {
    if input.chars().count() <= max_len {
        return input;
    }
    if max_len <= 3 {
        return input.chars().take(max_len).collect();
    }

    let prefix: String = input.chars().take(max_len - 3).collect();
    format!("{prefix}...")
}

fn trim_shell_quotes(raw: &str) -> &str {
    raw.trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('`')
        .trim()
}

fn strip_session_context(value: &str) -> &str {
    let trimmed = value.trim();
    if let Some((file, context)) = trimmed.split_once(" - ")
        && should_strip_dash_context(file, context)
    {
        return file.trim();
    }

    if let Some((file, context)) = trimmed.rsplit_once(" (")
        && context.ends_with(')')
        && is_file_like_label(file)
    {
        return file.trim();
    }

    trimmed
}

fn should_strip_dash_context(file: &str, context: &str) -> bool {
    is_file_like_label(file) && context.contains('(') && context.trim_end().ends_with(')')
}

fn is_file_like_label(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && !trimmed.contains("://")
        && !trimmed.contains('\n')
        && !trimmed.contains('\r')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_target_drops_project_and_branch_suffix() {
        assert_eq!(
            readable_file_target(
                "channel-events.ts - PropertyAlpha-Agent (feat/marketplace-addtochat-liveview-management)",
                72,
            ),
            "channel-events.ts"
        );
    }

    #[test]
    fn file_target_drops_bare_branch_suffix() {
        assert_eq!(
            readable_file_target(
                "channel-events.ts (feat/marketplace-addtochat-liveview-management)",
                72
            ),
            "channel-events.ts"
        );
    }

    #[test]
    fn file_target_keeps_plain_filename() {
        assert_eq!(readable_file_target("src/main.rs", 72), "main.rs");
    }
}
