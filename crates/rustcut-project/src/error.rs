//! Error types of this crate and their conversion to [`CommandError`]
//! (specific, actionable messages; AC-12).

use rustcut_model::time::round_ms;
use rustcut_model::{CommandError, ErrorCode, ReadOnlyReason};
use std::io;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Errors of [`crate::ProjectStore`] user commands. A command that fails
/// leaves the project unchanged.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StoreError {
    /// The project is open read-only (PRJ-05, PRJ-06, missing source).
    #[error("{}", read_only_message(.reason))]
    ReadOnly { reason: ReadOnlyReason },
    /// MRK-01: the range is shorter than the minimum clip duration `min` (seconds).
    #[error("Clips must be at least {} s long. Move In and Out further apart.", format_seconds(.min))]
    ClipTooShort { min: f64 },
    /// MRK-01 / AC-03: Out at or before In, In at or past the source end, or
    /// times that are not numbers.
    #[error("{0}")]
    InvalidRange(String),
    /// A clip id that is not (or no longer) in the project.
    #[error(
        "That clip no longer exists (it may have been deleted or undone). Refresh the queue and try again."
    )]
    ClipNotFound(Uuid),
    /// A tag id that is not in the project's palette (MRK-06).
    #[error(
        "The tag \"{0}\" is not in this project's palette. Choose an existing tag or add it to the palette first."
    )]
    UnknownTag(String),
    /// Any other invalid argument; the message says what to fix.
    #[error("{0}")]
    InvalidInput(String),
}

/// Errors reading or writing project files, backups, locks and lists.
#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    /// File-system failure; `op` is a verb phrase such as `"write"`.
    #[error(
        "Could not {op} \"{}\": {source}. Check that the drive is connected, the folder is writable and there is free space.",
        .path.display()
    )]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The file is not a readable RustCut project (callers fall back to backups, PRJ-06).
    #[error("The project file \"{}\" is damaged and can't be opened ({reason}).", .path.display())]
    Corrupt { path: PathBuf, reason: String },
    /// The project could not be serialised (should not happen).
    #[error("Could not encode the project for saving: {0}")]
    Encode(String),
}

impl PersistError {
    pub(crate) fn io(op: &'static str, path: &Path, source: io::Error) -> Self {
        PersistError::Io {
            op,
            path: path.to_path_buf(),
            source,
        }
    }
}

impl From<StoreError> for CommandError {
    fn from(e: StoreError) -> Self {
        let code = match &e {
            StoreError::ReadOnly { .. } => ErrorCode::ReadOnly,
            StoreError::ClipTooShort { .. } => ErrorCode::ClipTooShort,
            StoreError::InvalidRange(_) => ErrorCode::InvalidRange,
            StoreError::ClipNotFound(_) => ErrorCode::NotFound,
            StoreError::UnknownTag(_) | StoreError::InvalidInput(_) => ErrorCode::InvalidInput,
        };
        CommandError::new(code, e.to_string())
    }
}

impl From<PersistError> for CommandError {
    fn from(e: PersistError) -> Self {
        let code = match &e {
            PersistError::Io { .. } | PersistError::Corrupt { .. } => ErrorCode::Io,
            PersistError::Encode(_) => ErrorCode::Internal,
        };
        CommandError::new(code, e.to_string())
    }
}

/// User-facing explanation of a read-only project.
pub fn read_only_message(reason: &ReadOnlyReason) -> String {
    match reason {
        ReadOnlyReason::OpenElsewhere { holder } => {
            let by = holder.as_deref().map(|h| format!(" ({h})")).unwrap_or_default();
            format!(
                "This project is open in another RustCut window or instance{by}, so it is read-only here. \
                 Close it there and reopen it to make changes."
            )
        }
        ReadOnlyReason::NewerSchema { found, supported } => format!(
            "This project was saved by a newer version of RustCut (file format {found}; this version reads up to \
             {supported}), so it is read-only. Update RustCut to edit it."
        ),
        ReadOnlyReason::SourceMissing => {
            "The source video is missing, so the clip queue is read-only. Use Locate Missing Media to find the file."
                .to_string()
        }
        ReadOnlyReason::Unwritable { path } => format!(
            "RustCut cannot write to the project folder ({path}), so the project is read-only. \
             Check the folder's permissions or move the project somewhere writable."
        ),
    }
}

/// `0.5` -> `"0.5"`, `2.0` -> `"2"` (millisecond precision).
fn format_seconds(v: &f64) -> String {
    let v = round_ms(*v);
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}
