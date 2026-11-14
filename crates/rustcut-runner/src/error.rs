//! The runner's error type.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

/// Everything that can go wrong while locating or running the FFmpeg sidecars.
///
/// Messages are specific and say what to do next; the app converts them into
/// `rustcut_model::CommandError` (AC-12).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RunnerError {
    /// No searched folder contained both `ffmpeg` and `ffprobe` (PRD §7.9).
    #[error(
        "FFmpeg could not be found: no folder contains both ffmpeg and ffprobe (searched: {}). \
         Reinstall RustCut, or set {} to the folder that contains both programs",
        .searched.join("; "),
        crate::FFMPEG_DIR_ENV
    )]
    SidecarsNotFound {
        /// The places that were searched, in search order, as human-readable text.
        searched: Vec<String>,
    },

    /// The located FFmpeg is older than the pinned minimum (PRD §7.1).
    #[error("FFmpeg {found} is too old: RustCut needs FFmpeg {required} or newer")]
    FfmpegTooOld {
        /// The version string printed by `ffmpeg -version`.
        found: String,
        /// The minimum, as `major.minor`.
        required: String,
    },

    /// A specific folder does not contain both programs.
    #[error(
        "{} does not contain both ffmpeg{exe} and ffprobe{exe} \
         (or ffmpeg-{triple}{exe} and ffprobe-{triple}{exe})",
        .dir.display(),
        exe = std::env::consts::EXE_SUFFIX,
        triple = crate::TARGET_TRIPLE
    )]
    SidecarDirIncomplete {
        /// The folder that was checked (absolute).
        dir: PathBuf,
    },

    /// The operating system refused to start the program.
    #[error("could not start {}: {source}", .program.display())]
    Spawn {
        /// The program that was to be started.
        program: PathBuf,
        /// The OS error (for example `NotFound`, or `InvalidInput` for a relative path).
        source: io::Error,
    },

    /// Reading the output of, or waiting for, a running program failed.
    #[error("lost contact with {} while it was running: {source}", .program.display())]
    Io {
        /// The running program.
        program: PathBuf,
        /// The underlying I/O error.
        source: io::Error,
    },

    /// [`run_capture`](crate::run_capture) gave up waiting; the process was killed.
    #[error(
        "{} did not finish within {:.1} s and was stopped (pid {pid}); \
         the file may be on a slow or unresponsive drive",
        .program.display(),
        .timeout.as_secs_f64()
    )]
    Timeout {
        /// The program that timed out.
        program: PathBuf,
        /// The time limit that elapsed.
        timeout: Duration,
        /// PID of the killed process.
        pid: u32,
    },

    /// A helper command (such as `ffmpeg -filters`) exited unsuccessfully.
    #[error("{} failed while {action} ({}): {detail}", .program.display(), describe_exit(.exit_code))]
    CommandFailed {
        /// The program that failed.
        program: PathBuf,
        /// What the runner was doing, e.g. "listing the FFmpeg filters".
        action: &'static str,
        /// Its exit code (`None` when ended by a signal).
        exit_code: Option<i32>,
        /// The most relevant stderr line.
        detail: String,
    },

    /// A helper command printed something the runner cannot interpret.
    #[error(
        "{} printed unexpected output while {action}; make sure it is a working FFmpeg build",
        .program.display()
    )]
    UnexpectedOutput {
        /// The program whose output was not understood.
        program: PathBuf,
        /// What the runner was doing.
        action: &'static str,
    },
}

/// "exit code N", or a note that the process was ended by a signal.
pub(crate) fn describe_exit(code: &Option<i32>) -> String {
    match code {
        Some(code) => format!("exit code {code}"),
        None => "terminated by a signal".to_owned(),
    }
}
