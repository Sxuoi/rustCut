//! Errors of the media service. Every message is meant to be shown to the user as is (AC-12):
//! it says what failed and what to do about it.

use std::io;

use rustcut_runner::RunnerError;

/// Why a media operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MediaError {
    /// FFprobe could not read the file (truncated, corrupt, unsupported, unreadable). The text is
    /// `rustcut_ffmpeg::probe_failure_message` output, including the recovery hint (AC-12,
    /// PRD section 9: "Truncated or corrupt file").
    #[error("{message}")]
    Probe {
        /// User-facing explanation with a recovery hint.
        message: String,
    },

    /// The file has no video stream (PLY-01: audio-only sources are not supported).
    #[error("{message}")]
    NoVideo {
        /// User-facing explanation.
        message: String,
    },

    /// A file or folder could not be read or written.
    #[error("cannot access {path}: {source}")]
    Io {
        /// The path involved.
        path: String,
        /// The OS error.
        #[source]
        source: io::Error,
    },

    /// A sidecar could not be started or lost contact.
    #[error(transparent)]
    Runner(#[from] RunnerError),

    /// The frame index (PLY-06) could not be built.
    #[error("could not build the frame index of {file}: {detail}")]
    Index {
        /// Source file name.
        file: String,
        /// What FFprobe reported.
        detail: String,
    },

    /// The requested audio track does not exist (tracks are 1-based).
    #[error("{file} has no audio track {track}; it has {available} audio track(s)")]
    NoSuchTrack {
        /// Source file name.
        file: String,
        /// The requested 1-based track.
        track: u32,
        /// How many audio tracks the file has.
        available: usize,
    },

    /// Decoding the audio for the waveform (PLY-05) failed.
    #[error("could not read the audio of {file} for the waveform: {detail}")]
    Waveform {
        /// Source file name.
        file: String,
        /// What FFmpeg reported.
        detail: String,
    },

    /// Preparing a playback tier (remux or proxy, section 7.3) failed.
    #[error(
        "could not prepare the {tier} preview of {file}: {detail}. Export is not affected: it always reads the original file"
    )]
    Prepare {
        /// `"remux"` or `"proxy"`.
        tier: &'static str,
        /// Source file name.
        file: String,
        /// What FFmpeg reported.
        detail: String,
    },

    /// The operation was cancelled by the caller.
    #[error("cancelled")]
    Cancelled,

    /// Hardware-encoder detection could not run (EXP-13).
    #[error("could not detect hardware encoders: {0}")]
    HwDetect(String),
}

impl MediaError {
    pub(crate) fn io(path: impl AsRef<std::path::Path>, source: io::Error) -> Self {
        MediaError::Io {
            path: path.as_ref().display().to_string(),
            source,
        }
    }
}
