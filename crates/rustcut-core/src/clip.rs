//! Clip segment model - the non-destructive core of the app.
//!
//! A clip is a timestamp range over the source video. Nothing here ever
//! touches the source file.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

pub const DEFAULT_COLOR: &str = "#3B82F6";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: Uuid,
    pub index: u32,
    pub name: String,
    pub start_sec: f64,
    pub end_sec: f64,
    pub color: String,
    /// Horizontal pan-and-scan offset in `[-1.0, +1.0]` across the 16:9
    /// working canvas (`-1` left edge, `0` center, `+1` right edge).
    pub crop_x_offset: f64,
    pub selected_for_export: bool,
    pub export_status: ExportStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipError {
    NonPositiveDuration,
    NonFiniteTime,
}

impl fmt::Display for ClipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClipError::NonPositiveDuration => {
                write!(f, "end time must be greater than start time")
            }
            ClipError::NonFiniteTime => write!(f, "clip times must be finite numbers"),
        }
    }
}

impl std::error::Error for ClipError {}

/// Clamps a pan-and-scan offset into `[-1.0, +1.0]`; NaN maps to center.
pub fn clamp_offset(offset: f64) -> f64 {
    if offset.is_nan() {
        0.0
    } else {
        offset.clamp(-1.0, 1.0)
    }
}

impl Clip {
    /// Strict constructor: `end_sec` must be greater than `start_sec`.
    pub fn new(
        index: u32,
        name: impl Into<String>,
        start_sec: f64,
        end_sec: f64,
    ) -> Result<Self, ClipError> {
        if !start_sec.is_finite() || !end_sec.is_finite() {
            return Err(ClipError::NonFiniteTime);
        }
        if end_sec <= start_sec {
            return Err(ClipError::NonPositiveDuration);
        }
        Ok(Self {
            id: Uuid::new_v4(),
            index,
            name: name.into(),
            start_sec,
            end_sec,
            color: DEFAULT_COLOR.to_string(),
            crop_x_offset: 0.0,
            selected_for_export: true,
            export_status: ExportStatus::Pending,
        })
    }

    /// PRD edge case: when Mark Out lands before Mark In, swap the pair
    /// instead of rejecting the input.
    pub fn new_swapping(
        index: u32,
        name: impl Into<String>,
        mark_a: f64,
        mark_b: f64,
    ) -> Result<Self, ClipError> {
        let (start_sec, end_sec) = if mark_a <= mark_b {
            (mark_a, mark_b)
        } else {
            (mark_b, mark_a)
        };
        Self::new(index, name, start_sec, end_sec)
    }

    pub fn duration_sec(&self) -> f64 {
        self.end_sec - self.start_sec
    }

    pub fn validate(&self) -> Result<(), ClipError> {
        if !self.start_sec.is_finite() || !self.end_sec.is_finite() {
            return Err(ClipError::NonFiniteTime);
        }
        if self.end_sec <= self.start_sec {
            return Err(ClipError::NonPositiveDuration);
        }
        Ok(())
    }

    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = color.into();
        self
    }

    pub fn with_crop_x_offset(mut self, offset: f64) -> Self {
        self.crop_x_offset = clamp_offset(offset);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_and_measures() {
        let clip = Clip::new(1, "Opening Moment", 130.5, 180.0).unwrap();
        assert_eq!(clip.index, 1);
        assert_eq!(clip.name, "Opening Moment");
        assert!((clip.duration_sec() - 49.5).abs() < 1e-9);
        assert_eq!(clip.export_status, ExportStatus::Pending);
        assert!(clip.selected_for_export);
        assert_eq!(clip.color, DEFAULT_COLOR);
    }

    #[test]
    fn rejects_zero_or_reversed_ranges() {
        assert_eq!(
            Clip::new(1, "x", 5.0, 5.0),
            Err(ClipError::NonPositiveDuration)
        );
        assert_eq!(
            Clip::new(1, "x", 9.0, 5.0),
            Err(ClipError::NonPositiveDuration)
        );
        assert_eq!(
            Clip::new(1, "x", f64::NAN, 5.0),
            Err(ClipError::NonFiniteTime)
        );
    }

    #[test]
    fn swaps_marks_when_out_precedes_in() {
        let clip = Clip::new_swapping(2, "Jump Scare React", 955.800, 920.120).unwrap();
        assert_eq!(clip.start_sec, 920.120);
        assert_eq!(clip.end_sec, 955.800);
    }

    #[test]
    fn clamps_crop_offsets() {
        assert_eq!(clamp_offset(0.25), 0.25);
        assert_eq!(clamp_offset(-3.0), -1.0);
        assert_eq!(clamp_offset(42.0), 1.0);
        assert_eq!(clamp_offset(f64::NAN), 0.0);
        let clip = Clip::new(1, "x", 0.0, 1.0)
            .unwrap()
            .with_crop_x_offset(7.5);
        assert_eq!(clip.crop_x_offset, 1.0);
    }
}
