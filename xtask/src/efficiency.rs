//! Reads a running process's power-throttling (EcoQoS) state and priority class,
//! read-only. Windows only.

use anyhow::{Result, bail};

#[cfg(not(windows))]
pub fn probe(_pid: u32) -> Result<String> {
    bail!("The Windows efficiency probe runs only on Windows")
}

#[cfg(windows)]
pub fn probe(pid: u32) -> Result<String> {
    use std::ffi::c_void;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const PROCESS_POWER_THROTTLING: i32 = 4;
    const IDLE_PRIORITY_CLASS: u32 = 0x40;

    #[repr(C)]
    #[derive(Default)]
    struct PowerThrottlingState {
        version: u32,
        control_mask: u32,
        state_mask: u32,
    }

    // SAFETY: these are the documented kernel32 signatures; kernel32 is always linked on Windows.
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetProcessInformation(
            process: *mut c_void,
            kind: i32,
            value: *mut c_void,
            size: u32,
        ) -> i32;
        fn GetPriorityClass(process: *mut c_void) -> u32;
        fn QueryFullProcessImageNameW(
            process: *mut c_void,
            flags: u32,
            name: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    /// Closes the process handle on every exit path.
    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle came from a successful OpenProcess and is closed once.
            unsafe { CloseHandle(self.0) };
        }
    }

    // SAFETY: OpenProcess takes plain values and returns null on failure.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        bail!("Unable to open the requested process for read-only inspection");
    }
    let handle = Handle(raw);

    let mut state = PowerThrottlingState {
        version: 1,
        ..Default::default()
    };
    // SAFETY: `state` is a live, correctly sized out-parameter for the power-throttling class.
    let read = unsafe {
        GetProcessInformation(
            handle.0,
            PROCESS_POWER_THROTTLING,
            std::ptr::from_mut(&mut state).cast(),
            std::mem::size_of::<PowerThrottlingState>() as u32,
        )
    };
    if read == 0 {
        bail!("Windows did not return process power-throttling state");
    }
    // SAFETY: the handle is open with query access.
    let priority = unsafe { GetPriorityClass(handle.0) };

    let mut buffer = vec![0u16; 32_768];
    let mut length = buffer.len() as u32;
    // SAFETY: `buffer` holds `length` UTF-16 units and `length` is updated to the written count.
    let named =
        unsafe { QueryFullProcessImageNameW(handle.0, 0, buffer.as_mut_ptr(), &mut length) };
    let path = if named == 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buffer[..length as usize])
    };

    let report = serde_json::json!({
        "pid": pid,
        "path": path,
        "eco_qos": state.control_mask & 1 != 0 && state.state_mask & 1 != 0,
        "idle_priority": priority == IDLE_PRIORITY_CLASS,
        "priority_class": priority,
        "control_mask": state.control_mask,
        "state_mask": state.state_mask,
    });
    Ok(serde_json::to_string_pretty(&report)?)
}
