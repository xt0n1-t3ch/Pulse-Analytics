//! Child processes that Pulse starts and must reap on every exit path.
//!
//! A plain `Child::kill` ends only the direct child. Provider CLIs are often
//! shims (`cmd.exe` running an npm script, or a Node launcher that starts the
//! native binary), so the real worker is a grandchild. Windows children run in
//! a kill-on-close job object; Unix children lead their own process group, and
//! Linux children also get SIGTERM when the process that spawned them dies.

use std::io;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus};

pub struct OwnedChild {
    child: Child,
    direct_reaped: bool,
    terminated: bool,
    #[cfg(windows)]
    job: Option<job::Job>,
}

impl OwnedChild {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
            #[cfg(target_os = "linux")]
            {
                let parent = libc::pid_t::try_from(std::process::id()).unwrap_or(0);
                // SAFETY: the closure runs between fork and exec and calls only
                // `prctl` and `getppid`, which are async-signal-safe.
                unsafe {
                    command.pre_exec(move || die_with_parent(parent));
                }
            }
        }
        let child = command.spawn()?;
        #[cfg(windows)]
        let job = match job::Job::kill_on_close().and_then(|job| job.assign(&child).map(|()| job)) {
            Ok(job) => Some(job),
            Err(error) => {
                tracing::warn!(%error, "child process tree is not contained by a job object");
                None
            }
        };
        Ok(Self {
            child,
            direct_reaped: false,
            terminated: false,
            #[cfg(windows)]
            job,
        })
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        self.direct_reaped |= status.is_some();
        Ok(status)
    }

    /// Kills every process in the tree and reaps the direct child. Idempotent.
    pub fn terminate(&mut self) {
        if self.terminated {
            return;
        }
        self.terminated = true;
        #[cfg(windows)]
        if let Some(job) = self.job.as_ref() {
            job.terminate();
        }
        // A reaped pid may already belong to an unrelated process group.
        #[cfg(unix)]
        if !self.direct_reaped {
            kill_process_group(self.child.id());
        }
        if !self.direct_reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.direct_reaped = true;
        }
        #[cfg(windows)]
        {
            self.job = None;
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// Asks the kernel to send SIGTERM to this child when its parent dies, so a
/// crash or `panic = "abort"` cannot leave provider CLIs running. The signal
/// follows the thread that spawned the child, which is the supervising thread
/// of every `OwnedChild` in Pulse.
///
/// Fails when the parent is already gone: the signal would never come.
#[cfg(target_os = "linux")]
fn die_with_parent(expected_parent: libc::pid_t) -> io::Result<()> {
    // SAFETY: `PR_SET_PDEATHSIG` takes a signal number as its only argument.
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `getppid` has no preconditions.
    if unsafe { libc::getppid() } != expected_parent {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
    }
    Ok(())
}

#[cfg(unix)]
fn kill_process_group(pgid: u32) {
    if let Ok(pgid) = libc::pid_t::try_from(pgid)
        && pgid > 0
    {
        // SAFETY: `kill` has no memory-safety preconditions; a negative pid
        // addresses the group this child leads.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
mod job {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    pub(super) struct Job(HANDLE);

    // SAFETY: a job object handle is a kernel handle that any thread may use.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub(super) fn kill_on_close() -> io::Result<Self> {
            // SAFETY: null attributes and name request an anonymous job.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(handle);
            // SAFETY: the struct is plain data; all-zero is a valid value.
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: the pointer and length describe `info` exactly.
            let ok = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }

        pub(super) fn assign(&self, child: &Child) -> io::Result<()> {
            // SAFETY: both handles are open for the duration of the call.
            let ok = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub(super) fn terminate(&self) {
            // SAFETY: the handle is open until `Drop`.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }

        #[cfg(test)]
        pub(super) fn active_processes(&self) -> u32 {
            use windows_sys::Win32::System::JobObjects::{
                JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
                QueryInformationJobObject,
            };
            // SAFETY: plain data; the query fills it.
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
            // SAFETY: the pointer and length describe `info` exactly.
            let ok = unsafe {
                QueryInformationJobObject(
                    self.0,
                    JobObjectBasicAccountingInformation,
                    (&raw mut info).cast(),
                    size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                )
            };
            assert_ne!(ok, 0, "job accounting query failed");
            info.ActiveProcesses
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle was returned by CreateJobObjectW and is
            // closed exactly once. Closing it kills any remaining member.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    fn long_running_tree() -> Command {
        #[cfg(windows)]
        {
            // cmd.exe stays the direct child; ping is the grandchild that a
            // plain `Child::kill` would orphan.
            let mut command = crate::util::silent_command("cmd.exe");
            command.args(["/d", "/c", "ping -n 60 127.0.0.1 >nul"]);
            command
        }
        #[cfg(not(windows))]
        {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 60 & wait"]);
            command
        }
    }

    #[test]
    fn terminate_reaps_the_direct_child_and_is_idempotent() {
        let mut child = OwnedChild::spawn(
            long_running_tree()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .expect("spawn fixture");
        child.terminate();
        assert!(child.direct_reaped);
        child.terminate();
        assert!(
            child.child.try_wait().expect("try_wait").is_some(),
            "the direct child must be reaped"
        );
    }

    #[cfg(windows)]
    #[test]
    fn terminating_the_job_ends_the_grandchild_too() {
        let mut child = OwnedChild::spawn(
            long_running_tree()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .expect("spawn fixture");
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.job.as_ref().expect("job").active_processes() < 2 {
            assert!(Instant::now() < deadline, "grandchild never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        child.job.as_ref().expect("job").terminate();
        let _ = child.child.wait();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.job.as_ref().expect("job").active_processes() > 0 {
            assert!(
                Instant::now() < deadline,
                "a job member survived termination"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        child.direct_reaped = true;
        drop(child);
    }

    #[cfg(unix)]
    #[test]
    fn dropping_the_guard_ends_the_process_group() {
        let child = OwnedChild::spawn(
            long_running_tree()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .expect("spawn fixture");
        let pgid = child.child.id() as libc::pid_t;
        drop(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        // Signal 0 probes for any live member of the group.
        // SAFETY: `kill` has no memory-safety preconditions.
        while unsafe { libc::kill(-pgid, 0) } == 0 {
            assert!(Instant::now() < deadline, "a group member survived");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn die_with_parent_arms_sigterm_and_rejects_a_wrong_parent() {
        fn pdeathsig() -> libc::c_int {
            let mut signal: libc::c_int = -1;
            // SAFETY: `PR_GET_PDEATHSIG` writes one int through the pointer.
            let rc = unsafe { libc::prctl(libc::PR_GET_PDEATHSIG, &raw mut signal) };
            assert_eq!(rc, 0);
            signal
        }
        // SAFETY: `getppid` has no preconditions.
        let parent = unsafe { libc::getppid() };
        assert_eq!(pdeathsig(), 0);
        die_with_parent(parent).expect("the parent is still alive");
        assert_eq!(pdeathsig(), libc::SIGTERM);
        let error = die_with_parent(parent + 1).expect_err("not our parent");
        assert_eq!(error.raw_os_error(), Some(libc::ESRCH));
        // SAFETY: clears the signal this test armed on its own thread.
        unsafe {
            libc::prctl(libc::PR_SET_PDEATHSIG, 0 as libc::c_ulong);
        }
    }
}
