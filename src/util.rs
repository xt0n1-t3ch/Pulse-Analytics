//! Shared helpers: silent child processes, process probes, tracing setup and number formatting.

use std::process::Command;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use tracing_subscriber::{EnvFilter, fmt};

/// Create a `Command` that won't flash a console window on Windows.
pub fn silent_command(program: &str) -> Command {
    let cmd = Command::new(program);
    #[cfg(windows)]
    let cmd = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let mut cmd = cmd;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    };
    cmd
}

/// Whether a process with this id exists, probed with `kill(pid, 0)`.
///
/// Signal 0 delivers nothing. `EPERM` means the process exists but belongs to
/// another user, so it counts as alive. Ids that do not fit a positive `pid_t`
/// are never probed: `kill` gives 0 and negative ids a process-group meaning.
#[cfg(unix)]
pub fn process_is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists and can be signalled.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub fn setup_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = fmt().with_env_filter(filter).without_time().try_init();
}

pub fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

pub fn format_delta_tokens(tokens: u64) -> String {
    format_tokens(tokens)
}

pub fn format_token_triplet(_delta: Option<u64>, last: Option<u64>, total: Option<u64>) -> String {
    let mut parts = Vec::new();
    if let Some(value) = last {
        parts.push(format!("Last response {}", format_tokens(value)));
    }
    if let Some(value) = total {
        parts.push(format!("Session total {}", format_tokens(value)));
    }
    if parts.is_empty() {
        "Tokens: unavailable".to_string()
    } else {
        format!("Tokens: {}", parts.join(" | "))
    }
}

pub fn format_time_until(target: Option<DateTime<Utc>>) -> String {
    let Some(target) = target else {
        return "n/a".to_string();
    };

    let now = Utc::now();
    if target <= now {
        return "now".to_string();
    }

    let delta = (target - now).to_std().unwrap_or_default();
    human_duration(delta)
}

pub fn human_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    let seconds = secs % 60;

    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

pub fn progress_bar(percent: f64, width: usize) -> String {
    let pct = percent.clamp(0.0, 100.0);
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("{}{}", "#".repeat(filled), "-".repeat(empty))
}

pub fn truncate(input: &str, max_len: usize) -> String {
    let char_count = input.chars().count();
    if char_count <= max_len {
        return input.to_string();
    }
    if max_len <= 3 {
        return input.chars().take(max_len).collect();
    }
    let truncated: String = input.chars().take(max_len - 3).collect();
    format!("{truncated}...")
}

pub fn now_local() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn format_cost(cost: f64) -> String {
    format!("${:.2}", if cost.is_finite() { cost.max(0.0) } else { 0.0 })
}

pub fn format_time_until_reset(reset_time: DateTime<Utc>) -> String {
    let duration = reset_time.signed_duration_since(Utc::now());
    if duration.num_seconds() < 0 {
        return "now".to_string();
    }
    human_duration(duration.to_std().unwrap_or_default())
}

/// Decodes text a Windows command printed: UTF-16LE when it starts with a
/// byte-order mark or its first characters interleave NUL bytes (as `wsl.exe`
/// prints), UTF-8 otherwise. A trailing odd byte is dropped.
#[cfg(any(windows, test))]
pub fn decode_windows_text_output(bytes: &[u8]) -> String {
    /// UTF-16 characters inspected for a NUL high byte before choosing UTF-16.
    const UTF16_PROBE_CHARS: usize = 64;
    let has_interleaved_nuls = bytes
        .iter()
        .skip(1)
        .step_by(2)
        .take(UTF16_PROBE_CHARS)
        .any(|byte| *byte == 0);
    if bytes.starts_with(&[0xFF, 0xFE]) || has_interleaved_nuls {
        let (pairs, _odd) = bytes.as_chunks::<2>();
        let utf16: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
        return String::from_utf16_lossy(&utf16);
    }
    String::from_utf8_lossy(bytes).to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn windows_command_output_decodes_utf16_and_utf8() {
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("Ubuntu\r\n".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(decode_windows_text_output(&utf16), "\u{feff}Ubuntu\r\n");

        let no_bom: Vec<u8> = "C:\\".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(decode_windows_text_output(&no_bom), "C:\\");

        // A trailing odd byte is dropped, never read past the end.
        let mut odd = no_bom.clone();
        odd.push(b'x');
        assert_eq!(decode_windows_text_output(&odd), "C:\\");

        assert_eq!(
            decode_windows_text_output("plain ascii".as_bytes()),
            "plain ascii"
        );
    }

    use super::*;

    #[cfg(unix)]
    #[test]
    fn process_is_alive_sees_the_current_process() {
        assert!(process_is_alive(std::process::id()));
    }

    #[cfg(unix)]
    #[test]
    fn process_is_alive_is_false_for_a_reaped_child_and_for_pid_zero() {
        let mut child = Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn short-lived child");
        let pid = child.id();
        child.wait().expect("reap child");
        assert!(!process_is_alive(pid));
        assert!(!process_is_alive(0));
        assert!(!process_is_alive(u32::MAX));
    }

    #[test]
    fn token_formatting() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_500), "1.5K");
        assert_eq!(format_tokens(1_500_000), "1.5M");
    }

    #[test]
    fn token_triplet_formatting() {
        assert_eq!(
            format_token_triplet(Some(1500), Some(2500), Some(60_000)),
            "Tokens: Last response 2.5K | Session total 60.0K"
        );
        assert_eq!(
            format_token_triplet(None, None, None),
            "Tokens: unavailable"
        );
    }

    #[test]
    fn cost_formatting() {
        assert_eq!(format_cost(0.001), "$0.00");
        assert_eq!(format_cost(0.042), "$0.04");
        assert_eq!(format_cost(1.50), "$1.50");
        assert_eq!(format_cost(12.345), "$12.35");
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(human_duration(Duration::from_secs(45)), "45s");
        assert_eq!(human_duration(Duration::from_secs(125)), "2m 5s");
        assert_eq!(human_duration(Duration::from_secs(3700)), "1h 1m");
        assert_eq!(human_duration(Duration::from_secs(90_000)), "1d 1h");
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("a very long string here", 10), "a very ...");
        let unicode = "██████╗██╗ █████╗";
        assert_eq!(truncate(unicode, 50), unicode);
        assert_eq!(truncate(unicode, 10), "██████╗...");
    }

    #[test]
    fn progress_bar_rendering() {
        assert_eq!(progress_bar(50.0, 10), "#####-----");
        assert_eq!(progress_bar(0.0, 10), "----------");
        assert_eq!(progress_bar(100.0, 10), "##########");
    }

    #[test]
    fn provider_version_probe_uses_the_hidden_launcher() {
        let app = include_str!("app.rs");
        assert!(
            app.contains("crate::util::silent_command(program)"),
            "doctor/provider version probes must apply CREATE_NO_WINDOW on Windows"
        );
        assert!(
            !app.contains(
                "fn command_available(program: &str) -> bool {\n    Command::new(program)"
            ),
            "provider probes must not bypass the shared hidden launcher"
        );
    }
}
