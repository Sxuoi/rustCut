//! ProjectStore rules: MRK-01/03/04/06, QUE-02/04/05, PRJ-03/04 store side,
//! patch semantics, read-only mode.

mod common;

use chrono::TimeZone;
use common::*;
use rustcut_model::{
    AddClipRequest, ClipEdit, ClipOutcome, CommandError, ErrorCode, ExportSettings, ExportStatus,
    ImportedClip, Preset, ReadOnlyReason, SaveState, SourceStatus, Tag, UNDO_LIMIT, ViewState,
    default_tags,
};
use rustcut_project::{ProjectStore, StoreError, StoreOptions, Urgency};
use uuid::Uuid;

fn code(e: StoreError) -> ErrorCode {
    CommandError::from(e).code
}

#[test]
fn mrk01_add_clamps_to_source_and_rounds_to_ms() {
    let mut s = store(100.0);
    let a = add(&mut s, -5.0, 10.12345);
    assert_eq!((a.start_sec, a.end_sec), (0.0, 10.123));
    let b = add(&mut s, 90.0004, 150.0);
    assert_eq!((b.start_sec, b.end_sec), (90.0, 100.0));
    let c = s
        .add_clip(
            &AddClipRequest {
                start_sec: 1.0,
                end_sec: 3.0,
                name: Some("  Jump  ".into()),
                tag_id: Some("funny".into()),
            },
            &opts(),
        )
        .unwrap()
        .clip;
    assert_eq!(c.name, "Jump");
    assert_eq!(c.tag_id.as_deref(), Some("funny"));
    assert!(c.selected_for_export);
    assert_eq!(c.export_status, ExportStatus::Pending);
}

#[test]
fn mrk01_minimum_duration_with_setting_and_message() {
    let mut s = store(100.0);
    let e = s.add_clip(&req(10.0, 10.4), &opts()).unwrap_err();
    assert_eq!(e, StoreError::ClipTooShort { min: 0.5 });
    assert!(e.to_string().contains("at least 0.5 s"), "{e}");
    assert_eq!(code(e), ErrorCode::ClipTooShort);
    assert!(
        s.add_clip(&req(10.0, 10.5), &opts()).is_ok(),
        "exactly the minimum is valid"
    );

    let two = StoreOptions {
        min_clip_duration_sec: 2.0,
    };
    let e = s.add_clip(&req(20.0, 21.5), &two).unwrap_err();
    assert!(e.to_string().contains("at least 2 s"), "{e}");
    assert!(s.add_clip(&req(20.0, 22.0), &two).is_ok());
    assert_eq!(
        StoreOptions {
            min_clip_duration_sec: 60.0
        }
        .min_duration(),
        5.0
    );
    assert_eq!(
        StoreOptions {
            min_clip_duration_sec: 0.01
        }
        .min_duration(),
        0.1
    );
    assert_eq!(
        StoreOptions {
            min_clip_duration_sec: f64::NAN
        }
        .min_duration(),
        0.5
    );
}

#[test]
fn ac03_no_clip_with_out_not_after_in_or_in_past_the_end() {
    let mut s = store(100.0);
    for (start, end) in [
        (20.0, 10.0),
        (10.0, 10.0),
        (100.0, 120.0),
        (150.0, 160.0),
        (99.9996, 100.0),
        (f64::NAN, 1.0),
        (1.0, f64::INFINITY),
        (-10.0, -2.0),
        (1e300, 2e300),
    ] {
        let e = s.add_clip(&req(start, end), &opts()).unwrap_err();
        assert_eq!(
            code(e.clone()),
            ErrorCode::InvalidRange,
            "{start}..{end}: {e}"
        );
    }
    assert!(s.project().clips.is_empty());
    assert_eq!(s.revision(), 0);
    let e = s.add_clip(&req(120.0, 130.0), &opts()).unwrap_err();
    assert!(e.to_string().contains("past the end"), "{e}");
}

#[test]
fn ac01_add_returns_patch_with_clip_and_next_revision() {
    let mut s = store(100.0);
    let r = s.add_clip(&req(1.0, 2.0), &opts()).unwrap();
    assert_eq!(r.patch.revision, 1);
    assert_eq!(r.patch.upserted, vec![r.clip.clone()]);
    assert!(r.patch.removed.is_empty());
    assert_eq!(r.patch.next_index, 2);
    assert_eq!(r.patch.label, None);
    assert!(r.patch.history.can_undo);
    assert_eq!(r.patch.history.undo_label.as_deref(), Some("Undo Add clip"));
    let save = s.take_save_request().unwrap();
    assert_eq!(save.urgency, Urgency::Immediate);
    assert_eq!(save.revision, 1);
    assert!(!s.is_dirty());
}

#[test]
fn mrk03_identical_ranges_are_allowed_and_flagged() {
    let mut s = store(100.0);
    assert_eq!(
        s.add_clip(&req(10.0, 20.0), &opts()).unwrap().duplicate_of,
        None
    );
    assert_eq!(
        s.add_clip(&req(10.0, 20.0), &opts()).unwrap().duplicate_of,
        Some(1)
    );
    assert_eq!(
        s.add_clip(&req(15.0, 20.0), &opts()).unwrap().duplicate_of,
        None,
        "overlap is not a duplicate"
    );
    assert_eq!(s.project().clips.len(), 3);
}

#[test]
fn que02_numbers_never_reused_and_next_index_monotonic_across_undo() {
    let mut s = store(100.0);
    let a = add(&mut s, 1.0, 2.0);
    let b = add(&mut s, 3.0, 4.0);
    let c = add(&mut s, 5.0, 6.0);
    assert_eq!((a.index, b.index, c.index), (1, 2, 3));
    s.delete_clips(&[c.id]).unwrap();
    assert_eq!(add(&mut s, 7.0, 8.0).index, 4);
    s.undo().unwrap(); // removes clip 4
    assert_eq!(s.project().next_index, 5, "undo never lowers next_index");
    s.undo().unwrap(); // restores clip 3
    assert_eq!(s.project().next_index, 5);
    assert_eq!(add(&mut s, 9.0, 10.0).index, 5);
    s.undo().unwrap();
    s.undo().unwrap();
    s.undo().unwrap();
    assert_eq!(s.project().next_index, 6);
    let indices: Vec<u32> = s.project().clips.iter().map(|c| c.index).collect();
    assert_eq!(indices, vec![1]);
    assert_eq!(add(&mut s, 11.0, 12.0).index, 6);
}

#[test]
fn every_patch_increments_revision_by_one_and_sets_updated_at() {
    let mut p = project(100.0);
    p.updated_at = chrono::Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
    let mut s = ProjectStore::new(p, None);
    let a = add(&mut s, 1.0, 2.0);
    assert!(s.project().updated_at.timestamp() > 1_600_000_000);
    add(&mut s, 0.1, 0.9); // makes the renumber below a real change
    let revisions = vec![
        s.revision(),
        s.edit_clips(&[a.id], &ClipEdit::Rename { name: "x".into() }, &opts())
            .unwrap()
            .revision,
        s.undo().unwrap().revision,
        s.redo().unwrap().revision,
        s.renumber_chronologically().unwrap().revision,
        s.mark_exporting(&[a.id]).revision,
        s.apply_export_record(&record(&a, ClipOutcome::Cancelled))
            .unwrap()
            .revision,
        s.delete_clips(&[a.id]).unwrap().revision,
    ];
    assert_eq!(revisions, (2..=9).collect::<Vec<u64>>());
}

#[test]
fn noop_commands_create_no_history_and_keep_revision() {
    let mut s = store(100.0);
    let a = s
        .add_clip(
            &AddClipRequest {
                start_sec: 1.0,
                end_sec: 2.0,
                name: Some("Name".into()),
                tag_id: None,
            },
            &opts(),
        )
        .unwrap()
        .clip;
    let _ = s.take_save_request();
    let rev = s.revision();
    let depth = s.undo_depth();
    let patches = vec![
        s.edit_clips(
            &[a.id],
            &ClipEdit::Rename {
                name: "  Name ".into(),
            },
            &opts(),
        )
        .unwrap(),
        s.edit_clips(
            &[a.id],
            &ClipEdit::Retime {
                start_sec: Some(1.0),
                end_sec: None,
            },
            &opts(),
        )
        .unwrap(),
        s.edit_clips(
            &[a.id],
            &ClipEdit::Retime {
                start_sec: None,
                end_sec: None,
            },
            &opts(),
        )
        .unwrap(),
        s.edit_clips(&[a.id], &ClipEdit::SetIncluded { included: true }, &opts())
            .unwrap(),
        s.edit_clips(&[], &ClipEdit::SetCrop { crop_x_offset: 0.5 }, &opts())
            .unwrap(),
        s.delete_clips(&[]).unwrap(),
        s.renumber_chronologically().unwrap(),
        s.set_tags(default_tags()).unwrap(),
        s.set_export_settings(ExportSettings::default()).unwrap(),
        s.mark_exporting(&[Uuid::now_v7()]),
    ];
    for p in patches {
        assert_eq!(p.revision, rev);
        assert!(
            p.upserted.is_empty()
                && p.removed.is_empty()
                && p.tags.is_none()
                && p.export_settings.is_none()
        );
    }
    assert_eq!(s.undo_depth(), depth);
    assert!(!s.is_dirty());
}

#[test]
fn mrk04_bulk_edit_and_bulk_delete_are_one_undo_step() {
    let mut s = store(100.0);
    let ids: Vec<Uuid> = (0..4)
        .map(|i| add(&mut s, f64::from(i) * 10.0, f64::from(i) * 10.0 + 5.0).id)
        .collect();
    let before = s.project().clips.clone();
    let p = s
        .edit_clips(
            &ids[..3],
            &ClipEdit::SetTag {
                tag_id: Some("funny".into()),
            },
            &opts(),
        )
        .unwrap();
    assert_eq!(p.upserted.len(), 3);
    assert_eq!(
        p.history.undo_label.as_deref(),
        Some("Undo Change tag (3 clips)")
    );
    let undo = s.undo().unwrap();
    assert_eq!(undo.label.as_deref(), Some("Undo Change tag (3 clips)"));
    assert_eq!(s.project().clips, before);

    let p = s.delete_clips(&[ids[0], ids[2], ids[0]]).unwrap();
    assert_eq!(p.removed, vec![ids[0], ids[2]]);
    assert_eq!(s.project().clips.len(), 2);
    let undo = s.undo().unwrap();
    assert_eq!(undo.label.as_deref(), Some("Undo Delete 2 clips"));
    assert_eq!(s.project().clips, before, "order restored exactly");
    let redo = s.redo().unwrap();
    assert_eq!(redo.label.as_deref(), Some("Redo Delete 2 clips"));
    assert_eq!(redo.removed, vec![ids[0], ids[2]]);
}

#[test]
fn rename_trims_crop_clamps_and_retime_validates() {
    let mut s = store(100.0);
    let a = add(&mut s, 10.0, 20.0);
    let p = s
        .edit_clips(
            &[a.id],
            &ClipEdit::Rename {
                name: "\t Big play \n".into(),
            },
            &opts(),
        )
        .unwrap();
    assert_eq!(p.upserted[0].name, "Big play");
    let p = s
        .edit_clips(&[a.id], &ClipEdit::SetCrop { crop_x_offset: 7.0 }, &opts())
        .unwrap();
    assert_eq!(p.upserted[0].crop_x_offset, 1.0);
    let p = s
        .edit_clips(
            &[a.id],
            &ClipEdit::SetCrop {
                crop_x_offset: -0.123456,
            },
            &opts(),
        )
        .unwrap();
    assert_eq!(p.upserted[0].crop_x_offset, -0.1235);
    assert_eq!(
        code(
            s.edit_clips(
                &[a.id],
                &ClipEdit::SetCrop {
                    crop_x_offset: f64::NAN
                },
                &opts()
            )
            .unwrap_err()
        ),
        ErrorCode::InvalidInput
    );

    let p = s
        .edit_clips(
            &[a.id],
            &ClipEdit::Retime {
                start_sec: Some(12.3456),
                end_sec: None,
            },
            &opts(),
        )
        .unwrap();
    assert_eq!(
        (p.upserted[0].start_sec, p.upserted[0].end_sec),
        (12.346, 20.0)
    );
    let p = s
        .edit_clips(
            &[a.id],
            &ClipEdit::Retime {
                start_sec: None,
                end_sec: Some(500.0),
            },
            &opts(),
        )
        .unwrap();
    assert_eq!(p.upserted[0].end_sec, 100.0, "Out clamps to the source end");
    let rev = s.revision();
    let e = s
        .edit_clips(
            &[a.id],
            &ClipEdit::Retime {
                start_sec: Some(99.8),
                end_sec: None,
            },
            &opts(),
        )
        .unwrap_err();
    assert_eq!(e, StoreError::ClipTooShort { min: 0.5 });
    let e = s
        .edit_clips(
            &[a.id],
            &ClipEdit::Retime {
                start_sec: None,
                end_sec: Some(5.0),
            },
            &opts(),
        )
        .unwrap_err();
    assert_eq!(code(e), ErrorCode::InvalidRange);
    assert_eq!(s.revision(), rev, "failed edits change nothing");
}

#[test]
fn unknown_clip_or_tag_is_rejected_without_changes() {
    let mut s = store(100.0);
    let a = add(&mut s, 10.0, 20.0);
    let rev = s.revision();
    let ghost = Uuid::now_v7();
    assert_eq!(
        code(
            s.edit_clips(
                &[a.id, ghost],
                &ClipEdit::Rename { name: "x".into() },
                &opts()
            )
            .unwrap_err()
        ),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(s.delete_clips(&[a.id, ghost]).unwrap_err()),
        ErrorCode::NotFound
    );
    let e = s
        .edit_clips(
            &[a.id],
            &ClipEdit::SetTag {
                tag_id: Some("nope".into()),
            },
            &opts(),
        )
        .unwrap_err();
    assert_eq!(e, StoreError::UnknownTag("nope".into()));
    assert_eq!(code(e), ErrorCode::InvalidInput);
    let e = s.add_clip(
        &AddClipRequest {
            start_sec: 1.0,
            end_sec: 2.0,
            name: None,
            tag_id: Some("nope".into()),
        },
        &opts(),
    );
    assert!(matches!(e, Err(StoreError::UnknownTag(_))));
    assert_eq!(s.revision(), rev);
    assert_eq!(s.project().clips, vec![a]);
}

#[test]
fn read_only_blocks_every_user_mutation() {
    let reasons = [
        ReadOnlyReason::OpenElsewhere {
            holder: Some("4242@studio-pc".into()),
        },
        ReadOnlyReason::NewerSchema {
            found: 3,
            supported: 2,
        },
        ReadOnlyReason::SourceMissing,
    ];
    for reason in reasons {
        let mut p = project(100.0);
        let existing = clip(1, 1.0, 2.0);
        p.clips.push(existing.clone());
        p.next_index = 2;
        let mut s = ProjectStore::new(p.clone(), Some(reason.clone()));
        let id = existing.id;
        let errors = vec![
            s.add_clip(&req(10.0, 20.0), &opts())
                .map(|_| ())
                .unwrap_err(),
            s.edit_clips(&[id], &ClipEdit::Rename { name: "x".into() }, &opts())
                .map(|_| ())
                .unwrap_err(),
            s.delete_clips(&[id]).map(|_| ()).unwrap_err(),
            s.renumber_chronologically().map(|_| ()).unwrap_err(),
            s.set_tags(Vec::new()).map(|_| ()).unwrap_err(),
            s.set_export_settings(ExportSettings {
                parallel_jobs: 3,
                ..ExportSettings::default()
            })
            .map(|_| ())
            .unwrap_err(),
            s.import_clips(&[], &opts()).map(|_| ()).unwrap_err(),
        ];
        for e in errors {
            assert_eq!(
                e,
                StoreError::ReadOnly {
                    reason: reason.clone()
                }
            );
            let ce = CommandError::from(e);
            assert_eq!(ce.code, ErrorCode::ReadOnly);
            assert!(ce.message.contains("read-only"), "{}", ce.message);
        }
        assert!(s.undo().is_none() && s.redo().is_none());
        assert!(!s.history().can_undo && !s.history().can_redo);
        s.set_view_state(ViewState {
            playhead_sec: 5.0,
            ..ViewState::default()
        });
        assert!(
            s.take_save_request().is_none(),
            "read-only projects are never saved"
        );
        assert_eq!(s.project().clips, p.clips);
        assert_eq!(s.revision(), 0);
    }
    let held = StoreError::ReadOnly {
        reason: ReadOnlyReason::OpenElsewhere {
            holder: Some("4242@studio-pc".into()),
        },
    };
    assert!(held.to_string().contains("4242@studio-pc"));
}

#[test]
fn mrk06_set_tags_validates_and_removed_tag_is_cleared_in_one_undo_step() {
    let mut s = store(100.0);
    let a = s
        .add_clip(
            &AddClipRequest {
                start_sec: 1.0,
                end_sec: 2.0,
                name: None,
                tag_id: Some("funny".into()),
            },
            &opts(),
        )
        .unwrap()
        .clip;
    let b = s
        .add_clip(
            &AddClipRequest {
                start_sec: 3.0,
                end_sec: 4.0,
                name: None,
                tag_id: Some("edu".into()),
            },
            &opts(),
        )
        .unwrap()
        .clip;
    let tag = |id: &str, label: &str, color: &str| Tag {
        id: id.into(),
        label: label.into(),
        color: color.into(),
    };
    for bad in [
        vec![tag("", "X", "#112233")],
        vec![tag("x", "  ", "#112233")],
        vec![tag("x", "X", "red")],
        vec![tag("x", "X", "#11223")],
        vec![tag("x", "X", "#112233"), tag("x", "Y", "#445566")],
    ] {
        assert_eq!(code(s.set_tags(bad).unwrap_err()), ErrorCode::InvalidInput);
    }
    let before_tags = s.project().tags.clone();
    let before_clips = s.project().clips.clone();
    let new_tags = vec![
        tag(" edu ", " Lessons ", "#3b82f6"),
        tag("new", "New", "#abcdef"),
    ];
    let p = s.set_tags(new_tags).unwrap();
    let expected = vec![
        tag("edu", "Lessons", "#3B82F6"),
        tag("new", "New", "#ABCDEF"),
    ];
    assert_eq!(p.tags.as_ref(), Some(&expected));
    assert_eq!(p.upserted.len(), 1);
    assert_eq!(p.upserted[0].id, a.id);
    assert_eq!(
        s.project().clip(a.id).unwrap().tag_id,
        None,
        "removed tag is cleared"
    );
    assert_eq!(
        s.project().clip(b.id).unwrap().tag_id.as_deref(),
        Some("edu")
    );
    let undo = s.undo().unwrap();
    assert_eq!(undo.label.as_deref(), Some("Undo Edit tags"));
    assert_eq!(undo.tags.as_ref(), Some(&before_tags));
    assert_eq!(s.project().tags, before_tags);
    assert_eq!(
        s.project().clips,
        before_clips,
        "one step restores tags and clip tags"
    );
    s.redo().unwrap();
    assert_eq!(s.project().tags, expected);
    assert_eq!(s.project().clip(a.id).unwrap().tag_id, None);
}

#[test]
fn export_settings_are_normalized_and_not_undoable() {
    let mut s = store(100.0);
    add(&mut s, 1.0, 2.0);
    let depth = s.undo_depth();
    let settings = ExportSettings {
        parallel_jobs: 9,
        naming_template: "  ".into(),
        ..ExportSettings::default()
    };
    let p = s.set_export_settings(settings).unwrap();
    let applied = p.export_settings.unwrap();
    assert_eq!(applied.parallel_jobs, 4);
    assert_eq!(applied.naming_template, "{index}_{name}");
    assert_eq!(s.undo_depth(), depth);
    assert_eq!(s.undo().unwrap().label.as_deref(), Some("Undo Add clip"));
    assert_eq!(
        s.project().export_settings.parallel_jobs,
        4,
        "undo does not touch export settings"
    );
}

#[test]
fn prj03_view_state_is_not_undoable_and_produces_no_revision() {
    let mut s = store(100.0);
    let a = add(&mut s, 1.0, 2.0);
    let _ = s.take_save_request();
    let rev = s.revision();
    s.set_view_state(ViewState {
        playhead_sec: 150.0,
        zoom_window_sec: 1.0,
        selected_clip_id: Some(a.id),
    });
    assert_eq!(s.revision(), rev);
    assert!(s.is_dirty());
    assert_eq!(s.save_urgency(), Some(Urgency::Debounced));
    let v = &s.project().view_state;
    assert_eq!(
        (v.playhead_sec, v.zoom_window_sec, v.selected_clip_id),
        (100.0, 2.0, Some(a.id))
    );
    s.set_view_state(ViewState {
        playhead_sec: 12.3456,
        zoom_window_sec: 9999.0,
        selected_clip_id: Some(Uuid::now_v7()),
    });
    let v = &s.project().view_state;
    assert_eq!(
        (v.playhead_sec, v.zoom_window_sec, v.selected_clip_id),
        (12.346, 600.0, None)
    );
    assert_eq!(s.undo().unwrap().label.as_deref(), Some("Undo Add clip"));
}

#[test]
fn import_adds_valid_items_as_one_undo_step() {
    let mut s = store(100.0);
    let item = |line, start, end, name: &str, error: Option<&str>| ImportedClip {
        line,
        start_sec: start,
        end_sec: end,
        name: name.into(),
        error: error.map(str::to_string),
    };
    let items = vec![
        item(1, 10.0, 20.0, " Intro ", None),
        item(2, 30.0, 30.2, "Too short", None),
        item(3, 0.0, 0.0, "Bad line", Some("Could not read the time")),
        item(4, 150.0, 160.0, "Past end", None),
        item(5, 40.0, 50.0, "Second", None),
    ];
    let r = s.import_clips(&items, &opts()).unwrap();
    assert_eq!(r.added.len(), 2);
    assert_eq!(r.patch.revision, 1);
    assert_eq!(
        r.patch
            .upserted
            .iter()
            .map(|c| (c.index, c.name.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "Intro"), (2, "Second")]
    );
    let skipped: Vec<u32> = r.skipped.iter().map(|i| i.line).collect();
    assert_eq!(skipped, vec![2, 3, 4]);
    assert!(r.skipped.iter().all(|i| i.error.is_some()));
    assert!(r.skipped[0].error.as_deref().unwrap().contains("0.5 s"));
    assert_eq!(s.save_urgency(), Some(Urgency::Immediate));
    assert_eq!(
        s.undo().unwrap().label.as_deref(),
        Some("Undo Import 2 clips")
    );
    assert!(s.project().clips.is_empty());
    let none = s.import_clips(&items[1..4], &opts()).unwrap();
    assert!(none.added.is_empty());
    assert_eq!(none.patch.revision, s.revision());
}

#[test]
fn que05_export_status_lifecycle() {
    let mut s = store(100.0);
    let a = add(&mut s, 10.0, 20.0);
    let depth = s.undo_depth();
    let p = s.mark_exporting(&[a.id]);
    assert_eq!(p.upserted[0].export_status, ExportStatus::Exporting);
    let p = s
        .apply_export_record(&record(&a, done("D:/out/01.mp4")))
        .unwrap();
    let c = &p.upserted[0];
    assert_eq!(c.export_status, ExportStatus::Completed);
    let le = c.last_export.as_ref().unwrap();
    assert_eq!(le.output_path.as_deref(), Some("D:/out/01.mp4"));
    assert_eq!(le.preset, Some(Preset::Lossless));
    assert_eq!(
        le.content_hash.as_deref(),
        Some(a.content_hash(Preset::Lossless).as_str())
    );
    assert_eq!(le.settings_hash, "abcd1234");

    // Edited while exporting: the finished export is already outdated.
    s.mark_exporting(&[a.id]);
    let edited = s
        .edit_clips(&[a.id], &ClipEdit::Rename { name: "New".into() }, &opts())
        .unwrap();
    assert_eq!(edited.upserted[0].export_status, ExportStatus::Exporting);
    let p = s
        .apply_export_record(&record(&a, done("D:/out/01.mp4")))
        .unwrap();
    assert_eq!(p.upserted[0].export_status, ExportStatus::Outdated);

    // Failed keeps the previous output path and the diagnostics.
    s.mark_exporting(&[a.id]);
    let failed = ClipOutcome::Failed {
        exit_code: Some(1),
        message: "Invalid data found when processing input".into(),
        log_tail: vec!["line 1".into(), "line 2".into()],
        log_path: Some("D:/logs/01.log".into()),
    };
    let current = s.project().clip(a.id).unwrap().clone();
    let c = s
        .apply_export_record(&record(&current, failed))
        .unwrap()
        .upserted[0]
        .clone();
    assert_eq!(c.export_status, ExportStatus::Failed);
    let le = c.last_export.unwrap();
    assert_eq!(
        le.error.as_deref(),
        Some("Invalid data found when processing input")
    );
    assert_eq!(le.exit_code, Some(1));
    assert_eq!(le.log_tail.len(), 2);
    assert_eq!(le.log_path.as_deref(), Some("D:/logs/01.log"));
    assert_eq!(le.output_path.as_deref(), Some("D:/out/01.mp4"));

    // Cancelled / skipped restore the status from before the export.
    s.mark_exporting(&[a.id]);
    let c = s
        .apply_export_record(&record(&current, ClipOutcome::Cancelled))
        .unwrap()
        .upserted[0]
        .clone();
    assert_eq!(c.export_status, ExportStatus::Failed);
    let b = add(&mut s, 30.0, 40.0);
    s.mark_exporting(&[b.id]);
    let skipped = ClipOutcome::Skipped {
        reason: "exists".into(),
    };
    assert_eq!(
        s.apply_export_record(&record(&b, skipped))
            .unwrap()
            .upserted[0]
            .export_status,
        ExportStatus::Pending
    );
    assert_eq!(
        s.undo_depth(),
        depth + 2,
        "only the rename and the add are undoable"
    );
}

#[test]
fn undo_never_rolls_back_export_status() {
    let mut s = store(100.0);
    let a = add(&mut s, 10.0, 20.0);
    s.edit_clips(
        &[a.id],
        &ClipEdit::Rename {
            name: "Edited".into(),
        },
        &opts(),
    )
    .unwrap();
    let edited = s.project().clip(a.id).unwrap().clone();
    s.mark_exporting(&[a.id]);
    s.apply_export_record(&record(&edited, done("D:/out/01_Edited.mp4")))
        .unwrap();
    let p = s.undo().unwrap();
    assert_eq!(p.label.as_deref(), Some("Undo Rename clip"));
    assert_eq!(
        p.upserted[0].export_status,
        ExportStatus::Outdated,
        "export of the edited clip is outdated, not pending"
    );
    assert!(p.upserted[0].last_export.is_some());
    let p = s.redo().unwrap();
    assert_eq!(p.upserted[0].export_status, ExportStatus::Completed);

    // Undo of the add, then redo: the clip comes back with its export.
    s.undo().unwrap();
    s.undo().unwrap();
    assert!(s.project().clips.is_empty());
    s.redo().unwrap();
    let back = s.project().clip(a.id).unwrap();
    assert_eq!(back.export_status, ExportStatus::Outdated);
    assert!(back.last_export.is_some());
}

#[test]
fn export_result_for_a_deleted_clip_applies_when_undo_restores_it() {
    let mut s = store(100.0);
    let a = add(&mut s, 10.0, 20.0);
    s.mark_exporting(&[a.id]);
    s.delete_clips(&[a.id]).unwrap();
    assert!(
        s.apply_export_record(&record(&a, done("D:/out/01.mp4")))
            .is_none()
    );
    let p = s.undo().unwrap();
    assert_eq!(p.upserted[0].export_status, ExportStatus::Completed);
    assert_eq!(
        p.upserted[0]
            .last_export
            .as_ref()
            .unwrap()
            .output_path
            .as_deref(),
        Some("D:/out/01.mp4")
    );
}

#[test]
fn que02_renumber_chronologically_is_undoable_and_keeps_next_index() {
    let mut s = store(100.0);
    let late = add(&mut s, 50.0, 60.0);
    let early = add(&mut s, 5.0, 8.0);
    let mid = add(&mut s, 20.0, 30.0);
    s.delete_clips(&[mid.id]).unwrap();
    let p = s.renumber_chronologically().unwrap();
    assert_eq!(p.upserted.len(), 2);
    assert_eq!(s.project().clip(early.id).unwrap().index, 1);
    assert_eq!(s.project().clip(late.id).unwrap().index, 2);
    assert_eq!(s.project().next_index, 4);
    assert_eq!(
        s.undo().unwrap().label.as_deref(),
        Some("Undo Renumber clips")
    );
    assert_eq!(s.project().clip(late.id).unwrap().index, 1);
    assert_eq!(s.project().clip(early.id).unwrap().index, 2);
}

#[test]
fn prj03_normalize_on_load_resets_exporting_and_flags_out_of_range() {
    let mut p = project(100.0);
    let mut exporting = clip(1, 10.0, 20.0);
    exporting.export_status = ExportStatus::Exporting;
    let mut stale = completed(clip(2, 30.0, 40.0), Preset::Lossless);
    stale.name = "changed after export".into();
    let beyond = clip(2, 120.0, 130.0);
    let mut dangling = clip(4, 50.0, 60.0);
    dangling.tag_id = Some("deleted-tag".into());
    p.clips = vec![exporting, stale, beyond, dangling];
    p.next_index = 3;
    p.view_state.playhead_sec = 1e9;
    let mut s = ProjectStore::new(p, None);
    let notices = s.normalize_on_load();
    let codes: Vec<&str> = notices.iter().map(|n| n.code.as_str()).collect();
    assert_eq!(codes, vec!["export_interrupted", "out_of_range_clips"]);
    assert!(
        notices[1].message.starts_with("1 clip starts"),
        "{}",
        notices[1].message
    );
    let c = &s.project().clips;
    assert_eq!(c[0].export_status, ExportStatus::Pending);
    assert_eq!(c[1].export_status, ExportStatus::Outdated);
    assert_eq!(c[2].index, 5, "duplicate number repaired");
    assert_eq!(c[3].tag_id, None);
    assert_eq!(s.project().next_index, 6);
    assert_eq!(s.project().view_state.playhead_sec, 100.0);
    assert_eq!(s.revision(), 0);
    assert!(s.is_dirty());
}

#[test]
fn prj04_relink_reports_out_of_range_and_clears_source_missing() {
    let mut p = project(100.0);
    let keep = clip(1, 10.0, 20.0);
    let gone = clip(2, 80.0, 90.0);
    p.clips = vec![keep.clone(), gone.clone()];
    p.next_index = 3;
    let mut s = ProjectStore::new(p, Some(ReadOnlyReason::SourceMissing));
    assert_eq!(s.clips_out_of_range_for(50.0), vec![gone.id]);
    let mut moved = source(50.0);
    moved.path = "E:/Moved/test stream.mp4".into();
    let out = s.relink(moved.clone());
    assert_eq!(out, vec![gone.id]);
    assert_eq!(s.project().source, moved);
    assert_eq!(s.project().clips, vec![keep, gone], "markers are kept");
    assert!(s.read_only().is_none());
    assert_eq!(s.revision(), 1);
    assert_eq!(s.take_save_request().unwrap().urgency, Urgency::Immediate);
    // Same file moved: everything restored, nothing out of range.
    assert!(s.relink(source(100.0)).is_empty());
}

#[test]
fn snapshot_reflects_state() {
    let mut s = store(100.0);
    add(&mut s, 1.0, 2.0);
    let save = SaveState {
        saved_at: None,
        pending: true,
        error: None,
        project_path: Some("x.rustcut".into()),
        location: Default::default(),
    };
    let snap = s.snapshot(save.clone(), SourceStatus::Ok);
    assert_eq!(snap.revision, 1);
    assert_eq!(&snap.project, s.project());
    assert_eq!(snap.history, s.history());
    assert_eq!(snap.save, save);
    assert_eq!(snap.read_only, None);
}

#[test]
fn mrk04_history_of_250_steps_undoes_and_redoes_exactly() {
    let mut s = store(10_000.0);
    let initial = s.project().clips.clone();
    for i in 0..250 {
        let start = f64::from(i) * 10.0;
        let a = add(&mut s, start, start + 1.0);
        if i % 2 == 0 {
            s.edit_clips(
                &[a.id],
                &ClipEdit::Rename {
                    name: format!("n{i}"),
                },
                &opts(),
            )
            .unwrap();
        }
    }
    let last = s.project().clips.clone();
    let mut undone = 0;
    while s.undo().is_some() {
        undone += 1;
    }
    assert_eq!(undone, 375);
    assert_eq!(s.project().clips, initial);
    while s.redo().is_some() {}
    assert_eq!(s.project().clips, last);
    assert_eq!(s.project().next_index, 251);
}

#[test]
fn undo_history_is_capped_at_undo_limit() {
    let mut s = store(100.0);
    let a = add(&mut s, 1.0, 2.0);
    for i in 0..UNDO_LIMIT + 5 {
        s.edit_clips(
            &[a.id],
            &ClipEdit::Rename {
                name: format!("n{i}"),
            },
            &opts(),
        )
        .unwrap();
    }
    assert_eq!(s.undo_depth(), UNDO_LIMIT);
    let mut n = 0;
    while s.undo().is_some() {
        n += 1;
    }
    assert_eq!(n, UNDO_LIMIT);
    assert!(
        s.project().clip(a.id).is_some(),
        "the oldest entries (the add) fell off the stack"
    );
}

#[test]
fn a_new_operation_clears_redo() {
    let mut s = store(100.0);
    add(&mut s, 1.0, 2.0);
    s.undo().unwrap();
    assert!(s.history().can_redo);
    add(&mut s, 3.0, 4.0);
    assert!(!s.history().can_redo);
    assert!(s.redo().is_none());
}

#[test]
fn store_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<ProjectStore>();
}
