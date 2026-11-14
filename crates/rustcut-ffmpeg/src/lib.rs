//! Pure FFmpeg layer of RustCut (ARCHITECTURE §5, PRD §7.6 A–F).
//!
//! This crate never spawns a process and never touches the file system: it
//! builds argument vectors and parses what FFmpeg/FFprobe print. Spawning is
//! `rustcut-runner`'s job.
//!
//! * [`args`]: the export command model ([`ExportCommand`], [`VideoPlan`],
//!   [`AudioPlan`], [`Muxer`]) and its argv, plus the FFprobe argument
//!   vectors ([`probe_args`], [`packet_index_args`], [`verify_args`]).
//! * [`build`]: [`build_export_command`] turns a clip + settings into the
//!   exact §7.6 B/C/D/E/F argv (EXP-02/03/04/12, FRM-06/07).
//! * [`hw`]: hardware encoder rate-control mapping and the 1-second
//!   detection encode (EXP-13).
//! * [`progress`]: `-progress pipe:1` parser (EXP-09).
//! * [`probe`]: FFprobe JSON → [`rustcut_model::SourceInfo`] (PLY-01, §9)
//!   and actionable probe failure messages.
//! * [`mime`]: RFC 6381 codec strings for the WebView tier check (§7.3).
//! * [`index`]: keyframe/frame index from packet listings (PLY-06, MRK-07).
//! * [`compat`]: lossless output container decision (EXP-03).
//! * [`estimate`]: pre-flight size estimates (EXP-11).
//! * [`failure`]: FFmpeg failure classification (EXP-10, EXP-12, EXP-13).
//!
//! # FFmpeg 9 notes
//! Every command shape was run against FFmpeg 9.0.2 (the PRD was verified on
//! 6.1.1); see `tests/real_ffmpeg.rs`. Differences found and handled:
//! * `ffprobe -of csv=p=0` packet lines end with a trailing comma
//!   (`1.466667,K__,`); [`index::parse_packet_csv`] accepts both forms.
//! * The first `-progress` block reports `out_time_us=N/A` and `speed=N/A`.
//! * AMF's QVBR rate control fails to initialise on tiny frames (256×256 →
//!   `encoder->Init() failed with error 4`) while 320×240 and larger work, so
//!   the detection encode uses 640×360 ([`hw::HW_TEST_SIZE`]). QVBR also needs
//!   AMF's pre-analysis, which held encodes near 60 frames per second on a
//!   Radeon 780M, so AMF uses `vbr_peak` or `cqp` instead ([`hw::hw_video_args`]).
//! * The mp4 muxer now accepts PCM (`ipcm`), MP2, Vorbis, DTS, MPEG-2, MJPEG
//!   and FFV1 via stream copy; players do not, so [`compat`] keeps a
//!   stricter, playback-driven allow-list.

pub mod args;
pub mod build;
pub mod compat;
pub mod estimate;
pub mod failure;
pub mod hw;
pub mod index;
pub mod mime;
pub mod probe;
pub mod progress;
#[cfg(test)]
mod testutil;

pub use args::{
    AudioPlan, ExportCommand, Muxer, VideoCodec, VideoEncode, VideoFilter, VideoPlan, fmt_bitrate,
    fmt_time, fmt_time_reencode, packet_index_args, probe_args, safe_path_arg, verify_args,
};
pub use build::{JobSpec, ResolvedEncoder, build_export_command};
pub use compat::{ContainerDecision, LosslessAudio, lossless_container};
pub use failure::{FailureKind, classify, first_error_line, rejected_arguments};
pub use hw::{HwVideoArgs, hw_test_args, hw_video_args};
pub use index::{MediaIndex, parse_packet_csv};
pub use mime::mime_type;
pub use probe::{
    Probe, ProbeError, ProbeWarning, StreamCounts, parse_probe_json, probe_failure_message,
};
pub use progress::{ProgressParser, ProgressUpdate, fraction};
