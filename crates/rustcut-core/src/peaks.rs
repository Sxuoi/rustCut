//! Loud-moment detection (PRD Phase 3: "automated audio peak detection").
//!
//! Pure logic over a normalized peak envelope: no FFmpeg, no I/O. The app
//! decodes audio into one peak per hop, then this turns threshold crossings
//! into candidate clip ranges with padding and merging.

/// A candidate range where the audio sits well above its own baseline.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LoudSegment {
    pub start_sec: f64,
    pub end_sec: f64,
    /// Highest peak inside the segment (0.0..=1.0).
    pub score: f32,
}

/// Finds loud moments in a normalized peak envelope.
///
/// `peaks` are 0.0..=1.0 magnitudes, one per equal slice of `duration_sec`.
/// `sensitivity` is in standard deviations above the mean: larger is stricter.
/// Runs shorter than a second are dropped, gaps under 0.6 s are merged, and
/// every segment is padded by 0.5 s so the moment is not clipped.
pub fn detect_loud_segments(peaks: &[f32], duration_sec: f64, sensitivity: f64) -> Vec<LoudSegment> {
    if peaks.is_empty() || duration_sec <= 0.0 {
        return Vec::new();
    }
    let n = peaks.len();
    let mean = peaks.iter().map(|p| *p as f64).sum::<f64>() / n as f64;
    let variance = peaks
        .iter()
        .map(|p| {
            let d = *p as f64 - mean;
            d * d
        })
        .sum::<f64>()
        / n as f64;
    let std_dev = variance.sqrt();
    // A flat envelope has no "moment" to highlight; without this, a constant
    // tone would put the threshold on top of every sample and select all of it.
    if std_dev < 0.01 {
        return Vec::new();
    }
    // Never let the adaptive threshold climb above the loudest peak: with a
    // clip that is loud for a large share of its length, mean + k*sigma can
    // exceed every sample and detect nothing at all.
    let max_peak = peaks.iter().copied().fold(0.0f32, f32::max);
    let adaptive = (mean + sensitivity.max(0.0) * std_dev) as f32;
    let threshold = adaptive.min(max_peak * 0.85).max(0.12);

    let hop = duration_sec / n as f64;
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    for (i, peak) in peaks.iter().enumerate() {
        if *peak >= threshold {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            runs.push((s, i));
        }
    }
    if let Some(s) = start {
        runs.push((s, n));
    }

    // Merge runs separated by less than 0.6 s.
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in runs {
        if let Some(last) = merged.last_mut() {
            if (s - last.1) as f64 * hop <= 0.6 {
                last.1 = e;
                continue;
            }
        }
        merged.push((s, e));
    }

    let mut out = Vec::new();
    for (s, e) in merged {
        if (e - s) as f64 * hop < 1.0 {
            continue;
        }
        let start_sec = (s as f64 * hop - 0.5).max(0.0);
        let end_sec = ((e as f64 * hop + 0.5).min(duration_sec)).min(start_sec + 60.0);
        let score = peaks[s..e].iter().copied().fold(0.0f32, f32::max);
        out.push(LoudSegment {
            start_sec,
            end_sec,
            score,
        });
    }
    out.truncate(24);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(bursts: &[(usize, usize)], n: usize) -> Vec<f32> {
        let mut v = vec![0.02f32; n];
        for (s, e) in bursts {
            for slot in v.iter_mut().take(*e).skip(*s) {
                *slot = 0.9;
            }
        }
        v
    }

    #[test]
    fn finds_a_single_loud_burst() {
        let peaks = envelope(&[(40, 60)], 100); // 4-6 s of a 10 s clip
        let found = detect_loud_segments(&peaks, 10.0, 1.5);
        assert_eq!(found.len(), 1);
        assert!((found[0].start_sec - 3.5).abs() < 0.2, "{:?}", found[0]);
        assert!((found[0].end_sec - 6.5).abs() < 0.2, "{:?}", found[0]);
        assert!(found[0].score > 0.8);
    }

    #[test]
    fn merges_nearby_bursts_and_keeps_distant_ones_apart() {
        let peaks = envelope(&[(20, 30), (32, 42), (70, 80)], 100);
        let found = detect_loud_segments(&peaks, 10.0, 1.5);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].end_sec > 4.0); // first two merged
        assert!(found[1].start_sec > 6.0);
    }

    #[test]
    fn ignores_short_spikes_and_flat_input() {
        let spikes = envelope(&[(50, 51)], 100); // 0.1 s
        assert!(detect_loud_segments(&spikes, 10.0, 1.5).is_empty());
        assert!(detect_loud_segments(&vec![0.3f32; 100], 10.0, 1.5).is_empty());
        assert!(detect_loud_segments(&[], 10.0, 1.5).is_empty());
    }

    #[test]
    fn segments_stay_inside_the_clip() {
        let peaks = envelope(&[(0, 20), (85, 100)], 100);
        let found = detect_loud_segments(&peaks, 10.0, 1.0);
        assert_eq!(found.len(), 2);
        assert!(found[0].start_sec >= 0.0);
        assert!(found[1].end_sec <= 10.0);
    }
}
