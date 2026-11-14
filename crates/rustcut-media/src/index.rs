//! Media index (PLY-06): keyframe and frame times, from the MP4/MOV sample tables when possible,
//! else from a cancellable background packet scan; cached as versioned JSON.

use std::path::{Path, PathBuf};

use rustcut_ffmpeg::{MediaIndex, packet_index_args, parse_packet_csv};
use rustcut_model::{IndexSource, SourceInfo};
use rustcut_runner::{CancelToken, StreamOptions, run_streaming};
use serde::{Deserialize, Serialize};

use crate::mp4::{self, SampleTimes};
use crate::{MediaError, MediaService, file_name};

const CACHE_VERSION: u32 = 1;

/// A built index and where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltIndex {
    /// The index.
    pub index: MediaIndex,
    /// [`IndexSource::SampleTable`], [`IndexSource::PacketScan`], or [`IndexSource::Cache`].
    pub source: IndexSource,
}

#[derive(Serialize, Deserialize)]
struct IndexCache {
    version: u32,
    /// `"sample_table"` or `"packet_scan"`: how the cached data was originally obtained.
    origin: String,
    frame_times: Vec<f64>,
    keyframes: Vec<f64>,
}

fn round_us(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

/// Converts absolute sample-table times into times relative to the source start, like the
/// packet scan (`rustcut_ffmpeg::parse_packet_csv`).
fn from_sample_times(t: &SampleTimes, start_time_sec: f64) -> MediaIndex {
    let origin = if start_time_sec.is_finite() {
        start_time_sec
    } else {
        0.0
    };
    let rel = |v: &Vec<f64>| v.iter().map(|x| round_us(x - origin)).collect::<Vec<_>>();
    MediaIndex::from_raw(rel(&t.frame_times), rel(&t.keyframes))
}

fn is_mov_family(source: &SourceInfo) -> bool {
    source
        .format_name
        .as_deref()
        .is_some_and(|f| f.split(',').any(|n| n == "mov" || n == "mp4"))
}

impl MediaService {
    fn index_cache_path(&self, source: &SourceInfo) -> PathBuf {
        self.cache_subdir("index")
            .join(format!("{}.json", source.fingerprint.key()))
    }

    /// Builds the media index of `source` (PLY-06) and reports progress in `0.0..=1.0`.
    /// See [`build_index_cancellable`](Self::build_index_cancellable).
    ///
    /// # Errors
    /// As [`build_index_cancellable`](Self::build_index_cancellable), except cancellation.
    pub async fn build_index(
        &self,
        path: &Path,
        source: &SourceInfo,
        on_progress: impl Fn(f64) + Send,
    ) -> Result<MediaIndex, MediaError> {
        Ok(self
            .build_index_cancellable(path, source, on_progress, CancelToken::new())
            .await?
            .index)
    }

    /// Builds the media index of `source`:
    ///
    /// 1. the cache `<cache>/index/<fingerprint key>.json`;
    /// 2. MP4/MOV: the sample tables, reading only the `moov` box (instant);
    /// 3. anything else, or a fragmented/unusual MP4: an FFprobe packet scan streamed through
    ///    `run_streaming`, with progress from the packet times, cancellable through `cancel`.
    ///
    /// Frame times and keyframes are relative to the source start (section 7.3 time origin),
    /// identical between steps 2 and 3. The result is cached.
    ///
    /// # Errors
    /// [`MediaError::Cancelled`], [`MediaError::Index`] (FFprobe failed), [`MediaError::Runner`].
    pub async fn build_index_cancellable(
        &self,
        path: &Path,
        source: &SourceInfo,
        on_progress: impl Fn(f64) + Send,
        cancel: CancelToken,
    ) -> Result<BuiltIndex, MediaError> {
        let cache_path = self.index_cache_path(source);
        if let Some(index) = load_cache(&cache_path) {
            on_progress(1.0);
            return Ok(BuiltIndex {
                index,
                source: IndexSource::Cache,
            });
        }
        let built = match self.sample_table_index(path, source).await {
            Some(index) => BuiltIndex {
                index,
                source: IndexSource::SampleTable,
            },
            None => BuiltIndex {
                index: self
                    .scan_packets(path, source, on_progress, &cancel)
                    .await?,
                source: IndexSource::PacketScan,
            },
        };
        save_cache(&cache_path, &built);
        Ok(built)
    }

    async fn sample_table_index(&self, path: &Path, source: &SourceInfo) -> Option<MediaIndex> {
        if !is_mov_family(source) {
            return None;
        }
        let file = path.to_path_buf();
        let start = source.start_time_sec;
        let result = tokio::task::spawn_blocking(move || {
            let mut f = std::fs::File::open(&file).map_err(mp4::Mp4Error::Io)?;
            mp4::read_sample_times(&mut f)
        })
        .await;
        match result {
            Ok(Ok(times)) if !times.frame_times.is_empty() => {
                Some(from_sample_times(&times, start))
            }
            Ok(Ok(_)) => None,
            Ok(Err(err)) => {
                tracing::debug!(%err, "sample tables not usable, scanning packets instead");
                None
            }
            Err(err) => {
                tracing::warn!(%err, "the sample-table reader panicked, scanning packets instead");
                None
            }
        }
    }

    async fn scan_packets(
        &self,
        path: &Path,
        source: &SourceInfo,
        on_progress: impl Fn(f64) + Send,
        cancel: &CancelToken,
    ) -> Result<MediaIndex, MediaError> {
        let args = packet_index_args(path);
        let start = if source.start_time_sec.is_finite() {
            source.start_time_sec
        } else {
            0.0
        };
        let duration = source.duration_sec;
        let mut text = String::new();
        let mut last_reported = 0.0_f64;
        let mut max_pts = f64::NEG_INFINITY;
        let text_ref = &mut text;
        let outcome = run_streaming(
            &self.inner.config.sidecars.ffprobe,
            &args,
            StreamOptions::default(),
            |_pid| {},
            move |line| {
                text_ref.push_str(line);
                text_ref.push('\n');
                if let Some(pts) = line
                    .split(',')
                    .next()
                    .and_then(|f| f.trim().parse::<f64>().ok())
                {
                    max_pts = max_pts.max(pts);
                    if duration > 0.0 {
                        let p = ((max_pts - start) / duration).clamp(0.0, 0.99);
                        if p - last_reported >= 0.01 {
                            last_reported = p;
                            on_progress(p);
                        }
                    }
                }
            },
            cancel.clone(),
        )
        .await?;
        if outcome.cancelled {
            return Err(MediaError::Cancelled);
        }
        if !outcome.success {
            return Err(MediaError::Index {
                file: file_name(path),
                detail: rustcut_ffmpeg::first_error_line(&outcome.stderr_tail.join("\n"))
                    .unwrap_or_else(|| "FFprobe exited with an error".to_string()),
            });
        }
        let index = parse_packet_csv(&text, start);
        if index.is_empty() {
            return Err(MediaError::Index {
                file: file_name(path),
                detail: "the file has no readable video packets".to_string(),
            });
        }
        Ok(index)
    }
}

fn load_cache(path: &Path) -> Option<MediaIndex> {
    let text = std::fs::read(path).ok()?;
    let cache: IndexCache = serde_json::from_slice(&text).ok()?;
    if cache.version != CACHE_VERSION || cache.frame_times.is_empty() {
        return None;
    }
    Some(MediaIndex::from_raw(cache.frame_times, cache.keyframes))
}

fn save_cache(path: &Path, built: &BuiltIndex) {
    let cache = IndexCache {
        version: CACHE_VERSION,
        origin: match built.source {
            IndexSource::SampleTable => "sample_table",
            _ => "packet_scan",
        }
        .to_string(),
        frame_times: built.index.frame_times.clone(),
        keyframes: built.index.keyframes.clone(),
    };
    let result = serde_json::to_vec(&cache)
        .map_err(std::io::Error::other)
        .and_then(|bytes| crate::write_atomic(path, &bytes));
    if let Err(err) = result {
        tracing::warn!(path = %path.display(), %err, "could not cache the media index");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_times_become_relative_to_the_source_start_and_rounded() {
        let t = SampleTimes {
            frame_times: vec![10.0333333333, 10.0, 10.0666666667],
            keyframes: vec![10.0],
        };
        let idx = from_sample_times(&t, 10.0);
        assert_eq!(idx.frame_times, vec![0.0, 0.033333, 0.066667]);
        assert_eq!(idx.keyframes, vec![0.0]);
    }

    #[test]
    fn cache_round_trip_and_version_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index/k.json");
        let built = BuiltIndex {
            index: MediaIndex::from_raw(vec![0.0, 0.5, 1.0], vec![0.0]),
            source: IndexSource::PacketScan,
        };
        save_cache(&path, &built);
        assert_eq!(load_cache(&path), Some(built.index));
        std::fs::write(
            &path,
            r#"{"version":99,"origin":"x","frame_times":[0.0],"keyframes":[]}"#,
        )
        .unwrap();
        assert_eq!(load_cache(&path), None);
        std::fs::write(&path, "garbage").unwrap();
        assert_eq!(load_cache(&path), None);
    }
}
