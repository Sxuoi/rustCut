//! Media services: probe (PLY-01), media index (PLY-06), waveform (PLY-05), playback tiers with an
//! LRU remux/proxy cache (PRD section 7.3), hardware-encoder detection (EXP-13) and the
//! growing-file watch (PRD section 9).
//!
//! [`MediaService`] is the entry point: cheap to clone (an `Arc` inside), all methods are async
//! and every long-running one takes a [`CancelToken`]. Export never uses anything from here: it
//! always reads the original file with FFmpeg (PLY-01, section 7.3).
//!
//! Performance contract (PRD section 8.1, "open a 20 GB file to first frame without reading the
//! whole file"): [`MediaService::probe_source`] runs `ffprobe -show_format -show_streams` (headers
//! only) and hashes the first and last MiB of the file; [`MediaService::build_index`] reads only
//! the `moov` tables of MP4/MOV files.
#![warn(missing_docs)]

mod cache;
mod error;
mod growth;
mod hw;
mod index;
pub mod mp4;
mod playback;
mod probe;
mod waveform;

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use rustcut_runner::Sidecars;

pub use cache::{CacheEntry, plan_evictions};
pub use error::MediaError;
pub use growth::{GrowthChange, GrowthWatch, watch_growth};
pub use hw::{HW_TEST_TIMEOUT, pick_best};
pub use index::BuiltIndex;
pub use playback::{
    PROXY_HEIGHT, audio_copyable_in_mp4, container_unplayable, initial_playback, proxy_args,
    proxy_mime, remux_args, remux_mime,
};
pub use probe::{PROBE_TIMEOUT, ProbedSource};
pub use rustcut_runner::CancelToken;
pub use waveform::{PEAKS_PER_SEC, total_peaks_for};

/// Default size cap of the playback cache (PRD section 7.3): 50 GB.
pub const DEFAULT_CACHE_CAP_BYTES: u64 = 50 * 1024 * 1024 * 1024;

/// Where the service finds FFmpeg and keeps its files.
#[derive(Debug, Clone)]
pub struct MediaConfig {
    /// The FFmpeg/FFprobe sidecars.
    pub sidecars: Sidecars,
    /// Cache folder: `index/`, `waveform/`, `remux/`, `proxy/` are created inside.
    pub cache_dir: PathBuf,
    /// App data folder (holds `hw_encoders.json`).
    pub data_dir: PathBuf,
    /// Size cap of the remux + proxy files; least recently used files go first.
    pub cache_cap_bytes: u64,
}

pub(crate) struct Inner {
    pub(crate) config: MediaConfig,
    pub(crate) cap: AtomicU64,
    /// Playback files that a player may have open; never evicted.
    pub(crate) in_use: Mutex<HashSet<PathBuf>>,
    /// Last use of cache files (see [`cache`]).
    pub(crate) stamps: Mutex<HashMap<PathBuf, SystemTime>>,
    /// One preparation at a time per output file.
    pub(crate) prepare_locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    pub(crate) hw: tokio::sync::Mutex<Option<Vec<rustcut_model::HwEncoderInfo>>>,
}

/// Probe, index, waveform, playback preparation and hardware detection. Clone freely.
#[derive(Clone)]
pub struct MediaService {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for MediaService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaService")
            .field("cache_dir", &self.inner.config.cache_dir)
            .finish_non_exhaustive()
    }
}

impl MediaService {
    /// Creates the service. No file is touched until a method needs it.
    pub fn new(config: MediaConfig) -> Self {
        let cap = AtomicU64::new(config.cache_cap_bytes);
        Self {
            inner: Arc::new(Inner {
                config,
                cap,
                in_use: Mutex::default(),
                stamps: Mutex::default(),
                prepare_locks: Mutex::default(),
                hw: tokio::sync::Mutex::new(None),
            }),
        }
    }

    /// The sidecars in use.
    pub fn sidecars(&self) -> &Sidecars {
        &self.inner.config.sidecars
    }

    pub(crate) fn cache_subdir(&self, name: &str) -> PathBuf {
        self.inner.config.cache_dir.join(name)
    }
}

/// Writes `bytes` to `path` through a sibling temp file and a rename, creating parent folders.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}
