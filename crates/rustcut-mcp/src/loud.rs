//! Ranking of loud moments from momentary loudness samples (EBU R128 "M", LUFS).

/// Loudness floor used for silence (ebur128 reports -inf / -70 for digital silence).
pub const FLOOR_LUFS: f64 = -70.0;

/// Ranking options.
#[derive(Debug, Clone, Copy)]
pub struct RankOpts {
    /// How many moments to return.
    pub top_n: usize,
    /// Minimum distance between two returned moments, seconds.
    pub min_gap_sec: f64,
    /// Averaging window, seconds.
    pub window_sec: f64,
    /// Video duration, seconds (clamps the suggested clips).
    pub duration_sec: f64,
    /// Seconds before the peak in the suggested clip.
    pub lead_sec: f64,
    /// Seconds after the peak in the suggested clip.
    pub tail_sec: f64,
}

/// A loud moment.
#[derive(Debug, Clone, PartialEq)]
pub struct Moment {
    /// Time of the loudest stretch (window centre), seconds.
    pub time: f64,
    /// Windowed loudness, LUFS.
    pub loudness_lufs: f64,
    /// Windowed loudness minus the median of the track, dB.
    pub above_median_db: f64,
    /// Suggested clip start, seconds.
    pub clip_start: f64,
    /// Suggested clip end, seconds.
    pub clip_end: f64,
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return FLOOR_LUFS;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Ranks moments. `samples` are `(time_sec, momentary LUFS)` in time order.
pub fn rank_moments(samples: &[(f64, f64)], opts: RankOpts) -> Vec<Moment> {
    let clean: Vec<(f64, f64)> = samples
        .iter()
        .filter(|(t, _)| t.is_finite())
        .map(|&(t, l)| {
            (
                t,
                if l.is_finite() {
                    l.max(FLOOR_LUFS)
                } else {
                    FLOOR_LUFS
                },
            )
        })
        .collect();
    if clean.is_empty() || opts.top_n == 0 {
        return Vec::new();
    }
    // Moving average of the dB values over the window (prefix sums + two pointers).
    let mut prefix = Vec::with_capacity(clean.len() + 1);
    prefix.push(0.0);
    for (_, l) in &clean {
        prefix.push(prefix.last().copied().unwrap_or(0.0) + l);
    }
    let half = opts.window_sec.max(0.0) / 2.0;
    let (mut lo, mut hi) = (0usize, 0usize);
    let mut smooth = Vec::with_capacity(clean.len());
    for (i, (t, _)) in clean.iter().enumerate() {
        while clean[lo].0 < t - half {
            lo += 1;
        }
        hi = hi.max(i);
        while hi + 1 < clean.len() && clean[hi + 1].0 <= t + half {
            hi += 1;
        }
        smooth.push((prefix[hi + 1] - prefix[lo]) / (hi + 1 - lo) as f64);
    }
    let med = median(&smooth);
    let mut order: Vec<usize> = (0..clean.len()).collect();
    order.sort_by(|&a, &b| smooth[b].total_cmp(&smooth[a]));
    let mut picked: Vec<usize> = Vec::new();
    for i in order {
        if picked.len() >= opts.top_n || smooth[i] - med <= 0.0 {
            break;
        }
        let t = clean[i].0;
        if picked
            .iter()
            .all(|&j| (clean[j].0 - t).abs() >= opts.min_gap_sec)
        {
            picked.push(i);
        }
    }
    picked
        .into_iter()
        .map(|i| {
            let t = clean[i].0;
            let start = (t - opts.lead_sec).max(0.0);
            let end = (t + opts.tail_sec).min(opts.duration_sec.max(t));
            Moment {
                time: t,
                loudness_lufs: smooth[i],
                above_median_db: smooth[i] - med,
                clip_start: start,
                clip_end: end,
            }
        })
        .collect()
}

/// Parses `ametadata=mode=print:key=lavfi.r128.M:file=-` output into `(time, LUFS)`.
pub fn parse_ametadata(text: &str) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut time: Option<f64> = None;
    for line in text.lines() {
        if let Some(rest) = line.split("pts_time:").nth(1) {
            time = rest.split_whitespace().next().and_then(|v| v.parse().ok());
        } else if let Some(v) = line.trim().strip_prefix("lavfi.r128.M=")
            && let Some(t) = time.take()
        {
            let l = v.trim().parse::<f64>().unwrap_or(FLOOR_LUFS);
            out.push((t, l));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> RankOpts {
        RankOpts {
            top_n: 3,
            min_gap_sec: 20.0,
            window_sec: 2.0,
            duration_sec: 300.0,
            lead_sec: 10.0,
            tail_sec: 5.0,
        }
    }

    /// 10 Hz samples of a quiet track with two loud bursts (at 100 s and 200 s) and a near
    /// neighbour of the first (at 105 s, quieter).
    fn synthetic() -> Vec<(f64, f64)> {
        (0..3000)
            .map(|i| {
                let t = f64::from(i) / 10.0;
                let l = if (99.0..101.0).contains(&t) {
                    -8.0
                } else if (199.0..201.0).contains(&t) {
                    -12.0
                } else if (104.0..106.0).contains(&t) {
                    -15.0
                } else {
                    -40.0
                };
                (t, l)
            })
            .collect()
    }

    #[test]
    fn ranks_bursts_loudest_first_and_respects_the_gap() {
        let m = rank_moments(&synthetic(), opts());
        assert_eq!(m.len(), 2, "{m:?}");
        assert!((m[0].time - 100.0).abs() < 1.5);
        assert!((m[1].time - 200.0).abs() < 1.5);
        assert!(m[0].above_median_db > m[1].above_median_db);
        assert!(m[0].above_median_db > 25.0);
        assert!((m[0].clip_end - m[0].clip_start - 15.0).abs() < 2.0);
    }

    #[test]
    fn clamps_clip_to_the_video() {
        let samples: Vec<(f64, f64)> = (0..100)
            .map(|i| (f64::from(i) / 10.0, if i < 5 { -5.0 } else { -50.0 }))
            .collect();
        let o = RankOpts {
            duration_sec: 10.0,
            ..opts()
        };
        let m = rank_moments(&samples, o);
        assert_eq!(m[0].clip_start, 0.0);
        assert!(m[0].clip_end <= 10.0);
    }

    #[test]
    fn flat_or_empty_input_yields_nothing() {
        assert!(rank_moments(&[], opts()).is_empty());
        let flat: Vec<(f64, f64)> = (0..100).map(|i| (f64::from(i) / 10.0, -30.0)).collect();
        assert!(rank_moments(&flat, opts()).is_empty());
    }

    #[test]
    fn parses_ametadata_output() {
        let text = "frame:0    pts:0       pts_time:0\nlavfi.r128.M=-70.000\nframe:1    pts:4800    pts_time:0.1\nlavfi.r128.M=-23.5\n";
        let s = parse_ametadata(text);
        assert_eq!(s, vec![(0.0, -70.0), (0.1, -23.5)]);
    }
}
