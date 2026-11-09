//! Caption model and SRT generation (PRD Phase 3).
//!
//! Transcripts are stored once per source, in source time. Each exported clip
//! gets its own SRT re-timed to the clip, and optionally burns the same text
//! into the video during re-encode.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Caption {
    pub start_sec: f64,
    pub end_sec: f64,
    pub text: String,
}

/// `HH:MM:SS,mmm` as SRT expects.
pub fn format_srt_time(secs: f64) -> String {
    let clamped = secs.max(0.0);
    let total_ms = (clamped * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    format!("{h:02}:{m:02}:{s:02},{ms:03}")
}

/// Builds the SRT for a clip that starts at `offset_sec` and ends at
/// `clip_end_sec` (both in source time). Cues outside the clip are dropped,
/// partial cues are clamped to the clip, and empty text is skipped.
pub fn build_srt(captions: &[Caption], offset_sec: f64, clip_end_sec: f64) -> String {
    let mut out = String::new();
    let mut index = 1;
    for caption in captions {
        let start = caption.start_sec.max(offset_sec);
        let end = caption.end_sec.min(clip_end_sec);
        if end <= start {
            continue;
        }
        let text = caption.text.trim();
        if text.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "{index}\n{} --> {}\n{text}\n\n",
            format_srt_time(start - offset_sec),
            format_srt_time(end - offset_sec)
        ));
        index += 1;
    }
    out
}

/// Escapes a Windows/Unix path for FFmpeg's `subtitles=` filter: the drive
/// colon and single quotes are the two characters that break it.
pub fn escape_subtitles_path(path: &str) -> String {
    path.replace('\\', "/")
        .replace(':', "\\:")
        .replace('\'', "\\'")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(start: f64, end: f64, text: &str) -> Caption {
        Caption {
            start_sec: start,
            end_sec: end,
            text: text.to_string(),
        }
    }

    #[test]
    fn formats_srt_timestamps() {
        assert_eq!(format_srt_time(0.0), "00:00:00,000");
        assert_eq!(format_srt_time(1.5), "00:00:01,500");
        assert_eq!(format_srt_time(3661.234), "01:01:01,234");
        assert_eq!(format_srt_time(-2.0), "00:00:00,000");
    }

    #[test]
    fn shifts_and_clamps_cues_to_the_clip() {
        let captions = vec![
            cap(9.0, 11.0, "before"),
            cap(12.0, 14.0, "inside"),
            cap(19.5, 21.0, "tail"),
            cap(21.0, 23.0, "after"),
        ];
        let srt = build_srt(&captions, 10.0, 20.0);
        assert!(srt.contains("00:00:02,000 --> 00:00:04,000"));
        assert!(srt.contains("inside"));
        // The cue crossing the start is clamped to 0 and kept.
        assert!(srt.contains("00:00:00,000 --> 00:00:01,000"));
        assert!(srt.contains("before"));
        // A cue running past the end is cut at the clip boundary ...
        assert!(srt.contains("00:00:09,500 --> 00:00:10,000"));
        assert!(srt.contains("tail"));
        // ... and a cue starting after the clip end is dropped entirely.
        assert!(!srt.contains("after"));
    }

    #[test]
    fn numbers_cues_and_skips_empty_text() {
        let captions = vec![cap(0.0, 1.0, "one"), cap(1.0, 2.0, "  "), cap(2.0, 3.0, "two")];
        let srt = build_srt(&captions, 0.0, 10.0);
        assert!(srt.starts_with("1\n00:00:00,000"));
        assert!(srt.contains("2\n00:00:02,000"));
        assert_eq!(srt.matches("-->").count(), 2);
    }

    #[test]
    fn escapes_windows_paths_for_the_filter() {
        assert_eq!(
            escape_subtitles_path(r"C:\Clips\out\01_test.srt"),
            "C\\:/Clips/out/01_test.srt"
        );
    }
}
