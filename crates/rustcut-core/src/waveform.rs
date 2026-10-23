//! Audio waveform peaks (PRD 4.1): the pure part.
//!
//! The app layer decodes audio with FFmpeg into mono s16le PCM (see
//! `build_waveform_args`); everything here is deterministic math.

/// Sample rate used for waveform decoding: 1 kHz keeps a 5-hour VOD at
/// ~36 MB of PCM in memory while giving far more resolution than the
/// on-screen buckets can show.
pub const WAVEFORM_SAMPLE_RATE: u32 = 1000;

/// FFmpeg argv that writes mono s16le PCM to stdout. Audio-less inputs
/// produce an error, which the caller reports as "no audio track".
pub fn build_waveform_args(input: &std::path::Path, sample_rate: u32) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-map".into(),
        "0:a:0".into(),
        "-ac".into(),
        "1".into(),
        "-ar".into(),
        sample_rate.to_string(),
        "-f".into(),
        "s16le".into(),
        "-".into(),
    ]
}

/// Decodes raw little-endian 16-bit PCM bytes into samples. A trailing odd
/// byte is dropped.
pub fn pcm_s16le_samples(bytes: &[u8]) -> Vec<i16> {
    let (chunks, _remainder) = bytes.as_chunks::<2>();
    chunks.iter().map(|b| i16::from_le_bytes(*b)).collect()
}

/// Downsamples raw mono samples into `buckets` peak magnitudes in `0.0..=1.0`.
pub fn compute_peaks(samples: &[i16], buckets: usize) -> Vec<f32> {
    if buckets == 0 {
        return Vec::new();
    }
    if samples.is_empty() {
        return vec![0.0; buckets];
    }
    let mut peaks = vec![0.0f32; buckets];
    let per_bucket = (samples.len() as f64 / buckets as f64).ceil().max(1.0) as usize;
    for (i, chunk) in samples.chunks(per_bucket).enumerate() {
        if i >= buckets {
            break;
        }
        let max = chunk
            .iter()
            .map(|s| s.unsigned_abs() as u32)
            .max()
            .unwrap_or(0);
        peaks[i] = (max as f32) / (i16::MAX as f32);
    }
    peaks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_little_endian_pcm() {
        let bytes = [0x00, 0x40, 0xFF, 0xFF, 0x00, 0x80, 0x01];
        // 0x4000 = 16384, 0xFFFF = -1, 0x8000 = -32768; trailing odd byte
        // is dropped by chunks_exact(2).
        assert_eq!(pcm_s16le_samples(&bytes), vec![16384, -1, -32768]);
    }

    #[test]
    fn peaks_normalize_to_unit_range() {
        let samples = vec![i16::MAX, 0, i16::MAX / 2, 0];
        let peaks = compute_peaks(&samples, 2);
        assert_eq!(peaks.len(), 2);
        assert!((peaks[0] - 1.0).abs() < 1e-3);
        assert!((peaks[1] - 0.5).abs() < 1e-3);
        assert!(peaks.iter().all(|p| (0.0..=1.0).contains(p)));
        // Negative samples count by magnitude, and a bucket holds its peak.
        let loud = compute_peaks(&[0, -i16::MAX], 1);
        assert!((loud[0] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn silent_input_yields_flat_zero_waveform() {
        let peaks = compute_peaks(&[0i16; 1000], 10);
        assert_eq!(peaks, vec![0.0; 10]);
    }

    #[test]
    fn buckets_cover_the_whole_clip() {
        // 10 samples, 3 buckets -> per_bucket 4 -> chunks 4/4/2.
        let samples = vec![1i16, 1, 1, 1, 2, 2, 2, 2, 4, 4];
        let peaks = compute_peaks(&samples, 3);
        assert_eq!(peaks.len(), 3);
        assert!(peaks[2] > peaks[0], "loudest tail must show up");
    }

    #[test]
    fn edge_cases_do_not_panic() {
        assert!(compute_peaks(&[], 0).is_empty());
        assert_eq!(compute_peaks(&[], 4), vec![0.0; 4]);
        assert!(compute_peaks(&[1, 2, 3], 0).is_empty());
        // More buckets than samples still returns exactly `buckets` values.
        assert_eq!(compute_peaks(&[1, 2], 5).len(), 5);
    }

    #[test]
    fn waveform_args_send_pcm_to_stdout() {
        let args = build_waveform_args(std::path::Path::new("vod.mp4"), 1000);
        assert_eq!(
            args,
            vec![
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                "vod.mp4",
                "-map",
                "0:a:0",
                "-ac",
                "1",
                "-ar",
                "1000",
                "-f",
                "s16le",
                "-",
            ]
        );
    }
}
