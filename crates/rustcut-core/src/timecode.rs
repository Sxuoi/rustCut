//! Timecode parsing and formatting.
//!
//! Internal representation is always `f64` seconds. FFmpeg receives
//! `HH:MM:SS.mmm` strings; file names use `HH-MM-SS`.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimecodeError {
    Empty,
    Malformed(String),
    Negative,
    OutOfRange(String),
}

impl fmt::Display for TimecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TimecodeError::Empty => write!(f, "empty timecode"),
            TimecodeError::Malformed(s) => write!(f, "malformed timecode: {s:?}"),
            TimecodeError::Negative => write!(f, "negative timecode"),
            TimecodeError::OutOfRange(s) => {
                write!(f, "timecode component out of range: {s:?}")
            }
        }
    }
}

impl std::error::Error for TimecodeError {}

/// Parses `HH:MM:SS.mmm`, `MM:SS.mmm`, or `SS.mmm` (fraction optional).
///
/// Minutes and seconds components must be < 60 when a larger unit is present.
pub fn parse_tc(input: &str) -> Result<f64, TimecodeError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(TimecodeError::Empty);
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 {
        return Err(TimecodeError::Malformed(s.to_string()));
    }
    let mut seconds = 0.0_f64;
    for (i, part) in parts.iter().enumerate() {
        let value: f64 = part
            .parse()
            .map_err(|_| TimecodeError::Malformed(s.to_string()))?;
        if !value.is_finite() {
            return Err(TimecodeError::Malformed(s.to_string()));
        }
        if value < 0.0 {
            return Err(TimecodeError::Negative);
        }
        // Place of this component: 2 = hours, 1 = minutes, 0 = seconds.
        let place = (parts.len() - 1 - i) as u32;
        if place > 0 && value >= 60.0 {
            return Err(TimecodeError::OutOfRange(part.to_string()));
        }
        seconds += value * 60f64.powi(place as i32);
    }
    Ok(seconds)
}

fn total_millis(secs: f64) -> i64 {
    if !secs.is_finite() || secs <= 0.0 {
        0
    } else {
        (secs * 1000.0).round() as i64
    }
}

/// `HH:MM:SS.mmm` - the exact form passed to FFmpeg `-ss` / `-to`.
/// Negative or non-finite input clamps to zero.
pub fn format_tc(secs: f64) -> String {
    let ms = total_millis(secs);
    let h = ms / 3_600_000;
    let m = (ms / 60_000) % 60;
    let s = (ms / 1_000) % 60;
    let frac = ms % 1_000;
    format!("{h:02}:{m:02}:{s:02}.{frac:03}")
}

/// `HH:MM:SS` for UI display.
pub fn format_tc_display(secs: f64) -> String {
    let ms = total_millis(secs);
    format!(
        "{:02}:{:02}:{:02}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1_000) % 60
    )
}

/// `HH-MM-SS` for the `{start_time}` / `{end_time}` filename templates.
pub fn format_tc_filename(secs: f64) -> String {
    format_tc_display(secs).replace(':', "-")
}

/// Compact human duration as shown in the clip queue: `50s`, `1m30s`.
pub fn format_duration(secs: f64) -> String {
    let total = total_millis(secs) / 1000;
    if total >= 60 {
        format!("{}m{}s", total / 60, total % 60)
    } else {
        format!("{total}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffmpeg_style_timecodes() {
        // Values straight from the PRD clip queue example.
        assert!((parse_tc("00:15:20.120").unwrap() - 920.120).abs() < 1e-9);
        assert!((parse_tc("00:02:10.500").unwrap() - 130.500).abs() < 1e-9);
        assert!((parse_tc("02:30:00").unwrap() - 9000.0).abs() < 1e-9);
        assert!((parse_tc("90.25").unwrap() - 90.25).abs() < 1e-9);
        assert!((parse_tc("12:10").unwrap() - 730.0).abs() < 1e-9);
    }

    #[test]
    fn formats_timecodes() {
        assert_eq!(format_tc(920.120), "00:15:20.120");
        assert_eq!(format_tc(130.5), "00:02:10.500");
        assert_eq!(format_tc(730.0), "00:12:10.000");
        assert_eq!(format_tc_filename(730.0), "00-12-10");
        assert_eq!(format_tc_display(9000.0), "02:30:00");
    }

    #[test]
    fn rejects_bad_timecodes() {
        assert_eq!(parse_tc(""), Err(TimecodeError::Empty));
        assert!(matches!(
            parse_tc("1:2:3:4"),
            Err(TimecodeError::Malformed(_))
        ));
        assert_eq!(parse_tc("-5"), Err(TimecodeError::Negative));
        assert!(matches!(
            parse_tc("00:99:00"),
            Err(TimecodeError::OutOfRange(_))
        ));
        assert!(matches!(parse_tc("abc"), Err(TimecodeError::Malformed(_))));
    }

    #[test]
    fn formats_durations_like_the_wireframe() {
        assert_eq!(format_duration(50.0), "50s");
        assert_eq!(format_duration(90.0), "1m30s");
    }
}
