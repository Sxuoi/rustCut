//! Sidecar location and child-process lifetime for `ffmpeg`/`ffprobe`.
//!
//! This crate is the only place RustCut starts processes (ARCHITECTURE §2, PRD §7.9):
//!
//! * [`Sidecars::locate`] finds the bundled (or developer-provided) `ffmpeg` and `ffprobe`;
//!   nothing else is ever run.
//! * [`spawn_tracked`] starts one of them from an argument vector (never a shell string,
//!   EXP-08) with stdin closed, stdout/stderr piped, no console window on Windows, and an
//!   OS-level guarantee that it dies with the app (PRD §7.8, §8.2): a kill-on-close Job Object
//!   on Windows, `PR_SET_PDEATHSIG` on Linux. On macOS the PID is handed to the caller for
//!   journaling and [`kill_stale_sidecar`] removes leftovers on the next launch.
//! * [`run_capture`] collects the whole output of a short command (ffprobe) with a timeout.
//! * [`run_streaming`] runs a long FFmpeg job: stdout lines go to a callback (the caller parses
//!   `-progress pipe:1`, EXP-09), every stderr line is appended to a per-clip log and the last
//!   ones are kept for the failure report (EXP-10, PRD §7.8), and a [`CancelToken`] kills the
//!   process promptly (EXP-09).
//! * [`probe_capabilities`] reports the FFmpeg version, filters and encoders (cached).
//!
//! The caller owns the argument vector, including `-nostdin` (EXP-08); this crate does not
//! parse FFmpeg output beyond splitting it into lines.
#![warn(missing_docs)]

mod cancel;
mod capabilities;
mod error;
mod platform;
mod process;
mod run;
mod sidecars;
mod spawn;

pub use cancel::CancelToken;
pub use capabilities::{Capabilities, probe_capabilities};
pub use error::RunnerError;
pub use process::{StaleProcess, is_process_running, kill_stale_sidecar};
pub use run::{
    CapturedOutput, DEFAULT_TAIL_LINES, RunOutcome, StreamOptions, run_capture, run_streaming,
};
pub use sidecars::{FFMPEG_DIR_ENV, Sidecars, TARGET_TRIPLE};
pub use spawn::{SpawnOptions, spawn_tracked};
