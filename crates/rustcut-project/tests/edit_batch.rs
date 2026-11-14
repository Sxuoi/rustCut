//! `edit_batch` (one undo step for multi-value include changes) and crop-nudge
//! undo coalescing.

mod common;

use common::*;
use rustcut_model::{ClipEdit, EditClipsRequest, ReadOnlyReason};
use rustcut_project::{CROP_COALESCE_WINDOW, ProjectStore, StoreError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn inc(ids: &[uuid::Uuid], included: bool) -> EditClipsRequest {
    EditClipsRequest {
        ids: ids.to_vec(),
        edit: ClipEdit::SetIncluded { included },
    }
}

fn crop(id: uuid::Uuid, v: f64) -> (Vec<uuid::Uuid>, ClipEdit) {
    (vec![id], ClipEdit::SetCrop { crop_x_offset: v })
}

fn selected(s: &ProjectStore) -> Vec<bool> {
    s.project()
        .clips
        .iter()
        .map(|c| c.selected_for_export)
        .collect()
}

#[test]
fn batch_is_one_history_entry_and_undo_redo_are_exact() {
    let mut s = store(100.0);
    let a = add(&mut s, 0.0, 5.0);
    let b = add(&mut s, 10.0, 15.0);
    let c = add(&mut s, 20.0, 25.0);
    s.edit_batch(&[inc(&[b.id], false)], &opts()).unwrap();
    let before = selected(&s);
    let depth = s.undo_depth();
    let rev = s.revision();
    let patch = s
        .edit_batch(&[inc(&[a.id, c.id], false), inc(&[b.id], true)], &opts())
        .unwrap();
    assert_eq!(patch.revision, rev + 1);
    assert_eq!(s.undo_depth(), depth + 1);
    assert_eq!(
        patch.history.undo_label.as_deref(),
        Some("Undo Change include (3 clips)")
    );
    let after = selected(&s);
    assert_ne!(before, after);
    s.undo().unwrap();
    assert_eq!(selected(&s), before);
    s.redo().unwrap();
    assert_eq!(selected(&s), after);
}

#[test]
fn batch_validates_everything_before_applying() {
    let mut s = store(100.0);
    let a = add(&mut s, 0.0, 5.0);
    let rev = s.revision();
    let depth = s.undo_depth();
    let before = selected(&s);
    let err = s
        .edit_batch(
            &[
                inc(&[a.id], !before[0]),
                inc(&[uuid::Uuid::from_u128(0xdead_beef)], true),
            ],
            &opts(),
        )
        .unwrap_err();
    assert!(matches!(err, StoreError::ClipNotFound(_)));
    assert_eq!(selected(&s), before);
    assert_eq!((s.revision(), s.undo_depth()), (rev, depth));
}

#[test]
fn batch_noop_records_nothing_and_net_change_wins() {
    let mut s = store(100.0);
    let a = add(&mut s, 0.0, 5.0);
    let cur = a.selected_for_export;
    let depth = s.undo_depth();
    let rev = s.revision();
    // Flip and flip back: no net change.
    let p = s
        .edit_batch(&[inc(&[a.id], !cur), inc(&[a.id], cur)], &opts())
        .unwrap();
    assert_eq!((p.revision, s.undo_depth()), (rev, depth));
    assert!(p.upserted.is_empty());
    let p = s.edit_batch(&[], &opts()).unwrap();
    assert_eq!(p.revision, rev);
}

#[test]
fn batch_read_only_is_refused() {
    let mut s = ProjectStore::new(project(10.0), Some(ReadOnlyReason::SourceMissing));
    assert!(matches!(
        s.edit_batch(&[], &opts()),
        Err(StoreError::ReadOnly { .. })
    ));
    let mut s = ProjectStore::new(
        project(10.0),
        Some(ReadOnlyReason::Unwritable {
            path: "D:/x".into(),
        }),
    );
    assert!(matches!(
        s.edit_batch(&[], &opts()),
        Err(StoreError::ReadOnly { .. })
    ));
}

fn clocked(s: &mut ProjectStore) -> Arc<Mutex<Duration>> {
    let base = Instant::now();
    let offset = Arc::new(Mutex::new(Duration::ZERO));
    let o = offset.clone();
    s.set_clock(move || base + *o.lock().unwrap());
    offset
}

fn nudge(s: &mut ProjectStore, id: uuid::Uuid, v: f64) -> u64 {
    let (ids, edit) = crop(id, v);
    s.edit_clips(&ids, &edit, &opts()).unwrap().revision
}

#[test]
fn crop_nudges_within_window_merge_into_one_entry() {
    let mut s = store(100.0);
    let a = add(&mut s, 0.0, 5.0);
    let t = clocked(&mut s);
    let depth = s.undo_depth();
    let r1 = nudge(&mut s, a.id, 0.1);
    *t.lock().unwrap() += Duration::from_millis(1000);
    let r2 = nudge(&mut s, a.id, 0.2);
    *t.lock().unwrap() += Duration::from_millis(1000); // sliding window
    let r3 = nudge(&mut s, a.id, 0.3);
    assert_eq!((r2, r3), (r1 + 1, r1 + 2));
    assert_eq!(s.undo_depth(), depth + 1);
    assert_eq!(s.project().clips[0].crop_x_offset, 0.3);
    s.undo().unwrap();
    assert_eq!(s.project().clips[0].crop_x_offset, a.crop_x_offset);
    s.redo().unwrap();
    assert_eq!(s.project().clips[0].crop_x_offset, 0.3);
}

#[test]
fn coalesced_crop_after_save_is_dirty_and_saves_the_final_revision() {
    let mut s = store(100.0);
    let a = add(&mut s, 0.0, 5.0);
    clocked(&mut s);
    s.take_save_request().unwrap();
    nudge(&mut s, a.id, 0.1);
    let saved = s.take_save_request().unwrap();
    assert!(!s.is_dirty());
    let final_revision = nudge(&mut s, a.id, 0.2);
    assert!(s.is_dirty());
    let final_save = s.take_save_request().unwrap();
    assert_eq!(final_save.revision, saved.revision + 1);
    assert_eq!(final_save.revision, final_revision);
    assert_eq!(final_save.project.clips[0].crop_x_offset, 0.2);
    s.undo().unwrap();
    assert_eq!(s.project().clips[0].crop_x_offset, a.crop_x_offset);
    assert!(s.is_dirty());
    s.redo().unwrap();
    assert_eq!(s.project().clips[0].crop_x_offset, 0.2);
}

#[test]
fn crop_chain_compares_requested_sets_including_unchanged_clips() {
    let mut s = store(100.0);
    let a = add(&mut s, 0.0, 5.0);
    let b = add(&mut s, 10.0, 15.0);
    nudge(&mut s, b.id, 0.1);
    clocked(&mut s);
    let depth = s.undo_depth();
    s.edit_clips(
        &[a.id, b.id],
        &ClipEdit::SetCrop { crop_x_offset: 0.1 },
        &opts(),
    )
    .unwrap();
    // Only A changed above, but both requested clips participate in this burst.
    s.edit_clips(
        &[b.id, a.id, a.id],
        &ClipEdit::SetCrop { crop_x_offset: 0.2 },
        &opts(),
    )
    .unwrap();
    assert_eq!(s.undo_depth(), depth + 1);
    s.undo().unwrap();
    assert_eq!(
        s.project().clip(a.id).unwrap().crop_x_offset,
        a.crop_x_offset
    );
    assert_eq!(s.project().clip(b.id).unwrap().crop_x_offset, 0.1);
    s.redo().unwrap();
    s.edit_clips(&[a.id], &ClipEdit::SetCrop { crop_x_offset: 0.3 }, &opts())
        .unwrap();
    // Different requested sets break a chain even when their changed sets coincide.
    s.edit_clips(
        &[a.id, b.id],
        &ClipEdit::SetCrop { crop_x_offset: 0.2 },
        &opts(),
    )
    .unwrap();
    assert_eq!(s.undo_depth(), depth + 3);
}

#[test]
fn crop_coalescing_preserves_the_500_entry_history_limit() {
    let mut s = store(100.0);
    let a = add(&mut s, 0.0, 5.0);
    let t = clocked(&mut s);
    for i in 0..rustcut_model::UNDO_LIMIT + 5 {
        *t.lock().unwrap() += Duration::from_secs(2);
        nudge(&mut s, a.id, if i % 2 == 0 { 0.1 } else { 0.2 });
    }
    assert_eq!(s.undo_depth(), 500);
    nudge(&mut s, a.id, 0.5);
    assert_eq!(s.undo_depth(), 500);
    s.undo().unwrap();
    assert_eq!(s.project().clips[0].crop_x_offset, 0.2);
    s.redo().unwrap();
    assert_eq!(s.project().clips[0].crop_x_offset, 0.5);
}

#[test]
fn crop_chain_breaks_on_gap_other_edit_and_undo() {
    let mut s = store(100.0);
    let a = add(&mut s, 0.0, 5.0);
    let t = clocked(&mut s);
    let depth = s.undo_depth();
    nudge(&mut s, a.id, 0.1);
    *t.lock().unwrap() += CROP_COALESCE_WINDOW + Duration::from_millis(1);
    nudge(&mut s, a.id, 0.2); // gap: new entry
    assert_eq!(s.undo_depth(), depth + 2);
    s.edit_clips(&[a.id], &ClipEdit::Rename { name: "x".into() }, &opts())
        .unwrap();
    nudge(&mut s, a.id, 0.3); // other edit in between: new entry
    assert_eq!(s.undo_depth(), depth + 4);
    s.undo().unwrap();
    s.redo().unwrap();
    nudge(&mut s, a.id, 0.4); // undo/redo in between: new entry
    assert_eq!(s.undo_depth(), depth + 5);
    // Different clip set does not merge.
    let b = add(&mut s, 10.0, 15.0);
    nudge(&mut s, a.id, 0.5);
    nudge(&mut s, b.id, 0.5);
    assert_eq!(s.undo_depth(), depth + 8);
}
