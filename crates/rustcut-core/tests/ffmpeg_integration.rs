//! End-to-end tests against the real FFmpeg binary.
//!
//! These tests generate a short test video, plan exports with the real
//! pipeline, run the generated FFmpeg argv exactly as the app will, and
//! verify the outputs with ffprobe. They skip gracefully when the tools
//! are not on PATH.

use std::path::{Path, PathBuf};
use std::process::Command;

use rustcut_core::clip::Clip;
use rustcut_core::export::{plan_export, SourceSpec};
use rustcut_core::ffmpeg::VideoEncoder;
use rustcut_core::probe::parse_ffprobe_json;
use rustcut_core::project::{ExportMode, ExportSettings, ProjectState, SourceVideo};

fn tools_present() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok()
        && Command::new("ffprobe").arg("-version").output().is_ok()
}

fn probe(path: &Path) -> rustcut_core::probe::MediaInfo {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .expect("ffprobe must run");
    assert!(output.status.success(), "ffprobe failed on {}", path.display());
    parse_ffprobe_json(&String::from_utf8_lossy(&output.stdout)).expect("probe output parses")
}

fn run_job(args: &[String]) {
    let status = Command::new("ffmpeg")
        .args(args)
        .status()
        .expect("ffmpeg must run");
    assert!(status.success(), "ffmpeg failed with args: {args:?}");
}

fn make_source(dir: &Path) -> PathBuf {
    let src = dir.join("testsrc.mp4");
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=6:size=1920x1080:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=6",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "28",
            // Keyframes every 30 frames (1 s): realistic for a VOD and
            // required for the lossless test - stream copy can only cut at
            // keyframes (PRD 9), so a sparse GOP shifts the cut to t=0.
            "-g",
            "30",
            "-keyint_min",
            "30",
            "-sc_threshold",
            "0",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&src)
        .status()
        .expect("ffmpeg must run");
    assert!(status.success(), "test clip generation failed");
    src
}

fn project_for(src: &Path, out_dir: &Path) -> ProjectState {
    let mut state = ProjectState::new(
        SourceVideo {
            path: src.to_string_lossy().into_owned(),
            duration_sec: 6.0,
            resolution: "1920x1080".to_string(),
            fps: 30.0,
        },
        ExportSettings::default_for(out_dir.to_string_lossy()),
    );
    state.clips.push(Clip::new(1, "Marker One", 2.0, 4.0).unwrap());
    state
}

#[test]
fn lossless_export_cuts_a_real_file() {
    if !tools_present() {
        eprintln!("ffmpeg/ffprobe not on PATH; skipping integration test");
        return;
    }
    let dir = std::env::temp_dir().join(format!("rustcut-it-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = make_source(&dir);

    let state = project_for(&src, &dir);
    let plan = plan_export(
        &state,
        &SourceSpec {
            width: 1920,
            height: 1080,
            bitrate_bps: 8_000_000.0,
        },
        &VideoEncoder::software_default(),
    );
    assert_eq!(plan.jobs.len(), 1);
    run_job(&plan.jobs[0].args);

    let out = &plan.jobs[0].output;
    assert!(out.exists(), "output missing: {}", out.display());
    let info = probe(out);
    // Stream copy cuts at keyframes, so allow slack on the exact length.
    assert!(
        (info.duration_sec - 2.0).abs() < 0.75,
        "unexpected duration {}",
        info.duration_sec
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn reencode_export_produces_9_16_crop_at_offset() {
    if !tools_present() {
        eprintln!("ffmpeg/ffprobe not on PATH; skipping integration test");
        return;
    }
    let dir = std::env::temp_dir().join(format!("rustcut-it-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = make_source(&dir);

    let mut state = project_for(&src, &dir);
    state.export_settings.mode = ExportMode::SmartReencode;
    state.clips[0].crop_x_offset = 0.5;

    let plan = plan_export(
        &state,
        &SourceSpec {
            width: 1920,
            height: 1080,
            bitrate_bps: 8_000_000.0,
        },
        &VideoEncoder::software_default(),
    );
    run_job(&plan.jobs[0].args);

    let info = probe(&plan.jobs[0].output);
    // 9:16 window on a 1080-high canvas is 608x1080 (PRD 4.4 / 7.4).
    assert_eq!(info.width, 608, "crop width");
    assert_eq!(info.height, 1080, "crop height");
    assert!((info.duration_sec - 2.0).abs() < 0.2, "duration {}", info.duration_sec);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn naming_template_produces_the_expected_file_name_on_disk() {
    if !tools_present() {
        eprintln!("ffmpeg/ffprobe not on PATH; skipping integration test");
        return;
    }
    let dir = std::env::temp_dir().join(format!("rustcut-it-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = make_source(&dir);

    let mut state = project_for(&src, &dir);
    state.export_settings.naming_pattern = "{index}_{clip_name}".to_string();
    state.clips[0].name = "Jump Scare Reaction".to_string();

    let plan = plan_export(
        &state,
        &SourceSpec {
            width: 1920,
            height: 1080,
            bitrate_bps: 8_000_000.0,
        },
        &VideoEncoder::software_default(),
    );
    run_job(&plan.jobs[0].args);

    let expected = dir.join("01_Jump_Scare_Reaction.mp4");
    assert!(
        expected.exists(),
        "expected {} on disk",
        expected.display()
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn blurred_background_export_keeps_the_full_frame_in_9_16() {
    if !tools_present() {
        eprintln!("ffmpeg/ffprobe not on PATH; skipping integration test");
        return;
    }
    let dir = std::env::temp_dir().join(format!("rustcut-it-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = make_source(&dir);

    let mut state = project_for(&src, &dir);
    state.export_settings.mode = ExportMode::SmartReencode;
    state.export_settings.crop_style = rustcut_core::ffmpeg::CropStyle::BlurredBackground;

    let plan = plan_export(
        &state,
        &SourceSpec {
            width: 1920,
            height: 1080,
            bitrate_bps: 8_000_000.0,
        },
        &VideoEncoder::software_default(),
    );
    run_job(&plan.jobs[0].args);

    let info = probe(&plan.jobs[0].output);
    assert_eq!(info.width, 608, "9:16 window width");
    assert_eq!(info.height, 1080, "window height");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn waveform_peaks_come_out_of_real_audio() {
    if !tools_present() {
        eprintln!("ffmpeg/ffprobe not on PATH; skipping integration test");
        return;
    }
    let dir = std::env::temp_dir().join(format!("rustcut-it-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = make_source(&dir);

    let args = rustcut_core::waveform::build_waveform_args(&src, 1000);
    let output = Command::new("ffmpeg")
        .args(&args)
        .output()
        .expect("ffmpeg must run");
    assert!(output.status.success(), "waveform decode failed");
    let samples = rustcut_core::waveform::pcm_s16le_samples(&output.stdout);
    assert!(
        samples.len() > 1000,
        "expected ~6s of audio at 1 kHz, got {}",
        samples.len()
    );
    let peaks = rustcut_core::waveform::compute_peaks(&samples, 60);
    assert_eq!(peaks.len(), 60);
    assert!(peaks.iter().any(|p| *p > 0.0), "sine tone must register");
    assert!(peaks.iter().all(|p| (0.0..=1.0).contains(p)));

    std::fs::remove_dir_all(&dir).ok();
}
