//! Shared builders for unit tests.

use rustcut_model::{AudioTrack, Fingerprint, SourceInfo, VideoInfo};

/// A 1920×1080 60 fps source of `video` codec with one stereo track per
/// entry of `audio`, 60 s long, 8 Mbps.
pub(crate) fn source(format: &str, path: &str, video: &str, audio: &[&str]) -> SourceInfo {
    SourceInfo {
        path: path.into(),
        relative_path: String::new(),
        fingerprint: Fingerprint {
            size_bytes: 60_000_000,
            xxh3_head_tail: String::new(),
        },
        duration_sec: 60.0,
        start_time_sec: 0.0,
        video: VideoInfo {
            codec: video.into(),
            coded_width: 1920,
            coded_height: 1080,
            display_width: 1920,
            display_height: 1080,
            rotation: 0,
            sar: "1:1".into(),
            frame_rate: "60/1".into(),
            vfr: false,
            hdr: false,
            stream_index: Some(0),
            pix_fmt: Some("yuv420p".into()),
            profile: None,
            level: None,
            color_transfer: None,
            bit_depth: Some(8),
        },
        audio_tracks: audio
            .iter()
            .enumerate()
            .map(|(i, c)| AudioTrack {
                stream_index: i as u32 + 1,
                codec: (*c).into(),
                channels: 2,
                title: None,
                language: None,
                sample_rate: Some(48_000),
                is_default: i == 0,
            })
            .collect(),
        format_name: if format.is_empty() {
            None
        } else {
            Some(format.into())
        },
        bit_rate: Some(8_000_000),
    }
}
