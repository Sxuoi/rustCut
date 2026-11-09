//! FFmpeg execution layer: media probing and batch export runs.
//!
//! Planning, naming and progress parsing live in `rustcut-core`; this module
//! spawns the external tools and streams progress to the UI as Tauri events
//! (`export://progress` per clip, `export://finished` at the end).
//!
//! One failing clip never stops the batch (PRD 8.3): the error is reported
//! as a `failed` phase event and the remaining clips continue.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use rustcut_core::clip::ExportStatus;
use rustcut_core::export::{plan_export, summarize, ExportPlan, ExportResult, ProgressParser, SourceSpec};
use rustcut_core::naming::{build_output_stem, NamingContext};
use rustcut_core::probe::{parse_ffprobe_json, MediaInfo};
use rustcut_core::project::ProjectState;
use rustcut_core::VideoEncoder;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub const EVENT_PROGRESS: &str = "export://progress";
pub const EVENT_FINISHED: &str = "export://finished";

/// Tool resolution: `RUSTCUT_FFMPEG` / `RUSTCUT_FFPROBE` override the binary
/// name; otherwise the tools are expected on PATH (they ship as Tauri
/// sidecars in the packaged build).
pub(crate) fn resolve_tool(env_key: &str, default: &str) -> String {
    std::env::var(env_key).unwrap_or_else(|_| default.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct ProgressEvent {
    pub clip_id: String,
    pub index: u32,
    pub fraction: f64,
    /// `running` | `done` | `failed` | `skipped`
    pub phase: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FinishedEvent {
    pub completed: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub skipped: Vec<(String, String)>,
}

/// Runs ffprobe and decodes the result (duration, size, fps, bitrate).
#[tauri::command]
pub fn probe_media(path: String) -> Result<MediaInfo, String> {
    let output = Command::new(resolve_tool("RUSTCUT_FFPROBE", "ffprobe"))
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            &path,
        ])
        .output()
        .map_err(|e| format!("failed to start ffprobe: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffprobe failed: {}", stderr.trim()));
    }
    parse_ffprobe_json(&String::from_utf8_lossy(&output.stdout)).map_err(|e| e.to_string())
}

/// Renders a naming template for one clip so the UI can show the exact
/// filename before exporting (same code path as the export planner, so the
/// preview can never drift from what is written).
#[tauri::command]
pub fn preview_filename(
    pattern: String,
    index: u32,
    clip_name: String,
    source_name: String,
    start_sec: f64,
    end_sec: f64,
) -> String {
    let ctx = NamingContext {
        index,
        clip_name: &clip_name,
        source_name: &source_name,
        start_sec,
        end_sec,
    };
    format!("{}.mp4", build_output_stem(&pattern, &ctx))
}

/// 16:9 working canvas for a source, plus the 9:16 crop width on it. The
/// preview uses the same numbers as the export filter chain so the framed
/// area is exactly what will be written (PRD 4.4 WYSIWYG).
#[derive(Debug, Clone, Serialize)]
pub struct CanvasInfo {
    pub canvas_w: u32,
    pub canvas_h: u32,
    pub crop_w: u32,
}

#[tauri::command]
pub fn canvas_info(width: u32, height: u32) -> CanvasInfo {
    let (canvas_w, canvas_h) = rustcut_core::ffmpeg::canvas_dims(width, height);
    CanvasInfo {
        canvas_w,
        canvas_h,
        crop_w: rustcut_core::ffmpeg::crop_width_px(canvas_h),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EncoderOption {
    pub id: String,
    pub label: String,
    pub available: bool,
    pub hardware: bool,
}

static ENCODERS: OnceLock<Vec<EncoderOption>> = OnceLock::new();

/// One-second smoke encode: ffmpeg can list an encoder whose hardware is
/// missing, so availability is proven by actually encoding a frame.
fn probe_encoder(ffmpeg: &str, name: &str) -> bool {
    Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=128x128:rate=30",
            "-t",
            "0.2",
            "-c:v",
            name,
            "-f",
            "null",
            "-",
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Which H.264 encoders actually work on this machine (cached after the
/// first call — each probe costs ~0.3 s).
#[tauri::command]
pub fn detect_encoders() -> Vec<EncoderOption> {
    ENCODERS
        .get_or_init(|| {
            let ffmpeg = resolve_tool("RUSTCUT_FFMPEG", "ffmpeg");
            let candidates = [
                ("software", "Software (x264)", "libx264", false),
                ("nvenc", "NVIDIA NVENC", "h264_nvenc", true),
                ("amf", "AMD AMF", "h264_amf", true),
                ("qsv", "Intel Quick Sync", "h264_qsv", true),
                ("videotoolbox", "Apple VideoToolbox", "h264_videotoolbox", true),
            ];
            candidates
                .iter()
                .map(|(id, label, encoder, hardware)| EncoderOption {
                    id: id.to_string(),
                    label: label.to_string(),
                    available: probe_encoder(&ffmpeg, encoder),
                    hardware: *hardware,
                })
                .collect()
        })
        .clone()
}

/// `auto` picks the first working hardware encoder, else software.
fn pick_auto_encoder() -> VideoEncoder {
    detect_encoders()
        .iter()
        .find(|o| o.hardware && o.available)
        .map(|o| VideoEncoder::from_setting(&o.id))
        .unwrap_or_else(VideoEncoder::software_default)
}

/// Plans and launches a batch export on background worker threads. Returns
/// the number of jobs immediately; progress arrives via `export://progress`.
#[tauri::command]
pub fn start_export(
    app: AppHandle,
    state: ProjectState,
    source: SourceSpec,
    encoder: Option<String>,
    workers: Option<u32>,
) -> Result<usize, String> {
    let chosen = match encoder.as_deref() {
        None | Some("auto") => pick_auto_encoder(),
        Some(other) => VideoEncoder::from_setting(other),
    };
    let ExportPlan {
        jobs,
        skipped,
        estimated_bytes: _,
    } = plan_export(&state, &source, &chosen);

    if jobs.is_empty() && skipped.is_empty() {
        return Err("no clips are selected for export".to_string());
    }

    for s in &skipped {
        let _ = app.emit(
            EVENT_PROGRESS,
            ProgressEvent {
                clip_id: s.clip_id.to_string(),
                index: s.index,
                fraction: 0.0,
                phase: "skipped".to_string(),
                message: Some(s.reason.clone()),
            },
        );
    }

    let job_count = jobs.len();
    let ffmpeg = resolve_tool("RUSTCUT_FFMPEG", "ffmpeg");
    let worker_count = workers
        .unwrap_or(state.export_settings.parallel_jobs)
        .clamp(1, 8)
        .min(job_count.max(1) as u32);

    // PRD 4.4: sequential or 2-4 clips in parallel. Jobs are pulled from a
    // shared queue so a slow clip never blocks the others.
    let queue: Arc<Mutex<Vec<_>>> = Arc::new(Mutex::new(jobs));
    let collected: Arc<Mutex<Vec<ExportResult>>> = Arc::new(Mutex::new(Vec::new()));

    thread::spawn(move || {
        let mut handles = Vec::new();
        for _ in 0..worker_count {
            let queue = Arc::clone(&queue);
            let collected = Arc::clone(&collected);
            let app = app.clone();
            let ffmpeg = ffmpeg.clone();
            handles.push(thread::spawn(move || loop {
                let next = {
                    let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                    q.pop()
                };
                let Some(job) = next else { break };
                let clip_id = job.clip_id.to_string();
                let _ = app.emit(
                    EVENT_PROGRESS,
                    ProgressEvent {
                        clip_id: clip_id.clone(),
                        index: job.index,
                        fraction: 0.0,
                        phase: "running".to_string(),
                        message: None,
                    },
                );

                let outcome = run_ffmpeg(
                    &ffmpeg,
                    &job.args,
                    job.end_sec - job.start_sec,
                    &app,
                    job.index,
                    clip_id,
                );
                let (status, message) = match outcome {
                    Ok(()) => (ExportStatus::Completed, None),
                    Err(e) => (ExportStatus::Failed, Some(e)),
                };
                collected
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(ExportResult {
                        clip_id: job.clip_id,
                        index: job.index,
                        output: job.output,
                        status,
                        message,
                    });
            }));
        }

        for handle in handles {
            let _ = handle.join();
        }

        let mut results = Arc::try_unwrap(collected)
            .map(|m| m.into_inner().unwrap_or_default())
            .unwrap_or_default();
        results.sort_by_key(|r| r.index);

        let summary = summarize(&results, skipped);
        let _ = app.emit(
            EVENT_FINISHED,
            FinishedEvent {
                completed: summary.completed.iter().map(|u| u.to_string()).collect(),
                failed: summary
                    .failed
                    .iter()
                    .map(|(u, m)| (u.to_string(), m.clone()))
                    .collect(),
                skipped: summary
                    .skipped
                    .iter()
                    .map(|s| (s.clip_id.to_string(), s.reason.clone()))
                    .collect(),
            },
        );
    });

    Ok(job_count)
}

/// Extracts timeline waveform peaks (PRD 4.1) by decoding the audio track
/// to mono PCM and reducing it to `buckets` peak magnitudes.
#[tauri::command]
pub fn waveform_peaks(path: String, buckets: usize) -> Result<Vec<f32>, String> {
    let args = rustcut_core::waveform::build_waveform_args(
        Path::new(&path),
        rustcut_core::waveform::WAVEFORM_SAMPLE_RATE,
    );
    let output = Command::new(resolve_tool("RUSTCUT_FFMPEG", "ffmpeg"))
        .args(&args)
        .output()
        .map_err(|e| format!("failed to start ffmpeg: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("waveform extraction failed: {}", stderr.trim()));
    }
    let samples = rustcut_core::waveform::pcm_s16le_samples(&output.stdout);
    Ok(rustcut_core::waveform::compute_peaks(&samples, buckets))
}

fn run_ffmpeg(
    ffmpeg: &str,
    args: &[String],
    total_sec: f64,
    app: &AppHandle,
    index: u32,
    clip_id: String,
) -> Result<(), String> {
    let args_with_progress = with_progress_args(args);

    let mut child = Command::new(ffmpeg)
        .args(&args_with_progress)
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

    let mut parser = ProgressParser::new(total_sec);
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(fraction) = parser.push_line(&line) {
            let _ = app.emit(
                EVENT_PROGRESS,
                ProgressEvent {
                    clip_id: clip_id.clone(),
                    index,
                    fraction,
                    phase: "running".to_string(),
                    message: None,
                },
            );
        }
    }

    let status = child.wait().map_err(|e| format!("wait failed: {e}"))?;
    let stderr_text = stderr_thread.join().unwrap_or_default();

    if status.success() {
        let _ = app.emit(
            EVENT_PROGRESS,
            ProgressEvent {
                clip_id,
                index,
                fraction: 1.0,
                phase: "done".to_string(),
                message: None,
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

/// Inserts `-progress pipe:1 -nostats` immediately before the output path
/// (the last argument), where FFmpeg expects global options.
pub fn with_progress_args(args: &[String]) -> Vec<String> {
    let mut out = args.to_vec();
    if let Some(output_arg) = out.pop() {
        out.push("-progress".to_string());
        out.push("pipe:1".to_string());
        out.push("-nostats".to_string());
        out.push(output_arg);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::with_progress_args;

    #[test]
    fn progress_flags_land_before_the_output_path() {
        let args: Vec<String> = ["-hide_banner", "-y", "-i", "in.mp4", "-c", "copy", "out.mp4"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = with_progress_args(&args);
        assert_eq!(
            out,
            vec![
                "-hide_banner",
                "-y",
                "-i",
                "in.mp4",
                "-c",
                "copy",
                "-progress",
                "pipe:1",
                "-nostats",
                "out.mp4",
            ]
        );
    }

    #[test]
    fn empty_args_stay_empty() {
        assert!(with_progress_args(&[]).is_empty());
    }
}
