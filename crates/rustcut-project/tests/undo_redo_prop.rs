//! AC-15 / MRK-04: undo/redo returns the exact prior state across random
//! operation sequences (10 000 cases by default; `RUSTCUT_PROPTEST_CASES`
//! overrides). A reference model keeps the stack of expected states; after
//! every operation the store must equal the model, and at the end undo-all
//! must restore the initial clips and tags exactly and redo-all the newest.
//! Invariants checked after every step: AC-03 (valid ranges), QUE-02
//! (unique numbers, `next_index` monotonic), tags referenced exist, patch
//! revisions +1 (or unchanged for no-ops), failed commands change nothing.

mod common;

use common::*;
use proptest::collection::vec;
use proptest::prelude::*;
use rustcut_model::time::to_ms;
use rustcut_model::{
    AddClipRequest, Clip, ClipEdit, ExportStatus, ImportedClip, Preset, StatePatch, Tag,
};
use rustcut_project::{ProjectStore, StoreError, StoreOptions};
use std::collections::HashSet;
use uuid::Uuid;

const DURATION_MS: i64 = 600_000;
const NAMES: [&str; 6] = ["Intro", "  Jump scare  ", "", "Ünïcödé 名前", "Clip", "x"];
const TAGS: [Option<&str>; 5] = [
    None,
    Some("funny"),
    Some("edu"),
    Some("highlight"),
    Some("custom"),
];

#[derive(Debug, Clone)]
enum Op {
    Add {
        start_ms: u32,
        len_ms: u32,
        tag: u8,
        name: u8,
    },
    Delete(Vec<u8>),
    Rename(u8, u8),
    Retime(u8, Option<i32>, Option<i32>),
    Tag(Vec<u8>, u8),
    Include(Vec<u8>, bool),
    Crop(Vec<u8>, i16),
    Renumber,
    SetTags(u8),
    Import(Vec<(u32, u32)>),
    Undo,
    Redo,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0u32..620_000, 0u32..40_000, 0u8..5, 0u8..6)
            .prop_map(|(start_ms, len_ms, tag, name)| Op::Add { start_ms, len_ms, tag, name }),
        2 => vec(any::<u8>(), 1..4).prop_map(Op::Delete),
        2 => (any::<u8>(), 0u8..6).prop_map(|(c, n)| Op::Rename(c, n)),
        2 => (any::<u8>(), proptest::option::of(-30_000i32..30_000), proptest::option::of(-30_000i32..30_000))
            .prop_map(|(c, s, e)| Op::Retime(c, s, e)),
        1 => (vec(any::<u8>(), 1..4), 0u8..5).prop_map(|(c, t)| Op::Tag(c, t)),
        1 => (vec(any::<u8>(), 1..4), any::<bool>()).prop_map(|(c, b)| Op::Include(c, b)),
        1 => (vec(any::<u8>(), 1..4), -12_000i16..12_000).prop_map(|(c, x)| Op::Crop(c, x)),
        1 => Just(Op::Renumber),
        1 => any::<u8>().prop_map(Op::SetTags),
        1 => vec((0u32..620_000, 0u32..20_000), 0..4).prop_map(Op::Import),
        3 => Just(Op::Undo),
        2 => Just(Op::Redo),
    ]
}

fn cases() -> u32 {
    std::env::var("RUSTCUT_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000)
}

fn named(mut c: Clip, tag: Option<&str>, crop: f64) -> Clip {
    c.tag_id = tag.map(str::to_string);
    c.crop_x_offset = crop;
    c
}

/// Mixed starting state: pending, completed (with content hashes, so status
/// recomputation is exercised), failed; numbers with gaps.
fn initial_store() -> ProjectStore {
    let mut p = project(DURATION_MS as f64 / 1000.0);
    p.tags.push(Tag {
        id: "custom".into(),
        label: "Custom".into(),
        color: "#123456".into(),
    });
    let mut failed = clip(7, 300.0, 300.5);
    failed.export_status = ExportStatus::Failed;
    p.clips = vec![
        clip(1, 10.0, 20.0),
        completed(
            named(clip(3, 30.0, 45.5), Some("edu"), 0.0),
            Preset::Lossless,
        ),
        completed(
            named(clip(5, 100.25, 160.0), Some("custom"), 0.3),
            Preset::Vcrop,
        ),
        failed,
    ];
    p.next_index = 10;
    let mut s = ProjectStore::new(p, None);
    // One history entry per command here: every call is 2 s after the last, so
    // crop nudges never coalesce (that is tested in edit_batch.rs).
    let base = std::time::Instant::now();
    let ticks = std::sync::atomic::AtomicU64::new(0);
    s.set_clock(move || {
        base + std::time::Duration::from_secs(
            2 * ticks.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        )
    });
    s
}

type State = (Vec<Clip>, Vec<Tag>);

fn state(s: &ProjectStore) -> State {
    (s.project().clips.clone(), s.project().tags.clone())
}

fn picks(s: &ProjectStore, sel: &[u8]) -> Vec<Uuid> {
    let clips = &s.project().clips;
    if clips.is_empty() {
        return Vec::new();
    }
    sel.iter()
        .map(|i| clips[*i as usize % clips.len()].id)
        .collect()
}

fn tag_pool(mask: u8) -> Vec<Tag> {
    let pool = [
        ("funny", "Funny", "#EF4444"),
        ("edu", "Educational", "#3B82F6"),
        ("highlight", "Highlight", "#22C55E"),
        ("custom", "Custom", "#123456"),
        ("extra", "Extra", "#ABCDEF"),
    ];
    let mut tags: Vec<Tag> = pool
        .iter()
        .enumerate()
        .filter(|(i, _)| mask & (1 << i) != 0)
        .map(|(_, (id, label, color))| Tag {
            id: (*id).into(),
            label: (*label).into(),
            color: (*color).into(),
        })
        .collect();
    if mask & 0x80 != 0
        && let Some(first) = tags.first_mut()
    {
        first.label = format!("{} (renamed)", first.label);
    }
    tags
}

/// Run a non-undo/redo op against the store.
fn run(s: &mut ProjectStore, op: &Op, o: &StoreOptions) -> Result<StatePatch, StoreError> {
    let edit = |s: &mut ProjectStore, sel: &[u8], e: ClipEdit| s.edit_clips(&picks(s, sel), &e, o);
    match op {
        Op::Add {
            start_ms,
            len_ms,
            tag,
            name,
        } => {
            let start = f64::from(*start_ms) / 1000.0;
            let request = AddClipRequest {
                start_sec: start,
                end_sec: start + f64::from(*len_ms) / 1000.0,
                name: Some(NAMES[*name as usize].into()),
                tag_id: TAGS[*tag as usize].map(str::to_string),
            };
            s.add_clip(&request, o).map(|r| r.patch)
        }
        Op::Delete(sel) => s.delete_clips(&picks(s, sel)),
        Op::Rename(c, n) => edit(
            s,
            &[*c],
            ClipEdit::Rename {
                name: NAMES[*n as usize].into(),
            },
        ),
        Op::Retime(c, ds, de) => {
            let Some(id) = picks(s, &[*c]).first().copied() else {
                return s.delete_clips(&[]);
            };
            let current = s.project().clip(id).cloned().expect("picked clip exists");
            let shift = |base: f64, d: &Option<i32>| d.map(|d| base + f64::from(d) / 1000.0);
            let e = ClipEdit::Retime {
                start_sec: shift(current.start_sec, ds),
                end_sec: shift(current.end_sec, de),
            };
            s.edit_clips(&[id], &e, o)
        }
        Op::Tag(sel, t) => edit(
            s,
            sel,
            ClipEdit::SetTag {
                tag_id: TAGS[*t as usize].map(str::to_string),
            },
        ),
        Op::Include(sel, b) => edit(s, sel, ClipEdit::SetIncluded { included: *b }),
        Op::Crop(sel, x) => edit(
            s,
            sel,
            ClipEdit::SetCrop {
                crop_x_offset: f64::from(*x) / 10_000.0,
            },
        ),
        Op::Renumber => s.renumber_chronologically(),
        Op::SetTags(mask) => s.set_tags(tag_pool(*mask)),
        Op::Import(items) => {
            let items: Vec<ImportedClip> = items
                .iter()
                .enumerate()
                .map(|(i, (start, len))| ImportedClip {
                    line: i as u32 + 1,
                    start_sec: f64::from(*start) / 1000.0,
                    end_sec: f64::from(start + len) / 1000.0,
                    name: format!("Imported {i}"),
                    error: None,
                })
                .collect();
            s.import_clips(&items, o).map(|r| r.patch)
        }
        Op::Undo | Op::Redo => unreachable!("handled by the caller"),
    }
}

fn check_invariants(s: &ProjectStore, min_next: u32) -> Result<(), TestCaseError> {
    let p = s.project();
    prop_assert!(p.next_index >= min_next, "next_index decreased");
    let tag_ids: HashSet<&str> = p.tags.iter().map(|t| t.id.as_str()).collect();
    let mut ids = HashSet::new();
    let mut numbers = HashSet::new();
    for c in &p.clips {
        prop_assert!(ids.insert(c.id), "duplicate id");
        prop_assert!(numbers.insert(c.index), "duplicate clip number {}", c.index);
        prop_assert!(c.index >= 1 && c.index < p.next_index);
        let (start, end) = (to_ms(c.start_sec), to_ms(c.end_sec));
        prop_assert!(
            start >= 0 && end <= DURATION_MS && end - start >= 500,
            "invalid range {start}..{end}"
        );
        prop_assert!((-1.0..=1.0).contains(&c.crop_x_offset));
        prop_assert!(
            c.tag_id.as_deref().is_none_or(|t| tag_ids.contains(t)),
            "dangling tag"
        );
        prop_assert!(c.export_status != ExportStatus::Exporting);
    }
    Ok(())
}

fn check_sequence(ops: &[Op]) -> Result<(), TestCaseError> {
    let o = StoreOptions::default();
    let mut s = initial_store();
    let mut states = vec![state(&s)];
    let mut pos = 0usize;
    let mut min_next = s.project().next_index;
    for op in ops {
        let rev = s.revision();
        match op {
            Op::Undo => {
                let patch = s.undo();
                prop_assert_eq!(patch.is_some(), pos > 0);
                if let Some(p) = patch {
                    prop_assert_eq!(p.revision, rev + 1);
                    prop_assert!(p.label.as_deref().is_some_and(|l| l.starts_with("Undo ")));
                    pos -= 1;
                }
            }
            Op::Redo => {
                let patch = s.redo();
                prop_assert_eq!(patch.is_some(), pos + 1 < states.len());
                if let Some(p) = patch {
                    prop_assert_eq!(p.revision, rev + 1);
                    pos += 1;
                }
            }
            _ => {
                let before = state(&s);
                match run(&mut s, op, &o) {
                    Err(_) => {
                        prop_assert_eq!(&state(&s), &before, "a failed command changed the state");
                        prop_assert_eq!(s.revision(), rev);
                    }
                    Ok(p) if p.revision == rev => {
                        prop_assert_eq!(&state(&s), &before, "a no-op changed the state");
                        prop_assert!(p.upserted.is_empty() && p.removed.is_empty());
                    }
                    Ok(p) => {
                        prop_assert_eq!(p.revision, rev + 1);
                        prop_assert!(state(&s) != before, "revision bumped without a change");
                        states.truncate(pos + 1);
                        states.push(state(&s));
                        pos += 1;
                    }
                }
            }
        }
        prop_assert_eq!(&state(&s), &states[pos]);
        let h = s.history();
        prop_assert_eq!(h.can_undo, pos > 0);
        prop_assert_eq!(h.can_redo, pos + 1 < states.len());
        check_invariants(&s, min_next)?;
        min_next = s.project().next_index;
    }
    while s.undo().is_some() {}
    prop_assert_eq!(
        &state(&s),
        &states[0],
        "undo-all must restore the initial state exactly"
    );
    while s.redo().is_some() {}
    prop_assert_eq!(
        &state(&s),
        &states[states.len() - 1],
        "redo-all must restore the newest state exactly"
    );
    check_invariants(&s, min_next)?;
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), ..ProptestConfig::default() })]

    #[test]
    fn ac15_undo_redo_restores_exact_states(ops in vec(op(), 1..40)) {
        check_sequence(&ops)?;
    }
}
