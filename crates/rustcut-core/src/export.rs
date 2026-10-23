//! Batch export engine: job planning, progress parsing, fault tolerance.
//!
//! This module stays pure: it plans jobs and interprets FFmpeg output.
//! Actually spawning the FFmpeg processes lives in the Tauri layer.
//!
//! PRD 8.3: if one clip out of ten fails, the batch must NOT halt - the
//! failure is recorded and the remaining clips proceed.

use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::clip::Clip;
use crate::ffmpeg::{self, VideoEncoder};
use crate::naming::{build_output_stem, dedupe_names, NamingContext};
use crate::project::{estimated_export_bytes, ExportMode, ProjectState};

/// Decoded facts about the source video needed for planning.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct SourceSpec {
    pub width: u32,
    pub height: u32,
    pub bitrate_bps: f64,
}

/// One clip that cannot be exported, with the reason it was skipped.
#[derive(Debug, Clone)]
pub struct SkippedClip {
    pub clip_id: Uuid,
    pub index: u32,
    pub reason: String,
}

/// A single planned export: exactly the FFmpeg argv for one clip.
#[derive(Debug, Clone)]
pub struct ExportJob {
    pub clip_id: Uuid,
    pub index: u32,
    pub start_sec: f64,
    pub end_sec: f64,
    pub output: PathBuf,
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ExportPlan {
    pub jobs: Vec<ExportJob>,
    pub skipped: Vec<SkippedClip>,
    /// Preflight disk-space estimate: bitrate x total selected duration.
    pub estimated_bytes: u64,
}

/// Result of one job after FFmpeg exited (or failed to start).
#[derive(Debug, Clone)]
pub struct ExportResult {
    pub clip_id: Uuid,
    pub index: u32,
    pub output: PathBuf,
    pub status: crate::clip::ExportStatus,
    pub message: Option<String>,
}

#[derive(Debug, Default)]
pub struct BatchSummary {
    pub completed: Vec<Uuid>,
    pub failed: Vec<(Uuid, String)>,
    pub skipped: Vec<SkippedClip>,
}

impl BatchSummary {
    /// True only when every planned clip completed.
    pub fn all_ok(&self) -> bool {
        self.failed.is_empty() && self.skipped.is_empty()
    }
}

/// `D:/Recordings/livestream_full.mp4` -> `livestream_full`.
pub fn source_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Builds the full job list for the checked clips. Clips that are invalid
/// (non-finite or zero-length ranges) are skipped with a reason instead of
/// aborting the batch.
pub fn plan_export(
    state: &ProjectState,
    source: &SourceSpec,
    encoder: &VideoEncoder,
) -> ExportPlan {
    let settings = &state.export_settings;
    let input = Path::new(&state.source_video.path);
    let canvas = ffmpeg::canvas_dims(source.width, source.height);
    let source_name = source_stem(&state.source_video.path);

    let mut valid: Vec<&Clip> = Vec::new();
    let mut skipped: Vec<SkippedClip> = Vec::new();
    for clip in state.clips.iter().filter(|c| c.selected_for_export) {
        match clip.validate() {
            Ok(()) => valid.push(clip),
            Err(e) => skipped.push(SkippedClip {
                clip_id: clip.id,
                index: clip.index,
                reason: e.to_string(),
            }),
        }
    }

    let mut stems: Vec<String> = valid
        .iter()
        .map(|clip| {
            let ctx = NamingContext {
                index: clip.index,
                clip_name: &clip.name,
                source_name: &source_name,
                start_sec: clip.start_sec,
                end_sec: clip.end_sec,
            };
            build_output_stem(&settings.naming_pattern, &ctx)
        })
        .collect();
    dedupe_names(&mut stems);

    let mut total_duration = 0.0;
    let mut jobs = Vec::with_capacity(valid.len());
    for (clip, stem) in valid.iter().zip(stems.iter()) {
        let output = Path::new(&settings.output_dir).join(format!("{stem}.mp4"));
        let args = match settings.mode {
            ExportMode::LosslessCopy => {
                ffmpeg::build_lossless_args(input, clip.start_sec, clip.end_sec, &output)
            }
            ExportMode::SmartReencode => ffmpeg::build_reencode_args(
                input,
                clip.start_sec,
                clip.end_sec,
                &output,
                canvas,
                settings.crop_style,
                clip.crop_x_offset,
                encoder,
            ),
        };
        total_duration += clip.duration_sec();
        jobs.push(ExportJob {
            clip_id: clip.id,
            index: clip.index,
            start_sec: clip.start_sec,
            end_sec: clip.end_sec,
            output,
            args,
        });
    }

    ExportPlan {
        jobs,
        skipped,
        estimated_bytes: estimated_export_bytes(source.bitrate_bps, total_duration),
    }
}

/// Folds per-job results into batch totals. A failed job is recorded and the
/// batch continues (PRD 8.3).
pub fn summarize(results: &[ExportResult], skipped: Vec<SkippedClip>) -> BatchSummary {
    let mut summary = BatchSummary {
        skipped,
        ..Default::default()
    };
    for r in results {
        match r.status {
            crate::clip::ExportStatus::Completed => summary.completed.push(r.clip_id),
            _ => summary.failed.push((
                r.clip_id,
                r.message
                    .clone()
                    .unwrap_or_else(|| "export did not complete".to_string()),
            )),
        }
    }
    summary
}

/// Reads FFmpeg `-progress pipe:1` lines and turns them into a completion
/// fraction for one job. Uses `out_time=` (parsed with the shared timecode
/// parser) because `out_time_ms` is historically misnamed and unreliable.
#[derive(Debug)]
pub struct ProgressParser {
    total_sec: f64,
    fraction: f64,
}

impl ProgressParser {
    pub fn new(total_sec: f64) -> Self {
        Self {
            total_sec: total_sec.max(0.0),
            fraction: 0.0,
        }
    }

    /// Feed one output line; returns the updated fraction when the line
    /// carried timing information.
    pub fn push_line(&mut self, line: &str) -> Option<f64> {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("out_time=")
            && let Ok(secs) = crate::timecode::parse_tc(value)
        {
            if self.total_sec > 0.0 {
                self.fraction = (secs / self.total_sec).clamp(0.0, 1.0);
            }
            return Some(self.fraction);
        }
        if line == "progress=end" {
            self.fraction = 1.0;
            return Some(self.fraction);
        }
        None
    }

    pub fn fraction(&self) -> f64 {
        self.fraction
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{Clip, ExportStatus};
    use crate::project::{ExportSettings, SourceVideo};

    fn state_with(clips: Vec<Clip>) -> ProjectState {
        let mut state = ProjectState::new(
            SourceVideo {
                path: "D:/Recordings/livestream_full.mp4".to_string(),
                duration_sec: 9000.0,
                resolution: "1920x1080".to_string(),
                fps: 60.0,
            },
            ExportSettings::default_for("D:/Recordings/Clips_Output"),
        );
        state.clips = clips;
        state
    }

    fn spec() -> SourceSpec {
        SourceSpec {
            width: 1920,
            height: 1080,
            bitrate_bps: 8_000_000.0,
        }
    }

    #[test]
    fn plans_clips_and_dedupes_identical_output_names() {
        let a = Clip::new(1, "Jump Scare Reaction", 920.120, 955.800).unwrap();
        let b = Clip::new(2, "Jump Scare Reaction", 130.5, 180.0).unwrap();
        let mut state = state_with(vec![a, b]);
        // Pattern without {index} makes both stems collide on purpose.
        state.export_settings.naming_pattern = "{name}".to_string();

        let plan = plan_export(&state, &spec(), &VideoEncoder::software_default());
        assert_eq!(plan.jobs.len(), 2);
        assert!(plan.skipped.is_empty());
        // Duplicate names must not overwrite each other.
        assert_eq!(
            plan.jobs[0].output.file_name().unwrap().to_string_lossy(),
            "Jump_Scare_Reaction.mp4"
        );
        assert_eq!(
            plan.jobs[1].output.file_name().unwrap().to_string_lossy(),
            "Jump_Scare_Reaction_2.mp4"
        );
    }

    #[test]
    fn skips_unchecked_and_invalid_clips_without_aborting() {
        let mut checked = Clip::new(1, "ok", 10.0, 20.0).unwrap();
        checked.selected_for_export = true;
        let mut unchecked = Clip::new(2, "off", 30.0, 40.0).unwrap();
        unchecked.selected_for_export = false;
        let mut invalid = Clip::new(3, "bad", 50.0, 60.0).unwrap();
        invalid.end_sec = 50.0; // zero-length range smuggled past the constructor
        let state = state_with(vec![checked, unchecked, invalid]);

        let plan = plan_export(&state, &spec(), &VideoEncoder::software_default());
        assert_eq!(plan.jobs.len(), 1);
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].index, 3);
    }

    #[test]
    fn builds_lossless_and_reencode_args_per_mode() {
        let clip = Clip::new(1, "Opening Moment", 130.5, 180.0).unwrap();
        let mut state = state_with(vec![clip]);

        let plan = plan_export(&state, &spec(), &VideoEncoder::software_default());
        assert!(plan.jobs[0].args.iter().any(|a| a == "copy"));

        state.export_settings.mode = ExportMode::SmartReencode;
        let plan = plan_export(&state, &spec(), &VideoEncoder::software_default());
        assert!(plan.jobs[0].args.iter().any(|a| a == "-vf"));
        assert!(!plan.jobs[0].args.iter().any(|a| a == "copy"));
    }

    #[test]
    fn estimates_disk_space_from_bitrate() {
        // 8 Mbit/s x 49.5 s = 49.5 MB.
        let clip = Clip::new(1, "Opening Moment", 130.5, 180.0).unwrap();
        let state = state_with(vec![clip]);
        let plan = plan_export(&state, &spec(), &VideoEncoder::software_default());
        assert_eq!(plan.estimated_bytes, 49_500_000);
    }

    #[test]
    fn progress_lines_become_fractions() {
        let mut p = ProgressParser::new(10.0);
        assert_eq!(p.push_line("frame=100"), None);
        assert_eq!(p.push_line("out_time_us=5000000"), None); // ignored by design
        assert_eq!(p.push_line("out_time=00:00:05.000000"), Some(0.5));
        assert_eq!(p.push_line("progress=continue"), None);
        assert_eq!(p.push_line("progress=end"), Some(1.0));
    }

    #[test]
    fn one_failure_never_sinks_the_batch() {
        let results = vec![
            ExportResult {
                clip_id: Uuid::new_v4(),
                index: 1,
                output: PathBuf::from("a.mp4"),
                status: ExportStatus::Completed,
                message: None,
            },
            ExportResult {
                clip_id: Uuid::new_v4(),
                index: 2,
                output: PathBuf::from("b.mp4"),
                status: ExportStatus::Failed,
                message: Some("corrupted frame".to_string()),
            },
        ];
        let summary = summarize(&results, Vec::new());
        assert_eq!(summary.completed.len(), 1);
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].1, "corrupted frame");
        assert!(!summary.all_ok());
    }

    #[test]
    fn derives_the_source_stem_for_naming() {
        assert_eq!(source_stem("D:/Recordings/livestream_full.mp4"), "livestream_full");
        assert_eq!(source_stem("C:\\clips\\vod.mkv"), "vod");
    }
}
