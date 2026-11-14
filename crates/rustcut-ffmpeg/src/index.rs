//! Media index (PLY-06): keyframes and frame presentation times relative to
//! the source's first PTS (§7.3 time origin), for frame stepping on VFR
//! sources (PLY-04), the lossless lead-in (EXP-02, EXP-11) and *Snap In to
//! keyframe* (MRK-07).
//!
//! Built from the §7.6 A packet scan ([`parse_packet_csv`]) or, for MP4/MOV,
//! from the sample tables read by `rustcut-media` ([`MediaIndex::from_raw`]).

use rustcut_model::{IndexSource, MediaIndexInfo};

/// Frame times are sent to the UI only up to this many frames.
pub const MAX_FRAME_TIMES_IN_INFO: usize = 500_000;

/// Tolerance when comparing a time with a frame/keyframe time (float noise
/// only; packet times have microsecond precision).
const EPS: f64 = 1e-7;
/// Half a millisecond: stored times are rounded to ms, so a position within
/// 0.5 ms of a frame is "on" that frame for stepping.
const STEP_EPS: f64 = 0.0005;

/// Keyframe and frame times of the first video stream (seconds, ascending).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaIndex {
    pub keyframes: Vec<f64>,
    /// Every presented frame's time.
    pub frame_times: Vec<f64>,
    /// Median frame duration (`None` with fewer than two frames).
    pub frame_duration: Option<f64>,
    /// Frame durations vary (more than 1 % of intervals off the median).
    pub vfr: bool,
}

impl MediaIndex {
    /// Build from source-relative times in any order (sample tables, caches).
    /// Non-finite values are dropped; duplicates are merged.
    pub fn from_raw(frame_times: Vec<f64>, keyframes: Vec<f64>) -> Self {
        let frame_times = sorted_unique(frame_times);
        let keyframes = sorted_unique(keyframes);
        let (frame_duration, vfr) = analyse(&frame_times);
        MediaIndex {
            keyframes,
            frame_times,
            frame_duration,
            vfr,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.frame_times.is_empty() && self.keyframes.is_empty()
    }

    /// The keyframe FFmpeg's input `-ss t` starts a stream copy from: the
    /// last keyframe at or before `t`.
    pub fn keyframe_at_or_before(&self, t: f64) -> Option<f64> {
        let idx = self.keyframes.partition_point(|k| *k <= t + EPS);
        idx.checked_sub(1).map(|i| self.keyframes[i])
    }

    /// Lossless lead-in of a clip starting at `in_sec` (EXP-02): how much
    /// earlier than In the output starts. `Some(0.0)` when In precedes the
    /// first keyframe; `None` without keyframes (unknown; EXP-11 assumes 5 s).
    pub fn lead_in(&self, in_sec: f64) -> Option<f64> {
        if self.keyframes.is_empty() {
            return None;
        }
        Some(
            self.keyframe_at_or_before(in_sec)
                .map_or(0.0, |k| (in_sec - k).max(0.0)),
        )
    }

    /// First frame strictly after `t` (more than 0.5 ms later).
    pub fn next_frame_after(&self, t: f64) -> Option<f64> {
        let idx = self.frame_times.partition_point(|f| *f <= t + STEP_EPS);
        self.frame_times.get(idx).copied()
    }

    /// Last frame strictly before `t` (more than 0.5 ms earlier).
    pub fn prev_frame_before(&self, t: f64) -> Option<f64> {
        let idx = self.frame_times.partition_point(|f| *f < t - STEP_EPS);
        idx.checked_sub(1).map(|i| self.frame_times[i])
    }

    /// The UI summary (PLY-06). Frame times are included only for VFR
    /// sources with at most [`MAX_FRAME_TIMES_IN_INFO`] frames.
    pub fn to_info(&self, source: IndexSource, complete: bool) -> MediaIndexInfo {
        let send_frames = self.vfr && self.frame_times.len() <= MAX_FRAME_TIMES_IN_INFO;
        MediaIndexInfo {
            keyframes: self.keyframes.clone(),
            frame_duration_sec: self.frame_duration,
            vfr: self.vfr,
            frame_times: send_frames.then(|| self.frame_times.clone()),
            source,
            complete,
        }
    }
}

/// Parse the §7.6 A packet scan (`pts_time,flags` per video packet; FFmpeg 9
/// adds a trailing comma). Times become relative to `start_time_sec` (the
/// format start time). Packets arrive in decode order and are sorted;
/// `N/A` times and discarded packets (`_D_`) are skipped.
pub fn parse_packet_csv(text: &str, start_time_sec: f64) -> MediaIndex {
    let origin = if start_time_sec.is_finite() {
        start_time_sec
    } else {
        0.0
    };
    let mut frames = Vec::new();
    let mut keys = Vec::new();
    for line in text.lines() {
        let mut fields = line.trim().split(',');
        let Some(Ok(pts)) = fields.next().map(|f| f.trim().parse::<f64>()) else {
            continue;
        };
        let flags = fields.next().unwrap_or("").trim().as_bytes();
        if flags.get(1) == Some(&b'D') || !pts.is_finite() {
            continue;
        }
        let t = round_us(pts - origin);
        frames.push(t);
        if flags.first() == Some(&b'K') {
            keys.push(t);
        }
    }
    MediaIndex::from_raw(frames, keys)
}

fn round_us(t: f64) -> f64 {
    (t * 1_000_000.0).round() / 1_000_000.0
}

fn sorted_unique(mut v: Vec<f64>) -> Vec<f64> {
    v.retain(|t| t.is_finite());
    v.sort_by(f64::total_cmp);
    v.dedup_by(|a, b| (*a - *b).abs() <= EPS);
    v
}

/// Median frame duration and VFR flag. An interval counts as irregular when
/// it differs from the median by more than max(10 %, 1.5 ms); the 1.5 ms
/// floor absorbs Matroska's millisecond timestamp rounding (144 fps →
/// 6/7 ms alternation). VFR = more than 1 % of intervals irregular.
fn analyse(frames: &[f64]) -> (Option<f64>, bool) {
    let mut gaps: Vec<f64> = frames
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|d| *d > EPS)
        .collect();
    if gaps.is_empty() {
        return (None, false);
    }
    gaps.sort_by(f64::total_cmp);
    let mid = gaps.len() / 2;
    let median = if gaps.len().is_multiple_of(2) {
        (gaps[mid - 1] + gaps[mid]) / 2.0
    } else {
        gaps[mid]
    };
    let tolerance = (median * 0.1).max(0.0015);
    let irregular = gaps
        .iter()
        .filter(|g| (**g - median).abs() > tolerance)
        .count();
    let vfr = gaps.len() >= 2 && irregular as f64 > gaps.len() as f64 * 0.01;
    (Some(median), vfr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfr(fps: f64, n: usize, gop: usize) -> MediaIndex {
        let frames: Vec<f64> = (0..n).map(|i| i as f64 / fps).collect();
        let keys: Vec<f64> = frames.iter().step_by(gop).copied().collect();
        MediaIndex::from_raw(frames, keys)
    }

    #[test]
    fn ply06_packet_csv_relative_sorted_with_trailing_comma() {
        let csv = "1.466667,K__,\n1.600000,___,\n1.533333,___,\nN/A,___,\n1.500000,___\r\n2.466667,K__,\n1.400000,_D_,\n";
        let idx = parse_packet_csv(csv, 1.4);
        assert_eq!(
            idx.frame_times,
            vec![0.066667, 0.1, 0.133333, 0.2, 1.066667]
        );
        assert_eq!(idx.keyframes, vec![0.066667, 1.066667]);
    }

    #[test]
    fn exp02_keyframe_at_or_before_and_lead_in() {
        let idx = cfr(30.0, 300, 60); // keyframes every 2 s
        assert_eq!(idx.keyframe_at_or_before(3.0), Some(2.0));
        assert_eq!(idx.keyframe_at_or_before(2.0), Some(2.0));
        assert_eq!(idx.keyframe_at_or_before(1.999), Some(0.0));
        assert!((idx.lead_in(3.0).unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(idx.lead_in(4.0), Some(0.0));
        let late = MediaIndex::from_raw(vec![0.5, 1.0], vec![0.5]);
        assert_eq!(late.keyframe_at_or_before(0.2), None);
        assert_eq!(late.lead_in(0.2), Some(0.0));
        assert_eq!(MediaIndex::default().lead_in(3.0), None);
    }

    #[test]
    fn ply04_frame_stepping_tolerates_ms_rounding() {
        let idx = cfr(30.0, 90, 30);
        // 1/30 = 0.033333…; a stored position of 0.033 is "on" that frame.
        assert!((idx.next_frame_after(0.033).unwrap() - 2.0 / 30.0).abs() < 1e-9);
        assert!((idx.prev_frame_before(0.067).unwrap() - 1.0 / 30.0).abs() < 1e-9);
        assert_eq!(idx.prev_frame_before(0.0), None);
        assert_eq!(idx.next_frame_after(10.0), None);
    }

    #[test]
    fn cfr_is_not_vfr_even_with_matroska_ms_rounding() {
        let idx = cfr(59.94, 600, 120);
        assert!(!idx.vfr);
        assert!((idx.frame_duration.unwrap() - 1.0 / 59.94).abs() < 1e-6);
        // 144 fps stored with 1 ms precision alternates 6/7 ms intervals.
        let frames: Vec<f64> = (0..1440)
            .map(|i| (i as f64 / 144.0 * 1000.0).round() / 1000.0)
            .collect();
        assert!(!MediaIndex::from_raw(frames, vec![0.0]).vfr);
    }

    #[test]
    fn vfr_detected_and_frame_times_sent() {
        // Phone-style VFR: 30 fps with stretches at 24 fps.
        let mut t = 0.0;
        let mut frames = Vec::new();
        for i in 0..600 {
            frames.push(t);
            t += if (i / 50) % 2 == 0 {
                1.0 / 30.0
            } else {
                1.0 / 24.0
            };
        }
        let idx = MediaIndex::from_raw(frames, vec![0.0]);
        assert!(idx.vfr);
        let info = idx.to_info(IndexSource::PacketScan, true);
        assert_eq!(info.frame_times.as_ref().map(Vec::len), Some(600));
        assert!(info.vfr && info.complete);
        let cfr_info = cfr(30.0, 60, 30).to_info(IndexSource::SampleTable, false);
        assert_eq!(cfr_info.frame_times, None);
        assert_eq!(cfr_info.keyframes, vec![0.0, 1.0]);
    }

    #[test]
    fn frame_times_capped_at_500k() {
        let mut idx = cfr(30.0, 10, 5);
        idx.vfr = true;
        idx.frame_times = vec![0.0; MAX_FRAME_TIMES_IN_INFO + 1];
        assert_eq!(idx.to_info(IndexSource::Cache, true).frame_times, None);
    }

    #[test]
    fn from_raw_sorts_dedups_and_drops_nan() {
        let idx = MediaIndex::from_raw(vec![0.2, f64::NAN, 0.1, 0.1, 0.0], vec![0.1, 0.0, 0.0]);
        assert_eq!(idx.frame_times, vec![0.0, 0.1, 0.2]);
        assert_eq!(idx.keyframes, vec![0.0, 0.1]);
        assert!(!idx.is_empty());
        assert!(MediaIndex::from_raw(vec![], vec![]).is_empty());
        assert_eq!(
            MediaIndex::from_raw(vec![1.0], vec![1.0]).frame_duration,
            None
        );
    }
}
