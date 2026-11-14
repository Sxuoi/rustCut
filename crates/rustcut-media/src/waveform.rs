//! Audio waveform (PLY-05): FFmpeg decodes one track to mono 8 kHz signed 16-bit PCM on stdout;
//! we reduce it to 100 min/max peaks per second, send them progressively in chunks of about
//! 2 s and cache the result per fingerprint and track.

use std::path::{Path, PathBuf};

use rustcut_model::{SourceInfo, WaveformChunk};
use rustcut_runner::{CancelToken, SpawnOptions, spawn_tracked};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::{MediaError, MediaService, file_name};

/// Peaks per second of media (PLY-05 asks for at least 50).
pub const PEAKS_PER_SEC: u32 = 100;
const SAMPLE_RATE: u32 = 8000;
const SAMPLES_PER_PEAK: u32 = SAMPLE_RATE / PEAKS_PER_SEC;
/// A chunk carries about 2 s of peaks.
const CHUNK_PEAKS: usize = 2 * PEAKS_PER_SEC as usize;
const CACHE_MAGIC: &[u8; 4] = b"RCWF";
const CACHE_VERSION: u32 = 1;
const CACHE_HEADER: usize = 12;

/// Number of peaks of a waveform for a source of `duration_sec`: `ceil(duration * 100)`, robust
/// against float noise such as `12.0 * 100 = 1200.0000000000002`.
pub fn total_peaks_for(duration_sec: f64) -> u32 {
    if !duration_sec.is_finite() || duration_sec <= 0.0 {
        return 0;
    }
    ((duration_sec * f64::from(PEAKS_PER_SEC)) - 1e-6)
        .ceil()
        .max(0.0) as u32
}

/// Scales a 16-bit sample to `-127..=127`.
fn scale(v: i16) -> i8 {
    (f32::from(v) * 127.0 / 32767.0)
        .round()
        .clamp(-127.0, 127.0) as i8
}

/// Collects peaks, sends them in chunks and pads or trims to exactly `total` peaks.
struct PeakSink<F: FnMut(WaveformChunk)> {
    track: u32,
    total: u32,
    /// Peaks accepted so far.
    pushed: u32,
    /// Index of the first peak in `pending`.
    pending_start: u32,
    pending: Vec<i8>,
    all: Vec<i8>,
    on_chunk: F,
}

impl<F: FnMut(WaveformChunk)> PeakSink<F> {
    fn new(track: u32, total: u32, on_chunk: F) -> Self {
        Self {
            track,
            total,
            pushed: 0,
            pending_start: 0,
            pending: Vec::new(),
            all: Vec::with_capacity(total as usize * 2),
            on_chunk,
        }
    }

    fn push(&mut self, min: i8, max: i8) {
        if self.pushed >= self.total {
            return;
        }
        self.pending.extend([min, max]);
        self.all.extend([min, max]);
        self.pushed += 1;
        if self.pending.len() >= CHUNK_PEAKS * 2 {
            self.flush(false);
        }
    }

    fn flush(&mut self, done: bool) {
        let peaks = std::mem::take(&mut self.pending);
        let start_peak = self.pending_start;
        self.pending_start += (peaks.len() / 2) as u32;
        (self.on_chunk)(WaveformChunk {
            track: self.track,
            peaks_per_sec: PEAKS_PER_SEC,
            start_peak,
            peaks,
            total_peaks: self.total,
            done,
        });
    }

    /// Pads with silence up to `total`, sends the final chunk and returns every peak.
    fn finish(mut self) -> Vec<i8> {
        while self.pushed < self.total {
            self.push(0, 0);
        }
        self.flush(true);
        self.all
    }
}

/// Reduces PCM bytes to peaks.
struct PeakReducer {
    carry: Option<u8>,
    count: u32,
    min: i16,
    max: i16,
}

impl PeakReducer {
    fn new() -> Self {
        Self {
            carry: None,
            count: 0,
            min: i16::MAX,
            max: i16::MIN,
        }
    }

    fn feed<F: FnMut(WaveformChunk)>(&mut self, bytes: &[u8], sink: &mut PeakSink<F>) {
        let mut bytes = bytes;
        if let Some(lo) = self.carry.take() {
            let Some((&hi, rest)) = bytes.split_first() else {
                self.carry = Some(lo);
                return;
            };
            bytes = rest;
            self.sample(i16::from_le_bytes([lo, hi]), sink);
        }
        let (pairs, rest) = bytes.as_chunks::<2>();
        for p in pairs {
            self.sample(i16::from_le_bytes(*p), sink);
        }
        if let [lo] = rest {
            self.carry = Some(*lo);
        }
    }

    fn sample<F: FnMut(WaveformChunk)>(&mut self, s: i16, sink: &mut PeakSink<F>) {
        self.min = self.min.min(s);
        self.max = self.max.max(s);
        self.count += 1;
        if self.count == SAMPLES_PER_PEAK {
            self.flush_peak(sink);
        }
    }

    fn flush_peak<F: FnMut(WaveformChunk)>(&mut self, sink: &mut PeakSink<F>) {
        if self.count > 0 {
            sink.push(scale(self.min), scale(self.max));
        }
        self.count = 0;
        self.min = i16::MAX;
        self.max = i16::MIN;
    }
}

fn encode_cache(total: u32, peaks: &[i8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(CACHE_HEADER + peaks.len());
    out.extend_from_slice(CACHE_MAGIC);
    out.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    out.extend_from_slice(&total.to_le_bytes());
    out.extend(peaks.iter().map(|p| *p as u8));
    out
}

fn decode_cache(bytes: &[u8]) -> Option<(u32, Vec<i8>)> {
    if bytes.len() < CACHE_HEADER || &bytes[..4] != CACHE_MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let total = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let data = &bytes[CACHE_HEADER..];
    (version == CACHE_VERSION && data.len() == total as usize * 2)
        .then(|| (total, data.iter().map(|b| *b as i8).collect()))
}

fn replay<F: FnMut(WaveformChunk)>(track: u32, total: u32, peaks: &[i8], on_chunk: F) {
    let mut sink = PeakSink::new(track, total, on_chunk);
    for pair in peaks.as_chunks::<2>().0 {
        sink.push(pair[0], pair[1]);
    }
    sink.finish();
}

/// FFmpeg arguments that decode audio `track` (1-based) to mono 8 kHz s16le on stdout.
pub(crate) fn waveform_args(path: &Path, track: u32) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = ["-hide_banner", "-nostdin", "-v", "error", "-i"]
        .iter()
        .map(Into::into)
        .collect();
    args.push(rustcut_ffmpeg::safe_path_arg(path.as_os_str()));
    let tail = [
        "-map".to_string(),
        format!("0:a:{}", track.saturating_sub(1)),
        "-vn".into(),
        "-ac".into(),
        "1".into(),
        "-ar".into(),
        SAMPLE_RATE.to_string(),
        "-f".into(),
        "s16le".into(),
        "pipe:1".into(),
    ];
    args.extend(tail.into_iter().map(Into::into));
    args
}

async fn drain_stderr<R: AsyncRead + Unpin>(mut r: R) -> String {
    let mut buf = Vec::new();
    let _ = r.read_to_end(&mut buf).await;
    String::from_utf8_lossy(&buf).into_owned()
}

impl MediaService {
    fn waveform_cache_path(&self, source: &SourceInfo, track: u32) -> PathBuf {
        self.cache_subdir("waveform")
            .join(format!("{}-t{track}.bin", source.fingerprint.key()))
    }

    /// Generates the waveform of audio `track` (1-based) and delivers it through `on_chunk`
    /// (PLY-05): 100 peaks per second as interleaved min/max pairs scaled to `-127..=127`, in
    /// chunks of about 2 s (`start_peak`, `total_peaks = ceil(duration * 100)`), the last one with
    /// `done = true`. A cached waveform is replayed in the same chunks without running FFmpeg.
    /// If the decoded audio is shorter than the source it is padded with silence, if longer it is
    /// cut, so the peak count always equals `total_peaks`. A source without audio completes with
    /// no chunk.
    ///
    /// # Errors
    /// [`MediaError::NoSuchTrack`], [`MediaError::Waveform`] (FFmpeg could not decode the track),
    /// [`MediaError::Cancelled`] (the FFmpeg process is killed; nothing is cached), or
    /// [`MediaError::Runner`] when FFmpeg cannot be started.
    pub async fn generate_waveform(
        &self,
        path: &Path,
        source: &SourceInfo,
        track: u32,
        mut on_chunk: impl FnMut(WaveformChunk) + Send,
        cancel: CancelToken,
    ) -> Result<(), MediaError> {
        if source.audio_tracks.is_empty() {
            return Ok(());
        }
        if track == 0 || track as usize > source.audio_tracks.len() {
            return Err(MediaError::NoSuchTrack {
                file: file_name(path),
                track,
                available: source.audio_tracks.len(),
            });
        }
        let total = total_peaks_for(source.duration_sec);
        let cache_path = self.waveform_cache_path(source, track);
        if let Ok(bytes) = tokio::fs::read(&cache_path).await
            && let Some((cached_total, peaks)) = decode_cache(&bytes)
        {
            replay(track, cached_total, &peaks, on_chunk);
            return Ok(());
        }

        let args = waveform_args(path, track);
        let program = &self.inner.config.sidecars.ffmpeg;
        let mut child =
            spawn_tracked(program, &args, &SpawnOptions::default()).map_err(|source| {
                rustcut_runner::RunnerError::Spawn {
                    program: program.clone(),
                    source,
                }
            })?;
        let (Some(mut stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return Err(MediaError::Waveform {
                file: file_name(path),
                detail: "FFmpeg's output pipes are missing".into(),
            });
        };
        let stderr_task = tokio::spawn(drain_stderr(stderr));

        let mut sink = PeakSink::new(track, total, &mut on_chunk);
        let mut reducer = PeakReducer::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let read = tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    stderr_task.abort();
                    return Err(MediaError::Cancelled);
                }
                r = stdout.read(&mut buf) => r,
            };
            match read {
                Ok(0) => break,
                Ok(n) => reducer.feed(&buf[..n], &mut sink),
                Err(err) => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return Err(MediaError::Waveform {
                        file: file_name(path),
                        detail: err.to_string(),
                    });
                }
            }
        }
        reducer.flush_peak(&mut sink);
        let had_peaks = sink.pushed > 0;
        let status = child.wait().await;
        let stderr_text = stderr_task.await.unwrap_or_default();
        let success = status.as_ref().is_ok_and(|s| s.success());
        if !success && !had_peaks {
            return Err(MediaError::Waveform {
                file: file_name(path),
                detail: rustcut_ffmpeg::first_error_line(&stderr_text)
                    .unwrap_or_else(|| "FFmpeg could not decode the audio track".into()),
            });
        }
        let peaks = sink.finish();
        if success {
            let bytes = encode_cache(total, &peaks);
            if let Err(err) = crate::write_atomic(&cache_path, &bytes) {
                tracing::warn!(path = %cache_path.display(), %err, "could not cache the waveform");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(total: u32, samples: &[i16]) -> (Vec<WaveformChunk>, Vec<i8>) {
        let mut chunks = Vec::new();
        let mut sink = PeakSink::new(1, total, |c| chunks.push(c));
        let mut r = PeakReducer::new();
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        // feed in awkward odd-sized pieces to exercise the carry byte
        for piece in bytes.chunks(7) {
            r.feed(piece, &mut sink);
        }
        r.flush_peak(&mut sink);
        let all = sink.finish();
        (chunks, all)
    }

    #[test]
    fn total_peaks_is_ceil_of_duration_times_100() {
        assert_eq!(total_peaks_for(12.0), 1200);
        assert_eq!(total_peaks_for(12.0117), 1202);
        assert_eq!(total_peaks_for(0.004), 1);
        assert_eq!(total_peaks_for(0.0), 0);
        assert_eq!(total_peaks_for(f64::NAN), 0);
    }

    #[test]
    fn scaling_covers_the_full_range() {
        assert_eq!(scale(0), 0);
        assert_eq!(scale(32767), 127);
        assert_eq!(scale(-32768), -127);
        assert_eq!(scale(16384), 64);
        assert_eq!(scale(-1), 0);
    }

    #[test]
    fn peaks_are_min_max_pairs_per_80_samples() {
        let mut samples = vec![0i16; 160];
        samples[10] = 32767;
        samples[20] = -16384;
        samples[100] = 8192;
        let (chunks, all) = collect(2, &samples);
        assert_eq!(all, vec![-64, 127, 0, 32]);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].done);
        assert_eq!(chunks[0].total_peaks, 2);
        assert_eq!(chunks[0].peaks_per_sec, 100);
    }

    #[test]
    fn short_audio_is_padded_and_long_audio_is_cut() {
        let (chunks, all) = collect(450, &vec![1000i16; 80 * 10]);
        assert_eq!(all.len(), 900);
        assert!(all[..20].iter().all(|v| *v == scale(1000)));
        assert!(all[20..].iter().all(|v| *v == 0));
        let (_, all) = collect(5, &vec![1000i16; 80 * 10]);
        assert_eq!(all.len(), 10);
        // 450 peaks -> chunks of 200, 200, 50 peaks; contiguous start_peak; only the last is done
        let starts: Vec<u32> = chunks.iter().map(|c| c.start_peak).collect();
        assert_eq!(starts, vec![0, 200, 400]);
        let lens: Vec<usize> = chunks.iter().map(|c| c.peaks.len()).collect();
        assert_eq!(lens, vec![400, 400, 100]);
        assert_eq!(chunks.iter().filter(|c| c.done).count(), 1);
        assert!(chunks.last().unwrap().done);
    }

    #[test]
    fn partial_last_window_becomes_a_peak() {
        let (_, all) = collect(2, &[500i16; 80 + 10]);
        assert_eq!(all, vec![scale(500); 4]);
    }

    #[test]
    fn zero_total_sends_one_empty_done_chunk() {
        let (chunks, all) = collect(0, &[1, 2, 3]);
        assert!(all.is_empty());
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].done && chunks[0].peaks.is_empty());
    }

    #[test]
    fn cache_format_round_trips_and_rejects_damage() {
        let peaks: Vec<i8> = vec![-5, 5, -127, 127];
        let bytes = encode_cache(2, &peaks);
        assert_eq!(decode_cache(&bytes), Some((2, peaks.clone())));
        assert_eq!(decode_cache(&bytes[..bytes.len() - 1]), None);
        assert_eq!(decode_cache(b"nope"), None);
        let mut bad = bytes.clone();
        bad[4] = 9;
        assert_eq!(decode_cache(&bad), None);
    }

    #[test]
    fn replay_chunks_like_a_fresh_run() {
        let peaks: Vec<i8> = (0..1000).map(|i| (i % 100) as i8).collect();
        let mut chunks = Vec::new();
        replay(1, 500, &peaks, |c| chunks.push(c));
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks.iter().map(|c| c.peaks.len()).sum::<usize>(), 1000);
        assert!(chunks[2].done && !chunks[1].done);
    }
}
