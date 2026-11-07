//! `cargo xtask check`: the full local gate, one summary line per step.

use std::path::Path;
use std::process::Command;
use std::time::Instant;

use anyhow::{Result, bail};

use crate::{catalog, changelog, llms};

enum Kind {
    /// An external tool: program and arguments, run from the repository root.
    Tool(&'static str, &'static [&'static str]),
    /// A check implemented in this crate.
    Native(fn(&Path) -> Result<String>),
}

struct Step {
    id: &'static str,
    label: &'static str,
    kind: Kind,
}

/// The gate, in the order it runs: cheap format checks first, tests last.
const STEPS: [Step; 10] = [
    Step {
        id: "fmt",
        label: "cargo fmt --check",
        kind: Kind::Tool("cargo", &["fmt", "--all", "--", "--check"]),
    },
    Step {
        id: "bun-fmt",
        label: "bun run fmt:check",
        kind: Kind::Tool("bun", &["run", "fmt:check"]),
    },
    Step {
        id: "lint",
        label: "bun run lint (oxlint)",
        kind: Kind::Tool("bun", &["run", "lint"]),
    },
    Step {
        id: "typecheck",
        label: "bun run typecheck",
        kind: Kind::Tool("bun", &["run", "typecheck"]),
    },
    Step {
        id: "clippy",
        label: "cargo clippy -D warnings",
        kind: Kind::Tool(
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        ),
    },
    Step {
        id: "bun-test",
        label: "bun run test",
        kind: Kind::Tool("bun", &["run", "test"]),
    },
    Step {
        id: "nextest",
        label: "cargo nextest run --workspace",
        kind: Kind::Tool("cargo", &["nextest", "run", "--workspace"]),
    },
    Step {
        id: "changelog",
        label: "changelog check",
        kind: Kind::Native(changelog::check),
    },
    Step {
        id: "llms",
        label: "llms --check",
        kind: Kind::Native(|root| llms::run(root, true)),
    },
    Step {
        id: "catalog-parity",
        label: "catalog-parity",
        kind: Kind::Native(|root| catalog::parity(root, None)),
    },
];

/// Ids accepted by `--skip`.
pub fn step_ids() -> Vec<&'static str> {
    STEPS.iter().map(|step| step.id).collect()
}

fn run_tool(root: &Path, program: &str, arguments: &[&str]) -> Result<String> {
    let program = if program == "cargo" {
        std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
    } else {
        program.into()
    };
    let output = Command::new(&program)
        .args(arguments)
        .current_dir(root)
        .output()
        .map_err(|error| {
            anyhow::anyhow!("failed to start {}: {error}", program.to_string_lossy())
        })?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        Ok(String::new())
    } else {
        bail!("{}\n{}", output.status, text.trim_end())
    }
}

/// Runs every step not named in `skip`, prints one line each, and fails when any failed.
pub fn run(root: &Path, skip: &[String]) -> Result<()> {
    for id in skip {
        if !STEPS.iter().any(|step| step.id == id) {
            bail!("unknown step {id:?}; steps are {}", step_ids().join(", "));
        }
    }
    let (mut passed, mut failed, mut skipped) = (0, 0, 0);
    for step in &STEPS {
        if skip.iter().any(|id| id == step.id) {
            println!("SKIP  {}", step.label);
            skipped += 1;
            continue;
        }
        let started = Instant::now();
        let outcome = match &step.kind {
            Kind::Tool(program, arguments) => run_tool(root, program, arguments),
            Kind::Native(check) => check(root),
        };
        let seconds = started.elapsed().as_secs_f32();
        match outcome {
            Ok(_) => {
                println!("PASS  {} ({seconds:.1}s)", step.label);
                passed += 1;
            }
            Err(error) => {
                println!("FAIL  {} ({seconds:.1}s)", step.label);
                for line in format!("{error:#}").lines() {
                    println!("      {line}");
                }
                failed += 1;
            }
        }
    }
    println!("{passed} passed, {failed} failed, {skipped} skipped");
    if failed > 0 {
        bail!("{failed} of {} checks failed", STEPS.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_runs_in_the_documented_order() {
        assert_eq!(
            step_ids(),
            [
                "fmt",
                "bun-fmt",
                "lint",
                "typecheck",
                "clippy",
                "bun-test",
                "nextest",
                "changelog",
                "llms",
                "catalog-parity"
            ]
        );
    }

    #[test]
    fn an_unknown_skip_id_is_rejected_before_anything_runs() {
        let error = run(Path::new("."), &["nope".to_owned()]).unwrap_err();
        assert!(format!("{error}").contains("unknown step"));
    }
}
