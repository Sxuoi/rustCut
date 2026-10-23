//! ffprobe output parsing (pure logic).
//!
//! Spawning ffprobe belongs to the app layer; this module only turns its
//! JSON (`-print_format json -show_format -show_streams`) into `MediaInfo`.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaInfo {
    pub duration_sec: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// Bits per second, from the container or the video stream.
    pub bitrate_bps: f64,
    pub codec_name: String,
    pub format_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    InvalidJson(String),
    MissingField(String),
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProbeError::InvalidJson(e) => write!(f, "ffprobe returned invalid JSON: {e}"),
            ProbeError::MissingField(field) => write!(f, "ffprobe output missing {field}"),
        }
    }
}

impl std::error::Error for ProbeError {}

/// Parses an ffprobe frame rate such as `30000/1001`, `25/1`, `0/0` or `30`.
fn parse_rate(value: &str) -> f64 {
    let value = value.trim();
    if let Some((num, den)) = value.split_once('/') {
        let n: f64 = num.trim().parse().unwrap_or(0.0);
        let d: f64 = den.trim().parse().unwrap_or(0.0);
        if d == 0.0 || !n.is_finite() || !d.is_finite() {
            return 0.0;
        }
        return n / d;
    }
    value.parse().unwrap_or(0.0)
}

fn f64_field(value: &serde_json::Value, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
}

/// Parses `ffprobe -v error -print_format json -show_format -show_streams`.
pub fn parse_ffprobe_json(raw: &str) -> Result<MediaInfo, ProbeError> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| ProbeError::InvalidJson(e.to_string()))?;

    let streams = value
        .get("streams")
        .and_then(|s| s.as_array())
        .ok_or_else(|| ProbeError::MissingField("streams".to_string()))?;
    let video = streams
        .iter()
        .find(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("video"))
        .ok_or_else(|| ProbeError::MissingField("a video stream".to_string()))?;
    let format = value.get("format").cloned().unwrap_or(serde_json::Value::Null);

    let width = video.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let height = video.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

    let fps = video
        .get("avg_frame_rate")
        .and_then(|v| v.as_str())
        .map(parse_rate)
        .filter(|f| *f > 0.0)
        .or_else(|| {
            video
                .get("r_frame_rate")
                .and_then(|v| v.as_str())
                .map(parse_rate)
                .filter(|f| *f > 0.0)
        })
        .unwrap_or(0.0);

    // Duration lives on the stream for most containers, on the format for
    // some (e.g. MPEG-TS). Prefer the stream, fall back to the container.
    let duration_sec = f64_field(video, "duration")
        .or_else(|| f64_field(&format, "duration"))
        .unwrap_or(0.0);

    let bitrate_bps = f64_field(video, "bit_rate")
        .or_else(|| f64_field(&format, "bit_rate"))
        .unwrap_or(0.0);

    let codec_name = video
        .get("codec_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let format_name = format
        .get("format_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(MediaInfo {
        duration_sec,
        width,
        height,
        fps,
        bitrate_bps,
        codec_name,
        format_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shaped exactly like real `ffprobe -print_format json` output.
    const FIXTURE: &str = r##"{
      "streams": [
        {
          "codec_name": "h264",
          "codec_type": "video",
          "width": 1920,
          "height": 1080,
          "r_frame_rate": "30000/1001",
          "avg_frame_rate": "30000/1001",
          "duration": "9000.021333",
          "bit_rate": "8000000"
        },
        {
          "codec_name": "aac",
          "codec_type": "audio",
          "duration": "9000.000000"
        }
      ],
      "format": {
        "format_name": "mov,mp4,m4a,3gp,3g2,mj2",
        "duration": "9000.021333",
        "bit_rate": "8123456"
      }
    }"##;

    #[test]
    fn parses_real_world_probe_output() {
        let info = parse_ffprobe_json(FIXTURE).unwrap();
        assert_eq!(info.width, 1920);
        assert_eq!(info.height, 1080);
        assert!((info.fps - 29.97).abs() < 0.01);
        assert!((info.duration_sec - 9000.021333).abs() < 1e-6);
        assert_eq!(info.bitrate_bps, 8_000_000.0);
        assert_eq!(info.codec_name, "h264");
        assert_eq!(info.format_name, "mov,mp4,m4a,3gp,3g2,mj2");
    }

    #[test]
    fn falls_back_to_container_duration_and_bitrate() {
        let raw = r##"{
          "streams": [{"codec_type": "video", "codec_name": "vp9", "width": 640, "height": 360, "avg_frame_rate": "25/1"}],
          "format": {"format_name": "matroska", "duration": "12.5", "bit_rate": "1000000"}
        }"##;
        let info = parse_ffprobe_json(raw).unwrap();
        assert_eq!(info.duration_sec, 12.5);
        assert_eq!(info.bitrate_bps, 1_000_000.0);
        assert_eq!(info.fps, 25.0);
    }

    #[test]
    fn handles_zero_and_odd_frame_rates() {
        assert_eq!(parse_rate("0/0"), 0.0);
        assert_eq!(parse_rate("30000/1001"), 30000.0 / 1001.0);
        assert_eq!(parse_rate("25/1"), 25.0);
        assert_eq!(parse_rate("30"), 30.0);
    }

    #[test]
    fn rejects_non_video_and_garbage_input() {
        assert!(matches!(
            parse_ffprobe_json("not json"),
            Err(ProbeError::InvalidJson(_))
        ));
        let audio_only = r##"{"streams": [{"codec_type": "audio"}], "format": {}}"##;
        assert!(matches!(
            parse_ffprobe_json(audio_only),
            Err(ProbeError::MissingField(_))
        ));
    }
}
