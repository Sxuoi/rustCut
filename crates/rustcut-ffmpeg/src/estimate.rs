//! Pre-flight size estimates (EXP-11). All results are **bytes**: bit rates
//! are divided by 8 exactly once.
//!
//! * Lossless: `bytes ≈ source_bitrate_bps / 8 × Σ (duration + lead_in)`,
//!   lead-in from the media index, [`UNKNOWN_LEAD_IN_SEC`] when unknown.
//! * Re-encode: `bytes ≈ ((video_maxrate_bps + audio_bitrate_bps) × Σ duration
//!   + bufsize_bits × clip_count) / 8`, an upper bound for capped presets.
//! * Frame-Accurate *Original* has no bit-rate cap, so its nominal video
//!   rate is `max(source bit rate, 8 Mbps) × 1.5` (CRF 18 rarely exceeds the
//!   source by half; low-bit-rate sources still get a realistic floor), plus
//!   192 kbps per audio track and no `bufsize` term.

use crate::args::{AAC_KBPS, VERTICAL_BUFSIZE_BITS, VERTICAL_MAXRATE_BPS};
use rustcut_model::{ExportSettings, Preset, SourceInfo};

/// Lead-in assumed per lossless clip when the media index is not ready.
pub const UNKNOWN_LEAD_IN_SEC: f64 = 5.0;
/// Floor of the Frame-Accurate *Original* video estimate.
pub const ACCURATE_MIN_VIDEO_BPS: u64 = 8_000_000;
/// Headroom factor of the Frame-Accurate *Original* video estimate.
pub const ACCURATE_HEADROOM: f64 = 1.5;
/// Audio bit rate of one re-encoded track.
pub const AUDIO_TRACK_BPS: u64 = AAC_KBPS as u64 * 1000;

/// Nominal rates of a re-encode preset (EXP-11 inputs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresetRates {
    pub video_bps: u64,
    pub audio_bps: u64,
    pub bufsize_bits: u64,
}

fn non_negative(v: f64) -> f64 {
    if v.is_finite() && v > 0.0 { v } else { 0.0 }
}

fn to_bytes(bits: f64) -> u64 {
    // Rounded (not ceiled: 35.7 s is inexact in binary) and saturating.
    (bits / 8.0).round() as u64
}

/// Lossless estimate in bytes. `clips` are `(duration_sec, lead_in_sec)`;
/// a `None` lead-in counts as [`UNKNOWN_LEAD_IN_SEC`].
pub fn lossless_bytes(source_bitrate_bps: u64, clips: &[(f64, Option<f64>)]) -> u64 {
    let seconds: f64 = clips
        .iter()
        .map(|(d, lead)| non_negative(*d) + lead.map_or(UNKNOWN_LEAD_IN_SEC, non_negative))
        .sum();
    to_bytes(source_bitrate_bps as f64 * seconds)
}

/// Re-encode estimate in bytes (upper bound when the preset caps the rate).
pub fn reencode_bytes(
    video_maxrate_bps: u64,
    audio_bitrate_bps: u64,
    bufsize_bits: u64,
    durations: &[f64],
) -> u64 {
    let seconds: f64 = durations.iter().copied().map(non_negative).sum();
    let rate = video_maxrate_bps as f64 + audio_bitrate_bps as f64;
    to_bytes(rate * seconds + bufsize_bits as f64 * durations.len() as f64)
}

/// Source bit rate: the probed overall rate, else file size × 8 ÷ duration.
pub fn source_bitrate_bps(source: &SourceInfo) -> Option<u64> {
    source.bit_rate.filter(|b| *b > 0).or_else(|| {
        let d = source.duration_sec;
        (d.is_finite() && d > 0.0 && source.fingerprint.size_bytes > 0)
            .then(|| (source.fingerprint.size_bytes as f64 * 8.0 / d) as u64)
    })
}

/// Nominal rates of the selected re-encode preset; `None` for Lossless.
pub fn nominal_rates(source: &SourceInfo, settings: &ExportSettings) -> Option<PresetRates> {
    let tracks = source.audio_tracks.len() as u64;
    match settings.preset() {
        Preset::Lossless => None,
        Preset::Accurate => {
            let base = source_bitrate_bps(source)
                .unwrap_or(0)
                .max(ACCURATE_MIN_VIDEO_BPS);
            Some(PresetRates {
                video_bps: (base as f64 * ACCURATE_HEADROOM) as u64,
                audio_bps: AUDIO_TRACK_BPS * tracks,
                bufsize_bits: 0,
            })
        }
        Preset::Vcrop | Preset::Vblur => Some(PresetRates {
            video_bps: VERTICAL_MAXRATE_BPS,
            // One stereo track (chosen or mixed), none for silent sources.
            audio_bps: if tracks > 0 { AUDIO_TRACK_BPS } else { 0 },
            bufsize_bits: VERTICAL_BUFSIZE_BITS,
        }),
    }
}

/// Batch estimate in bytes for the selected preset. `clips` are
/// `(duration_sec, lead_in_sec)`; lead-ins only matter for Lossless.
/// `None` when a lossless source's bit rate cannot be determined.
pub fn estimate_bytes(
    source: &SourceInfo,
    settings: &ExportSettings,
    clips: &[(f64, Option<f64>)],
) -> Option<u64> {
    match nominal_rates(source, settings) {
        None => source_bitrate_bps(source).map(|bps| lossless_bytes(bps, clips)),
        Some(r) => {
            let durations: Vec<f64> = clips.iter().map(|c| c.0).collect();
            Some(reencode_bytes(
                r.video_bps,
                r.audio_bps,
                r.bufsize_bits,
                &durations,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::source;

    #[test]
    fn exp11_lossless_formula_with_unknown_lead_in() {
        // 8 Mbps = 1 MB/s; 10 s + 2 s lead-in, and 20 s + 5 s assumed.
        assert_eq!(
            lossless_bytes(8_000_000, &[(10.0, Some(2.0)), (20.0, None)]),
            37_000_000
        );
        assert_eq!(lossless_bytes(8_000_000, &[]), 0);
        assert_eq!(lossless_bytes(8_000_000, &[(-3.0, Some(-1.0))]), 0);
    }

    #[test]
    fn exp11_reencode_formula_is_bytes_not_bits() {
        // PRD: a 35.7 s vertical clip averaged 16.7 Mbps against the 16 Mbps cap.
        let est = reencode_bytes(16_000_000, 192_000, 32_000_000, &[35.7]);
        assert_eq!(est, 76_256_800);
        let measured = (16_700_000.0 * 35.7 / 8.0) as u64;
        assert!(
            est >= measured,
            "estimate must be an upper bound: {est} < {measured}"
        );
        // The 8x trap: a bit count would be ~610 MB.
        assert!(est < 100_000_000);
        assert_eq!(
            reencode_bytes(16_000_000, 192_000, 32_000_000, &[10.0, 10.0]),
            (323_840_000 + 64_000_000) / 8
        );
    }

    #[test]
    fn exp11_nominal_rates_per_preset() {
        let mut s = ExportSettings::default();
        let src = source("mov,mp4", "a.mp4", "h264", &["aac", "aac"]);
        assert_eq!(nominal_rates(&src, &s), None);
        s.apply_preset(Preset::Vcrop);
        let v = nominal_rates(&src, &s).unwrap();
        assert_eq!(
            v,
            PresetRates {
                video_bps: 16_000_000,
                audio_bps: 192_000,
                bufsize_bits: 32_000_000
            }
        );
        s.apply_preset(Preset::Accurate);
        // 8 Mbps source → max(8M, 8M) × 1.5; two tracks.
        let a = nominal_rates(&src, &s).unwrap();
        assert_eq!(
            a,
            PresetRates {
                video_bps: 12_000_000,
                audio_bps: 384_000,
                bufsize_bits: 0
            }
        );
        let mut rich = src.clone();
        rich.bit_rate = Some(50_000_000);
        assert_eq!(nominal_rates(&rich, &s).unwrap().video_bps, 75_000_000);
        let silent = source("mov,mp4", "a.mp4", "h264", &[]);
        s.apply_preset(Preset::Vblur);
        assert_eq!(nominal_rates(&silent, &s).unwrap().audio_bps, 0);
    }

    #[test]
    fn exp11_estimate_bytes_end_to_end() {
        let mut src = source("mov,mp4", "a.mp4", "h264", &["aac"]);
        let s = ExportSettings::default();
        assert_eq!(
            estimate_bytes(&src, &s, &[(10.0, Some(0.0))]),
            Some(10_000_000)
        );
        src.bit_rate = None; // falls back to size / duration: 60 MB over 60 s
        assert_eq!(source_bitrate_bps(&src), Some(8_000_000));
        src.fingerprint.size_bytes = 0;
        assert_eq!(estimate_bytes(&src, &s, &[(10.0, None)]), None);
        let mut v = ExportSettings::default();
        v.apply_preset(Preset::Vcrop);
        assert_eq!(estimate_bytes(&src, &v, &[(35.7, None)]), Some(76_256_800));
    }
}
