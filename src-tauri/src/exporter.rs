//! FFmpeg execution layer: media probing and batch export runs.
//!
//! Planning, naming and progress parsing live in `rustcut-core`; this module
//! spawns the external tools and streams progress to the UI as Tauri events
//! (`export://progress` per clip, `export://finished` at the end).
//!
//! One failing clip never stops the batch (PRD 8.3): the error is reported
//! as a `failed` phase event and the remaining clips continue.

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::thread;

use rustcut_core::clip::ExportStatus;
use rustcut_core::export::{plan_export, summarize, ExportPlan, ExportResult, ProgressParser, SourceSpec};
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
fn resolve_tool(env_key: &str, default: &str) -> String {
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

/// Plans and launches a batch export on a background thread. Returns the
/// number of jobs immediately; progress arrives via `export://progress`.
#[tauri::command]
pub fn start_export(
    app: AppHandle,
    state: ProjectState,
    source: SourceSpec,
) -> Result<usize, String> {
    let ExportPlan {
        jobs,
        skipped,
        estimated_bytes: _,
    } = plan_export(&state, &source, &VideoEncoder::software_default());

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
    thread::spawn(move || {
        let mut results: Vec<ExportResult> = Vec::new();
        for job in jobs {
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
                clip_id.clone(),
            );

            let (status, message) = match outcome {
                Ok(()) => (ExportStatus::Completed, None),
                Err(e) => (ExportStatus::Failed, Some(e)),
            };
            results.push(ExportResult {
                clip_id: job.clip_id,
                index: job.index,
                output: job.output,
                status,
                message,
            });
        }

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
