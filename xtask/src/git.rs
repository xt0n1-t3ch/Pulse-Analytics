//! Thin wrapper over the `git` executable.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

pub struct GitOutput {
    pub success: bool,
    /// Non-empty trimmed lines of stdout.
    pub lines: Vec<String>,
    /// stdout then stderr, for error messages.
    pub text: String,
}

/// Runs `git [-C dir] args`. A non-zero exit is reported in the result, not as an error.
pub fn run(dir: Option<&Path>, args: &[&str]) -> Result<GitOutput> {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    let output = command
        .args(args)
        .output()
        .with_context(|| format!("failed to start git {}", args.join(" ")))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(GitOutput {
        success: output.status.success(),
        lines: stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
        text: format!("{}{}", stdout.trim_end(), stderr.trim_end()),
    })
}

/// Like [`run`], but a non-zero exit is an error: `git <args> failed: <output>`.
pub fn run_checked(dir: Option<&Path>, args: &[&str]) -> Result<Vec<String>> {
    let output = run(dir, args)?;
    if !output.success {
        bail!("git {} failed: {}", args.join(" "), output.text);
    }
    Ok(output.lines)
}

/// First line of a checked git command.
pub fn first_line(dir: Option<&Path>, args: &[&str]) -> Result<String> {
    run_checked(dir, args)?
        .into_iter()
        .next()
        .with_context(|| format!("git {} printed nothing", args.join(" ")))
}

/// First line of a checked git command, lowercased: a commit id.
pub fn first_line_lower(dir: Option<&Path>, args: &[&str]) -> Result<String> {
    first_line(dir, args).map(|line| line.to_ascii_lowercase())
}
