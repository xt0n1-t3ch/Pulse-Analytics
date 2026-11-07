//! Detects the OpenCode and Codex desktop processes from one in-process list
//! (see `crate::process_list`); nothing here launches another program.

use crate::process_list::{self, RunningProcess};

pub fn is_opencode_running() -> bool {
    opencode_in(&process_list::snapshot())
}

pub fn is_codex_app_running() -> bool {
    codex_app_in(&process_list::snapshot())
}

pub fn is_desktop_surface_running() -> bool {
    let processes = process_list::snapshot();
    opencode_in(&processes) || codex_app_in(&processes)
}

/// The OpenCode app or CLI. Exact and case-insensitive, so `opencode-helper`
/// and `not-opencode` do not count.
pub fn opencode_in(processes: &[RunningProcess]) -> bool {
    processes
        .iter()
        .any(|process| process.names().any(is_opencode_name))
}

/// The Codex desktop app. Case-sensitive, so the `codex` CLI does not count.
pub fn codex_app_in(processes: &[RunningProcess]) -> bool {
    processes
        .iter()
        .any(|process| process.names().any(is_codex_app_name))
}

fn is_opencode_name(name: &str) -> bool {
    ["opencode", "opencode-cli"]
        .iter()
        .any(|expected| without_exe(name).eq_ignore_ascii_case(expected))
}

fn is_codex_app_name(name: &str) -> bool {
    name == "Codex" || name == "Codex.exe"
}

fn without_exe(name: &str) -> &str {
    name.rsplit_once('.')
        .filter(|(_, extension)| extension.eq_ignore_ascii_case("exe"))
        .map_or(name, |(stem, _)| stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(name: &str, argv0_name: Option<&str>) -> RunningProcess {
        RunningProcess {
            pid: 1,
            parent_pid: 0,
            name: name.into(),
            argv0_name: argv0_name.map(Into::into),
        }
    }

    #[test]
    fn opencode_is_found_by_name_or_argv0_on_every_platform() {
        for found in [
            process("OpenCode", None),
            process("OpenCode.exe", None),
            process("opencode-cli.exe", None),
            process("node", Some("opencode-cli")),
        ] {
            assert!(
                opencode_in(&[process("bash", None), found.clone()]),
                "{found:?}"
            );
        }
    }

    #[test]
    fn partial_opencode_names_do_not_count() {
        let processes = [
            process("opencode-helper", None),
            process("not-opencode.exe", None),
            process("opencode-helper.exe", None),
        ];
        assert!(!opencode_in(&processes));
    }

    #[test]
    fn only_the_codex_desktop_app_counts_not_the_cli() {
        assert!(!codex_app_in(&[
            process("codex", None),
            process("codex.exe", None)
        ]));
        assert!(codex_app_in(&[process("Codex", None)]));
        assert!(codex_app_in(&[process("Codex.exe", None)]));
        assert!(!codex_app_in(&[process("Codex Helper", None)]));
    }
}
