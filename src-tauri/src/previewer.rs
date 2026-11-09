//! Preview preparation: decide whether the embedded player can decode a file
//! and, when it cannot, build a cached H.264 proxy for the timeline preview.
//!
//! The source video is never modified — exports keep reading the original.
//! Proxies live under `%LOCALAPPDATA%\rustcut\previews` and are keyed by the
//! source path, size, modification time and proxy format version, so editing
//! the source (or changing the proxy command) produces a new proxy instead of
//! a stale one.
//!
//! Encoders are tried in order — hardware Quick Sync first, then libx264 — so
//! a machine without a usable Intel iGPU still gets a preview, just slower.

use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::UNIX_EPOCH;

use rustcut_core::preview::{
    build_proxy_args, proxy_reason, PreviewQuality, ProxyEncoder, PREVIEW_FORMAT_VERSION,
};
use rustcut_core::probe::{parse_ffprobe_json, MediaInfo};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::exporter::{resolve_tool, with_progress_args};
use crate::fileserver;

pub const EVENT_PREVIEW_PROGRESS: &str = "preview://progress";

/// Serialises proxy builds. Two callers for the same source (the UI mounts
/// twice in React StrictMode) would otherwise both transcode into the same
/// path and produce a corrupt file.
static ENCODE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize)]
pub struct PreviewInfo {
    /// File the webview should load (kept for diagnostics and cache paths).
    pub preview_path: String,
    /// Loopback HTTP URL the player actually loads. `asset://` stalls the
    /// WebView2 media pipeline, so media is streamed over localhost instead.
    pub preview_url: String,
    /// True when a proxy was built instead of using the source directly.
    pub transcoded: bool,
    pub source_codec: String,
    pub source_format: String,
    /// Why a proxy was needed; `None` when the source plays directly.
    pub reason: Option<String>,
    /// True when an existing proxy was reused without re-encoding.
    pub cached: bool,
    /// Encoder that produced the proxy (`qsv`, `software`), or `None` if cached.
    pub encoder: Option<String>,
    /// Resolution actually requested for this proxy (`720p`, `1080p`).
    pub quality: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreviewProgress {
    /// 0.0..=1.0 of the transcode.
    pub fraction: f64,
    pub encoder: String,
}

fn probe(path: &str) -> Result<MediaInfo, String> {
    let output = Command::new(resolve_tool("RUSTCUT_FFPROBE", "ffprobe"))
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            path,
        ])
        .output()
        .map_err(|e| format!("failed to start ffprobe: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffprobe failed: {}", stderr.trim()));
    }
    parse_ffprobe_json(&String::from_utf8_lossy(&output.stdout)).map_err(|e| e.to_string())
}

fn preview_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("rustcut")
        .join("previews")
}

/// Stable cache file name for a source: path + size + mtime + format version
/// + quality, so switching quality builds a separate proxy.
fn cache_path(source: &Path, quality: PreviewQuality) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.to_string_lossy().hash(&mut hasher);
    PREVIEW_FORMAT_VERSION.hash(&mut hasher);
    quality.label().hash(&mut hasher);
    if let Ok(meta) = std::fs::metadata(source) {
        meta.len().hash(&mut hasher);
        if let Ok(modified) = meta.modified() {
            if let Ok(since) = modified.duration_since(UNIX_EPOCH) {
                since.as_nanos().hash(&mut hasher);
            }
        }
    }
    preview_dir().join(format!("{:016x}.mp4", hasher.finish()))
}

/// Probes the file and returns the path the preview player should load.
/// `quality` is `"high"` for a 1080p proxy or anything else for 720p.
#[tauri::command]
pub async fn prepare_preview(
    app: AppHandle,
    path: String,
    quality: Option<String>,
) -> Result<PreviewInfo, String> {
    let quality = match quality.as_deref() {
        Some("high") => PreviewQuality::High,
        _ => PreviewQuality::Fast,
    };
    tauri::async_runtime::spawn_blocking(move || prepare_preview_blocking(&app, &path, quality))
        .await
        .map_err(|e| format!("preview task failed: {e}"))?
}

fn prepare_preview_blocking(
    app: &AppHandle,
    path: &str,
    quality: PreviewQuality,
) -> Result<PreviewInfo, String> {
    let source = Path::new(path);
    let info = probe(path)?;

    let Some(reason) = proxy_reason(source, &info) else {
        return Ok(PreviewInfo {
            preview_path: path.to_string(),
            preview_url: fileserver::register(source),
            transcoded: false,
            source_codec: info.codec_name,
            source_format: info.format_name,
            reason: None,
            cached: false,
            encoder: None,
            quality: quality.label().to_string(),
        });
    };

    let target = cache_path(source, quality);
    if std::fs::metadata(&target)
        .map(|m| m.len() > 4096)
        .unwrap_or(false)
    {
        return Ok(PreviewInfo {
            preview_path: target.to_string_lossy().into_owned(),
            preview_url: fileserver::register(&target),
            transcoded: true,
            source_codec: info.codec_name,
            source_format: info.format_name,
            reason: Some(reason),
            cached: true,
            encoder: None,
            quality: quality.label().to_string(),
        });
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create preview cache directory: {e}"))?;
    }

    let _guard = ENCODE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // Another caller may have finished while this one waited for the lock.
    if std::fs::metadata(&target)
        .map(|m| m.len() > 4096)
        .unwrap_or(false)
    {
        return Ok(PreviewInfo {
            preview_path: target.to_string_lossy().into_owned(),
            preview_url: fileserver::register(&target),
            transcoded: true,
            source_codec: info.codec_name,
            source_format: info.format_name,
            reason: Some(reason),
            cached: true,
            encoder: None,
            quality: quality.label().to_string(),
        });
    }

    // Encode into a side file and rename into place, so the player can never
    // open a half-written proxy.
    let part = PathBuf::from(format!("{}.part.mp4", target.to_string_lossy()));
    let ffmpeg = resolve_tool("RUSTCUT_FFMPEG", "ffmpeg");
    let mut last_error = String::new();
    for encoder in ProxyEncoder::ALL {
        let args = build_proxy_args(source, &part, info.duration_sec, encoder, quality);
        match run_ffmpeg(&ffmpeg, &args, info.duration_sec, app, encoder) {
            Ok(()) => {
                std::fs::rename(&part, &target)
                    .map_err(|e| format!("cannot finalise preview: {e}"))?;
                return Ok(PreviewInfo {
                    preview_path: target.to_string_lossy().into_owned(),
                    preview_url: fileserver::register(&target),
                    transcoded: true,
                    source_codec: info.codec_name,
                    source_format: info.format_name,
                    reason: Some(reason),
                    cached: false,
                    encoder: Some(encoder.label().to_string()),
                    quality: quality.label().to_string(),
                });
            }
            Err(err) => {
                let _ = std::fs::remove_file(&part);
                last_error = format!("{} ({})", err, encoder.label());
            }
        }
    }

    Err(format!("preview transcode failed: {last_error}"))
}

fn run_ffmpeg(
    ffmpeg: &str,
    args: &[String],
    duration_sec: f64,
    app: &AppHandle,
    encoder: ProxyEncoder,
) -> Result<(), String> {
    let args = with_progress_args(args);
    let mut child = Command::new(ffmpeg)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start ffmpeg: {e}"))?;

    let stdout = child.stdout.take().ok_or_else(|| "no stdout".to_string())?;
    let stderr = child.stderr.take().ok_or_else(|| "no stderr".to_string())?;

    // Drain stderr on its own thread so the pipes can never deadlock.
    let stderr_thread = thread::spawn(move || {
        let mut text = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut text);
        text
    });

    let label = encoder.label().to_string();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(rest) = line.strip_prefix("out_time_us=") {
            if let Ok(us) = rest.trim().parse::<f64>() {
                let fraction = if duration_sec > 0.0 {
                    (us / 1_000_000.0 / duration_sec).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let _ = app.emit(
                    EVENT_PREVIEW_PROGRESS,
                    PreviewProgress {
                        fraction,
                        encoder: label.clone(),
                    },
                );
            }
        }
    }

    let status = child.wait().map_err(|e| format!("wait failed: {e}"))?;
    let stderr_text = stderr_thread.join().unwrap_or_default();

    if status.success() {
        let _ = app.emit(
            EVENT_PREVIEW_PROGRESS,
            PreviewProgress {
                fraction: 1.0,
                encoder: label,
            },
        );
        Ok(())
    } else {
        let tail = stderr_text
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .join(" ");
        Err(format!("ffmpeg exited with {status}: {tail}"))
    }
}
