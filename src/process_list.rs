//! The running processes, read in-process: `/proc` on Linux, a Toolhelp
//! snapshot on Windows and libproc on macOS. Nothing here launches
//! `tasklist`, PowerShell or `ps`.

/// One running process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningProcess {
    pub pid: u32,
    /// Zero where the platform reader does not report it (macOS).
    pub parent_pid: u32,
    /// The executable name: `Codex.exe` on Windows, `Codex` elsewhere.
    pub name: String,
    /// The file name of `argv[0]` on Linux, where `comm` can be a launcher or
    /// thread name instead of the executable.
    pub argv0_name: Option<String>,
}

impl RunningProcess {
    /// Every name the process is known by.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.name.as_str()).chain(self.argv0_name.as_deref())
    }
}

/// Every process the current user can see.
pub fn snapshot() -> Vec<RunningProcess> {
    imp::snapshot()
}

/// Whether a process with this id is still running.
pub fn is_alive(pid: u32) -> bool {
    imp::is_alive(pid)
}

/// Ends a process at once; true when it was ended or had already exited.
#[cfg(windows)]
pub fn terminate(pid: u32) -> bool {
    imp::terminate(pid)
}

/// The full executable path of a process, where the platform provides it.
#[cfg(windows)]
pub fn executable_path(pid: u32) -> Option<String> {
    imp::executable_path(pid)
}

/// Reads a `/proc`-shaped directory. Entries that vanish while it is read are skipped.
#[cfg(any(target_os = "linux", test))]
fn from_proc(root: &std::path::Path) -> Vec<RunningProcess> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let dir = entry.path();
            let name = std::fs::read_to_string(dir.join("comm")).ok()?;
            let argv0_name = std::fs::read(dir.join("cmdline")).ok().and_then(|cmdline| {
                let argv0 = cmdline.split(|byte| *byte == 0).next().unwrap_or_default();
                let argv0 = String::from_utf8_lossy(argv0);
                let base = argv0.rsplit('/').next().unwrap_or_default();
                (!base.is_empty()).then(|| base.to_string())
            });
            Some(RunningProcess {
                pid,
                parent_pid: std::fs::read_to_string(dir.join("stat"))
                    .ok()
                    .and_then(|stat| parent_from_stat(&stat))
                    .unwrap_or(0),
                name: name.trim_end_matches('\n').to_string(),
                argv0_name,
            })
        })
        .collect()
}

/// `/proc/<pid>/stat` is `pid (comm) state ppid ...`; `comm` may contain
/// spaces and parentheses, so the fields after the last `)` are counted.
#[cfg(any(target_os = "linux", test))]
fn parent_from_stat(stat: &str) -> Option<u32> {
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(target_os = "linux")]
mod imp {
    use super::RunningProcess;

    pub fn snapshot() -> Vec<RunningProcess> {
        super::from_proc(std::path::Path::new("/proc"))
    }

    pub fn is_alive(pid: u32) -> bool {
        crate::util::process_is_alive(pid)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::RunningProcess;
    use std::ffi::c_int;

    /// Room for processes that start between the count and the listing.
    const SPARE_SLOTS: usize = 64;

    pub fn snapshot() -> Vec<RunningProcess> {
        // SAFETY: a null buffer asks libproc for the current process count.
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        let Ok(count) = usize::try_from(count) else {
            return Vec::new();
        };
        let mut pids: Vec<libc::pid_t> = vec![0; count + SPARE_SLOTS];
        let bytes = c_int::try_from(pids.len() * size_of::<libc::pid_t>()).unwrap_or(c_int::MAX);
        // SAFETY: libproc writes at most `bytes` bytes, the size of `pids`.
        let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        let Ok(filled) = usize::try_from(filled) else {
            return Vec::new();
        };
        pids.truncate(filled.min(pids.len()));
        pids.into_iter()
            .filter(|pid| *pid > 0)
            .filter_map(|pid| {
                let mut buffer = [0u8; 256];
                // SAFETY: proc_name writes at most `buffer.len()` bytes into `buffer`.
                let length = unsafe {
                    libc::proc_name(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32)
                };
                let length = usize::try_from(length).ok().filter(|length| *length > 0)?;
                Some(RunningProcess {
                    pid: u32::try_from(pid).ok()?,
                    parent_pid: 0,
                    name: String::from_utf8_lossy(&buffer[..length.min(buffer.len())]).into_owned(),
                    argv0_name: None,
                })
            })
            .collect()
    }

    pub fn is_alive(pid: u32) -> bool {
        crate::util::process_is_alive(pid)
    }
}

#[cfg(windows)]
mod imp {
    use super::RunningProcess;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, STILL_ACTIVE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess,
    };

    pub fn snapshot() -> Vec<RunningProcess> {
        // SAFETY: the handle is checked before use and closed once, below.
        let handle = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if handle == INVALID_HANDLE_VALUE {
            return Vec::new();
        }
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut processes = Vec::new();
        // SAFETY: `entry` is a valid PROCESSENTRY32W with `dwSize` set, as the API requires.
        let mut more = unsafe { Process32FirstW(handle, &mut entry) } != 0;
        while more {
            let length = entry
                .szExeFile
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(entry.szExeFile.len());
            processes.push(RunningProcess {
                pid: entry.th32ProcessID,
                parent_pid: entry.th32ParentProcessID,
                name: String::from_utf16_lossy(&entry.szExeFile[..length]),
                argv0_name: None,
            });
            // SAFETY: same snapshot handle and entry as above.
            more = unsafe { Process32NextW(handle, &mut entry) } != 0;
        }
        // SAFETY: `handle` came from CreateToolhelp32Snapshot and is closed once.
        unsafe { CloseHandle(handle) };
        processes
    }

    pub fn is_alive(pid: u32) -> bool {
        // SAFETY: OpenProcess returns null on failure; a non-null handle is closed below.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        // SAFETY: `handle` is a live process handle and `code` is a valid out pointer.
        let read = unsafe { GetExitCodeProcess(handle, &mut code) } != 0;
        // SAFETY: closes the handle opened above, once.
        unsafe { CloseHandle(handle) };
        read && code == STILL_ACTIVE as u32
    }

    pub fn terminate(pid: u32) -> bool {
        // SAFETY: OpenProcess returns null on failure; a non-null handle is closed below.
        let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
        if handle.is_null() {
            return !is_alive(pid);
        }
        // SAFETY: `handle` was opened with PROCESS_TERMINATE.
        let ended = unsafe { TerminateProcess(handle, 1) } != 0;
        // SAFETY: closes the handle opened above, once.
        unsafe { CloseHandle(handle) };
        ended || !is_alive(pid)
    }

    pub fn executable_path(pid: u32) -> Option<String> {
        // SAFETY: OpenProcess returns null on failure; a non-null handle is closed below.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        // SAFETY: `length` holds the buffer capacity in UTF-16 units, as the API requires.
        let read = unsafe {
            QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut length)
        } != 0;
        // SAFETY: closes the handle opened above, once.
        unsafe { CloseHandle(handle) };
        read.then(|| String::from_utf16_lossy(&buffer[..length as usize]))
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
mod imp {
    use super::RunningProcess;

    pub fn snapshot() -> Vec<RunningProcess> {
        Vec::new()
    }

    pub fn is_alive(pid: u32) -> bool {
        crate::util::process_is_alive(pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn fake_process(root: &Path, pid: &str, comm: &str, argv: &[&str], ppid: u32) {
        let dir = root.join(pid);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
        let mut cmdline = argv.join("\0").into_bytes();
        cmdline.push(0);
        fs::write(dir.join("cmdline"), cmdline).unwrap();
        fs::write(dir.join("stat"), format!("{pid} ({comm}) S {ppid} 1 1 0")).unwrap();
    }

    #[test]
    fn proc_reader_reports_comm_argv0_and_parent() {
        let proc = tempfile::tempdir().unwrap();
        fake_process(
            proc.path(),
            "303",
            "node",
            &["/usr/lib/opencode/opencode-cli", "run"],
            7,
        );

        assert_eq!(
            from_proc(proc.path()),
            [RunningProcess {
                pid: 303,
                parent_pid: 7,
                name: "node".into(),
                argv0_name: Some("opencode-cli".into()),
            }]
        );
    }

    #[test]
    fn proc_reader_skips_non_pid_entries_and_vanished_processes() {
        let proc = tempfile::tempdir().unwrap();
        fake_process(proc.path(), "self", "opencode", &["opencode"], 1);
        fs::write(proc.path().join("cpuinfo"), "opencode").unwrap();
        fs::create_dir(proc.path().join("600")).unwrap();

        assert!(from_proc(proc.path()).is_empty());
        assert!(from_proc(&proc.path().join("missing")).is_empty());
    }

    #[test]
    fn parent_pid_survives_spaces_and_parentheses_in_comm() {
        assert_eq!(
            parent_from_stat("42 (Web (Content) x) S 17 42 42 0"),
            Some(17)
        );
        assert_eq!(parent_from_stat("garbage"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_live_snapshot_contains_this_test() {
        let me = std::process::id();
        assert!(snapshot().iter().any(|process| process.pid == me));
        assert!(is_alive(me));
    }
}
