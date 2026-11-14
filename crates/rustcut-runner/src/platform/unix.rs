//! Unix: `PR_SET_PDEATHSIG` on Linux so FFmpeg dies with the app (PRD §7.8); PID inspection
//! for journaled children (macOS has no OS mechanism to tie children to the app).

use std::io;
use std::path::{Path, PathBuf};

use tokio::process::{Child, Command};

use crate::StaleProcess;

/// Linux: the child receives `SIGKILL` when the spawning thread (and so the app) dies.
#[cfg(target_os = "linux")]
pub(crate) fn configure(command: &mut Command) {
    // SAFETY: getpid(2) has no preconditions.
    let parent = unsafe { libc::getpid() };
    // SAFETY: the closure runs in the forked child before exec, where only async-signal-safe
    // operations are allowed. It calls prctl(2) and getppid(2) and builds `io::Error`s from raw
    // errno values, none of which allocates or takes a lock.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) == -1 {
                return Err(io::Error::last_os_error());
            }
            // The app died between fork and prctl: the signal will never come, so give up.
            if libc::getppid() != parent {
                return Err(io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        });
    }
}

/// macOS and other Unix systems: nothing to configure; callers journal the PID instead.
#[cfg(not(target_os = "linux"))]
pub(crate) fn configure(_command: &mut Command) {}

/// Nothing to do after spawning on Unix.
pub(crate) fn adopt_child(_child: &Child) -> io::Result<()> {
    Ok(())
}

pub(crate) fn is_running(pid: u32) -> bool {
    to_pid(pid).is_some_and(running)
}

pub(crate) fn kill_if(pid: u32, is_sidecar: impl Fn(&Path) -> bool) -> io::Result<StaleProcess> {
    let Some(pid) = to_pid(pid) else {
        return Ok(StaleProcess::NotRunning);
    };
    if !running(pid) {
        return Ok(StaleProcess::NotRunning);
    }
    let exe = match executable_path(pid) {
        Ok(exe) => exe,
        Err(_) if !running(pid) => return Ok(StaleProcess::NotRunning),
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            return Ok(StaleProcess::NotASidecar);
        }
        Err(err) => return Err(err),
    };
    if !is_sidecar(&exe) {
        return Ok(StaleProcess::NotASidecar);
    }
    // SAFETY: kill(2) with a positive PID signals exactly that process.
    if unsafe { libc::kill(pid, libc::SIGKILL) } == -1 {
        let err = io::Error::last_os_error();
        return if err.raw_os_error() == Some(libc::ESRCH) {
            Ok(StaleProcess::NotRunning)
        } else {
            Err(err)
        };
    }
    Ok(StaleProcess::Killed)
}

/// Only positive PIDs: `kill(0, ..)` and `kill(-1, ..)` would address process groups.
fn to_pid(pid: u32) -> Option<libc::pid_t> {
    libc::pid_t::try_from(pid).ok().filter(|pid| *pid > 0)
}

fn running(pid: libc::pid_t) -> bool {
    // SAFETY: signal 0 only checks that the (positive) PID exists and may be signalled.
    let exists = unsafe { libc::kill(pid, 0) } == 0
        || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    exists && !is_zombie(pid)
}

#[cfg(target_os = "linux")]
fn is_zombie(pid: libc::pid_t) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(')')
            .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'))
    })
}

#[cfg(not(target_os = "linux"))]
fn is_zombie(_pid: libc::pid_t) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn executable_path(pid: libc::pid_t) -> io::Result<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
}

#[cfg(target_os = "macos")]
fn executable_path(pid: libc::pid_t) -> io::Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let mut buffer = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buffer` is writable for its full length, which is passed along.
    let len = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if len <= 0 {
        return Err(io::Error::last_os_error());
    }
    buffer.truncate(len as usize);
    Ok(PathBuf::from(OsString::from_vec(buffer)))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn executable_path(_pid: libc::pid_t) -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "reading a process's executable path is not supported on this system",
    ))
}
