//! Persistence: AC-05 round trip, PRJ-01 atomic writes and backups, PRJ-02/03/04
//! location and relink helpers, PRJ-05 lock, PRJ-06 migration / newer schema /
//! corrupt recovery, PRJ-07 recent projects.

mod common;

use common::*;
use rustcut_model::{
    ClipOutcome, CommandError, ErrorCode, ExportStatus, Preset, ProjectLocation, ReadOnlyReason,
    RecentProject, SCHEMA_VERSION, compute_fingerprint, now_utc,
};
use rustcut_project::location::{
    dir_is_writable, find_existing_project, find_relink_candidate, library_file,
    project_file_next_to, relative_source_path, resolve_new_project_location, resolve_source,
};
use rustcut_project::{
    BackupStore, LockOutcome, PersistError, ProjectLock, ProjectStore, RecentStore, load,
    load_with_recovery, migrate, persist, save_atomic,
};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

const V1_SAMPLE: &str = include_str!("fixtures/v1_project.rustcut");
const PRD_SAMPLE: &str = include_str!("../../rustcut-model/tests/prd_sample.rustcut");

fn rich_project() -> rustcut_model::Project {
    let mut s = store(9000.0);
    let a = s
        .add_clip(
            &rustcut_model::AddClipRequest {
                start_sec: 130.5,
                end_sec: 180.0,
                name: Some("Opening Moment".into()),
                tag_id: Some("edu".into()),
            },
            &opts(),
        )
        .unwrap()
        .clip;
    let b = add(&mut s, 920.12, 955.8);
    s.edit_clips(
        &[b.id],
        &rustcut_model::ClipEdit::SetCrop { crop_x_offset: 0.3 },
        &opts(),
    )
    .unwrap();
    s.edit_clips(
        &[b.id],
        &rustcut_model::ClipEdit::Rename {
            name: "Jump Scare Reaction".into(),
        },
        &opts(),
    )
    .unwrap();
    s.mark_exporting(&[a.id]);
    s.apply_export_record(&record(
        &a,
        done("D:/Recordings/clips/01_Opening_Moment.mp4"),
    ))
    .unwrap();
    s.set_view_state(rustcut_model::ViewState {
        playhead_sec: 924.123,
        zoom_window_sec: 37.5,
        selected_clip_id: Some(b.id),
    });
    s.into_project()
}

#[test]
fn ac05_save_and_load_restore_the_project_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stream.rustcut");
    let project = rich_project();
    save_atomic(&path, &project).unwrap();
    let loaded = load(&path).unwrap();
    assert_eq!(
        loaded.project, project,
        "clips, names, tags, statuses, playhead, zoom and selection"
    );
    assert!(loaded.notices.is_empty());
    assert_eq!(
        (
            loaded.read_only,
            loaded.migrated_from,
            loaded.recovered_from
        ),
        (None, None, None)
    );
    assert_eq!(
        loaded.project.clips[0].export_status,
        ExportStatus::Completed
    );

    // The PRD §7.5 sample loads as the current schema.
    fs::write(&path, PRD_SAMPLE).unwrap();
    let sample = load(&path).unwrap();
    assert_eq!(sample.project.schema_version, SCHEMA_VERSION);
    assert_eq!(sample.project.clips.len(), 3);
}

#[test]
fn prj01_atomic_save_replaces_the_file_and_leaves_no_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("p.rustcut");
    let mut project = rich_project();
    save_atomic(&path, &project).unwrap();
    project.clips.clear();
    save_atomic(&path, &project).unwrap();
    assert!(load(&path).unwrap().project.clips.is_empty());
    let names: Vec<String> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["p.rustcut".to_string()]);
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with("{\n  \"schema_version\": 2"),
        "pretty JSON"
    );
}

#[test]
fn prj06_corrupt_file_falls_back_to_the_newest_valid_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stream.rustcut");
    let backups = BackupStore::in_data_dir(&dir.path().join("data"));
    let mut project = rich_project();
    save_atomic(&path, &project).unwrap();
    backups.write(&path, &project, 1, true).unwrap().unwrap();
    project.clips.truncate(1);
    backups.write(&path, &project, 2, true).unwrap().unwrap();
    // A damaged backup that sorts newest is skipped.
    let key = project.source.fingerprint.key();
    fs::write(
        backups
            .root()
            .join(&key)
            .join("99991231T235959.999Z_r0000000003.rustcut"),
        "{ broken",
    )
    .unwrap();

    let full = fs::read(&path).unwrap();
    fs::write(&path, &full[..full.len() / 2]).unwrap();
    let err = load(&path).unwrap_err();
    assert!(matches!(err, PersistError::Corrupt { .. }), "{err}");
    assert_eq!(CommandError::from(err).code, ErrorCode::Io);

    let loaded = load_with_recovery(&path, &backups).unwrap();
    assert_eq!(loaded.project, project);
    assert_eq!(loaded.notices[0].code, "recovered_from_backup");
    assert!(
        loaded
            .recovered_from
            .unwrap()
            .to_string_lossy()
            .contains("_r0000000002")
    );
    assert!(dir.path().join("stream.rustcut.damaged").exists());

    // Without the index the fingerprint in the damaged text still finds the backups.
    fs::remove_file(backups.root().join("index.json")).unwrap();
    assert_eq!(
        load_with_recovery(&path, &backups).unwrap().project,
        project
    );
}

#[test]
fn prj06_unreadable_file_without_backups_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.rustcut");
    let backups = BackupStore::in_data_dir(dir.path());
    fs::write(&path, "this is not a project").unwrap();
    let err = load_with_recovery(&path, &backups).unwrap_err();
    assert!(err.to_string().contains("damaged"), "{err}");
    fs::write(&path, "{\"hello\": 1}").unwrap();
    assert!(
        load(&path)
            .unwrap_err()
            .to_string()
            .contains("no schema_version")
    );
    // Valid JSON with a broken clip but no backup: the readable rest is restored.
    let mut v: Value = serde_json::from_str(PRD_SAMPLE).unwrap();
    v["clips"][0]["start_sec"] = json!("soon");
    fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    let loaded = load_with_recovery(&path, &backups).unwrap();
    assert_eq!(loaded.project.clips.len(), 2);
    assert_eq!(loaded.notices[0].code, "partial_read");
}

#[test]
fn prj06_newer_schema_opens_read_only_and_leniently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.rustcut");
    let mut v: Value = serde_json::from_str(PRD_SAMPLE).unwrap();
    v["schema_version"] = json!(3);
    v["future_block"] = json!({ "anything": [1, 2, 3] });
    v["clips"][2]["future_field"] = json!(true);
    fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    let loaded = load(&path).unwrap();
    let reason = ReadOnlyReason::NewerSchema {
        found: 3,
        supported: SCHEMA_VERSION,
    };
    assert_eq!(loaded.read_only, Some(reason.clone()));
    assert_eq!(loaded.project.clips.len(), 3);
    assert_eq!(loaded.notices[0].code, "read_only_newer_schema");

    v["clips"][1]["end_sec"] = json!({ "unit": "frames", "value": 57348 });
    v["tags"] = json!("palette moved elsewhere");
    fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    let loaded = load(&path).unwrap();
    assert_eq!(loaded.read_only, Some(reason.clone()));
    assert_eq!(
        loaded.project.clips.len(),
        2,
        "the unreadable clip is left out"
    );
    assert!(loaded.notices.iter().any(|n| n.code == "partial_read"));
    let mut s = ProjectStore::new(loaded.project, loaded.read_only);
    assert!(s.add_clip(&req(1.0, 5.0), &opts()).is_err());
    assert!(s.take_save_request().is_none());
}

#[test]
fn prj06_v1_project_is_migrated_and_the_original_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stream_old.rustcut");
    fs::write(&path, V1_SAMPLE).unwrap();
    let loaded = load(&path).unwrap();
    assert_eq!(loaded.migrated_from, Some(1));
    assert_eq!(loaded.notices[0].code, "migrated_schema");
    assert!(
        loaded.notices[0]
            .message
            .contains("stream_old.rustcut.v1.bak")
    );
    let bak = dir.path().join("stream_old.rustcut.v1.bak");
    assert_eq!(fs::read_to_string(&bak).unwrap(), V1_SAMPLE);

    let p = &loaded.project;
    assert_eq!(p.schema_version, 2);
    assert_eq!(p.app_version, "1.2.0");
    let tag_ids: Vec<&str> = p.tags.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(tag_ids, vec!["funny", "edu", "highlight", "color-orange"]);
    assert_eq!(
        (p.tags[3].label.as_str(), p.tags[3].color.as_str()),
        ("Orange", "#F97316")
    );
    let clips: Vec<(u32, &str, Option<&str>, f64)> = p
        .clips
        .iter()
        .map(|c| {
            (
                c.index,
                c.name.as_str(),
                c.tag_id.as_deref(),
                c.crop_x_offset,
            )
        })
        .collect();
    assert_eq!(
        clips,
        vec![
            (1, "Funny bit", Some("funny"), -0.5),
            (2, "Lesson", Some("edu"), 0.25),
            (4, "Big play", Some("color-orange"), 0.25),
            (3, "No colour", None, 0.25),
        ]
    );
    assert_eq!(p.clips[3].export_status, ExportStatus::Failed);
    assert_eq!(p.export_settings.naming_template, "{index}_{name}");
    assert_eq!(p.export_settings.preset(), Preset::Vcrop);
    assert!(
        serde_json::to_value(&p.export_settings)
            .unwrap()
            .get("crop_x_offset")
            .is_none()
    );
    assert_eq!(p.next_index, 5);
    assert_eq!(p.source.video.display_width, 1920);
    assert_eq!(p.view_state.playhead_sec, 105.0);

    // Normalisation on open resets the interrupted export.
    let mut s = ProjectStore::new(loaded.project, None);
    let notices = s.normalize_on_load();
    assert_eq!(notices[0].code, "export_interrupted");
    assert_eq!(s.project().clips[2].export_status, ExportStatus::Pending);

    // Saved as v2, the file no longer migrates; an existing .bak is never overwritten.
    fs::write(&bak, "older original").unwrap();
    fs::write(&path, V1_SAMPLE).unwrap();
    load(&path).unwrap();
    assert_eq!(fs::read_to_string(&bak).unwrap(), "older original");
    save_atomic(&path, s.project()).unwrap();
    assert_eq!(load(&path).unwrap().migrated_from, None);
}

#[test]
fn migrate_v1_value_level_rules() {
    let v1 = json!({
        "project_version": "1.0",
        "source": { "path": "C:\\v\\a.mov", "duration_sec": 10.0 },
        "export_settings": { "naming_template": "{clip_name}-{index}" },
        "clips": [
            { "clip_name": "A", "start_sec": 1.0, "end_sec": 2.0, "color": "#ff0", "export_status": "done" },
            { "name": "B", "start_sec": 3.0, "end_sec": 4.0, "color": "Purple" },
            { "name": "C", "start_sec": 5.0, "end_sec": 6.0, "color": "#22c55e", "crop_x_offset": 0.5 },
            { "name": "broken", "start_sec": "x", "end_sec": 6.0 }
        ]
    });
    let v2 = migrate::v1_to_v2(v1).unwrap();
    assert_eq!(v2["schema_version"], 2);
    assert!(v2.get("project_version").is_none());
    assert_eq!(v2["export_settings"]["naming_template"], "{name}-{index}");
    assert_eq!(v2["clips"][0]["name"], "A");
    assert_eq!(v2["clips"][0]["tag_id"], "color-ffff00");
    assert_eq!(v2["clips"][0]["export_status"], "completed");
    assert_eq!(v2["clips"][1]["tag_id"], "color-purple");
    assert_eq!(v2["clips"][2]["tag_id"], "highlight");
    assert_eq!(v2["clips"][0]["crop_x_offset"], 0.0);
    let tags = v2["tags"].as_array().unwrap();
    assert!(
        tags.iter()
            .any(|t| t["id"] == "color-ffff00" && t["color"] == "#FFFF00")
    );
    assert!(
        tags.iter().any(|t| t["id"] == "color-purple"
            && t["label"] == "Purple"
            && t["color"] == "#A855F7")
    );

    let parsed = persist::parse_project_bytes(serde_json::to_string(&json!({
        "project_version": "1.0",
        "source": { "path": "C:\\v\\a.mov", "duration_sec": 10.0 },
        "clips": [ { "name": "ok", "start_sec": 1.0, "end_sec": 2.0 }, { "name": "broken", "start_sec": "x", "end_sec": 6.0 } ]
    })).unwrap().as_bytes()).unwrap();
    assert_eq!(parsed.project.clips.len(), 1);
    assert_eq!(parsed.project.clips[0].index, 1);
    assert_eq!(parsed.project.source.relative_path, "a.mov");
    assert!(parsed.notices.iter().any(|n| n.code == "partial_read"));
}

#[test]
fn prj01_backups_keep_the_newest_ten_and_are_throttled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.rustcut");
    let backups = BackupStore::in_data_dir(dir.path());
    let project = rich_project();
    let key = project.source.fingerprint.key();
    assert!(backups.write(&path, &project, 1, false).unwrap().is_some());
    assert!(
        backups.write(&path, &project, 2, false).unwrap().is_none(),
        "throttled within 30 s"
    );
    for rev in 3..15 {
        assert!(backups.write(&path, &project, rev, true).unwrap().is_some());
    }
    let list = backups.list(&key);
    assert_eq!(list.len(), 10);
    assert!(
        list[0].to_string_lossy().ends_with("_r0000000014.rustcut"),
        "{:?}",
        list[0]
    );
    assert!(
        list[9].to_string_lossy().ends_with("_r0000000005.rustcut"),
        "{:?}",
        list[9]
    );
    assert_eq!(backups.key_for_project(&path), Some(key.clone()));
    let (newest, parsed) = backups.newest_valid(&key).unwrap();
    assert_eq!(newest, list[0]);
    assert_eq!(parsed.project, project);
}

#[test]
fn prj05_second_lock_is_held_elsewhere_and_released_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.rustcut");
    let LockOutcome::Acquired(lock) = ProjectLock::try_acquire(&path).unwrap() else {
        panic!("first lock")
    };
    assert!(
        lock.holder()
            .starts_with(&format!("{}@", std::process::id()))
    );
    assert!(lock.lock_path().exists());
    match ProjectLock::try_acquire(&path).unwrap() {
        LockOutcome::HeldElsewhere { holder } => assert_eq!(holder.as_deref(), Some(lock.holder())),
        LockOutcome::Acquired(_) => panic!("second lock must be refused"),
    }
    let lock_path = lock.lock_path().to_path_buf();
    drop(lock);
    assert!(!lock_path.exists(), "lock file removed on release");
    let LockOutcome::Acquired(again) = ProjectLock::try_acquire(&path).unwrap() else {
        panic!("free after drop")
    };
    drop(again);

    // A lock file left by a crashed process is not locked: taken over.
    fs::write(ProjectLock::lock_path_for(&path), "999@ghost").unwrap();
    assert!(matches!(
        ProjectLock::try_acquire(&path).unwrap(),
        LockOutcome::Acquired(_)
    ));
}

#[test]
fn prj02_location_next_to_video_or_app_library() {
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("talk.mp4");
    let fp = rustcut_model::Fingerprint {
        size_bytes: 42,
        xxh3_head_tail: "00000000000000ff".into(),
    };
    assert_eq!(
        project_file_next_to(&video),
        dir.path().join("talk.rustcut")
    );

    // talk.rustcut belongs to talk.mkv: talk.mp4 gets talk.mp4.rustcut.
    let mut other = project(10.0);
    other.source.path = dir.path().join("talk.mkv").to_string_lossy().into_owned();
    save_atomic(&dir.path().join("talk.rustcut"), &other).unwrap();
    assert_eq!(
        project_file_next_to(&video),
        dir.path().join("talk.mp4.rustcut")
    );
    assert_eq!(
        project_file_next_to(&dir.path().join("talk.mkv")),
        dir.path().join("talk.rustcut")
    );

    let data = dir.path().join("appdata");
    assert_eq!(
        library_file(&data, &fp),
        data.join("library").join("00000000000000ff-42.rustcut")
    );
    let writable = |_: &Path| true;
    let read_only = |_: &Path| false;
    let checked = std::cell::RefCell::new(Vec::new());
    let spy = |p: &Path| {
        checked.borrow_mut().push(p.to_path_buf());
        true
    };
    assert_eq!(
        resolve_new_project_location(&video, &fp, ProjectLocation::NextToVideo, &data, &spy),
        (
            dir.path().join("talk.mp4.rustcut"),
            ProjectLocation::NextToVideo
        )
    );
    assert_eq!(
        checked.borrow().as_slice(),
        [dir.path().to_path_buf()],
        "the video's folder is checked"
    );
    assert_eq!(
        resolve_new_project_location(&video, &fp, ProjectLocation::NextToVideo, &data, &read_only),
        (library_file(&data, &fp), ProjectLocation::AppLibrary)
    );
    assert_eq!(
        resolve_new_project_location(&video, &fp, ProjectLocation::AppLibrary, &data, &writable),
        (library_file(&data, &fp), ProjectLocation::AppLibrary)
    );
    assert!(dir_is_writable(dir.path()));
    assert!(!dir_is_writable(&dir.path().join("does-not-exist")));
    assert_eq!(
        fs::read_dir(dir.path()).unwrap().count(),
        1,
        "the writability probe is removed"
    );
}

#[test]
fn prj03_find_existing_project_next_to_video_then_library() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("appdata");
    let video = dir.path().join("Stream 01.mp4");
    let mut p = project(10.0);
    p.source.path = video.to_string_lossy().into_owned();
    let fp = p.source.fingerprint.clone();
    assert_eq!(find_existing_project(&video, &fp, &data), None);

    save_atomic(&library_file(&data, &fp), &p).unwrap();
    assert_eq!(
        find_existing_project(&video, &fp, &data),
        Some((library_file(&data, &fp), ProjectLocation::AppLibrary))
    );

    let next_to = dir.path().join("Stream 01.rustcut");
    save_atomic(&next_to, &p).unwrap();
    assert_eq!(
        find_existing_project(&video, &fp, &data),
        Some((next_to.clone(), ProjectLocation::NextToVideo))
    );

    // Renamed video: the stem project still matches by fingerprint.
    let renamed = dir.path().join("Stream 01.mkv");
    assert_eq!(
        find_existing_project(&renamed, &fp, &data).map(|f| f.0),
        Some(next_to)
    );
}

#[test]
fn prj04_relink_candidate_and_source_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let original = dir.path().join("a_original.mp4");
    fs::write(&original, &data).unwrap();
    let fp = compute_fingerprint(&original).unwrap();
    fs::remove_file(&original).unwrap();
    let mut different = data.clone();
    different[150_000] ^= 0xFF;
    different[0] ^= 0xFF;
    fs::write(dir.path().join("b_same_size.mp4"), &different).unwrap();
    assert_eq!(
        find_relink_candidate(dir.path(), &fp),
        None,
        "same size alone is not enough"
    );
    fs::write(dir.path().join("c_moved.mp4"), &data).unwrap();
    assert_eq!(
        find_relink_candidate(dir.path(), &fp),
        Some(dir.path().join("c_moved.mp4"))
    );

    // Relative path is tried first (the whole folder moved).
    let project_path = dir.path().join("proj.rustcut");
    let mut src = source(10.0);
    src.path = "Z:/old/place/c_moved.mp4".into();
    src.relative_path = relative_source_path(&project_path, &dir.path().join("c_moved.mp4"));
    assert_eq!(src.relative_path, "c_moved.mp4");
    assert_eq!(
        resolve_source(&project_path, &src),
        Some(dir.path().join("c_moved.mp4"))
    );
    src.relative_path = "missing.mp4".into();
    assert_eq!(resolve_source(&project_path, &src), None);
}

#[test]
fn prj07_recent_projects_most_recent_first_deduped_max_20() {
    let dir = tempfile::tempdir().unwrap();
    let recent = RecentStore::new(dir.path());
    assert!(recent.list().is_empty());
    let entry = |i: usize| {
        let path = dir.path().join(format!("p{i}.rustcut"));
        fs::write(&path, "{}").unwrap();
        RecentProject {
            project_path: path.to_string_lossy().into_owned(),
            source_path: format!("D:/v/{i}.mp4"),
            source_name: format!("{i}.mp4"),
            opened_at: now_utc() + chrono::Duration::seconds(i as i64),
            clip_count: i as u32,
            location: ProjectLocation::NextToVideo,
        }
    };
    for i in 0..25 {
        recent.add(entry(i)).unwrap();
    }
    let list = recent.list();
    assert_eq!(list.len(), 20);
    assert_eq!(list[0].clip_count, 24);
    assert_eq!(list[19].clip_count, 5);
    let mut again = entry(10);
    again.opened_at = now_utc() + chrono::Duration::seconds(100);
    let list = recent.add(again).unwrap();
    assert_eq!(list.len(), 20);
    assert_eq!(list[0].clip_count, 10);
    assert_eq!(
        list.iter().filter(|e| e.clip_count == 10).count(),
        1,
        "deduplicated by project path"
    );
    fs::remove_file(dir.path().join("p24.rustcut")).unwrap();
    assert!(
        recent.list().iter().all(|e| e.clip_count != 24),
        "missing projects are dropped"
    );
    let list = recent.remove(&list[0].project_path).unwrap();
    assert!(list.iter().all(|e| e.clip_count != 10));
    fs::write(recent.path(), "garbage").unwrap();
    assert!(recent.list().is_empty());
}

#[test]
fn errors_convert_to_command_errors_with_actionable_messages() {
    let io = PersistError::Io {
        op: "write",
        path: "D:/x.rustcut".into(),
        source: std::io::Error::new(std::io::ErrorKind::StorageFull, "disk full"),
    };
    let ce = CommandError::from(io);
    assert_eq!(ce.code, ErrorCode::Io);
    assert!(ce.message.contains("free space"), "{}", ce.message);
    let _ = ClipOutcome::Cancelled;
}

#[test]
fn prj01_temp_files_left_by_killed_writers_never_block_saves_and_are_cleaned() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.rustcut");
    // Leftovers of killed writers, including one with this process's pid
    // (pids are reused on Windows).
    let pid = std::process::id();
    let old = dir.path().join(format!(".p.rustcut.{pid}-0.tmp"));
    let fresh = dir.path().join(".p.rustcut.4242-7.tmp");
    let unrelated = dir.path().join(".other.rustcut.1-0.tmp");
    for f in [&old, &fresh, &unrelated] {
        fs::write(f, "partial").unwrap();
    }
    let two_minutes_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(120);
    for f in [&old, &unrelated] {
        fs::File::options()
            .write(true)
            .open(f)
            .unwrap()
            .set_modified(two_minutes_ago)
            .unwrap();
    }
    let project = rich_project();
    for _ in 0..20 {
        save_atomic(&path, &project).unwrap();
    }
    assert_eq!(load(&path).unwrap().project, project);
    assert!(!old.exists(), "stale temp of this project removed on load");
    assert!(fresh.exists(), "a recent temp may belong to a live writer");
    assert!(unrelated.exists(), "other projects' files are left alone");
}
