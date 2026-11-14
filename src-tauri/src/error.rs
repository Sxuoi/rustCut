//! Conversion of engine errors into the IPC [`CommandError`] (AC-12: every
//! message says what happened and what the user can do about it).

use rustcut_media::MediaError;
use rustcut_model::{CommandError, ErrorCode};

/// "No project is open" (a project command ran with no session).
pub fn no_project() -> CommandError {
    CommandError::new(
        ErrorCode::NoProject,
        "No project is open. Open a video or a .rustcut project first.",
    )
}

/// FFmpeg/FFprobe could not be located at startup.
pub fn ffmpeg_missing(detail: &str) -> CommandError {
    CommandError::new(
        ErrorCode::Ffmpeg,
        format!(
            "FFmpeg and FFprobe were not found, so videos cannot be opened or exported. Reinstall RustCut, or install FFmpeg and set RUSTCUT_FFMPEG_DIR to the folder that contains ffmpeg and ffprobe. ({detail})"
        ),
    )
}

/// A [`MediaError`] as a command error with a matching code.
pub fn media_error(e: MediaError) -> CommandError {
    let code = match &e {
        MediaError::Probe { .. } => ErrorCode::Probe,
        MediaError::NoVideo { .. } => ErrorCode::Unsupported,
        MediaError::Io { .. } => ErrorCode::Io,
        MediaError::NoSuchTrack { .. } => ErrorCode::InvalidInput,
        MediaError::Cancelled => ErrorCode::Busy,
        MediaError::Runner(_)
        | MediaError::Index { .. }
        | MediaError::Waveform { .. }
        | MediaError::Prepare { .. }
        | MediaError::HwDetect(_) => ErrorCode::Ffmpeg,
        _ => ErrorCode::Internal,
    };
    CommandError::new(code, e.to_string())
}

/// A plain I/O failure with the action and the path in the message.
pub fn io_error(action: &str, path: &std::path::Path, e: &std::io::Error) -> CommandError {
    let code = if e.kind() == std::io::ErrorKind::NotFound {
        ErrorCode::NotFound
    } else {
        ErrorCode::Io
    };
    CommandError::new(
        code,
        format!(
            "Could not {action} \"{}\": {e}. Check that the drive is connected, the folder is writable and there is free space.",
            path.display()
        ),
    )
}

/// A background task panicked or was dropped.
pub fn join_error(e: &tokio::task::JoinError) -> CommandError {
    CommandError::new(ErrorCode::Internal, format!("An internal task failed: {e}"))
}
