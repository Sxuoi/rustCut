//! Engine integration tests against real FFmpeg and the testkit fixtures
//! (skipped when FFmpeg is missing). Requirement ids are in the test names.

mod common;

use common::*;
use rustcut_export::{BatchRequest, ExportError, FsProbe, PreflightInput, preflight};
use rustcut_model::{
    BatchState, CheckLevel, ClipOutcome, ConflictPolicy, EncoderChoice, HwEncoderInfo, LastExport,
    PauseReason, PlannedAction, PreflightCheck, Preset, compute_fingerprint,
};
use rustcut_runner::is_process_running;
use rustcut_testkit::{
    Fixture, edge_luma, frame_count, frame_luma, packet_hashes, video_pts_times, video_size,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// AC-07 (EXP-02): lossless 3.0-8.0 keeps the source packets from the keyframe
/// at/before In (2.0 s) up to the last frame before Out; MP4; lead-in 1.0 s.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac07_lossless_packets_are_bit_identical() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::Baseline1080p60);
    let project = project_with(&src, &[(3.0, 8.0, "Lossless cut")]);
    let index = media_index(&src, project.source.start_time_sec);
    let settings = env.settings();
    let report = run_preflight(&project, &settings, Some(&index));
    assert_eq!(
        report.outputs[0].lead_in_sec,
        Some(1.0),
        "lead-in reported by pre-flight"
    );
    assert_eq!(report.extension, "mp4");
    let sink = TestSink::new();
    let summary = env.run(&project, &settings, Some(index), &sink).await;
    assert_eq!(
        summary.done.len(),
        1,
        "{:?}",
        sink.outcome(project.clips[0].id)
    );
    let out = sink.done_path(project.clips[0].id);
    assert_eq!(out.extension().unwrap(), "mp4");

    let src_hashes = packet_hashes(&src, "0:v:0");
    let out_hashes = packet_hashes(&out, "0:v:0");
    let n = out_hashes.len();
    let start = src_hashes
        .windows(n)
        .position(|w| w == out_hashes.as_slice())
        .expect("output packets are a contiguous run of the source packets");
    assert_eq!(
        start, 120,
        "starts on the 2.0 s keyframe (120 frames at 60 fps)"
    );
    assert!(
        (358..=362).contains(&n),
        "6 s of frames up to the last one before Out, got {n}"
    );
    // Every other stream is copied too: audio packets of the output are a
    // contiguous run of the source's audio packets (nothing dropped or re-encoded).
    let src_audio = packet_hashes(&src, "0:a:0");
    let out_audio = packet_hashes(&out, "0:a:0");
    assert_eq!(
        src_audio.is_empty(),
        out_audio.is_empty(),
        "audio stream presence"
    );
    if !out_audio.is_empty() {
        assert!(
            src_audio
                .windows(out_audio.len())
                .any(|w| w == out_audio.as_slice()),
            "audio packets are a contiguous run of the source audio packets"
        );
    }
    assert!(env.leftover_parts().is_empty());
}

/// AC-08 (EXP-02): frame-accurate output starts on the source frame at In.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac08_frame_accurate_starts_on_the_exact_frame() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::FrameNumbered);
    // frame 61 at 61/30 s (rounded to 2.033); luma(N) = (N*7 mod 200) + 30
    let project = project_with(&src, &[(2.033, 5.033, "exact")]);
    let mut settings = env.settings();
    settings.apply_preset(Preset::Accurate);
    let sink = TestSink::new();
    let summary = env.run(&project, &settings, None, &sink).await;
    assert_eq!(
        summary.done.len(),
        1,
        "{:?}",
        sink.outcome(project.clips[0].id)
    );
    let out = sink.done_path(project.clips[0].id);
    let src_luma = i32::from(frame_luma(&src, 2.033));
    let out_luma = i32::from(frame_luma(&out, 0.0));
    assert_eq!(src_luma, 57, "fixture sanity: frame 61");
    assert!(
        (src_luma - out_luma).abs() <= 3,
        "first frame luma {out_luma} vs source {src_luma}"
    );
    assert_eq!(frame_count(&out), 90, "round(3.0 s x 30 fps)");
}

/// AC-08, VFR fixture: frame count within one frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac08_vfr_frame_count_within_one_frame() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::Vfr);
    let project = project_with(&src, &[(2.0, 5.0, "vfr")]);
    let mut settings = env.settings();
    settings.apply_preset(Preset::Accurate);
    let sink = TestSink::new();
    let summary = env.run(&project, &settings, None, &sink).await;
    assert_eq!(
        summary.done.len(),
        1,
        "{:?}",
        sink.outcome(project.clips[0].id)
    );
    let out = sink.done_path(project.clips[0].id);
    let expected = video_pts_times(&src)
        .iter()
        .filter(|t| **t >= 2.0 && **t < 5.0)
        .count() as i64;
    let got = frame_count(&out) as i64;
    assert!(
        (got - expected).abs() <= 1,
        "expected {expected} frames, got {got}"
    );
    // The first output frame is the source frame at In (within one frame).
    let src_pts = video_pts_times(&src);
    let first_idx = src_pts.iter().position(|t| *t >= 2.0).expect("frame at In");
    let frame_gap = src_pts[first_idx + 1] - src_pts[first_idx];
    let out_first = video_pts_times(&out)[0];
    let want_first = src_pts[first_idx] - 2.0;
    assert!(
        (out_first - want_first).abs() <= frame_gap + 1e-3,
        "first output frame at {out_first}, source In frame at {want_first} (gap {frame_gap})"
    );
}

/// AC-09 (EXP-10): in a 10-clip batch one clip fails; the other nine complete.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac09_one_failing_clip_does_not_stop_the_batch() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::FrameNumbered);
    let ranges: Vec<(f64, f64, String)> = (0..10)
        .map(|i| (1.0 + f64::from(i), 2.0 + f64::from(i), format!("c{i}")))
        .collect();
    let ranges: Vec<(f64, f64, &str)> = ranges
        .iter()
        .map(|(a, b, n)| (*a, *b, n.as_str()))
        .collect();
    let mut project = project_with(&src, &ranges);
    // Built to fail: Out before In makes FFmpeg abort ("-to value smaller than -ss").
    project.clips[4].start_sec = 6.0;
    project.clips[4].end_sec = 5.0;
    let bad = project.clips[4].id;
    let sink = TestSink::new();
    let summary = env.run(&project, &env.settings(), None, &sink).await;
    assert_eq!(summary.done.len(), 9);
    assert_eq!(summary.failed, vec![bad]);
    assert_eq!(sink.finished_count(), 1, "exactly one Finished per batch");
    match sink.outcome(bad).unwrap() {
        ClipOutcome::Failed {
            message,
            log_tail,
            log_path,
            exit_code,
        } => {
            assert!(!message.trim().is_empty(), "readable reason");
            assert!(!log_tail.is_empty() && log_tail.len() <= 20);
            assert!(exit_code.is_some());
            let log =
                std::fs::read_to_string(log_path.expect("log path")).expect("full log on disk");
            assert!(log.contains("-to value smaller than -ss"), "{log}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    let rec = sink
        .records
        .lock()
        .unwrap()
        .iter()
        .find(|(r, _)| r.clip_id == bad)
        .cloned()
        .unwrap();
    assert_eq!(rec.0.preset, Preset::Lossless);
    assert!(env.leftover_parts().is_empty());
    assert!(!env.engine.is_running());
}

/// AC-10 (EXP-08/09): cancelling mid-clip leaves no `.part` and no FFmpeg process.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac10_cancel_mid_clip_leaves_nothing_behind() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::Baseline1080p60);
    let project = project_with(&src, &[(0.0, 12.0, "long")]);
    let mut settings = env.settings();
    settings.apply_preset(Preset::Vblur);
    let sink = TestSink::new();
    env.start(&project, &settings, None, &sink).expect("start");
    let journal = rustcut_export::Journal::new(&env.data_dir);
    sink.wait_until(
        Duration::from_secs(60),
        "the process to be journaled",
        |_| journal.entries().iter().any(|e| e.pid.is_some()),
    )
    .await;
    let entry = journal
        .entries()
        .into_iter()
        .find(|e| e.pid.is_some())
        .unwrap();
    let pid = entry.pid.unwrap();
    assert!(
        is_process_running(pid),
        "FFmpeg is running before the cancel"
    );
    assert!(entry.part_path.to_string_lossy().ends_with(".mp4.part"));
    tokio::time::sleep(Duration::from_millis(500)).await;
    env.engine.cancel_clip(project.clips[0].id);
    let summary = sink.wait_finished(Duration::from_secs(60)).await;
    assert_eq!(summary.cancelled, vec![project.clips[0].id]);
    assert!(matches!(
        sink.outcome(project.clips[0].id),
        Some(ClipOutcome::Cancelled)
    ));
    assert!(env.leftover_parts().is_empty(), "no .part left");
    assert!(!entry.part_path.exists());
    assert!(journal.entries().is_empty(), "journal entry dropped");
    assert!(!is_process_running(pid), "the FFmpeg process is gone");
    assert!(!env.engine.is_running());
}

/// EXP-09: cancel all kills the running clip and cancels the queued ones; Busy while running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exp09_cancel_all_cancels_running_and_queued_with_one_summary() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::Baseline1080p60);
    let project = project_with(
        &src,
        &[(0.0, 12.0, "a"), (0.0, 12.0, "b"), (0.0, 12.0, "c")],
    );
    let mut settings = env.settings();
    settings.apply_preset(Preset::Vblur);
    let sink = TestSink::new();
    env.start(&project, &settings, None, &sink).expect("start");
    sink.wait_until(Duration::from_secs(60), "first process", |s| {
        !s.pids.lock().unwrap().is_empty()
    })
    .await;
    assert!(env.engine.is_running());
    let again = env.start(&project, &settings, None, &TestSink::new());
    assert!(
        matches!(again, Err(ExportError::Busy { .. })),
        "EXP-12 one batch at a time"
    );
    env.engine.cancel_all();
    let summary = sink.wait_finished(Duration::from_secs(60)).await;
    assert_eq!(summary.cancelled.len(), 3);
    assert!(summary.done.is_empty() && summary.failed.is_empty());
    assert_eq!(
        summary.cancelled,
        project.clips.iter().map(|c| c.id).collect::<Vec<_>>(),
        "clip-number order"
    );
    assert_eq!(
        sink.pids.lock().unwrap().len(),
        1,
        "queued clips never spawned FFmpeg"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(sink.finished_count(), 1);
    assert!(env.leftover_parts().is_empty());
    assert!(!env.engine.is_running());
}

/// EXP-11: the engine refuses a report with errors.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exp11_start_refuses_a_blocked_report() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::FrameNumbered);
    let project = project_with(&src, &[(1.0, 2.0, "a")]);
    let settings = env.settings();
    let mut report = run_preflight(&project, &settings, None);
    report.items.push(rustcut_model::PreflightItem {
        check: PreflightCheck::FreeSpace,
        level: CheckLevel::Error,
        message: "Not enough free disk space".into(),
        clip_ids: vec![],
    });
    report.can_start = false;
    let r = env.engine.start(
        BatchRequest {
            project,
            settings,
            report,
            media_index: None,
        },
        TestSink::new(),
    );
    match r {
        Err(ExportError::PreflightFailed(m)) => assert!(m.contains("Not enough free disk space")),
        other => panic!("expected PreflightFailed, got {other:?}"),
    }
    assert!(!env.engine.is_running());
}

struct FakeFs {
    free: Option<u64>,
    writable: bool,
}

impl FsProbe for FakeFs {
    fn free_space(&self, _: &Path) -> Option<u64> {
        self.free
    }
    fn exists(&self, p: &Path) -> bool {
        p.exists()
    }
    fn writable(&self, _: &Path) -> bool {
        self.writable
    }
}

/// AC-16 (EXP-11) against a real project: free space < 1.2 x estimate, and an
/// unwritable folder, both block.
#[test]
fn ac16_preflight_blocks_on_free_space_and_unwritable_folder() {
    let Some(_sc) = sidecars() else { return };
    let src = fixture(Fixture::Baseline1080p60);
    let project = project_with(&src, &[(1.0, 6.0, "a"), (7.0, 11.0, "b")]);
    let tmp = tempfile::tempdir().unwrap();
    let settings = rustcut_model::ExportSettings {
        output_dir: Some(tmp.path().join("out").to_string_lossy().into_owned()),
        ..Default::default()
    };
    let ids: Vec<_> = project.clips.iter().map(|c| c.id).collect();
    let run = |fs: &dyn FsProbe| {
        preflight(&PreflightInput {
            project: &project,
            clip_ids: &ids,
            settings: &settings,
            media_index: None,
            current_fingerprint: compute_fingerprint(&src).ok(),
            zscale_available: false,
            today: today(),
            fs,
        })
    };
    let ok = run(&FakeFs {
        free: Some(u64::MAX / 4),
        writable: true,
    });
    assert!(ok.can_start, "{:?}", ok.items);
    let need = rustcut_export::required_free_bytes(ok.estimate_bytes);
    let low = run(&FakeFs {
        free: Some(need - 1),
        writable: true,
    });
    assert!(!low.can_start);
    assert!(
        low.items
            .iter()
            .any(|i| i.check == PreflightCheck::FreeSpace && i.level == CheckLevel::Error)
    );
    let ro = run(&FakeFs {
        free: Some(u64::MAX / 4),
        writable: false,
    });
    assert!(!ro.can_start);
    assert!(
        ro.items
            .iter()
            .any(|i| i.check == PreflightCheck::OutputWritable && i.level == CheckLevel::Error)
    );
}

/// AC-17 (FRM-02/03): vertical crop of every ramp at offsets -1, 0, +1 is 1080x1920
/// with no black edges; vertical blur of the 16:9 ramp is 1080x1920.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac17_vertical_crop_has_no_black_edges_for_every_aspect() {
    let sc = require_ffmpeg!();
    for fx in [
        Fixture::Ramp16x9,
        Fixture::Ramp4x3,
        Fixture::Ramp21x9,
        Fixture::Ramp9x16,
        Fixture::RampTall,
        Fixture::RampSar,
    ] {
        let env = env(sc.clone());
        let src = fixture(fx);
        let mut project = project_with(
            &src,
            &[
                (0.5, 1.5, "left"),
                (0.5, 1.5, "center"),
                (0.5, 1.5, "right"),
            ],
        );
        for (c, off) in project.clips.iter_mut().zip([-1.0, 0.0, 1.0]) {
            c.crop_x_offset = off;
        }
        let mut settings = env.settings();
        settings.apply_preset(Preset::Vcrop);
        settings.parallel_jobs = 3;
        let sink = TestSink::new();
        let summary = env.run(&project, &settings, None, &sink).await;
        assert_eq!(summary.done.len(), 3, "{fx:?}: {:?}", summary.failed);
        let mut means = Vec::new();
        for c in &project.clips {
            let out = sink.done_path(c.id);
            assert_eq!(
                video_size(&out),
                (1080, 1920),
                "{fx:?} offset {}",
                c.crop_x_offset
            );
            means.push(i32::from(frame_luma(&out, 0.3)));
            let e = edge_luma(&out, 0.3);
            assert!(
                e.min() > 10.0,
                "{fx:?} offset {}: black edge {e:?}",
                c.crop_x_offset
            );
        }
        // The exported region is the window: on a horizontal ramp the mean luma
        // rises with the offset (left < center < right) and the centered window
        // matches the source mean (ramp is symmetric). Full-width sources
        // (9:16 and taller) have no horizontal room, so all three are equal.
        let src_mean = i32::from(frame_luma(&src, 0.3));
        if matches!(fx, Fixture::Ramp16x9 | Fixture::Ramp4x3 | Fixture::Ramp21x9) {
            assert!(
                means[0] < means[1] && means[1] < means[2],
                "{fx:?}: window luma must follow the offset, got {means:?}"
            );
            assert!(
                (means[1] - src_mean).abs() <= 12,
                "{fx:?}: centered window {} vs source {src_mean}",
                means[1]
            );
        } else if matches!(fx, Fixture::Ramp9x16 | Fixture::RampTall) {
            assert!(
                means.iter().all(|m| (m - means[1]).abs() <= 3),
                "{fx:?}: no horizontal room, got {means:?}"
            );
        }
    }
    let env = env(sc);
    let src = fixture(Fixture::Ramp16x9);
    let project = project_with(&src, &[(0.5, 1.5, "blur")]);
    let mut settings = env.settings();
    settings.apply_preset(Preset::Vblur);
    let sink = TestSink::new();
    let summary = env.run(&project, &settings, None, &sink).await;
    assert_eq!(
        summary.done.len(),
        1,
        "{:?}",
        sink.outcome(project.clips[0].id)
    );
    assert_eq!(
        video_size(&sink.done_path(project.clips[0].id)),
        (1080, 1920)
    );
}

/// AC-19 (EXP-12): four parallel jobs never exceed four FFmpeg processes and
/// produce the same files as a sequential run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac19_parallel_jobs_cap_and_match_sequential() {
    let sc = require_ffmpeg!();
    let src = fixture(Fixture::Baseline1080p60);
    let ranges: Vec<(f64, f64, String)> = (0..8)
        .map(|i| (f64::from(i), f64::from(i) + 1.0, format!("p{i}")))
        .collect();
    let ranges: Vec<(f64, f64, &str)> = ranges
        .iter()
        .map(|(a, b, n)| (*a, *b, n.as_str()))
        .collect();
    let project = project_with(&src, &ranges);
    let mut result = Vec::new();
    for jobs in [1u32, 4] {
        let env = env(sc.clone());
        let mut settings = env.settings();
        settings.apply_preset(Preset::Accurate);
        settings.parallel_jobs = jobs;
        let sink = TestSink::new();
        let summary = env.run(&project, &settings, None, &sink).await;
        assert_eq!(summary.done.len(), 8, "jobs={jobs}");
        let peak = sink.peak.load(Ordering::SeqCst);
        assert!(
            peak <= jobs as usize,
            "jobs={jobs}: {peak} concurrent FFmpeg processes"
        );
        if jobs == 4 {
            assert!(
                peak >= 2,
                "the scheduler actually ran jobs in parallel (peak {peak})"
            );
        }
        let mut files: Vec<(String, u64, u8)> = project
            .clips
            .iter()
            .map(|c| {
                let p = sink.done_path(c.id);
                (
                    p.file_name().unwrap().to_string_lossy().into_owned(),
                    frame_count(&p),
                    frame_luma(&p, 0.0),
                )
            })
            .collect();
        files.sort();
        result.push(files);
        // clips start in clip-number order
        let started: Vec<_> = sink.started.lock().unwrap().iter().map(|s| s.0).collect();
        if jobs == 1 {
            assert_eq!(
                started,
                project.clips.iter().map(|c| c.id).collect::<Vec<_>>()
            );
        }
    }
    // Same names and frame counts; decoded first-frame luma equal within the
    // encoder rounding (content, not just metadata).
    assert_eq!(result[0].len(), result[1].len());
    for (a, b) in result[0].iter().zip(&result[1]) {
        assert_eq!((&a.0, a.1), (&b.0, b.1), "same files as a sequential run");
        assert!(
            (i32::from(a.2) - i32::from(b.2)).abs() <= 2,
            "{}: first-frame luma {} vs {}",
            a.0,
            a.2,
            b.2
        );
    }
}

/// EXP-03/04: OBS-style MKV with 4 audio tracks -> MP4 with all 4 tracks.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exp03_exp04_mkv_four_audio_tracks_to_mp4() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::ObsMkv4Audio);
    let project = project_with(&src, &[(1.0, 3.0, "obs")]);
    let sink = TestSink::new();
    let summary = env.run(&project, &env.settings(), None, &sink).await;
    assert_eq!(
        summary.done.len(),
        1,
        "{:?}",
        sink.outcome(project.clips[0].id)
    );
    let out = sink.done_path(project.clips[0].id);
    assert_eq!(out.extension().unwrap(), "mp4");
    assert_eq!(streams_of(&out, "audio").len(), 4);
    assert_eq!(streams_of(&out, "video").len(), 1);
}

/// EXP-03: PCM audio blocks MP4; the dialog offers AAC, accepted -> MP4 + AAC.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exp03_pcm_audio_offers_aac() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::PcmAudio);
    let project = project_with(&src, &[(1.0, 3.0, "pcm")]);
    let mut settings = env.settings();
    let before = run_preflight(&project, &settings, None);
    assert!(before.offer_aac_audio);
    assert_eq!(before.extension, "mov");
    settings.lossless_audio_to_aac = true;
    let after = run_preflight(&project, &settings, None);
    assert_eq!(after.extension, "mp4");
    let sink = TestSink::new();
    let summary = env.run(&project, &settings, None, &sink).await;
    assert_eq!(
        summary.done.len(),
        1,
        "{:?}",
        sink.outcome(project.clips[0].id)
    );
    let out = sink.done_path(project.clips[0].id);
    assert_eq!(out.extension().unwrap(), "mp4");
    let audio = streams_of(&out, "audio");
    assert_eq!(audio.len(), 1);
    assert_eq!(audio[0]["codec_name"], "aac");
    assert_eq!(streams_of(&out, "video")[0]["codec_name"], "h264");
}

/// EXP-03: HEVC lossless into MP4 gets the hvc1 tag and keeps the rotation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exp03_hevc_hlg_rotated_keeps_hvc1_tag_and_rotation() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::HevcHlgRotated);
    let project = project_with(&src, &[(0.5, 3.0, "hevc")]);
    assert!(
        project.source.video.hdr,
        "fixture is HDR (lossless is unaffected)"
    );
    let sink = TestSink::new();
    let summary = env.run(&project, &env.settings(), None, &sink).await;
    assert_eq!(
        summary.done.len(),
        1,
        "{:?}",
        sink.outcome(project.clips[0].id)
    );
    let out = sink.done_path(project.clips[0].id);
    assert_eq!(out.extension().unwrap(), "mp4");
    assert_eq!(streams_of(&out, "video")[0]["codec_tag_string"], "hvc1");
    assert_eq!(
        probe_source(&out).video.rotation,
        project.source.video.rotation
    );
    assert_ne!(project.source.video.rotation, 0);
}

/// EXP-04: no audio in, video only out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exp04_no_audio_source_exports_video_only() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::NoAudio);
    let project = project_with(&src, &[(0.5, 2.0, "silent")]);
    let mut settings = env.settings();
    for preset in [Preset::Lossless, Preset::Vcrop] {
        settings.apply_preset(preset);
        settings.naming_template = format!("{{index}}_{{name}}_{}", preset.as_str());
        let sink = TestSink::new();
        let summary = env.run(&project, &settings, None, &sink).await;
        assert_eq!(
            summary.done.len(),
            1,
            "{preset:?}: {:?}",
            sink.outcome(project.clips[0].id)
        );
        let out = sink.done_path(project.clips[0].id);
        assert!(streams_of(&out, "audio").is_empty());
        assert_eq!(streams_of(&out, "video").len(), 1);
    }
}

/// EXP-05/06: Unicode source path and Unicode output names.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exp06_unicode_source_path_and_output_names() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::UnicodePath);
    assert!(!src.to_string_lossy().is_ascii());
    let project = project_with(
        &src,
        &[(
            0.5,
            2.0,
            "Rekaman: Bahas Patch 2.0 \u{1F525} \u{65E5}\u{672C}",
        )],
    );
    let mut settings = env.settings();
    settings.output_dir = Some(
        env.out_dir
            .join("\u{00F1} \u{51FA}\u{529B}")
            .to_string_lossy()
            .into_owned(),
    );
    settings.naming_template = "{source_name}_{name}".into();
    let sink = TestSink::new();
    let summary = env.run(&project, &settings, None, &sink).await;
    assert_eq!(
        summary.done.len(),
        1,
        "{:?}",
        sink.outcome(project.clips[0].id)
    );
    let out = sink.done_path(project.clips[0].id);
    assert!(out.exists());
    let name = out.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.ends_with(".mp4") || name.ends_with(".mov"), "{name}");
    assert!(
        name.contains('\u{1F525}') && name.contains('\u{65E5}'),
        "{name}"
    );
    assert!(name.starts_with("clip_with_spaces"), "{name}");
}

fn seed_old(env: &Env, name: &str) -> PathBuf {
    std::fs::create_dir_all(&env.out_dir).unwrap();
    let p = env.out_dir.join(name);
    std::fs::write(&p, b"OLD").unwrap();
    p
}

/// EXP-07 / AC-11 (policy side): never overwrite outside the policy.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exp07_conflict_policies_never_overwrite_outside_the_policy() {
    let sc = require_ffmpeg!();
    let src = fixture(Fixture::FrameNumbered);
    for policy in [
        ConflictPolicy::Rename,
        ConflictPolicy::Overwrite,
        ConflictPolicy::Skip,
    ] {
        let env = env(sc.clone());
        let old = seed_old(&env, "01_Clip.mp4");
        let project = project_with(&src, &[(1.0, 2.0, "Clip")]);
        let mut settings = env.settings();
        settings.on_conflict = policy;
        let sink = TestSink::new();
        let summary = env.run(&project, &settings, None, &sink).await;
        let id = project.clips[0].id;
        match policy {
            ConflictPolicy::Rename => {
                assert_eq!(summary.done, vec![id]);
                assert_eq!(
                    std::fs::read(&old).unwrap(),
                    b"OLD",
                    "existing file untouched"
                );
                assert_eq!(sink.done_path(id), env.out_dir.join("01_Clip_2.mp4"));
            }
            ConflictPolicy::Overwrite => {
                assert_eq!(summary.done, vec![id]);
                assert_ne!(
                    std::fs::read(&old).unwrap(),
                    b"OLD",
                    "overwritten by policy"
                );
                assert_eq!(sink.done_path(id), old);
            }
            ConflictPolicy::Skip => {
                assert_eq!(summary.skipped, vec![id]);
                assert!(matches!(
                    sink.outcome(id),
                    Some(ClipOutcome::Skipped { .. })
                ));
                assert_eq!(std::fs::read(&old).unwrap(), b"OLD");
                assert!(
                    sink.pids.lock().unwrap().is_empty(),
                    "no FFmpeg for a skipped clip"
                );
            }
        }
        assert!(env.leftover_parts().is_empty());
    }
}

/// EXP-07: a clip's own previous output is replaced on re-export, whatever the policy.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exp07_previous_output_of_the_same_clip_is_replaced() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::FrameNumbered);
    let old = seed_old(&env, "01_Clip.mp4");
    let mut project = project_with(&src, &[(1.0, 2.0, "Clip")]);
    project.clips[0].last_export = Some(LastExport {
        output_path: Some(old.to_string_lossy().into_owned()),
        exported_at: chrono::Utc::now(),
        settings_hash: String::new(),
        error: None,
        preset: None,
        content_hash: None,
        exit_code: None,
        log_tail: vec![],
        log_path: None,
    });
    let settings = env.settings();
    let report = run_preflight(&project, &settings, None);
    assert_eq!(report.outputs[0].action, PlannedAction::ReplacePrevious);
    let sink = TestSink::new();
    let summary = env.run(&project, &settings, None, &sink).await;
    assert_eq!(summary.done.len(), 1);
    assert_eq!(sink.done_path(project.clips[0].id), old);
    assert_ne!(std::fs::read(&old).unwrap(), b"OLD");
    assert!(!env.out_dir.join("01_Clip_2.mp4").exists());
}

/// EXP-07/08: a file that appears during the export is never overwritten (re-checked before the rename).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exp07_file_appearing_during_export_is_not_overwritten() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::FrameNumbered);
    let project = project_with(&src, &[(1.0, 2.0, "Clip")]);
    let sink = TestSink::new();
    *sink.on_started.lock().unwrap() = Some(Box::new(|planned: &Path| {
        std::fs::write(planned, b"APPEARED").unwrap();
    }));
    let summary = env.run(&project, &env.settings(), None, &sink).await;
    assert_eq!(summary.done.len(), 1);
    assert_eq!(
        std::fs::read(env.out_dir.join("01_Clip.mp4")).unwrap(),
        b"APPEARED"
    );
    assert_eq!(
        sink.done_path(project.clips[0].id),
        env.out_dir.join("01_Clip_2.mp4")
    );
}

/// EXP-13: an unavailable hardware encoder is retried with libx264 and the log says why.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exp13_forced_nvenc_falls_back_to_libx264_and_logs_why() {
    let sc = require_ffmpeg!();
    let hw = vec![HwEncoderInfo {
        name: "h264_nvenc".into(),
        label: "NVIDIA NVENC".into(),
        available: true,
        error: None,
    }];
    let env = env_with(sc, hw);
    let src = fixture(Fixture::FrameNumbered);
    let project = project_with(&src, &[(1.0, 3.0, "hw")]);
    let mut settings = env.settings();
    settings.apply_preset(Preset::Accurate);
    settings.encoder = EncoderChoice::H264Nvenc;
    let sink = TestSink::new();
    let summary = env.run(&project, &settings, None, &sink).await;
    assert_eq!(
        summary.done.len(),
        1,
        "{:?}",
        sink.outcome(project.clips[0].id)
    );
    assert_eq!(
        sink.pids.lock().unwrap().len(),
        2,
        "one failed hardware attempt, one libx264 retry"
    );
    let out = sink.done_path(project.clips[0].id);
    assert_eq!(streams_of(&out, "video")[0]["codec_name"], "h264");
    let log_dir = env.log_dir.join("export").join(&summary.batch_id);
    let log = std::fs::read_dir(&log_dir)
        .unwrap()
        .flatten()
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect::<String>();
    assert!(log.contains("retrying this clip with libx264"), "{log}");
    assert!(log.contains("h264_nvenc"), "{log}");
}

/// EXP-10: a vanished output folder pauses the batch; resume re-creates it and finishes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exp10_vanished_output_folder_pauses_then_resumes() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::FrameNumbered);
    let project = project_with(&src, &[(1.0, 2.0, "a"), (3.0, 4.0, "b")]);
    let sink = TestSink::new();
    let out_dir = env.out_dir.clone();
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    *sink.on_started.lock().unwrap() = Some(Box::new(move |_| {
        if !fired.swap(true, Ordering::SeqCst) {
            std::fs::remove_dir_all(&out_dir).unwrap();
        }
    }));
    env.start(&project, &env.settings(), None, &sink)
        .expect("start");
    sink.wait_until(Duration::from_secs(60), "the pause", |s| {
        s.paused_count() > 0
    })
    .await;
    {
        let states = sink.states.lock().unwrap();
        let paused = states.iter().find_map(|s| match s {
            BatchState::Paused {
                reason, message, ..
            } => Some((*reason, message.clone())),
            _ => None,
        });
        let (reason, message) = paused.unwrap();
        assert_eq!(reason, PauseReason::OutputFolderMissing);
        assert!(message.contains("resume"), "{message}");
    }
    assert!(env.engine.is_running());
    assert_eq!(sink.finished_count(), 0);
    assert!(
        sink.records.lock().unwrap().is_empty(),
        "the clip returned to the queue, no outcome yet"
    );
    env.engine.resume().expect("resume");
    assert!(matches!(env.engine.resume(), Err(ExportError::NotPaused)));
    let summary = sink.wait_finished(Duration::from_secs(120)).await;
    assert_eq!(summary.done.len(), 2, "{:?}", summary);
    assert_eq!(sink.paused_count(), 1);
    assert!(env.out_dir.join("01_a.mp4").exists() && env.out_dir.join("02_b.mp4").exists());
}

/// EXP-09 / PRD 7.8: progress is weighted, monotonic and reaches 1.0; ExportRecords carry hashes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exp09_progress_and_records() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let src = fixture(Fixture::FrameNumbered);
    let project = project_with(&src, &[(0.0, 4.0, "a"), (5.0, 7.0, "b")]);
    let mut settings = env.settings();
    settings.apply_preset(Preset::Accurate);
    let sink = TestSink::new();
    let summary = env.run(&project, &settings, None, &sink).await;
    assert_eq!(summary.done.len(), 2);
    let progress = sink.progress.lock().unwrap();
    assert!(progress.len() >= 3);
    let fractions: Vec<f64> = progress.iter().map(|p| p.overall_fraction).collect();
    assert!(
        fractions.windows(2).all(|w| w[1] + 1e-9 >= w[0]),
        "monotonic: {fractions:?}"
    );
    let last = progress.last().unwrap();
    assert!((last.overall_fraction - 1.0).abs() < 1e-9);
    assert_eq!((last.done, last.failed, last.total), (2, 0, 2));
    assert!(progress.iter().any(|p| !p.running.is_empty()));
    for (rec, fin) in sink.records.lock().unwrap().iter() {
        let clip = project.clip(rec.clip_id).unwrap();
        assert_eq!(
            rec.settings_hash,
            rustcut_model::settings_hash(clip, &settings)
        );
        assert_eq!(rec.content_hash, clip.content_hash(Preset::Accurate));
        assert_eq!(rec.preset, Preset::Accurate);
        assert_eq!(fin.batch_id, summary.batch_id);
        match &rec.outcome {
            ClipOutcome::Done {
                size_bytes,
                output_path,
            } => {
                assert!(*size_bytes > 0);
                assert_eq!(std::fs::metadata(output_path).unwrap().len(), *size_bytes);
            }
            other => panic!("{other:?}"),
        }
    }
    let log_dir = env.log_dir.join("export").join(&summary.batch_id);
    assert_eq!(
        std::fs::read_dir(log_dir).unwrap().count(),
        2,
        "one full log per clip"
    );
}

/// Real-media regression: an MKV carrying ASS subtitles and a font attachment (as fansub
/// releases do) must export every preset with only video + audio streams, and decode cleanly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exp04_mkv_with_ass_subtitles_and_font_attachments_exports_only_av() {
    let sc = require_ffmpeg!();
    let env = env(sc);
    let dir = tempfile::tempdir().unwrap();
    let srt = dir.path().join("s.srt");
    std::fs::write(&srt, "1\n00:00:00,500 --> 00:00:03,000\nhello\n").unwrap();
    let font = dir.path().join("font.ttf");
    std::fs::write(&font, vec![0u8; 2048]).unwrap();
    let src = dir.path().join("[Group] Show - 07.mkv");
    rustcut_testkit::ffmpeg_ok([
        std::ffi::OsString::from("-y"),
        "-i".into(),
        fixture(Fixture::Baseline1080p60).into(),
        "-i".into(),
        srt.clone().into(),
        "-map".into(),
        "0".into(),
        "-map".into(),
        "1".into(),
        "-c".into(),
        "copy".into(),
        "-c:s".into(),
        "ass".into(),
        "-attach".into(),
        font.into(),
        "-metadata:s:t:0".into(),
        "mimetype=application/x-truetype-font".into(),
        src.clone().into(),
    ]);
    assert_eq!(
        streams_of(&src, "subtitle").len(),
        1,
        "fixture has subtitles"
    );
    assert_eq!(
        streams_of(&src, "attachment").len(),
        1,
        "fixture has an attachment"
    );
    let project = project_with(&src, &[(1.0, 3.0, "a"), (5.0, 7.0, "b")]);
    for preset in Preset::ALL {
        let mut settings = env.settings();
        settings.apply_preset(preset);
        let sink = TestSink::new();
        let summary = env.run(&project, &settings, None, &sink).await;
        assert_eq!(
            summary.done.len(),
            2,
            "{preset:?}: {:?}",
            sink.outcome(project.clips[0].id)
        );
        for clip in &project.clips {
            let out = sink.done_path(clip.id);
            assert_eq!(streams_of(&out, "video").len(), 1, "{preset:?}");
            assert_eq!(streams_of(&out, "audio").len(), 1, "{preset:?}");
            for kind in ["subtitle", "attachment", "data"] {
                assert!(
                    streams_of(&out, kind).is_empty(),
                    "{preset:?} leaked {kind}"
                );
            }
        }
    }
}
