//! Playback tiers (PRD section 7.3): Direct, Remux and Proxy, and the LRU cache they live in.
//!
//! The OS WebView decides what plays. The UI confirms the tier with `canPlayType` and a decoded
//! first frame, and asks for the next tier on failure; this module picks the starting tier,
//! builds the cache files, and keeps the cache under its size cap. Export never uses these
//! files: it always reads the original (PLY-01).
//!
//! Time origin: both derived files start at 0 = the source's first presentation timestamp (the
//! reference FFmpeg's `-ss` uses): the remux uses `-avoid_negative_ts make_zero`, the proxy
//! transcode resets timestamps and keeps every frame time (`-fps_mode passthrough`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use rustcut_ffmpeg::{ProgressParser, first_error_line, fraction, safe_path_arg};
use rustcut_model::{AudioTrack, PlaybackInfo, PlaybackTier, SourceInfo};
use rustcut_runner::{CancelToken, StreamOptions, run_streaming};

use crate::cache;
use crate::{MediaError, MediaService, file_name};

/// Height of the proxy (Tier 3): 540 lines (never upscaled).
pub const PROXY_HEIGHT: u32 = 540;
/// Proxy keyframe interval in frames, for fast seeking and frame stepping.
const PROXY_GOP: u32 = 15;
/// MIME type of the proxy: H.264 Main level 3.1 (`libx264 -profile:v main`, 540p).
const PROXY_VIDEO_CODEC: &str = "avc1.4d401f";

/// Whether the OS WebView can play `codec` inside an MP4 container, so a remux may copy it.
///
/// Decided per OS: AAC and MP3 everywhere; Opus-in-MP4 on Windows (WebView2) and macOS
/// (WKWebView) but not WebKitGTK. Everything else (PCM, FLAC, AC-3/E-AC-3, DTS, Vorbis, ALAC,
/// TrueHD, ...) is transcoded to AAC by the remux.
pub fn audio_copyable_in_mp4(codec: &str) -> bool {
    audio_copyable_on(codec, cfg!(target_os = "linux"))
}

fn audio_copyable_on(codec: &str, linux: bool) -> bool {
    match codec {
        "aac" | "mp3" => true,
        "opus" => !linux,
        _ => false,
    }
}

/// Whether the container is known not to play in this OS's WebView: FLV everywhere, Matroska
/// (but not WebM) on macOS (WKWebView has no MKV support).
pub fn container_unplayable(source: &SourceInfo) -> bool {
    container_unplayable_on(source, cfg!(target_os = "macos"))
}

fn container_unplayable_on(source: &SourceInfo, macos: bool) -> bool {
    let format = source
        .format_name
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let names: Vec<&str> = format.split(',').collect();
    let ext = source.extension();
    if names.contains(&"flv") || ext == "flv" {
        return true;
    }
    let matroska = names.contains(&"matroska") || matches!(ext.as_str(), "mkv" | "mka" | "mk3d");
    macos && matroska && ext != "webm"
}

/// The audio track a preview plays: the default one, else the first.
fn preview_audio(source: &SourceInfo) -> Option<(usize, &AudioTrack)> {
    source
        .audio_tracks
        .iter()
        .enumerate()
        .find(|(_, t)| t.is_default)
        .or_else(|| source.audio_tracks.iter().enumerate().next())
}

/// MIME type of the remuxed MP4 (video codec of the source, the preview audio track, AAC when it
/// is transcoded).
pub fn remux_mime(source: &SourceInfo) -> String {
    let mut s = source.clone();
    s.format_name = Some("mov,mp4,m4a,3gp,3g2,mj2".into());
    s.path = "preview.mp4".into();
    s.audio_tracks = preview_audio(source)
        .map(|(_, t)| {
            let mut t = t.clone();
            if !audio_copyable_in_mp4(&t.codec) {
                t.codec = "aac".into();
            }
            t
        })
        .into_iter()
        .collect();
    rustcut_ffmpeg::mime_type(&s)
}

/// MIME type of the proxy (H.264 Main 3.1, AAC when the source has audio).
pub fn proxy_mime(source: &SourceInfo) -> String {
    if source.audio_tracks.is_empty() {
        format!("video/mp4; codecs=\"{PROXY_VIDEO_CODEC}\"")
    } else {
        format!("video/mp4; codecs=\"{PROXY_VIDEO_CODEC}, mp4a.40.2\"")
    }
}

/// The tier a source starts at (section 7.3): Direct unless the container is known unplayable
/// (see [`container_unplayable`]), then Remux. `mime` is the source's MIME type from the probe;
/// a Remux start reports the remuxed file's MIME type instead. `next_tier` is the fallback the UI
/// requests when playback fails.
pub fn initial_playback(source: &SourceInfo, mime: &str) -> PlaybackInfo {
    if container_unplayable(source) {
        PlaybackInfo {
            tier: PlaybackTier::Remux,
            media_path: None,
            mime_type: remux_mime(source),
            preparing: true,
            progress: None,
            eta_sec: None,
            next_tier: Some(PlaybackTier::Proxy),
            unsupported_reason: None,
        }
    } else {
        PlaybackInfo {
            tier: PlaybackTier::Direct,
            media_path: Some(source.path.clone()),
            mime_type: mime.to_string(),
            preparing: false,
            progress: None,
            eta_sec: None,
            next_tier: Some(PlaybackTier::Remux),
            unsupported_reason: None,
        }
    }
}

fn progress_args() -> [&'static str; 6] {
    [
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-progress",
        "pipe:1",
    ]
}

fn push_all(args: &mut Vec<OsString>, items: &[&str]) {
    args.extend(items.iter().map(OsString::from));
}

/// FFmpeg arguments of the Tier 2 remux: stream copy of the first video and the preview audio
/// track into an MP4 (`-f mp4`, so a `.part` output name works), faststart, timestamps made
/// relative to the first one. Audio the WebView cannot play from MP4 is transcoded to stereo
/// AAC ([`audio_copyable_in_mp4`]); HEVC gets the `hvc1` tag Safari/WebView2 need.
pub fn remux_args(source: &SourceInfo, input: &Path, output: &Path) -> Vec<OsString> {
    let mut a: Vec<OsString> = Vec::new();
    push_all(&mut a, &progress_args());
    push_all(&mut a, &["-nostats", "-y", "-i"]);
    a.push(safe_path_arg(input.as_os_str()));
    push_all(&mut a, &["-map", "0:v:0", "-c:v", "copy"]);
    if source.video.codec == "hevc" {
        push_all(&mut a, &["-tag:v", "hvc1"]);
    }
    if let Some((pos, track)) = preview_audio(source) {
        let map = format!("0:a:{pos}");
        push_all(&mut a, &["-map", &map]);
        if audio_copyable_in_mp4(&track.codec) {
            push_all(&mut a, &["-c:a", "copy"]);
        } else {
            push_all(&mut a, &["-c:a", "aac", "-b:a", "192k", "-ac", "2"]);
        }
    }
    push_all(
        &mut a,
        &[
            "-sn",
            "-dn",
            "-avoid_negative_ts",
            "make_zero",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
        ],
    );
    a.push(safe_path_arg(output.as_os_str()));
    a
}

/// FFmpeg arguments of the Tier 3 proxy (P1): H.264 Main, at most 540 lines high (display
/// orientation, since FFmpeg autorotates), `-g 15`, AAC stereo, every frame time kept
/// (`-fps_mode passthrough`).
pub fn proxy_args(source: &SourceInfo, input: &Path, output: &Path) -> Vec<OsString> {
    let mut a: Vec<OsString> = Vec::new();
    push_all(&mut a, &progress_args());
    push_all(&mut a, &["-nostats", "-y", "-i"]);
    a.push(safe_path_arg(input.as_os_str()));
    push_all(&mut a, &["-map", "0:v:0"]);
    if let Some((pos, _)) = preview_audio(source) {
        let map = format!("0:a:{pos}");
        push_all(&mut a, &["-map", &map]);
    }
    let scale = format!("scale=-2:'min({PROXY_HEIGHT},trunc(ih/2)*2)'");
    let gop = PROXY_GOP.to_string();
    push_all(
        &mut a,
        &[
            "-vf",
            &scale,
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "26",
            "-profile:v",
            "main",
            "-level:v",
            "3.1",
            "-pix_fmt",
            "yuv420p",
            "-g",
            &gop,
            "-fps_mode",
            "passthrough",
        ],
    );
    if !source.audio_tracks.is_empty() {
        push_all(&mut a, &["-c:a", "aac", "-b:a", "128k", "-ac", "2"]);
    }
    push_all(
        &mut a,
        &["-sn", "-dn", "-movflags", "+faststart", "-f", "mp4"],
    );
    a.push(safe_path_arg(output.as_os_str()));
    a
}

fn tier_dir(tier: PlaybackTier) -> Option<&'static str> {
    match tier {
        PlaybackTier::Direct => None,
        PlaybackTier::Remux => Some("remux"),
        PlaybackTier::Proxy => Some("proxy"),
    }
}

impl MediaService {
    /// Makes the file for `tier` available and returns its path (PRD section 7.3).
    ///
    /// * Direct: `path` itself.
    /// * Remux / Proxy: `<cache>/{remux,proxy}/<fingerprint key>.mp4`, built through a `.part`
    ///   file and an atomic rename; a cached file is reused instantly and counts as used. The
    ///   returned file is pinned (never evicted) until [`release_playback`](Self::release_playback);
    ///   afterwards the cache is trimmed to its cap, least recently used first.
    ///
    /// `on_progress(fraction, eta_sec)` is called as FFmpeg reports its position.
    ///
    /// # Errors
    /// [`MediaError::Prepare`] when FFmpeg fails (the file is removed; the UI falls back to the
    /// next tier or shows the "undecodable codec" message), [`MediaError::Cancelled`],
    /// [`MediaError::Io`], [`MediaError::Runner`].
    pub async fn prepare(
        &self,
        tier: PlaybackTier,
        path: &Path,
        source: &SourceInfo,
        on_progress: impl Fn(f64, Option<f64>) + Send,
        cancel: CancelToken,
    ) -> Result<PathBuf, MediaError> {
        let Some(dir) = tier_dir(tier) else {
            return Ok(path.to_path_buf());
        };
        let out = self
            .cache_subdir(dir)
            .join(format!("{}.mp4", source.fingerprint.key()));
        let lock = self.prepare_lock(&out);
        let _guard = lock.lock().await;

        if std::fs::metadata(&out).is_ok_and(|m| m.len() > 0) {
            self.mark_used(&out);
            on_progress(1.0, Some(0.0));
            return Ok(out);
        }
        std::fs::create_dir_all(self.cache_subdir(dir)).map_err(|e| MediaError::io(&out, e))?;
        let mut part = out.clone().into_os_string();
        part.push(".part");
        let part = PathBuf::from(part);
        let args = match tier {
            PlaybackTier::Proxy => proxy_args(source, path, &part),
            _ => remux_args(source, path, &part),
        };
        let tier_name = if tier == PlaybackTier::Proxy {
            "proxy"
        } else {
            "remux"
        };
        let duration = source.duration_sec;
        let started = Instant::now();
        let mut parser = ProgressParser::new();
        let outcome = run_streaming(
            &self.inner.config.sidecars.ffmpeg,
            &args,
            StreamOptions::default(),
            |_pid| {},
            move |line| {
                if let Some(update) = parser.feed(line)
                    && let Some(t) = update.out_time_sec
                {
                    let f = fraction(t, duration);
                    let eta = (f > 0.02).then(|| started.elapsed().as_secs_f64() * (1.0 - f) / f);
                    on_progress(f, eta);
                }
            },
            cancel,
        )
        .await;
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                return Err(e.into());
            }
        };
        if outcome.cancelled {
            let _ = std::fs::remove_file(&part);
            return Err(MediaError::Cancelled);
        }
        let written = std::fs::metadata(&part).map_or(0, |m| m.len());
        if !outcome.success || written == 0 {
            let _ = std::fs::remove_file(&part);
            return Err(MediaError::Prepare {
                tier: tier_name,
                file: file_name(path),
                detail: first_error_line(&outcome.stderr_tail.join("\n"))
                    .unwrap_or_else(|| "FFmpeg exited with an error".to_string()),
            });
        }
        if let Err(e) = std::fs::rename(&part, &out) {
            let _ = std::fs::remove_file(&part);
            return Err(MediaError::io(&out, e));
        }
        self.mark_used(&out);
        self.enforce_cache_cap().await;
        Ok(out)
    }

    fn prepare_lock(&self, out: &Path) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .inner
            .prepare_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::sync::Arc::clone(locks.entry(out.to_path_buf()).or_default())
    }

    /// Records a use of `path`: LRU stamp, file timestamp, and the pin.
    fn mark_used(&self, path: &Path) {
        cache::touch(path);
        self.inner
            .stamps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path.to_path_buf(), SystemTime::now());
        self.pin_playback(path);
    }

    /// Pins a cache file the player has open: it is never evicted or cleared. [`prepare`](Self::prepare)
    /// pins what it returns.
    pub fn pin_playback(&self, path: &Path) {
        self.inner
            .in_use
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path.to_path_buf());
    }

    /// Releases a pin (the source was closed); the file becomes evictable, with its last use as
    /// its LRU position.
    pub fn release_playback(&self, path: &Path) {
        self.mark_stamp_only(path);
        self.inner
            .in_use
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(path);
    }

    fn mark_stamp_only(&self, path: &Path) {
        self.inner
            .stamps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path.to_path_buf(), SystemTime::now());
    }

    /// Changes the cache cap and trims the cache to it. Returns the bytes freed.
    pub async fn set_cache_cap(&self, cap_bytes: u64) -> u64 {
        self.inner
            .cap
            .store(cap_bytes, std::sync::atomic::Ordering::Relaxed);
        self.enforce_cache_cap().await
    }

    /// Trims remux + proxy files to the cap, least recently used first, never touching pinned
    /// files. Returns the bytes freed.
    pub async fn enforce_cache_cap(&self) -> u64 {
        let svc = self.clone();
        tokio::task::spawn_blocking(move || {
            let entries = svc.scan_cache();
            let cap = svc.inner.cap.load(std::sync::atomic::Ordering::Relaxed);
            let protected = svc.pinned();
            cache::delete_all(&cache::plan_evictions(&entries, cap, &protected))
        })
        .await
        .unwrap_or(0)
    }

    /// Deletes every cached remux and proxy file that is not pinned (the user's "clear cache").
    /// Returns the bytes freed.
    pub async fn clear_cache(&self) -> u64 {
        let svc = self.clone();
        tokio::task::spawn_blocking(move || {
            let protected = svc.pinned();
            let paths: Vec<PathBuf> = svc
                .scan_cache()
                .into_iter()
                .map(|e| e.path)
                .filter(|p| !protected.contains(p))
                .collect();
            cache::delete_all(&paths)
        })
        .await
        .unwrap_or(0)
    }

    /// Total size of the cached remux and proxy files.
    pub async fn cache_size_bytes(&self) -> u64 {
        let svc = self.clone();
        tokio::task::spawn_blocking(move || svc.scan_cache().iter().map(|e| e.size).sum())
            .await
            .unwrap_or(0)
    }

    fn pinned(&self) -> std::collections::HashSet<PathBuf> {
        self.inner
            .in_use
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn scan_cache(&self) -> Vec<cache::CacheEntry> {
        let stamps = self
            .inner
            .stamps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        cache::scan(&self.inner.config.cache_dir, &stamps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcut_model::{Fingerprint, VideoInfo};

    fn source(format: &str, file: &str, vcodec: &str, audio: &[(&str, bool)]) -> SourceInfo {
        SourceInfo {
            path: format!("C:/v/{file}"),
            relative_path: String::new(),
            fingerprint: Fingerprint {
                size_bytes: 1,
                xxh3_head_tail: "ab".into(),
            },
            duration_sec: 10.0,
            start_time_sec: 0.0,
            video: VideoInfo {
                codec: vcodec.into(),
                coded_width: 1920,
                coded_height: 1080,
                display_width: 1920,
                display_height: 1080,
                rotation: 0,
                sar: "1:1".into(),
                frame_rate: "30/1".into(),
                vfr: false,
                hdr: false,
                stream_index: Some(0),
                pix_fmt: None,
                profile: Some("High".into()),
                level: Some(40),
                color_transfer: None,
                bit_depth: Some(8),
            },
            audio_tracks: audio
                .iter()
                .enumerate()
                .map(|(i, (c, d))| AudioTrack {
                    stream_index: i as u32 + 1,
                    codec: (*c).into(),
                    channels: 2,
                    title: None,
                    language: None,
                    sample_rate: Some(48000),
                    is_default: *d,
                })
                .collect(),
            format_name: Some(format.into()),
            bit_rate: None,
        }
    }

    fn strs(a: &[OsString]) -> Vec<String> {
        a.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn s73_flv_starts_at_remux_everywhere_and_mp4_direct() {
        let flv = source("flv", "a.flv", "h264", &[("aac", true)]);
        assert!(container_unplayable_on(&flv, false));
        assert!(container_unplayable_on(&flv, true));
        let info = initial_playback(&flv, "video/x-flv; codecs=\"avc1.640028, mp4a.40.2\"");
        assert_eq!(info.tier, PlaybackTier::Remux);
        assert!(info.preparing && info.media_path.is_none());
        assert_eq!(info.next_tier, Some(PlaybackTier::Proxy));
        assert!(
            info.mime_type.starts_with("video/mp4"),
            "{}",
            info.mime_type
        );

        let mp4 = source("mov,mp4,m4a,3gp,3g2,mj2", "a.mp4", "h264", &[("aac", true)]);
        let info = initial_playback(&mp4, "video/mp4; codecs=\"x\"");
        assert_eq!(info.tier, PlaybackTier::Direct);
        assert_eq!(info.media_path.as_deref(), Some("C:/v/a.mp4"));
        assert_eq!(info.mime_type, "video/mp4; codecs=\"x\"");
        assert!(!info.preparing);
        assert_eq!(info.next_tier, Some(PlaybackTier::Remux));
    }

    #[test]
    fn s73_mkv_is_unplayable_on_macos_only_and_webm_never() {
        let mkv = source("matroska,webm", "a.mkv", "h264", &[("aac", true)]);
        assert!(container_unplayable_on(&mkv, true));
        assert!(!container_unplayable_on(&mkv, false));
        let webm = source("matroska,webm", "a.webm", "vp9", &[("opus", true)]);
        assert!(!container_unplayable_on(&webm, true));
        assert!(!container_unplayable_on(&webm, false));
    }

    #[test]
    fn remux_copies_aac_and_transcodes_pcm_and_flac_to_aac() {
        let aac = source(
            "matroska,webm",
            "a.mkv",
            "h264",
            &[("aac", false), ("aac", true)],
        );
        let a = strs(&remux_args(
            &aac,
            Path::new("in.mkv"),
            Path::new("out.mp4.part"),
        ));
        let joined = a.join(" ");
        assert!(joined.contains("-map 0:v:0 -c:v copy"), "{joined}");
        assert!(
            joined.contains("-map 0:a:1 -c:a copy"),
            "default track is mapped: {joined}"
        );
        assert!(joined.contains("-avoid_negative_ts make_zero"));
        assert!(joined.contains("-f mp4"));
        // PRD 7.9 defence in depth: relative paths get "./" so none can be read as an option.
        assert_eq!(a.last().unwrap(), "./out.mp4.part");
        assert!(joined.contains("-i ./in.mkv"), "{joined}");
        assert!(!joined.contains("hvc1"));
        let dashed = strs(&remux_args(
            &aac,
            Path::new("-in.mkv"),
            Path::new("-out.mp4"),
        ));
        assert!(
            dashed.contains(&"./-in.mkv".to_string()) && dashed.last().unwrap() == "./-out.mp4",
            "{dashed:?}"
        );
        let abs = strs(&proxy_args(
            &aac,
            Path::new(r"C:\v\-a.mkv"),
            Path::new(r"C:\c\-b.mp4"),
        ));
        assert_eq!(
            abs.last().unwrap(),
            r"C:\c\-b.mp4",
            "absolute paths are unchanged"
        );

        for codec in ["pcm_s16le", "flac", "ac3", "vorbis"] {
            let s = source("mov,mp4", "a.mov", "hevc", &[(codec, true)]);
            let joined = strs(&remux_args(&s, Path::new("a"), Path::new("b"))).join(" ");
            assert!(joined.contains("-c:a aac"), "{codec}: {joined}");
            assert!(joined.contains("-tag:v hvc1"));
            assert!(remux_mime(&s).contains("mp4a.40.2"), "{codec}");
        }
        let silent = source("flv", "a.flv", "h264", &[]);
        let joined = strs(&remux_args(&silent, Path::new("a"), Path::new("b"))).join(" ");
        assert!(!joined.contains("-map 0:a") && !joined.contains("-c:a"));
    }

    #[test]
    fn audio_copy_rules_per_os() {
        for c in ["aac", "mp3"] {
            assert!(audio_copyable_on(c, true) && audio_copyable_on(c, false));
        }
        assert!(audio_copyable_on("opus", false) && !audio_copyable_on("opus", true));
        for c in [
            "pcm_s16le",
            "pcm_s24le",
            "flac",
            "ac3",
            "eac3",
            "dts",
            "vorbis",
            "alac",
        ] {
            assert!(!audio_copyable_on(c, false), "{c}");
        }
    }

    #[test]
    fn proxy_is_540p_short_gop_h264_with_timestamps_preserved() {
        let s = source("mov,mp4", "a.mp4", "hevc", &[("aac", true)]);
        let joined = strs(&proxy_args(
            &s,
            Path::new("in.mp4"),
            Path::new("out.mp4.part"),
        ))
        .join(" ");
        assert!(
            joined.contains("scale=-2:'min(540,trunc(ih/2)*2)'"),
            "{joined}"
        );
        assert!(joined.contains("-c:v libx264"));
        assert!(joined.contains("-g 15"));
        assert!(joined.contains("-fps_mode passthrough"));
        assert!(joined.contains("-c:a aac"));
        assert!(proxy_mime(&s).contains("avc1.4d401f, mp4a.40.2"));
        let silent = source("mov,mp4", "a.mp4", "h264", &[]);
        assert!(!proxy_mime(&silent).contains("mp4a"));
        assert!(
            !strs(&proxy_args(&silent, Path::new("a"), Path::new("b")))
                .join(" ")
                .contains("-c:a")
        );
    }
}
