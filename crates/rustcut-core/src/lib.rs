//! rustcut-core: pure logic for RustCut / QuickClip.
//!
//! Clip model, naming templates, timecodes, FFmpeg command generation and
//! `.rustcut` project persistence. No Tauri, no async, no I/O beyond project
//! files - everything here is unit-testable in isolation.

pub mod clip;
pub mod export;
pub mod ffmpeg;
pub mod naming;
pub mod probe;
pub mod project;
pub mod timecode;
pub mod waveform;

pub use clip::{clamp_offset, Clip, ClipError, ExportStatus};
pub use ffmpeg::{
    build_lossless_args, build_reencode_args, canvas_dims, crop_width_px, crop_x_px, CropStyle,
    HwEncoder, VideoEncoder,
};
pub use naming::{
    build_output_stem, dedupe_names, expand_template, normalize_component, NamingContext,
};
pub use project::{ExportMode, ExportSettings, ProjectState, SourceVideo, PROJECT_VERSION};
pub use timecode::{format_tc, format_tc_display, format_tc_filename, parse_tc};
