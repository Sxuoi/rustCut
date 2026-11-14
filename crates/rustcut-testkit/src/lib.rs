//! Test support for RustCut: locates `ffmpeg`/`ffprobe`, generates the synthetic
//! golden-set fixtures of PRD section 11 on demand, and offers small media-inspection
//! helpers (probe, frame counts, packet hashes, decoded luma) for AC-07, AC-08, AC-12
//! and AC-17.
//!
//! This crate is a dev-dependency only and depends on no other workspace crate.
//! Everything here is allowed to panic with a clear message: it is test code.
//!
//! * Tools: [`ffmpeg`], [`ffprobe`], [`try_ffmpeg`], [`try_ffprobe`].
//! * Fixtures: [`Fixture`], [`fixture`], [`try_fixture`], [`fixtures_dir`].
//! * Helpers: [`probe_json`], [`duration_sec`], [`video_size`], [`frame_count`],
//!   [`packet_hashes`], [`frame_luma`], [`edge_luma`], [`keyframe_times`],
//!   [`video_pts_times`], [`temp_dir`], [`ffmpeg_ok`].

mod fixtures;
mod tools;

pub use fixtures::{
    FIXTURE_VERSION, Fixture, FixtureError, fixture, fixtures_dir, generation_count, try_fixture,
};
pub use tools::{
    EdgeLuma, duration_sec, edge_luma, ffmpeg, ffmpeg_ok, ffprobe, frame_count, frame_luma,
    keyframe_times, packet_hashes, probe_json, temp_dir, try_ffmpeg, try_ffprobe, try_probe_json,
    video_pts_times, video_size,
};
