//! End-to-end tests of the command layer over the real engine crates and FFmpeg, driven
//! through [`AppCore`] without a WebView. Cite: PRJ-02..06, MRK-01, AC-01, AC-05, AC-06,
//! EXP-07, EXP-09, PRD 7.9.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustcut_lib::core::{AppCore, CoreConfig};
use rustcut_lib::paths::AppPaths;
use rustcut_lib::testing::{NullPlatform, PlatformCall, VecSink};
use rustcut_model::{
    AddClipRequest, AppEvent, BatchState, ClipEdit, CutMode, EditClipsRequest, ErrorCode,
    ExportSettings, ExportStatus, Framing, OpenResult, Project, ReadOnlyReason, RelinkResult,
    SourceStatus, ViewState,
};
use rustcut_runner::Sidecars;
use rustcut_testkit::{Fixture, temp_dir, try_ffmpeg, try_fixture};
use uuid::Uuid;

struct Harness {
    core: AppCore,
    sink: Arc<VecSink>,
    platform: Arc<NullPlatform>,
    root: PathBuf,
}

/// A core with its own data dir, or `None` (with a note) when FFmpeg or the fixtures are missing.
fn harness() -> Option<Harness> {
    let Some(ffmpeg) = try_ffmpeg() else {
        eprintln!("SKIP: FFmpeg not found (set RUSTCUT_FFMPEG_DIR)");
        return None;
    };
    let sidecars = Sidecars::from_dir(ffmpeg.parent()?).ok()?;
    let root = temp_dir();
    let paths = AppPaths::under(&root);
    paths.ensure().ok()?;
    let (sink, platform) = (
        Arc::new(VecSink::default()),
        Arc::new(NullPlatform::default()),
    );
    let core = AppCore::new(CoreConfig {
        paths,
        sidecars: Ok(sidecars),
        sink: sink.clone(),
        platform: platform.clone(),
    });
    Some(Harness {
        core,
        sink,
        platform,
        root,
    })
}

fn copy_fixture(f: Fixture, dir: &Path, name: &str) -> Option<PathBuf> {
    let src = match try_fixture(f) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("SKIP: fixture unavailable: {e}");
            return None;
        }
    };
    std::fs::create_dir_all(dir).ok()?;
    let dst = dir.join(name);
    std::fs::copy(src, &dst).ok()?;
    Some(dst)
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn codes(r: &OpenResult) -> Vec<&str> {
    r.notices.iter().map(|n| n.code.as_str()).collect()
}

fn add(core: &AppCore, start: f64, end: f64, name: &str) -> rustcut_model::AddClipResult {
    core.add_clip(&AddClipRequest {
        start_sec: start,
        end_sec: end,
        name: Some(name.into()),
        tag_id: None,
    })
    .expect("add_clip")
}

fn project_on_disk(path: &Path) -> Option<Project> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

async fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if pred() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    pred()
}

fn finished_batches(sink: &VecSink) -> usize {
    sink.events()
        .iter()
        .filter(|e| {
            matches!(
                e,
                AppEvent::ExportBatch {
                    state: BatchState::Finished { .. }
                }
            )
        })
        .count()
}

async fn wait_batches(h: &Harness, n: usize) {
    let ok = wait_until(Duration::from_secs(120), || finished_batches(&h.sink) >= n).await;
    assert!(ok, "export batch {n} did not finish in time");
}

fn project_path_of(r: &OpenResult) -> PathBuf {
    PathBuf::from(r.snapshot.save.project_path.clone().expect("project path"))
}

/// AC-01: edits yield consecutive patches and reach the disk within about a second;
/// the project file exists as soon as a video is opened (PRJ-02).
#[tokio::test(flavor = "multi_thread")]
async fn new_project_edits_patch_and_autosave() {
    let Some(h) = harness() else { return };
    let Some(video) = copy_fixture(Fixture::Baseline1080p60, &h.root.join("v"), "game.mp4") else {
        return;
    };
    let opened = h.core.open_path(&s(&video)).await.expect("open");
    assert!(codes(&opened).contains(&"created"), "{:?}", codes(&opened));
    assert_eq!(opened.snapshot.revision, 0);
    assert_eq!(opened.snapshot.source_status, SourceStatus::Ok);
    assert!(opened.snapshot.read_only.is_none());
    assert_eq!(
        opened.playback.as_ref().map(|p| p.tier),
        Some(rustcut_model::PlaybackTier::Direct)
    );
    let project_file = project_path_of(&opened);
    assert_eq!(
        std::fs::canonicalize(&project_file).expect("project file"),
        std::fs::canonicalize(video.with_extension("rustcut")).expect("expected project file"),
        "PRJ-02: next to the video"
    );
    assert!(project_file.is_file(), "saved immediately");
    assert!(
        h.platform
            .calls()
            .iter()
            .any(|c| matches!(c, PlatformCall::Allow(p) if p.ends_with("game.mp4"))),
        "asset scope allows the source"
    );

    let a = add(&h.core, 1.0, 3.0, "First");
    assert_eq!(a.patch.revision, 1);
    add(&h.core, 4.0, 6.0, "Second");
    add(&h.core, 7.0, 10.0, "Third");
    let rename = h
        .core
        .edit_clips(&EditClipsRequest {
            ids: vec![a.clip.id],
            edit: ClipEdit::Rename {
                name: "Opening".into(),
            },
        })
        .expect("rename");
    assert_eq!(rename.revision, 4);
    let undone = h.core.undo().expect("undo").expect("a step to undo");
    assert_eq!(undone.revision, 5);
    assert!(undone.history.can_redo);
    let redone = h.core.redo().expect("redo").expect("a step to redo");
    assert_eq!(redone.revision, 6);

    let revisions: Vec<u64> = h
        .sink
        .events()
        .iter()
        .filter_map(|e| {
            if let AppEvent::Patch { patch } = e {
                Some(patch.revision)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        revisions,
        vec![1, 2, 3, 4, 5, 6],
        "one patch event per mutation, each previous + 1"
    );

    let saved = wait_until(Duration::from_millis(1500), || {
        project_on_disk(&project_file)
            .is_some_and(|p| p.clips.len() == 3 && p.clips.iter().any(|c| c.name == "Opening"))
    })
    .await;
    assert!(saved, "AC-01: the edits are on disk within ~1 s");
    assert!(
        h.sink
            .events()
            .iter()
            .any(|e| matches!(e, AppEvent::Saved { .. })),
        "Saved event sent"
    );
    h.core.close_project().await;
}

/// AC-05: clips, names, tags, statuses, playhead and zoom survive close and reopen.
#[tokio::test(flavor = "multi_thread")]
async fn reopen_restores_everything() {
    let Some(h) = harness() else { return };
    let Some(video) = copy_fixture(Fixture::Baseline1080p60, &h.root.join("v"), "match.mp4") else {
        return;
    };
    h.core.open_path(&s(&video)).await.expect("open");
    let a = add(&h.core, 0.5, 2.5, "Opening");
    let b = add(&h.core, 5.0, 8.0, "Jump scare");
    h.core
        .edit_clips(&EditClipsRequest {
            ids: vec![b.clip.id],
            edit: ClipEdit::SetTag {
                tag_id: Some("funny".into()),
            },
        })
        .expect("tag");
    h.core
        .edit_clips(&EditClipsRequest {
            ids: vec![a.clip.id],
            edit: ClipEdit::SetCrop {
                crop_x_offset: -0.4,
            },
        })
        .expect("crop");
    h.core
        .set_view_state(ViewState {
            playhead_sec: 5.25,
            zoom_window_sec: 33.0,
            selected_clip_id: Some(b.clip.id),
        })
        .expect("view");
    let before = h.core.get_snapshot().expect("snapshot");
    h.core.close_project().await;
    assert!(h.core.get_snapshot().is_none());

    let again = h.core.open_path(&s(&video)).await.expect("reopen");
    assert!(codes(&again).contains(&"restored"), "{:?}", codes(&again));
    assert!(!codes(&again).contains(&"created"));
    assert_eq!(
        again.snapshot.project.clips, before.project.clips,
        "clips, names, tags, crop, statuses"
    );
    assert_eq!(
        again.snapshot.project.view_state, before.project.view_state,
        "playhead, zoom, selection"
    );
    assert_eq!(again.snapshot.project.view_state.playhead_sec, 5.25);
    assert_eq!(
        again.snapshot.project.clips[1].tag_id.as_deref(),
        Some("funny")
    );
    assert_eq!(
        h.core.list_recent_projects().len(),
        1,
        "remembered as recent"
    );
    h.core.close_project().await;
}

/// PRJ-05: a second instance opens the same project read-only and cannot edit it.
#[tokio::test(flavor = "multi_thread")]
async fn second_open_is_read_only() {
    let Some(first) = harness() else { return };
    let Some(second) = harness() else { return };
    let Some(video) = copy_fixture(
        Fixture::Baseline1080p60,
        &first.root.join("v"),
        "shared.mp4",
    ) else {
        return;
    };
    first.core.open_path(&s(&video)).await.expect("first open");
    add(&first.core, 1.0, 3.0, "Mine");

    let r = second
        .core
        .open_path(&s(&video))
        .await
        .expect("second open");
    assert!(
        matches!(
            r.snapshot.read_only,
            Some(ReadOnlyReason::OpenElsewhere { .. })
        ),
        "{:?}",
        r.snapshot.read_only
    );
    assert!(codes(&r).contains(&"read_only_locked"));
    let err = second
        .core
        .add_clip(&AddClipRequest {
            start_sec: 1.0,
            end_sec: 2.0,
            name: None,
            tag_id: None,
        })
        .expect_err("read-only");
    assert_eq!(err.code, ErrorCode::ReadOnly);
    second.core.close_project().await;

    // Once the first instance closes, the project can be opened for editing again.
    first.core.close_project().await;
    let third = second
        .core
        .open_path(&s(&video))
        .await
        .expect("open after close");
    assert!(third.snapshot.read_only.is_none());
    assert_eq!(third.snapshot.project.clips.len(), 1);
    second.core.close_project().await;
}

/// AC-06: a moved source shows Missing with a suggestion; relinking restores the project;
/// a different file asks for confirmation with the out-of-range count.
#[tokio::test(flavor = "multi_thread")]
async fn missing_source_relink_and_mismatch() {
    let Some(h) = harness() else { return };
    let dir = h.root.join("v");
    let Some(video) = copy_fixture(Fixture::Baseline1080p60, &dir, "orig.mp4") else {
        return;
    };
    let Some(shorter) = copy_fixture(Fixture::ObsMkv4Audio, &h.root.join("other"), "other.mkv")
    else {
        return;
    };
    let opened = h.core.open_path(&s(&video)).await.expect("open");
    let project_file = project_path_of(&opened);
    add(&h.core, 1.0, 3.0, "Early");
    add(&h.core, 10.0, 11.5, "Late");
    h.core.close_project().await;

    let moved = dir.join("renamed.mp4");
    std::fs::rename(&video, &moved).expect("move the source");
    let r = h
        .core
        .open_path(&s(&project_file))
        .await
        .expect("open the project file");
    let SourceStatus::Missing { suggested_path } = &r.snapshot.source_status else {
        panic!("expected Missing, got {:?}", r.snapshot.source_status);
    };
    let suggested = suggested_path
        .clone()
        .expect("the renamed file in the same folder is suggested");
    assert!(suggested.ends_with("renamed.mp4"), "{suggested}");
    assert_eq!(r.snapshot.read_only, Some(ReadOnlyReason::SourceMissing));
    assert!(codes(&r).contains(&"source_missing"));
    assert!(r.playback.is_none());
    assert_eq!(r.snapshot.project.clips.len(), 2, "the queue is intact");
    let err = h
        .core
        .add_clip(&AddClipRequest {
            start_sec: 1.0,
            end_sec: 2.0,
            name: None,
            tag_id: None,
        })
        .expect_err("read-only while missing");
    assert_eq!(err.code, ErrorCode::ReadOnly);

    // A different file: confirmation required, with the clips that would fall out of range.
    let mismatch = h
        .core
        .relink_source(&s(&shorter), false)
        .await
        .expect("relink call");
    let RelinkResult::Mismatch {
        expected,
        found,
        clips_out_of_range,
        found_duration_sec,
    } = mismatch
    else {
        panic!("expected a mismatch");
    };
    assert_ne!(expected, found);
    assert_eq!(
        clips_out_of_range, 1,
        "the clip at 10 s is beyond an 8 s video"
    );
    assert!(found_duration_sec < 9.0);
    assert!(
        h.core.get_snapshot().expect("snapshot").read_only.is_some(),
        "nothing changed yet"
    );

    // The right file: relinked, editable again, clips preserved, new asset scope, snapshot event.
    let relinked = h
        .core
        .relink_source(&suggested, false)
        .await
        .expect("relink");
    let RelinkResult::Relinked { result } = relinked else {
        panic!("expected Relinked")
    };
    assert!(result.snapshot.read_only.is_none());
    assert_eq!(result.snapshot.source_status, SourceStatus::Ok);
    assert_eq!(result.snapshot.project.clips.len(), 2);
    assert!(result.playback.is_some());
    assert!(
        h.platform
            .calls()
            .iter()
            .any(|c| matches!(c, PlatformCall::Allow(p) if p.ends_with("renamed.mp4")))
    );
    assert!(
        h.sink
            .events()
            .iter()
            .any(|e| matches!(e, AppEvent::Snapshot { .. }))
    );
    add(&h.core, 4.0, 5.0, "Editable again");

    // Accepting a mismatch switches the source and flags the stray clip.
    let accepted = h
        .core
        .relink_source(&s(&shorter), true)
        .await
        .expect("accept");
    let RelinkResult::Relinked { result } = accepted else {
        panic!("expected Relinked")
    };
    assert!(
        result
            .notices
            .iter()
            .any(|n| n.code == "out_of_range_clips")
    );
    assert!(result.notices.iter().any(|n| n.code == "source_changed"));
    assert!(
        h.platform
            .calls()
            .iter()
            .all(|c| !matches!(c, PlatformCall::Forbid(_))),
        "relinking never forbids a path (forbidden patterns are permanent)"
    );
    h.core.close_project().await;
}

/// PRJ-04: a different file at the recorded path opens read-only with a notice.
#[tokio::test(flavor = "multi_thread")]
async fn replaced_source_opens_read_only() {
    let Some(h) = harness() else { return };
    let dir = h.root.join("v");
    let Some(video) = copy_fixture(Fixture::Baseline1080p60, &dir, "take.mp4") else {
        return;
    };
    let Some(other) = try_fixture(Fixture::LongGop10s).ok() else {
        return;
    };
    h.core.open_path(&s(&video)).await.expect("open");
    add(&h.core, 1.0, 3.0, "Keep me");
    h.core.close_project().await;
    std::fs::copy(other, &video).expect("overwrite the video with another recording");

    let r = h.core.open_path(&s(&video)).await.expect("open");
    assert_eq!(r.snapshot.read_only, Some(ReadOnlyReason::SourceMissing));
    assert!(matches!(
        r.snapshot.source_status,
        SourceStatus::Missing { .. }
    ));
    assert!(codes(&r).contains(&"source_changed"), "{:?}", codes(&r));
    assert_eq!(r.snapshot.project.clips.len(), 1);
    h.core.close_project().await;
}

/// A lossless export of 3 clips completes, is recorded in the project, goes stale after an
/// edit (EXP-10) and is replaced (not duplicated) by the next export (EXP-07); reveal is
/// restricted to what the project exported (PRD 7.9).
#[tokio::test(flavor = "multi_thread")]
async fn lossless_export_recorded_outdated_and_replaced() {
    let Some(h) = harness() else { return };
    let Some(video) = copy_fixture(Fixture::Baseline1080p60, &h.root.join("v"), "stream.mp4")
    else {
        return;
    };
    h.core.open_path(&s(&video)).await.expect("open");
    let ids: Vec<Uuid> = [(1.0, 3.0, "One"), (4.0, 6.0, "Two"), (7.0, 10.0, "Three")]
        .iter()
        .map(|(a, b, n)| add(&h.core, *a, *b, n).clip.id)
        .collect();
    let settings = ExportSettings::default();

    let empty = h
        .core
        .start_export(&[], &settings)
        .await
        .expect_err("nothing selected");
    assert_eq!(empty.code, ErrorCode::InvalidInput);
    let report = h
        .core
        .preview_export(&ids, &settings)
        .await
        .expect("preview");
    assert!(report.can_start, "{:?}", report.items);
    assert_eq!(report.outputs.len(), 3);

    let info = h.core.start_export(&ids, &settings).await.expect("start");
    assert_eq!(info.clip_ids.len(), 3);
    wait_batches(&h, 1).await;
    let summary = h
        .sink
        .events()
        .into_iter()
        .find_map(|e| match e {
            AppEvent::ExportBatch {
                state: BatchState::Finished { summary },
            } => Some(summary),
            _ => None,
        })
        .expect("finished");
    assert_eq!(summary.done.len(), 3, "{summary:?}");
    assert!(
        h.platform
            .calls()
            .iter()
            .any(|c| matches!(c, PlatformCall::Notify(..))),
        "EXP-14 notification"
    );

    let snap = h.core.get_snapshot().expect("snapshot");
    let output_dir = PathBuf::from(snap.project.output_dir());
    let mut outputs = Vec::new();
    for clip in &snap.project.clips {
        assert_eq!(
            clip.export_status,
            ExportStatus::Completed,
            "clip {}",
            clip.index
        );
        let out = PathBuf::from(
            clip.last_export
                .as_ref()
                .and_then(|l| l.output_path.clone())
                .expect("recorded output"),
        );
        assert!(out.is_file(), "{} exists", out.display());
        assert!(
            std::fs::metadata(&out)
                .map(|m| m.len() > 0)
                .unwrap_or(false)
        );
        outputs.push(out);
    }
    let listing = |dir: &Path| -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("output dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    let first_listing = listing(&output_dir);
    assert_eq!(first_listing.len(), 3, "{first_listing:?}");

    // Reveal: allowed for exported files and the output folder, nothing else.
    h.core
        .reveal_path(&s(&outputs[0]))
        .expect("reveal an exported file");
    h.core
        .reveal_path(&s(&output_dir))
        .expect("reveal the output folder");
    h.core.open_output_folder().expect("open the output folder");
    let denied = h
        .core
        .reveal_path(&s(&video))
        .expect_err("the source is not an export");
    assert_eq!(denied.code, ErrorCode::InvalidInput);
    let denied = h
        .core
        .reveal_path(&s(&output_dir.join("..").join("stream.rustcut")))
        .expect_err("traversal");
    assert_eq!(denied.code, ErrorCode::InvalidInput);
    let denied = h
        .core
        .reveal_path("C:\\Windows\\System32\\cmd.exe")
        .expect_err("arbitrary path");
    assert_eq!(denied.code, ErrorCode::InvalidInput);
    assert!(
        !h.platform
            .calls()
            .iter()
            .any(|c| matches!(c, PlatformCall::Reveal(p) if p == &video))
    );

    // Editing a clip makes its export stale.
    let patch = h
        .core
        .edit_clips(&EditClipsRequest {
            ids: vec![ids[0]],
            edit: ClipEdit::Retime {
                start_sec: Some(1.5),
                end_sec: None,
            },
        })
        .expect("retime");
    assert_eq!(patch.upserted[0].export_status, ExportStatus::Outdated);

    // Re-export replaces the previous file.
    h.core
        .start_export(&ids, &settings)
        .await
        .expect("re-export");
    wait_batches(&h, 2).await;
    let snap = h.core.get_snapshot().expect("snapshot");
    assert!(
        snap.project
            .clips
            .iter()
            .all(|c| c.export_status == ExportStatus::Completed)
    );
    assert_eq!(
        listing(&output_dir),
        first_listing,
        "EXP-07: no duplicate files, no leftovers"
    );
    h.core.close_project().await;
}

/// EXP-09: cancelling a running batch stops FFmpeg, leaves no partial files and restores the
/// clips' previous status.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_export_leaves_nothing_behind() {
    let Some(h) = harness() else { return };
    let Some(video) = copy_fixture(Fixture::LongGop10s, &h.root.join("v"), "long.mp4") else {
        return;
    };
    h.core.open_path(&s(&video)).await.expect("open");
    let ids: Vec<Uuid> = [(1.0, 12.0), (12.0, 22.0), (18.0, 29.0)]
        .iter()
        .enumerate()
        .map(|(i, (a, b))| add(&h.core, *a, *b, &format!("c{i}")).clip.id)
        .collect();
    let settings = ExportSettings {
        cut_mode: CutMode::Accurate,
        framing: Framing::VerticalBlur,
        ..ExportSettings::default()
    };
    let report = h
        .core
        .preview_export(&ids, &settings)
        .await
        .expect("preview");
    assert!(report.can_start, "{:?}", report.items);
    h.core.start_export(&ids, &settings).await.expect("start");
    let busy = h
        .core
        .start_export(&ids, &settings)
        .await
        .expect_err("a batch is running");
    assert_eq!(busy.code, ErrorCode::Busy);
    h.core.cancel_export(None);
    wait_batches(&h, 1).await;
    assert!(h.core.wait_export_idle(Duration::from_secs(10)).await);

    let summary = h
        .sink
        .events()
        .into_iter()
        .find_map(|e| match e {
            AppEvent::ExportBatch {
                state: BatchState::Finished { summary },
            } => Some(summary),
            _ => None,
        })
        .expect("finished");
    assert!(!summary.cancelled.is_empty(), "{summary:?}");
    let snap = h.core.get_snapshot().expect("snapshot");
    for clip in &snap.project.clips {
        assert_ne!(clip.export_status, ExportStatus::Exporting);
        if summary.cancelled.contains(&clip.id) {
            assert_eq!(clip.export_status, ExportStatus::Pending);
        }
    }
    let out = PathBuf::from(snap.project.output_dir());
    if out.is_dir() {
        let parts: Vec<_> = std::fs::read_dir(&out)
            .expect("dir")
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
            .collect();
        assert!(parts.is_empty(), "no .part files after cancel: {parts:?}");
    }
    h.core.close_project().await;
}

/// Settings round-trip through `settings.json` and are normalized; timestamps export and
/// import go through the store.
#[tokio::test(flavor = "multi_thread")]
async fn settings_timestamps_and_info() {
    let Some(h) = harness() else { return };
    let mut wanted = h.core.settings();
    wanted.min_clip_duration_sec = 99.0;
    wanted.cache_cap_gb = 0;
    let saved = h.core.set_settings(wanted).await.expect("set");
    assert_eq!(
        saved.min_clip_duration_sec, 5.0,
        "clamped to the allowed range"
    );
    assert_eq!(saved.cache_cap_gb, 1);
    assert!(h.root.join("data").join("settings.json").is_file());

    let info = h.core.app_info().await;
    assert!(info.ffmpeg_found && info.ffprobe_found);
    assert!(info.ffmpeg_version.is_some());
    assert_eq!(info.schema_version, rustcut_model::SCHEMA_VERSION);

    let err = h.core.get_media_index().expect_err("no project");
    assert_eq!(err.code, ErrorCode::NoProject);

    let mut relaxed = saved;
    relaxed.min_clip_duration_sec = 0.5;
    h.core.set_settings(relaxed).await.expect("set");
    let Some(video) = copy_fixture(Fixture::Baseline1080p60, &h.root.join("v"), "ts.mp4") else {
        return;
    };
    h.core.open_path(&s(&video)).await.expect("open");
    let preview = h
        .core
        .parse_timestamps("00:01-00:03 Intro\n00:05-00:07.5 Middle\nnonsense")
        .expect("parse");
    assert_eq!(preview.valid_count, 2);
    let patch = h.core.import_timestamps(&preview.items).expect("import");
    assert_eq!(patch.upserted.len(), 2);
    assert!(
        h.core.undo().expect("undo").is_some(),
        "import is one undo step"
    );
    assert!(
        h.core
            .get_snapshot()
            .expect("snapshot")
            .project
            .clips
            .is_empty()
    );
    h.core.close_project().await;
}

/// The frame index arrives as an event and via `get_media_index` (PLY-06); for Matroska only
/// once `start_media_index` asks for the packet scan, which the UI does after the first frame
/// (AC-14). Waveform chunks stream (PLY-05); a Remux request returns a playable cache file.
#[tokio::test(flavor = "multi_thread")]
async fn background_media_jobs() {
    let Some(h) = harness() else { return };
    let Some(video) = copy_fixture(Fixture::ObsMkv4Audio, &h.root.join("v"), "obs.mkv") else {
        return;
    };
    let opened = h.core.open_path(&s(&video)).await.expect("open");
    assert!(opened.playback.is_some());
    let early = h
        .sink
        .wait_for(Duration::from_secs(2), |e| {
            matches!(e, AppEvent::MediaIndex { .. })
        })
        .await;
    assert!(early.is_none(), "no packet scan during the open");
    assert!(h.core.get_media_index().expect("index").is_none());
    h.core.start_media_index().expect("start the packet scan");
    h.core
        .start_media_index()
        .expect("asking again while it runs is harmless");
    let got = h
        .sink
        .wait_for(Duration::from_secs(30), |e| {
            matches!(e, AppEvent::MediaIndex { .. })
        })
        .await;
    assert!(got.is_some(), "media index event");
    assert!(h.core.get_media_index().expect("index").is_some());

    assert!(h.core.start_waveform(9).is_err(), "track 9 does not exist");
    h.core.start_waveform(1).expect("waveform");
    let done = h
        .sink
        .wait_for(
            Duration::from_secs(60),
            |e| matches!(e, AppEvent::Waveform { chunk } if chunk.done),
        )
        .await;
    assert!(done.is_some(), "waveform finished");

    let info = h
        .core
        .request_playback(rustcut_model::PlaybackTier::Remux)
        .await
        .expect("remux");
    assert!(!info.preparing);
    assert!(
        info.media_path
            .as_deref()
            .is_some_and(|p| Path::new(p).is_file()),
        "{info:?}"
    );
    h.core.release_playback();
    h.core.close_project().await;
}

/// AC-14 / §7.3: an MP4's index comes from its sample tables during the open, without
/// `start_media_index`, because the Player reads its first keyframe at the first frame.
#[tokio::test(flavor = "multi_thread")]
async fn mp4_index_is_built_by_the_open() {
    let Some(h) = harness() else { return };
    let Some(video) = copy_fixture(Fixture::Baseline1080p60, &h.root.join("v"), "index.mp4") else {
        return;
    };
    h.core.open_path(&s(&video)).await.expect("open");
    let got = h
        .sink
        .wait_for(Duration::from_secs(30), |e| {
            matches!(e, AppEvent::MediaIndex { .. })
        })
        .await;
    assert!(got.is_some(), "sample-table index announced by the open");
    assert!(h.core.get_media_index().expect("index").is_some());
    h.core
        .start_media_index()
        .expect("nothing left to build: a no-op");
    h.core.close_project().await;
}

/// PRJ-05 / PRD 9: a folder that cannot take the lock file opens read-only instead of
/// failing, and the previously open project stays usable when the new open succeeds.
#[tokio::test(flavor = "multi_thread")]
async fn unwritable_lock_opens_read_only() {
    let Some(h) = harness() else { return };
    let Some(video) = copy_fixture(Fixture::Baseline1080p60, &h.root.join("v"), "locked.mp4")
    else {
        return;
    };
    h.core.open_path(&s(&video)).await.expect("first open");
    add(&h.core, 1.0, 3.0, "Keep");
    h.core.close_project().await;
    let project = std::fs::read_dir(h.root.join("v"))
        .expect("dir")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "rustcut"))
        .expect("project file");
    // A directory where the lock file must go makes lock creation fail.
    let lock = rustcut_project::ProjectLock::lock_path_for(&project);
    let _ = std::fs::remove_file(&lock);
    std::fs::create_dir_all(&lock).expect("block the lock path");

    let r = h
        .core
        .open_path(&s(&video))
        .await
        .expect("a lock failure must not fail the open");
    assert!(matches!(
        r.snapshot.read_only,
        Some(ReadOnlyReason::Unwritable { .. })
    ));
    assert!(
        codes(&r).contains(&"read_only_unwritable"),
        "{:?}",
        codes(&r)
    );
    assert_eq!(r.snapshot.project.clips.len(), 1);
    let err = h
        .core
        .add_clip(&AddClipRequest {
            start_sec: 5.0,
            end_sec: 6.0,
            name: None,
            tag_id: None,
        })
        .expect_err("read-only");
    assert_eq!(err.code, ErrorCode::ReadOnly);
    h.core.close_project().await;
}

/// PRJ-05: reopen through either entry point flushes edits and reacquires our own lock.
#[tokio::test(flavor = "multi_thread")]
async fn reopen_current_project_by_video_and_project_file_stays_writable() {
    let Some(h) = harness() else { return };
    let Some(video) = copy_fixture(Fixture::Baseline1080p60, &h.root.join("v"), "reopen.mp4")
    else {
        return;
    };
    let opened = h.core.open_path(&s(&video)).await.expect("open");
    let project_path = opened.snapshot.save.project_path.expect("project path");
    let a = add(&h.core, 0.5, 2.0, "Before reopen");
    let reopened = h.core.open_path(&s(&video)).await.expect("reopen video");
    assert!(reopened.snapshot.read_only.is_none());
    assert_eq!(reopened.snapshot.project.clips[0].id, a.clip.id);
    let b = add(&h.core, 2.5, 4.0, "Before project reopen");
    let reopened = h
        .core
        .open_path(&project_path)
        .await
        .expect("reopen project file");
    assert!(reopened.snapshot.read_only.is_none());
    assert_eq!(reopened.snapshot.project.clips[1].id, b.clip.id);
    assert!(matches!(
        rustcut_project::ProjectLock::try_acquire(Path::new(&project_path)).expect("try lock"),
        rustcut_project::LockOutcome::HeldElsewhere { .. }
    ));
    h.core.close_project().await;
}

/// QUE-04: the app command undoes both halves of a selection action together.
#[tokio::test(flavor = "multi_thread")]
async fn batch_edit_is_atomic_through_the_core() {
    let Some(h) = harness() else { return };
    let Some(video) = copy_fixture(Fixture::Baseline1080p60, &h.root.join("v"), "batch.mp4") else {
        return;
    };
    h.core.open_path(&s(&video)).await.expect("open");
    let a = add(&h.core, 0.5, 2.0, "A").clip;
    let b = add(&h.core, 2.5, 4.0, "B").clip;
    h.core
        .edit_clips(&EditClipsRequest {
            ids: vec![b.id],
            edit: ClipEdit::SetIncluded { included: false },
        })
        .expect("exclude B");
    let before = h.core.get_snapshot().expect("snapshot");
    let requests = vec![
        EditClipsRequest {
            ids: vec![a.id],
            edit: ClipEdit::SetIncluded { included: false },
        },
        EditClipsRequest {
            ids: vec![b.id],
            edit: ClipEdit::SetIncluded { included: true },
        },
    ];
    let patch = h.core.edit_clips_batch(&requests).expect("batch");
    assert_eq!(patch.revision, before.revision + 1);
    h.core.undo().expect("undo");
    assert_eq!(
        h.core.get_snapshot().expect("snapshot").project.clips,
        before.project.clips
    );
    let mut invalid = requests;
    invalid[1].ids = vec![Uuid::nil()];
    let revision = h.core.get_snapshot().expect("snapshot").revision;
    assert_eq!(
        h.core
            .edit_clips_batch(&invalid)
            .expect_err("invalid batch")
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(h.core.get_snapshot().expect("snapshot").revision, revision);
    h.core.close_project().await;
}

/// PRD 7.9: a relative output folder from the WebView is refused.
#[tokio::test(flavor = "multi_thread")]
async fn relative_output_dir_is_rejected() {
    let Some(h) = harness() else { return };
    let settings = ExportSettings {
        output_dir: Some("-rf/out".into()),
        ..ExportSettings::default()
    };
    let err = h
        .core
        .set_export_settings(settings.clone())
        .expect_err("relative");
    assert_eq!(err.code, ErrorCode::InvalidInput);
    let err = h
        .core
        .start_export(&[], &settings)
        .await
        .expect_err("relative");
    assert_eq!(err.code, ErrorCode::InvalidInput);
}

/// Shutdown flushes and releases the project lock; a second call is harmless.
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_flushes_and_is_repeatable() {
    let Some(h) = harness() else { return };
    let Some(video) = copy_fixture(Fixture::Baseline1080p60, &h.root.join("v"), "sd.mp4") else {
        return;
    };
    h.core.open_path(&s(&video)).await.expect("open");
    add(&h.core, 1.0, 3.0, "Last");
    assert!(h.core.try_shutdown().await.is_ok());
    h.core.shutdown().await;
    let r = h.core.open_path(&s(&video)).await.expect("reopen");
    assert!(r.snapshot.read_only.is_none());
    assert_eq!(r.snapshot.project.clips.len(), 1);
    h.core.close_project().await;
}

/// PRJ-05: when two instances race to open the same project, exactly one gets the lock
/// (taken before the project is loaded) and the other opens read-only.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_open_gives_exactly_one_writer() {
    let Some(a) = harness() else { return };
    let Some(b) = harness() else { return };
    let Some(video) = copy_fixture(Fixture::Baseline1080p60, &a.root.join("v"), "race.mp4") else {
        return;
    };
    let video_path = s(&video);
    let (ra, rb) = tokio::join!(a.core.open_path(&video_path), b.core.open_path(&video_path));
    let (ra, rb) = (ra.expect("a opens"), rb.expect("b opens"));
    let writers = [&ra, &rb]
        .iter()
        .filter(|r| r.snapshot.read_only.is_none())
        .count();
    assert_eq!(
        writers, 1,
        "{:?} / {:?}",
        ra.snapshot.read_only, rb.snapshot.read_only
    );
    let readonly = if ra.snapshot.read_only.is_some() {
        &ra
    } else {
        &rb
    };
    assert!(matches!(
        readonly.snapshot.read_only,
        Some(ReadOnlyReason::OpenElsewhere { .. })
    ));
    a.core.close_project().await;
    b.core.close_project().await;
}

/// A failed open leaves the previously open session intact, `get_snapshot` unchanged and
/// the lock still held, both for another file and for re-opening the same project.
#[tokio::test(flavor = "multi_thread")]
async fn failed_open_keeps_the_open_session() {
    let Some(h) = harness() else { return };
    let Some(other) = harness() else { return };
    let dir = h.root.join("v");
    let Some(video) = copy_fixture(Fixture::Baseline1080p60, &dir, "keep.mp4") else {
        return;
    };
    let opened = h.core.open_path(&s(&video)).await.expect("open");
    add(&h.core, 1.0, 3.0, "Keep");
    h.core.save_now().await.expect("save before snapshot");
    let project = project_path_of(&opened);
    let before = serde_json::to_string(&h.core.get_snapshot().expect("snapshot")).expect("json");

    // 1. A file that is not a video.
    let bogus = dir.join("bogus.mp4");
    std::fs::write(&bogus, b"not a video").expect("write");
    h.core.open_path(&s(&bogus)).await.expect_err("bogus video");
    let after = serde_json::to_string(&h.core.get_snapshot().expect("kept")).expect("json");
    assert_eq!(before, after);

    // 2. Reopening the same project whose source became unreadable retains our own
    // lock through the failed attempt.
    h.core.save_now().await.expect("save");
    let backup = dir.join("keep.bak");
    std::fs::rename(&video, &backup).expect("move");
    std::fs::write(&video, b"garbage").expect("replace");
    h.core
        .open_path(&s(&project))
        .await
        .expect_err("unreadable source");
    let after = serde_json::to_string(&h.core.get_snapshot().expect("kept")).expect("json");
    assert_eq!(before, after);
    add(&h.core, 5.0, 6.0, "Still editable");
    let r = other
        .core
        .open_path(&s(&backup.with_file_name("keep.mp4")))
        .await;
    // The other instance still sees the project as locked (or cannot probe the garbage).
    if let Ok(r) = r {
        assert!(matches!(
            r.snapshot.read_only,
            Some(ReadOnlyReason::OpenElsewhere { .. })
        ));
    }
    h.core.close_project().await;
}

/// `get_platform_presets`: bundled list by default; an invalid user file falls back to it
/// and raises a toast.
#[tokio::test(flavor = "multi_thread")]
async fn platform_presets_bundled_and_invalid_file_toast() {
    let Some(h) = harness() else { return };
    let presets = h.core.platform_presets();
    assert!(
        presets
            .iter()
            .any(|p| p.id.contains("shorts") || p.label.contains("Shorts"))
    );
    assert!(
        h.sink
            .events()
            .iter()
            .all(|e| !matches!(e, AppEvent::Toast { .. }))
    );
    let data = AppPaths::under(&h.root).data;
    std::fs::write(data.join(rustcut_export::PLATFORM_PRESETS_FILE), b"{ nope").expect("write");
    let again = h.core.platform_presets();
    assert_eq!(again.len(), presets.len());
    assert!(h.sink.events().iter().any(
        |e| matches!(e, AppEvent::Toast { notice } if notice.code == "platform_presets_invalid")
    ));
}
