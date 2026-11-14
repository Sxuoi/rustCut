//! `source` block of the project file (PRD §7.5).

use serde::{Deserialize, Serialize};
use std::path::Path;
use ts_rs::TS;

/// Identifies a source file across renames and moves (PRJ-04).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Fingerprint {
    #[ts(type = "number")]
    pub size_bytes: u64,
    pub xxh3_head_tail: String,
}

/// First video stream as probed. All framing math uses `display_*`
/// (rotation applied, square pixels; FRM-02).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VideoInfo {
    pub codec: String,
    pub coded_width: u32,
    pub coded_height: u32,
    pub display_width: u32,
    pub display_height: u32,
    /// Clockwise display rotation in degrees: 0, 90, 180 or 270.
    pub rotation: u32,
    /// Sample aspect ratio, `"num:den"` (`"1:1"` = square pixels).
    pub sar: String,
    /// Rational string, never a float (`"60000/1001"`).
    pub frame_rate: String,
    pub vfr: bool,
    /// PQ (`smpte2084`) or HLG (`arib-std-b67`) transfer (FRM-07).
    pub hdr: bool,
    // ---- additions beyond the PRD sample (optional on read) ----
    /// Absolute stream index of this video stream in the source.
    #[serde(default)]
    pub stream_index: Option<u32>,
    #[serde(default)]
    pub pix_fmt: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub level: Option<i32>,
    #[serde(default)]
    pub color_transfer: Option<String>,
    #[serde(default)]
    pub bit_depth: Option<u32>,
}

impl VideoInfo {
    /// Frame rate as f64 (None if unparsable).
    pub fn fps(&self) -> Option<f64> {
        crate::time::parse_rational(&self.frame_rate)
    }

    /// Nominal frame duration in seconds (falls back to 1/30).
    pub fn frame_duration(&self) -> f64 {
        self.fps()
            .filter(|f| *f > 0.0)
            .map(|f| 1.0 / f)
            .unwrap_or(1.0 / 30.0)
    }

    /// `(num, den)` of the SAR, defaulting to 1:1.
    pub fn sar_ratio(&self) -> (u32, u32) {
        let mut it = self.sar.split([':', '/']);
        match (
            it.next().and_then(|v| v.parse().ok()),
            it.next().and_then(|v| v.parse().ok()),
        ) {
            (Some(n), Some(d)) if n > 0 && d > 0 => (n, d),
            _ => (1, 1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AudioTrack {
    /// Absolute stream index in the source (ffprobe `index`).
    pub stream_index: u32,
    pub codec: String,
    pub channels: u32,
    #[serde(default)]
    pub title: Option<String>,
    // ---- additions ----
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    /// Disposition "default" flag (the preview plays the default track).
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SourceInfo {
    /// Absolute path as given by the OS (native separators).
    pub path: String,
    /// Path relative to the project file's folder; tried first when the
    /// whole folder moved (PRJ-04).
    pub relative_path: String,
    pub fingerprint: Fingerprint,
    pub duration_sec: f64,
    /// Absolute start PTS of the source; every stored time is relative to it.
    pub start_time_sec: f64,
    pub video: VideoInfo,
    pub audio_tracks: Vec<AudioTrack>,
    // ---- additions ----
    /// ffprobe `format_name`, e.g. `"mov,mp4,m4a,3gp,3g2,mj2"`, `"matroska,webm"`.
    #[serde(default)]
    pub format_name: Option<String>,
    /// Overall bit rate in bits per second (EXP-11 lossless estimate).
    #[serde(default)]
    #[ts(type = "number | null")]
    pub bit_rate: Option<u64>,
}

impl SourceInfo {
    /// File name with extension.
    pub fn file_name(&self) -> String {
        Path::new(&self.path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// `{source_name}`: file name without extension (EXP-05).
    pub fn stem(&self) -> String {
        Path::new(&self.path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Lower-case extension without the dot (`"mp4"`), or empty.
    pub fn extension(&self) -> String {
        Path::new(&self.path)
            .extension()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default()
    }

    /// Default export folder: `<source folder>/<source name>_clips` (§6.2).
    pub fn default_output_dir(&self) -> String {
        let p = Path::new(&self.path);
        let parent = p.parent().unwrap_or(Path::new("."));
        parent
            .join(format!("{}_clips", self.stem()))
            .to_string_lossy()
            .into_owned()
    }
}
