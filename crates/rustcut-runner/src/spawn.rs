//! Starting sidecar processes so that they never outlive RustCut (PRD §7.8, §7.9, §8.2).

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::{Child, Command};

use crate::platform;

/// Optional process settings for [`spawn_tracked`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpawnOptions {
    /// Working directory of the child; `None` inherits the app's.
    pub current_dir: Option<PathBuf>,
    /// Environment variables set for this child only (the app's environment is inherited).
    pub env: Vec<(OsString, OsString)>,
}

/// Starts a sidecar with the guarantees RustCut needs for every child process.
///
/// * `program` must be an absolute path — in practice [`Sidecars::ffmpeg`] or
///   [`Sidecars::ffprobe`] — so the OS never searches `PATH` or the working directory for it
///   (PRD §7.9). A relative path is rejected with [`io::ErrorKind::InvalidInput`].
/// * `args` are passed as an argument vector, never through a shell, so Unicode, spaces and
///   quotes reach the program unmodified (EXP-08). Adding `-nostdin` is the caller's job.
/// * stdin is the null device; stdout and stderr are piped and must be read concurrently by the
///   caller ([`run_capture`] and [`run_streaming`] do that).
/// * The child is killed if the returned [`Child`] is dropped before it exits.
/// * **Windows:** created with `CREATE_NO_WINDOW` (PRD §7.8) and *suspended* until it has joined
///   one process-wide Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so there is no moment
///   at which it runs outside the job. The job handle is never closed by RustCut: when the app
///   exits for any reason, including a crash or being killed, the OS closes it and terminates
///   every FFmpeg still running (PRD §7.8, §8.2). Should the job be unavailable (very unusual),
///   the child still runs and a warning is logged.
/// * **Linux:** the child gets `PR_SET_PDEATHSIG = SIGKILL`. That signal is tied to the spawning
///   *thread*, so call this from a long-lived thread (a tokio worker or the main thread), not
///   from a short-lived `spawn_blocking` thread.
/// * **macOS:** there is no such OS mechanism: journal [`Child::id`] and call
///   [`kill_stale_sidecar`] for journaled PIDs on the next launch (PRD §7.8).
///
/// Must be called within a tokio runtime.
///
/// # Errors
/// The OS error if the process cannot be created or started (`NotFound` for a missing program),
/// or `InvalidInput` for a relative `program`.
///
/// [`Sidecars::ffmpeg`]: crate::Sidecars::ffmpeg
/// [`Sidecars::ffprobe`]: crate::Sidecars::ffprobe
/// [`run_capture`]: crate::run_capture
/// [`run_streaming`]: crate::run_streaming
/// [`kill_stale_sidecar`]: crate::kill_stale_sidecar
pub fn spawn_tracked(
    program: impl AsRef<Path>,
    args: &[OsString],
    options: &SpawnOptions,
) -> io::Result<Child> {
    let program = program.as_ref();
    if !program.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing to start \"{}\": only the located FFmpeg sidecars, given as absolute \
                 paths, may be run",
                program.display()
            ),
        ));
    }
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = &options.current_dir {
        command.current_dir(dir);
    }
    command.envs(options.env.iter().map(|(key, value)| (key, value)));
    platform::configure(&mut command);
    let child = command.spawn()?;
    // On failure `child` is dropped here, which kills it (`kill_on_drop`).
    platform::adopt_child(&child)?;
    tracing::debug!(pid = ?child.id(), program = %program.display(), "started a sidecar");
    Ok(child)
}
