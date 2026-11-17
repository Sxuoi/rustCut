//! Batch export engine: pre-flight, scheduler, `.part` safe writes, progress,
//! cancel, fault tolerance, export journal (EXP-01..14, PRD 7.8).
//!
//! * [`preflight`] builds the [`rustcut_model::PreflightReport`] (EXP-11) with an
//!   injectable [`FsProbe`].
//! * [`ExportEngine`] runs a batch (EXP-08..10, 12, 13) and reports to an
//!   [`ExportSink`]; the app applies the [`rustcut_model::ExportRecord`]s to its
//!   project store.
//! * [`cleanup_stale`] removes stale `.part` files and orphaned FFmpeg processes
//!   at startup (PRD 7.8, 8.2).
//! * Every output is verified before its `.part` is renamed (PRD 7.8): MP4/MOV
//!   files from their `moov` box ([`mp4_verify_report`]), everything else and
//!   every doubtful case with ffprobe.
//! * [`SleepGuard`] blocks system sleep while a batch runs (EXP-14).

#![warn(missing_docs)]

mod engine;
mod error;
mod journal;
mod platform;
mod preflight;
mod sleep;
mod verify;

pub use engine::{
    BatchRequest, EngineConfig, ExportEngine, ExportSink, fresh_path, part_path, resolve_encoder,
};
pub use error::ExportError;
pub use journal::{JOURNAL_FILE, Journal, JournalEntry, cleanup_stale, cleanup_stale_with};
pub use platform::{
    BUNDLED_PLATFORM_PRESETS, PLATFORM_PRESETS_FILE, bundled_platform_presets,
    load_platform_presets, parse_platform_presets,
};
pub use preflight::{
    FREE_SPACE_FACTOR, FsProbe, PreflightInput, RealFs, WINDOWS_MAX_PATH_UNITS, fmt_bytes,
    preflight, required_free_bytes,
};
pub use sleep::SleepGuard;
pub use verify::{VERIFY_TOLERANCE_SEC, mp4_verify_report};
