//! Makes child processes (FFmpeg) die with the server, even when the server is killed hard.
//!
//! On Windows the server puts itself into a Job Object with `KILL_ON_JOB_CLOSE`; every child
//! inherits the membership, and the kernel closes the job handle when the server process ends
//! however it ends, which terminates the children. Elsewhere this is a no-op.

/// Joins the process to a kill-on-close job. Returns whether the guard is active.
pub fn install() -> bool {
    imp::install()
}

#[cfg(all(windows, target_pointer_width = "64"))]
mod imp {
    use std::ffi::c_void;

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;

    /// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` on 64-bit Windows (144 bytes); `LimitFlags` is at
    /// byte offset 16.
    #[repr(C)]
    struct ExtendedLimits {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        _pad: u32,
        rest: [u64; 15],
    }

    unsafe extern "system" {
        fn CreateJobObjectW(attrs: *const c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: i32,
            info: *const c_void,
            len: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn GetCurrentProcess() -> *mut c_void;
    }

    pub fn install() -> bool {
        const _: () = assert!(std::mem::size_of::<ExtendedLimits>() == 144);
        let limits = ExtendedLimits {
            per_process_user_time_limit: 0,
            per_job_user_time_limit: 0,
            limit_flags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            _pad: 0,
            rest: [0; 15],
        };
        // SAFETY: plain Win32 calls with valid, correctly sized arguments. The job handle is
        // deliberately never closed: it must stay open for the whole life of the process.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return false;
            }
            let set = SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                (&raw const limits).cast(),
                std::mem::size_of::<ExtendedLimits>() as u32,
            );
            set != 0 && AssignProcessToJobObject(job, GetCurrentProcess()) != 0
        }
    }
}

#[cfg(not(all(windows, target_pointer_width = "64")))]
mod imp {
    pub fn install() -> bool {
        false
    }
}
