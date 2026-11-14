//! Typed errors of the export engine.

use rustcut_model::{CommandError, ErrorCode};
use std::io;
use uuid::Uuid;

/// Why a batch could not be started, resumed or journaled.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// EXP-12: only one batch runs at a time.
    #[error(
        "An export is already running (batch {batch_id}). Wait for it to finish or cancel it first."
    )]
    Busy {
        /// The running batch.
        batch_id: String,
    },
    /// The pre-flight report carries a blocking item (EXP-11) or no outputs.
    #[error("The export cannot start: {0}")]
    PreflightFailed(String),
    /// The engine needs a Tokio runtime to schedule FFmpeg jobs.
    #[error("The export engine must be started from inside a Tokio runtime")]
    NoRuntime,
    /// The report plans a clip that the project does not contain.
    #[error(
        "Clip {0} is part of the pre-flight plan but no longer exists in the project; run the pre-flight again"
    )]
    UnknownClip(Uuid),
    /// The output folder could not be created (EXP-11: created at start).
    #[error(
        "Cannot create the output folder {path}: {source}. Choose another folder or check that the drive is connected"
    )]
    OutputDir {
        /// Folder that failed.
        path: String,
        /// OS error.
        source: io::Error,
    },
    /// [`crate::ExportEngine::resume`] without a paused batch.
    #[error("No export is paused")]
    NotPaused,
    /// Resume was requested but the folder/disk problem persists.
    #[error(
        "Cannot resume yet: the output folder {path} is still unavailable ({source}). Reconnect the drive or free disk space, then resume"
    )]
    StillUnavailable {
        /// Folder that is still missing.
        path: String,
        /// OS error.
        source: io::Error,
    },
    /// The export journal could not be written (EXP-08).
    #[error("Cannot write the export journal {path}: {source}")]
    Journal {
        /// Journal file.
        path: String,
        /// OS error.
        source: io::Error,
    },
}

impl From<ExportError> for CommandError {
    fn from(e: ExportError) -> Self {
        let code = match &e {
            ExportError::Busy { .. } => ErrorCode::Busy,
            ExportError::PreflightFailed(_)
            | ExportError::UnknownClip(_)
            | ExportError::NotPaused => ErrorCode::InvalidInput,
            ExportError::NoRuntime => ErrorCode::Internal,
            ExportError::OutputDir { .. }
            | ExportError::StillUnavailable { .. }
            | ExportError::Journal { .. } => ErrorCode::Io,
        };
        CommandError::new(code, e.to_string())
    }
}
