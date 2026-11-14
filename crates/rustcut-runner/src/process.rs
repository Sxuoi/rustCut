//! Inspecting and terminating sidecar processes by PID: the journaled-PID cleanup on the next
//! launch (PRD §7.8, macOS) and the "no orphaned FFmpeg" checks (PRD §8.2).

use std::io;

use crate::{Sidecars, platform};

/// What [`kill_stale_sidecar`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleProcess {
    /// No process with that PID is running; nothing was done.
    NotRunning,
    /// The PID belongs to a program other than the sidecars (the PID was reused, or the process
    /// belongs to another user); it was left alone.
    NotASidecar,
    /// The process was one of the sidecars and has been killed.
    Killed,
}

/// Whether a process with this PID is currently running. Exited processes (including
/// not-yet-reaped zombies on Linux) count as not running.
pub fn is_process_running(pid: u32) -> bool {
    platform::is_running(pid)
}

/// Kills a sidecar left over from an earlier session, typically a PID journaled from
/// `run_streaming`'s `on_spawn` callback on macOS, where the OS cannot tie children to the app
/// (PRD §7.8).
///
/// The process is killed only if its executable is [`Sidecars::ffmpeg`] or
/// [`Sidecars::ffprobe`], so a PID that was recycled by an unrelated program is never touched.
///
/// # Errors
/// The OS error if the process could be identified as a sidecar but not be inspected or killed.
pub fn kill_stale_sidecar(pid: u32, sidecars: &Sidecars) -> io::Result<StaleProcess> {
    platform::kill_if(pid, |exe| sidecars.is_sidecar_path(exe))
}
