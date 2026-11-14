//! Windows: no console window and a kill-on-close Job Object, so FFmpeg never outlives the app
//! (PRD §7.8, §8.2).

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tokio::process::{Child, Command};
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, INVALID_HANDLE_VALUE, WAIT_TIMEOUT};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenProcess, OpenThread, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, ResumeThread, THREAD_SUSPEND_RESUME, TerminateProcess,
    WaitForSingleObject,
};

use crate::StaleProcess;

/// How long [`kill_if`] waits for a terminated process to disappear.
const KILL_WAIT_MS: u32 = 5_000;

/// The process-wide Job Object (`None` if it could not be created).
///
/// Statics are never dropped, so the handle stays open until the OS closes it when RustCut
/// exits — normally, by crash or by being killed — and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` then
/// terminates every child still in the job. The handle is not inheritable, so no child can keep
/// the job alive.
static JOB: OnceLock<Option<OwnedHandle>> = OnceLock::new();

/// No console window (PRD §7.8); suspended until [`adopt_child`] has put it into the job.
pub(crate) fn configure(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
}

/// Puts a freshly spawned, still suspended child into the job and then lets it run.
pub(crate) fn adopt_child(child: &Child) -> io::Result<()> {
    let (Some(pid), Some(process)) = (child.id(), child.raw_handle()) else {
        return Err(io::Error::other(
            "the new process vanished before it could be started",
        ));
    };
    // SAFETY: both handles are valid for the duration of the call.
    if let Some(job) = job()
        && unsafe { AssignProcessToJobObject(job.as_raw_handle(), process) } == 0
    {
        let err = io::Error::last_os_error();
        tracing::warn!(%err, "could not add a sidecar to the Job Object; it may outlive a crash");
    }
    resume_threads(pid)
}

fn job() -> Option<&'static OwnedHandle> {
    JOB.get_or_init(|| match create_job() {
        Ok(job) => Some(job),
        Err(err) => {
            tracing::warn!(%err, "could not create the Job Object; sidecars may outlive a crash");
            None
        }
    })
    .as_ref()
}

fn create_job() -> io::Result<OwnedHandle> {
    // SAFETY: null attributes and name create an anonymous job whose handle is not inheritable.
    let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is a freshly created handle that nothing else owns.
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: the struct consists of integers only, so all-zero is a valid value.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    // Kill everything when the last handle closes; never block on a crash dialog.
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
    // SAFETY: the pointer and length describe `limits`, which outlives the call.
    let ok = unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(job)
}

/// Resumes the threads of a process created with `CREATE_SUSPENDED` (std does not expose the
/// primary thread handle, so the thread is found through a Toolhelp snapshot).
fn resume_threads(pid: u32) -> io::Result<()> {
    // SAFETY: plain snapshot request; failure is reported as INVALID_HANDLE_VALUE.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is a valid snapshot handle that nothing else owns.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: the struct consists of integers only, so all-zero is a valid value.
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<THREADENTRY32>() as u32;
    let mut resumed = 0_usize;
    // SAFETY: valid snapshot handle and a properly sized entry.
    let mut more = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            resume_thread(entry.th32ThreadID)?;
            resumed += 1;
        }
        // SAFETY: as above.
        more = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    if resumed == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the new process has no thread to resume",
        ));
    }
    Ok(())
}

fn resume_thread(thread_id: u32) -> io::Result<()> {
    // SAFETY: plain Win32 call; a null handle is reported below.
    let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is a valid thread handle that nothing else owns.
    let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: valid thread handle opened with THREAD_SUSPEND_RESUME.
    if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn is_running(pid: u32) -> bool {
    match open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE) {
        Ok(process) => is_active(&process),
        // The process exists but belongs to a more privileged context.
        Err(err) => is_access_denied(&err),
    }
}

pub(crate) fn kill_if(pid: u32, is_sidecar: impl Fn(&Path) -> bool) -> io::Result<StaleProcess> {
    let access = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE;
    let process = match open_process(pid, access) {
        Ok(process) => process,
        // Our own children always grant these rights: a refusal means someone else's process.
        Err(err) if is_access_denied(&err) => return Ok(StaleProcess::NotASidecar),
        Err(_) => return Ok(StaleProcess::NotRunning),
    };
    if !is_active(&process) {
        return Ok(StaleProcess::NotRunning);
    }
    if !is_sidecar(&image_path(&process)?) {
        return Ok(StaleProcess::NotASidecar);
    }
    // SAFETY: valid process handle opened with PROCESS_TERMINATE.
    if unsafe { TerminateProcess(process.as_raw_handle(), 1) } == 0 {
        let err = io::Error::last_os_error();
        return if is_active(&process) {
            Err(err)
        } else {
            Ok(StaleProcess::NotRunning)
        };
    }
    // SAFETY: valid process handle opened with PROCESS_SYNCHRONIZE.
    unsafe { WaitForSingleObject(process.as_raw_handle(), KILL_WAIT_MS) };
    Ok(StaleProcess::Killed)
}

fn open_process(pid: u32, access: u32) -> io::Result<OwnedHandle> {
    // SAFETY: plain Win32 call; a null handle is reported below.
    let raw = unsafe { OpenProcess(access, 0, pid) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is a valid process handle that nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

/// A process handle is signalled once the process has exited.
fn is_active(process: &OwnedHandle) -> bool {
    // SAFETY: valid process handle opened with SYNCHRONIZE; a zero timeout only polls.
    unsafe { WaitForSingleObject(process.as_raw_handle(), 0) == WAIT_TIMEOUT }
}

fn is_access_denied(err: &io::Error) -> bool {
    err.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32)
}

fn image_path(process: &OwnedHandle) -> io::Result<PathBuf> {
    let mut buffer = vec![0_u16; 32 * 1024];
    let mut len = buffer.len() as u32;
    // SAFETY: `buffer` is writable for `len` UTF-16 units; `len` receives the written length.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            process.as_raw_handle(),
            PROCESS_NAME_WIN32,
            buffer.as_mut_ptr(),
            &mut len,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    buffer.truncate(len as usize);
    Ok(PathBuf::from(OsString::from_wide(&buffer)))
}
