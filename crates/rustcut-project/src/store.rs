//! [`ProjectStore`]: the single owner of an open project's state (PRD §7.2).
//!
//! * Every user command first checks the read-only state, then validates its
//!   input (MRK-01: times clamped to `[0, source duration]` and rounded to
//!   milliseconds, `end - start >= min clip duration`), applies the change,
//!   records **one** undo entry (MRK-04; bulk edits and deletes are one entry)
//!   and returns a [`StatePatch`] whose `revision` is exactly one higher than
//!   before. A command that changes nothing returns the current revision with
//!   no upserts and records no history. A failed command changes nothing.
//! * Undo/redo restore the user-editable [`ClipFields`] (whole clips for
//!   add/delete). Export status is recomputed by [`Clip::set_fields`]; it is
//!   never rolled back. `next_index` never decreases, not even on undo (QUE-02).
//! * System updates (export status, load normalisation, relink, export and
//!   view settings) are not undoable and never touch the history.
//! * The store only marks itself dirty; [`ProjectStore::take_save_request`]
//!   hands the state to [`crate::AutoSaver`].

use crate::autosave::Urgency;
use crate::error::StoreError;
use rustcut_model::time::{format_timecode, round_ms, to_ms};
use rustcut_model::{
    AddClipRequest, AddClipResult, Clip, ClipEdit, ClipFields, ClipOutcome, DEFAULT_MIN_CLIP_SEC,
    EditClipsRequest, ExportRecord, ExportSettings, ExportStatus, HistoryState, ImportedClip,
    LastExport, MIN_CLIP_SEC_RANGE, Notice, NoticeLevel, Project, ProjectSnapshot, ReadOnlyReason,
    SaveState, SourceInfo, SourceStatus, StatePatch, Tag, UNDO_LIMIT, ViewState,
    ZOOM_WINDOW_RANGE_SEC, now_utc,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Consecutive crop nudges on the same clips within this window are one undo step.
pub const CROP_COALESCE_WINDOW: Duration = Duration::from_millis(1500);

/// Time source for undo coalescing; injectable so tests need no sleeping.
#[derive(Clone)]
struct Clock(Arc<dyn Fn() -> Instant + Send + Sync>);

impl std::fmt::Debug for Clock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Clock")
    }
}

/// The newest undo entry is a SetCrop edit that further nudges may merge into.
#[derive(Debug, Clone)]
struct CropChain {
    /// Sorted requested ids, including clips already at the requested value.
    ids: Vec<Uuid>,
    at: Instant,
}

/// Validation options passed with each command (app settings, MRK-01).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoreOptions {
    /// Minimum clip duration in seconds (setting 0.1–5 s, default 0.5 s).
    pub min_clip_duration_sec: f64,
}

impl Default for StoreOptions {
    fn default() -> Self {
        StoreOptions {
            min_clip_duration_sec: DEFAULT_MIN_CLIP_SEC,
        }
    }
}

impl StoreOptions {
    /// Effective minimum: clamped into [`MIN_CLIP_SEC_RANGE`] and rounded to
    /// milliseconds; a non-finite setting falls back to the 0.5 s default.
    pub fn min_duration(&self) -> f64 {
        let v = if self.min_clip_duration_sec.is_finite() {
            self.min_clip_duration_sec
        } else {
            DEFAULT_MIN_CLIP_SEC
        };
        round_ms(v.clamp(MIN_CLIP_SEC_RANGE.0, MIN_CLIP_SEC_RANGE.1))
    }
}

/// Result of [`ProjectStore::import_clips`] (EXP-17).
#[derive(Debug, Clone, PartialEq)]
pub struct ImportResult {
    /// One revision (and one undo step) for all imported clips; a no-op
    /// patch when nothing was valid.
    pub patch: StatePatch,
    /// Ids of the new clips, in input order.
    pub added: Vec<Uuid>,
    /// Items that were not imported, each with `error` set to the reason.
    pub skipped: Vec<ImportedClip>,
}

/// State to hand to [`crate::AutoSaver::schedule`].
#[derive(Debug, Clone, PartialEq)]
pub struct SaveRequest {
    pub project: Project,
    pub revision: u64,
    /// Immediate when any change since the last request added or deleted clips (PRJ-01).
    pub urgency: Urgency,
}

#[derive(Debug, Clone, PartialEq)]
struct FieldChange {
    id: Uuid,
    before: ClipFields,
    after: ClipFields,
}

/// One undoable operation. Entries are updated with the current clip state
/// whenever clips leave the project (undo of an add, redo of a delete), so
/// re-inserting them never rolls back an export status.
#[derive(Debug, Clone, PartialEq)]
enum Op {
    /// Clips appended to the end of the list, in order.
    Add {
        clips: Vec<Clip>,
    },
    /// Removed clips with their list positions before removal (ascending).
    Delete {
        removed: Vec<(usize, Clip)>,
    },
    Edit {
        changes: Vec<FieldChange>,
    },
    /// `(clip, old index, new index)`.
    Renumber {
        changes: Vec<(Uuid, u32, u32)>,
    },
    /// Palette change; `cleared` lists `(clip, tag)` pairs whose tag was removed with it.
    Tags {
        before: Vec<Tag>,
        after: Vec<Tag>,
        cleared: Vec<(Uuid, String)>,
    },
}

impl Op {
    fn urgency(&self) -> Urgency {
        match self {
            Op::Add { .. } | Op::Delete { .. } => Urgency::Immediate,
            _ => Urgency::Debounced,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    label: String,
    op: Op,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Undo,
    Redo,
}

/// What a change touched; turned into a [`StatePatch`] by `commit`.
#[derive(Debug, Default)]
struct Change {
    upserted: Vec<Uuid>,
    removed: Vec<Uuid>,
    tags: bool,
    export_settings: bool,
}

impl Change {
    fn upserted(ids: Vec<Uuid>) -> Self {
        Change {
            upserted: ids,
            ..Change::default()
        }
    }
    fn removed(ids: Vec<Uuid>) -> Self {
        Change {
            removed: ids,
            ..Change::default()
        }
    }
}

/// Owner of one open project: commands, undo/redo and system updates.
/// Synchronous and `Send`; the app wraps it in a `Mutex`.
#[derive(Debug)]
pub struct ProjectStore {
    project: Project,
    read_only: Option<ReadOnlyReason>,
    revision: u64,
    undo: VecDeque<Entry>,
    redo: Vec<Entry>,
    save_pending: Option<Urgency>,
    /// Status of each clip before `mark_exporting` (restored on cancel/skip).
    exporting_prev: HashMap<Uuid, ExportStatus>,
    /// Export results for clips deleted while exporting; applied if an undo restores them.
    orphan_records: HashMap<Uuid, ExportRecord>,
    clock: Clock,
    crop_chain: Option<CropChain>,
}

impl ProjectStore {
    /// Wrap a loaded or new project. `read_only` blocks every user mutation
    /// (PRJ-05 lock held elsewhere, PRJ-06 newer schema, missing source).
    /// Call [`ProjectStore::normalize_on_load`] for projects read from disk.
    pub fn new(project: Project, read_only: Option<ReadOnlyReason>) -> Self {
        ProjectStore {
            project,
            read_only,
            revision: 0,
            undo: VecDeque::new(),
            redo: Vec::new(),
            save_pending: None,
            exporting_prev: HashMap::new(),
            orphan_records: HashMap::new(),
            clock: Clock(Arc::new(Instant::now)),
            crop_chain: None,
        }
    }

    /// Replace the time source used to coalesce crop nudges (tests).
    pub fn set_clock(&mut self, clock: impl Fn() -> Instant + Send + Sync + 'static) {
        self.clock = Clock(Arc::new(clock));
    }

    /// Current project state.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Consume the store, returning the project.
    pub fn into_project(self) -> Project {
        self.project
    }

    /// Revision of the current state; increases by one per patch.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Why the project is read-only, if it is.
    pub fn read_only(&self) -> Option<&ReadOnlyReason> {
        self.read_only.as_ref()
    }

    /// Change the read-only state (e.g. the source vanished mid-session).
    pub fn set_read_only(&mut self, reason: Option<ReadOnlyReason>) {
        self.read_only = reason;
    }

    /// Undo/redo availability and labels ("Undo Add clip"); nothing is
    /// undoable while read-only.
    pub fn history(&self) -> HistoryState {
        let writable = self.read_only.is_none();
        HistoryState {
            can_undo: writable && !self.undo.is_empty(),
            can_redo: writable && !self.redo.is_empty(),
            undo_label: self
                .undo
                .back()
                .filter(|_| writable)
                .map(|e| format!("Undo {}", e.label)),
            redo_label: self
                .redo
                .last()
                .filter(|_| writable)
                .map(|e| format!("Redo {}", e.label)),
        }
    }

    /// Number of entries on the undo stack (at most [`UNDO_LIMIT`]).
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// True when the state changed since the last [`ProjectStore::take_save_request`].
    pub fn is_dirty(&self) -> bool {
        self.save_pending.is_some()
    }

    /// Urgency of the unsaved changes, if any.
    pub fn save_urgency(&self) -> Option<Urgency> {
        self.save_pending
    }

    /// Take the unsaved state for the auto-saver and clear the dirty flag.
    /// Returns `None` when nothing changed or the project is read-only (a
    /// read-only project is never written).
    pub fn take_save_request(&mut self) -> Option<SaveRequest> {
        let urgency = self.save_pending.take()?;
        if self.read_only.is_some() {
            return None;
        }
        Some(SaveRequest {
            project: self.project.clone(),
            revision: self.revision,
            urgency,
        })
    }

    /// Full state for the UI (open, relink, `get_snapshot`).
    pub fn snapshot(&self, save: SaveState, source_status: SourceStatus) -> ProjectSnapshot {
        ProjectSnapshot {
            revision: self.revision,
            project: self.project.clone(),
            history: self.history(),
            save,
            read_only: self.read_only.clone(),
            source_status,
        }
    }

    // ───────────────────────────── user commands ─────────────────────────────

    /// MRK-01: add `[start, end]` (clamped, rounded to ms) as a new clip with
    /// the next stable number (QUE-02). `duplicate_of` is the MRK-03 hint.
    pub fn add_clip(
        &mut self,
        req: &AddClipRequest,
        opts: &StoreOptions,
    ) -> Result<AddClipResult, StoreError> {
        self.ensure_writable()?;
        let (start, end) = self.validate_range(req.start_sec, req.end_sec, opts)?;
        let tag_id = self.validate_tag(req.tag_id.as_deref())?;
        let index = self.first_free_index();
        let next_index = index.checked_add(1).ok_or_else(too_many_clips)?;
        let duplicate_of = self.project.duplicate_of(start, end, None);
        let clip = new_clip(index, req.name.as_deref().unwrap_or(""), start, end, tag_id);
        self.project.next_index = next_index;
        self.project.clips.push(clip.clone());
        self.record(Entry {
            label: "Add clip".into(),
            op: Op::Add {
                clips: vec![clip.clone()],
            },
        });
        let patch = self.commit(Change::upserted(vec![clip.id]), None, Urgency::Immediate);
        Ok(AddClipResult {
            clip,
            patch,
            duplicate_of,
        })
    }

    /// Apply `edit` to every clip in `ids` as one undo step (QUE-07). All
    /// clips are validated before any is changed. Rename trims the name,
    /// SetCrop clamps to `[-1, 1]` (1e-4 precision), Retime validates the
    /// resulting range like [`ProjectStore::add_clip`].
    ///
    /// Consecutive SetCrop edits of the same clips within
    /// [`CROP_COALESCE_WINDOW`] merge into one history entry (first "before",
    /// latest "after"); each call still bumps the revision once.
    pub fn edit_clips(
        &mut self,
        ids: &[Uuid],
        edit: &ClipEdit,
        opts: &StoreOptions,
    ) -> Result<StatePatch, StoreError> {
        self.ensure_writable()?;
        let changes = self.plan_edits(&[(ids, edit)], opts)?;
        if changes.is_empty() {
            return Ok(self.noop_patch());
        }
        let changed_ids: Vec<Uuid> = changes.iter().map(|c| c.id).collect();
        for change in &changes {
            if let Some(clip) = self.project.clip_mut(change.id) {
                clip.set_fields(&change.after);
            }
        }
        if matches!(edit, ClipEdit::SetCrop { .. }) && self.try_coalesce_crop(ids, &changes) {
            return Ok(self.commit(Change::upserted(changed_ids), None, Urgency::Debounced));
        }
        let label = edit_label(edit.label(), changes.len());
        self.record(Entry {
            label,
            op: Op::Edit {
                changes: changes.clone(),
            },
        });
        if matches!(edit, ClipEdit::SetCrop { .. }) {
            self.start_crop_chain(ids);
        }
        Ok(self.commit(Change::upserted(changed_ids), None, Urgency::Debounced))
    }

    /// MRK-04 / QUE-04: several edits (e.g. include = true for some clips,
    /// false for others) as ONE undo step and ONE revision bump. Everything is
    /// validated first (edits apply in order, so a later edit sees the result
    /// of an earlier one); nothing changes on error; no net change records no
    /// history.
    pub fn edit_batch(
        &mut self,
        edits: &[EditClipsRequest],
        opts: &StoreOptions,
    ) -> Result<StatePatch, StoreError> {
        self.ensure_writable()?;
        let reqs: Vec<(&[Uuid], &ClipEdit)> =
            edits.iter().map(|e| (e.ids.as_slice(), &e.edit)).collect();
        let changes = self.plan_edits(&reqs, opts)?;
        if changes.is_empty() {
            return Ok(self.noop_patch());
        }
        for change in &changes {
            if let Some(clip) = self.project.clip_mut(change.id) {
                clip.set_fields(&change.after);
            }
        }
        let ids: Vec<Uuid> = changes.iter().map(|c| c.id).collect();
        let all_include = edits
            .iter()
            .all(|e| matches!(e.edit, ClipEdit::SetIncluded { .. }));
        let first = edits.first().map(|e| e.edit.label()).unwrap_or_default();
        let same = edits.iter().all(|e| e.edit.label() == first);
        let base = if all_include {
            "Change include"
        } else if same {
            first
        } else {
            "Edit clips"
        };
        let label = edit_label(base, changes.len());
        self.record(Entry {
            label,
            op: Op::Edit { changes },
        });
        Ok(self.commit(Change::upserted(ids), None, Urgency::Debounced))
    }

    /// Validate edits in order against a working copy; returns the net
    /// per-clip changes (first-seen order), without touching the project.
    fn plan_edits(
        &self,
        reqs: &[(&[Uuid], &ClipEdit)],
        opts: &StoreOptions,
    ) -> Result<Vec<FieldChange>, StoreError> {
        let mut order: Vec<Uuid> = Vec::new();
        let mut working: HashMap<Uuid, (ClipFields, ClipFields)> = HashMap::new();
        for (ids, edit) in reqs {
            for id in dedupe(ids) {
                let current = match working.get(&id) {
                    Some((_, current)) => current.clone(),
                    None => {
                        let clip = self.project.clip(id).ok_or(StoreError::ClipNotFound(id))?;
                        let fields = clip.fields();
                        order.push(id);
                        working.insert(id, (fields.clone(), fields.clone()));
                        fields
                    }
                };
                let after = self.edited_fields(&current, edit, opts)?;
                if let Some(entry) = working.get_mut(&id) {
                    entry.1 = after;
                }
            }
        }
        Ok(order
            .into_iter()
            .filter_map(|id| {
                let (before, after) = working.remove(&id)?;
                (before != after).then_some(FieldChange { id, before, after })
            })
            .collect())
    }

    /// Merge a SetCrop into the newest undo entry when it continues a chain.
    /// `changes` are already applied to the project.
    fn try_coalesce_crop(&mut self, requested: &[Uuid], changes: &[FieldChange]) -> bool {
        let now = (self.clock.0)();
        let mut ids = dedupe(requested);
        ids.sort();
        let continues = matches!(&self.crop_chain, Some(chain)
            if chain.ids == ids && now.saturating_duration_since(chain.at) <= CROP_COALESCE_WINDOW);
        if !continues {
            return false;
        }
        let Some(Entry {
            op: Op::Edit { changes: stored },
            ..
        }) = self.undo.back_mut()
        else {
            return false;
        };
        for new in changes {
            if let Some(previous) = stored.iter_mut().find(|c| c.id == new.id) {
                previous.after = new.after.clone();
            } else {
                stored.push(new.clone());
            }
        }
        self.crop_chain = Some(CropChain { ids, at: now });
        true
    }

    fn start_crop_chain(&mut self, requested: &[Uuid]) {
        let mut ids = dedupe(requested);
        ids.sort();
        self.crop_chain = Some(CropChain {
            ids,
            at: (self.clock.0)(),
        });
    }

    /// QUE-04: delete clips as one undo step (no confirmation needed).
    /// Numbers of deleted clips are never reused (QUE-02).
    pub fn delete_clips(&mut self, ids: &[Uuid]) -> Result<StatePatch, StoreError> {
        self.ensure_writable()?;
        let ids = dedupe(ids);
        if let Some(missing) = ids.iter().find(|id| self.project.clip(**id).is_none()) {
            return Err(StoreError::ClipNotFound(*missing));
        }
        if ids.is_empty() {
            return Ok(self.noop_patch());
        }
        let removed = self.remove_clips(&ids);
        self.clear_selection_of(&ids);
        let label = if ids.len() == 1 {
            "Delete clip".to_string()
        } else {
            format!("Delete {} clips", ids.len())
        };
        self.record(Entry {
            label,
            op: Op::Delete { removed },
        });
        Ok(self.commit(Change::removed(ids), None, Urgency::Immediate))
    }

    /// QUE-02 (P1): renumber clips 1..n by In (then Out, then old number).
    /// `next_index` is kept, so later clips still get unused numbers.
    pub fn renumber_chronologically(&mut self) -> Result<StatePatch, StoreError> {
        self.ensure_writable()?;
        let clips = &self.project.clips;
        let mut order: Vec<usize> = (0..clips.len()).collect();
        order.sort_by_key(|&i| {
            (
                to_ms(clips[i].start_sec),
                to_ms(clips[i].end_sec),
                clips[i].index,
            )
        });
        let mut changes = Vec::new();
        for (rank, &i) in order.iter().enumerate() {
            let new = u32::try_from(rank + 1).map_err(|_| too_many_clips())?;
            if clips[i].index != new {
                changes.push((clips[i].id, clips[i].index, new));
            }
        }
        if changes.is_empty() {
            return Ok(self.noop_patch());
        }
        for (id, _, new) in &changes {
            if let Some(clip) = self.project.clip_mut(*id) {
                clip.index = *new;
            }
        }
        let count = u32::try_from(self.project.clips.len()).unwrap_or(u32::MAX);
        self.project.next_index = self.project.next_index.max(count.saturating_add(1));
        let ids = changes.iter().map(|(id, ..)| *id).collect();
        self.record(Entry {
            label: "Renumber clips".into(),
            op: Op::Renumber { changes },
        });
        Ok(self.commit(Change::upserted(ids), None, Urgency::Debounced))
    }

    /// MRK-06: replace the tag palette. Ids must be unique and non-empty,
    /// labels non-empty, colours `#RRGGBB` (stored upper-case). Clips that
    /// reference a removed tag lose it in the same undo step.
    pub fn set_tags(&mut self, tags: Vec<Tag>) -> Result<StatePatch, StoreError> {
        self.ensure_writable()?;
        let tags = normalize_tags(tags)?;
        if tags == self.project.tags {
            return Ok(self.noop_patch());
        }
        let kept: HashSet<&str> = tags.iter().map(|t| t.id.as_str()).collect();
        let cleared: Vec<(Uuid, String)> = self
            .project
            .clips
            .iter()
            .filter_map(|c| {
                c.tag_id
                    .as_ref()
                    .filter(|t| !kept.contains(t.as_str()))
                    .map(|t| (c.id, t.clone()))
            })
            .collect();
        let before = std::mem::replace(&mut self.project.tags, tags.clone());
        self.set_clip_tags(&cleared, Direction::Redo);
        let ids = cleared.iter().map(|(id, _)| *id).collect();
        self.record(Entry {
            label: "Edit tags".into(),
            op: Op::Tags {
                before,
                after: tags,
                cleared,
            },
        });
        Ok(self.commit(
            Change {
                upserted: ids,
                tags: true,
                ..Change::default()
            },
            None,
            Urgency::Debounced,
        ))
    }

    /// Replace the export settings (normalised; not undoable).
    pub fn set_export_settings(
        &mut self,
        settings: ExportSettings,
    ) -> Result<StatePatch, StoreError> {
        self.ensure_writable()?;
        let settings = settings.normalized();
        if settings == self.project.export_settings {
            return Ok(self.noop_patch());
        }
        self.project.export_settings = settings;
        Ok(self.commit(
            Change {
                export_settings: true,
                ..Change::default()
            },
            None,
            Urgency::Debounced,
        ))
    }

    /// PRJ-03: remember playhead, zoom and selection. Not undoable, produces
    /// no patch and no revision; only marks the project dirty. Values are
    /// clamped (playhead to the source, zoom to 2 s–10 min) and a selection of
    /// an unknown clip is dropped.
    pub fn set_view_state(&mut self, view: ViewState) {
        let view = self.normalize_view(view);
        if view != self.project.view_state {
            self.project.view_state = view;
            self.mark_dirty(Urgency::Debounced);
        }
    }

    /// EXP-17: add the valid items as one undo step. Items with a parse
    /// error or an invalid range are returned in `skipped` with the reason.
    pub fn import_clips(
        &mut self,
        items: &[ImportedClip],
        opts: &StoreOptions,
    ) -> Result<ImportResult, StoreError> {
        self.ensure_writable()?;
        let mut valid = Vec::new();
        let mut skipped = Vec::new();
        for item in items {
            if item.error.is_some() {
                skipped.push(item.clone());
                continue;
            }
            match self.validate_range(item.start_sec, item.end_sec, opts) {
                Ok((start, end)) => valid.push((item.name.as_str(), start, end)),
                Err(e) => skipped.push(ImportedClip {
                    error: Some(e.to_string()),
                    ..item.clone()
                }),
            }
        }
        if valid.is_empty() {
            return Ok(ImportResult {
                patch: self.noop_patch(),
                added: Vec::new(),
                skipped,
            });
        }
        let first = self.first_free_index();
        let count = u32::try_from(valid.len()).map_err(|_| too_many_clips())?;
        let next_index = first.checked_add(count).ok_or_else(too_many_clips)?;
        let clips: Vec<Clip> = valid
            .iter()
            .zip(first..)
            .map(|((name, start, end), index)| new_clip(index, name, *start, *end, None))
            .collect();
        let added: Vec<Uuid> = clips.iter().map(|c| c.id).collect();
        self.project.next_index = next_index;
        self.project.clips.extend(clips.iter().cloned());
        let label = if clips.len() == 1 {
            "Import clip".to_string()
        } else {
            format!("Import {} clips", clips.len())
        };
        self.record(Entry {
            label,
            op: Op::Add { clips },
        });
        let patch = self.commit(Change::upserted(added.clone()), None, Urgency::Immediate);
        Ok(ImportResult {
            patch,
            added,
            skipped,
        })
    }

    /// Undo the newest entry; `None` when there is nothing to undo or the
    /// project is read-only. The patch label reads e.g. "Undo Add clip".
    pub fn undo(&mut self) -> Option<StatePatch> {
        if self.read_only.is_some() {
            return None;
        }
        self.crop_chain = None;
        let mut entry = self.undo.pop_back()?;
        let change = self.apply(&mut entry.op, Direction::Undo);
        let label = format!("Undo {}", entry.label);
        let urgency = entry.op.urgency();
        self.redo.push(entry);
        Some(self.commit(change, Some(label), urgency))
    }

    /// Redo the most recently undone entry; `None` when there is nothing to
    /// redo or the project is read-only.
    pub fn redo(&mut self) -> Option<StatePatch> {
        if self.read_only.is_some() {
            return None;
        }
        self.crop_chain = None;
        let mut entry = self.redo.pop()?;
        let change = self.apply(&mut entry.op, Direction::Redo);
        let label = format!("Redo {}", entry.label);
        let urgency = entry.op.urgency();
        self.undo.push_back(entry);
        self.trim_history();
        Some(self.commit(change, Some(label), urgency))
    }

    // ───────────────────────────── system updates ─────────────────────────────

    /// QUE-05: mark clips as Exporting (not undoable), remembering each
    /// clip's previous status for cancel/skip. Unknown ids are ignored.
    pub fn mark_exporting(&mut self, ids: &[Uuid]) -> StatePatch {
        let mut changed = Vec::new();
        for id in dedupe(ids) {
            if let Some(clip) = self.project.clips.iter_mut().find(|c| c.id == id)
                && clip.export_status != ExportStatus::Exporting
            {
                self.exporting_prev.insert(id, clip.export_status);
                clip.export_status = ExportStatus::Exporting;
                changed.push(id);
            }
        }
        if changed.is_empty() {
            return self.noop_patch();
        }
        self.commit(Change::upserted(changed), None, Urgency::Debounced)
    }

    /// QUE-05 / EXP-10: apply an export result (not undoable).
    ///
    /// * Done: `completed`, or `outdated` when the clip was edited while it
    ///   exported (its current `content_hash(preset)` differs from the
    ///   record's); `last_export` stores output path, preset and hashes.
    /// * Failed: `failed` with message, exit code, log tail and log path (the
    ///   previous output path is kept so a retry can replace it).
    /// * Cancelled / Skipped: the status from before `mark_exporting`.
    ///
    /// Returns `None` when the clip is not in the project (deleted while
    /// exporting); the record is then applied if an undo restores the clip.
    pub fn apply_export_record(&mut self, record: &ExportRecord) -> Option<StatePatch> {
        let previous = self.exporting_prev.remove(&record.clip_id);
        let Some(clip) = self
            .project
            .clips
            .iter_mut()
            .find(|c| c.id == record.clip_id)
        else {
            if let Some(status) = previous {
                self.exporting_prev.insert(record.clip_id, status);
            }
            self.orphan_records.insert(record.clip_id, record.clone());
            return None;
        };
        apply_record(clip, record, previous);
        Some(self.commit(
            Change::upserted(vec![record.clip_id]),
            None,
            Urgency::Debounced,
        ))
    }

    /// PRJ-03 load normalisation (no revision, not undoable): clips found
    /// Exporting become Pending, Completed/Outdated are recomputed, duplicate
    /// ids/numbers and dangling tags are repaired, `next_index` is raised
    /// above every number, the view state is clamped. Returns notices for
    /// interrupted exports and out-of-range clips (PRJ-04).
    pub fn normalize_on_load(&mut self) -> Vec<Notice> {
        let mut changed = false;
        let mut next = self.first_free_index();
        let tag_ids: HashSet<String> = self.project.tags.iter().map(|t| t.id.clone()).collect();
        let mut ids = HashSet::new();
        let mut indices = HashSet::new();
        let mut interrupted = 0usize;
        for clip in &mut self.project.clips {
            if !ids.insert(clip.id) {
                clip.id = Uuid::now_v7();
                ids.insert(clip.id);
                changed = true;
            }
            if clip.index == 0 || !indices.insert(clip.index) {
                clip.index = next;
                indices.insert(next);
                next = next.saturating_add(1);
                changed = true;
            }
            if clip.export_status == ExportStatus::Exporting {
                clip.export_status = ExportStatus::Pending;
                interrupted += 1;
                changed = true;
            }
            let status = clip.export_status;
            clip.refresh_status();
            changed |= status != clip.export_status;
            if !clip.crop_x_offset.is_finite() || clip.crop_x_offset.abs() > 1.0 {
                clip.crop_x_offset = if clip.crop_x_offset.is_finite() {
                    clip.crop_x_offset.clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                changed = true;
            }
            if clip.tag_id.as_ref().is_some_and(|t| !tag_ids.contains(t)) {
                clip.tag_id = None;
                changed = true;
            }
        }
        if self.project.next_index < next {
            self.project.next_index = next;
            changed = true;
        }
        let view = self.normalize_view(self.project.view_state.clone());
        if view != self.project.view_state {
            self.project.view_state = view;
            changed = true;
        }
        if changed {
            self.mark_dirty(Urgency::Debounced);
        }

        let mut notices = Vec::new();
        if interrupted > 0 {
            notices.push(Notice::new(
                NoticeLevel::Info,
                "export_interrupted",
                format!(
                    "{} {} exporting when RustCut last closed; they are pending again.",
                    clips(interrupted),
                    were(interrupted)
                ),
            ));
        }
        if let Some(notice) = self.out_of_range_notice() {
            notices.push(notice);
        }
        notices
    }

    /// PRJ-04: switch to a relinked (or re-probed) source. Clips are not
    /// changed; returns the ids of clips whose In is at or past the new end
    /// (*Out of range*, excluded from export). Clears a `SourceMissing`
    /// read-only state. Not undoable; the app sends a fresh snapshot.
    pub fn relink(&mut self, source: SourceInfo) -> Vec<Uuid> {
        self.project.source = source;
        if matches!(self.read_only, Some(ReadOnlyReason::SourceMissing)) {
            self.read_only = None;
        }
        self.project.view_state = self.normalize_view(self.project.view_state.clone());
        self.revision += 1;
        self.project.updated_at = now_utc();
        self.mark_dirty(Urgency::Immediate);
        self.project.out_of_range_ids()
    }

    /// Clips whose In is at or past `duration_sec` (for the PRJ-04 mismatch
    /// dialog before relinking to a file of that duration).
    pub fn clips_out_of_range_for(&self, duration_sec: f64) -> Vec<Uuid> {
        let end = to_ms(duration_sec.max(0.0));
        self.project
            .clips
            .iter()
            .filter(|c| to_ms(c.start_sec) >= end)
            .map(|c| c.id)
            .collect()
    }

    // ───────────────────────────── internals ─────────────────────────────

    fn ensure_writable(&self) -> Result<(), StoreError> {
        match &self.read_only {
            Some(reason) => Err(StoreError::ReadOnly {
                reason: reason.clone(),
            }),
            None => Ok(()),
        }
    }

    fn source_duration(&self) -> f64 {
        let d = self.project.source.duration_sec;
        if d.is_finite() && d > 0.0 {
            round_ms(d)
        } else {
            0.0
        }
    }

    /// MRK-01 range rules; returns the clamped, ms-rounded `(start, end)`.
    fn validate_range(
        &self,
        start: f64,
        end: f64,
        opts: &StoreOptions,
    ) -> Result<(f64, f64), StoreError> {
        if !start.is_finite() || !end.is_finite() {
            return Err(StoreError::InvalidRange(
                "Clip times must be numbers of seconds.".into(),
            ));
        }
        let duration = self.source_duration();
        let clamped_start = round_ms(start.clamp(0.0, duration + 1.0));
        if to_ms(clamped_start) >= to_ms(duration) {
            return Err(StoreError::InvalidRange(format!(
                "In ({}) is at or past the end of the source ({}). Set In before the end of the video.",
                format_timecode(start),
                format_timecode(duration)
            )));
        }
        let end = round_ms(end.clamp(0.0, duration));
        if to_ms(end) <= to_ms(clamped_start) {
            return Err(StoreError::InvalidRange(format!(
                "Out ({}) must be after In ({}).",
                format_timecode(end),
                format_timecode(clamped_start)
            )));
        }
        let min = opts.min_duration();
        if to_ms(end) - to_ms(clamped_start) < to_ms(min) {
            return Err(StoreError::ClipTooShort { min });
        }
        Ok((clamped_start, end))
    }

    fn validate_tag(&self, tag_id: Option<&str>) -> Result<Option<String>, StoreError> {
        match tag_id.map(str::trim).filter(|t| !t.is_empty()) {
            None => Ok(None),
            Some(id) if self.project.tag(id).is_some() => Ok(Some(id.to_string())),
            Some(id) => Err(StoreError::UnknownTag(id.to_string())),
        }
    }

    fn edited_fields(
        &self,
        before: &ClipFields,
        edit: &ClipEdit,
        opts: &StoreOptions,
    ) -> Result<ClipFields, StoreError> {
        let mut f = before.clone();
        match edit {
            ClipEdit::Rename { name } => f.name = name.trim().to_string(),
            ClipEdit::Retime { start_sec, end_sec } => {
                if start_sec.is_some() || end_sec.is_some() {
                    let start = start_sec.unwrap_or(before.start_sec);
                    let end = end_sec.unwrap_or(before.end_sec);
                    (f.start_sec, f.end_sec) = self.validate_range(start, end, opts)?;
                }
            }
            ClipEdit::SetTag { tag_id } => f.tag_id = self.validate_tag(tag_id.as_deref())?,
            ClipEdit::SetCrop { crop_x_offset } => {
                f.crop_x_offset = normalize_crop(*crop_x_offset)?
            }
            ClipEdit::SetIncluded { included } => f.selected_for_export = *included,
        }
        Ok(f)
    }

    /// Lowest number that is >= `next_index` and above every existing number.
    fn first_free_index(&self) -> u32 {
        let max = self
            .project
            .clips
            .iter()
            .map(|c| c.index)
            .max()
            .unwrap_or(0);
        self.project.next_index.max(max.saturating_add(1)).max(1)
    }

    fn normalize_view(&self, view: ViewState) -> ViewState {
        let duration = self.source_duration();
        let playhead_sec = if view.playhead_sec.is_finite() {
            round_ms(view.playhead_sec.clamp(0.0, duration))
        } else {
            0.0
        };
        let (lo, hi) = ZOOM_WINDOW_RANGE_SEC;
        let zoom_window_sec = if view.zoom_window_sec.is_finite() {
            round_ms(view.zoom_window_sec.clamp(lo, hi))
        } else {
            ViewState::default().zoom_window_sec
        };
        let selected_clip_id = view
            .selected_clip_id
            .filter(|id| self.project.clip(*id).is_some());
        ViewState {
            playhead_sec,
            zoom_window_sec,
            selected_clip_id,
        }
    }

    fn out_of_range_notice(&self) -> Option<Notice> {
        let n = self.project.out_of_range_ids().len();
        (n > 0).then(|| {
            Notice::new(
                NoticeLevel::Warning,
                "out_of_range_clips",
                format!(
                    "{} {} at or after the end of the source ({}); they are marked Out of range and will be skipped by export.",
                    clips(n),
                    if n == 1 { "starts" } else { "start" },
                    format_timecode(self.source_duration())
                ),
            )
        })
    }

    fn record(&mut self, entry: Entry) {
        self.crop_chain = None;
        self.redo.clear();
        self.undo.push_back(entry);
        self.trim_history();
    }

    fn trim_history(&mut self) {
        while self.undo.len() > UNDO_LIMIT {
            self.undo.pop_front();
        }
    }

    fn mark_dirty(&mut self, urgency: Urgency) {
        self.save_pending = Some(match (self.save_pending, urgency) {
            (Some(Urgency::Immediate), _) | (_, Urgency::Immediate) => Urgency::Immediate,
            _ => Urgency::Debounced,
        });
    }

    fn commit(&mut self, change: Change, label: Option<String>, urgency: Urgency) -> StatePatch {
        self.revision += 1;
        self.project.updated_at = now_utc();
        self.mark_dirty(urgency);
        self.patch(change, label)
    }

    fn patch(&self, change: Change, label: Option<String>) -> StatePatch {
        StatePatch {
            revision: self.revision,
            upserted: change
                .upserted
                .iter()
                .filter_map(|id| self.project.clip(*id).cloned())
                .collect(),
            removed: change.removed,
            next_index: self.project.next_index,
            tags: change.tags.then(|| self.project.tags.clone()),
            export_settings: change
                .export_settings
                .then(|| self.project.export_settings.clone()),
            history: self.history(),
            label,
        }
    }

    /// Patch for a command that changed nothing: current revision, no upserts.
    fn noop_patch(&self) -> StatePatch {
        self.patch(Change::default(), None)
    }

    /// Remove clips by id; returns `(position before removal, clip)` ascending.
    fn remove_clips(&mut self, ids: &[Uuid]) -> Vec<(usize, Clip)> {
        let set: HashSet<Uuid> = ids.iter().copied().collect();
        let mut removed = Vec::new();
        let mut kept = Vec::with_capacity(self.project.clips.len());
        for (pos, clip) in std::mem::take(&mut self.project.clips)
            .into_iter()
            .enumerate()
        {
            if set.contains(&clip.id) {
                removed.push((pos, clip));
            } else {
                kept.push(clip);
            }
        }
        self.project.clips = kept;
        removed
    }

    /// Re-insert clips at their recorded positions (ascending order restores
    /// the exact original order).
    fn insert_clips(&mut self, removed: &[(usize, Clip)]) {
        for (pos, clip) in removed {
            let at = (*pos).min(self.project.clips.len());
            self.project.clips.insert(at, clip.clone());
        }
    }

    fn clear_selection_of(&mut self, ids: &[Uuid]) {
        if self
            .project
            .view_state
            .selected_clip_id
            .is_some_and(|s| ids.contains(&s))
        {
            self.project.view_state.selected_clip_id = None;
        }
    }

    fn set_clip_tags(&mut self, pairs: &[(Uuid, String)], dir: Direction) {
        for (id, tag) in pairs {
            if let Some(clip) = self.project.clip_mut(*id) {
                let mut fields = clip.fields();
                fields.tag_id = (dir == Direction::Undo).then(|| tag.clone());
                clip.set_fields(&fields);
            }
        }
    }

    fn apply_orphan_records(&mut self, ids: &[Uuid]) {
        for id in ids {
            if let Some(record) = self.orphan_records.remove(id) {
                let previous = self.exporting_prev.remove(id);
                if let Some(clip) = self.project.clips.iter_mut().find(|c| c.id == *id) {
                    apply_record(clip, &record, previous);
                }
            }
        }
    }

    /// Apply `op` in `dir`, updating stored clips with their current state
    /// when they leave the project.
    fn apply(&mut self, op: &mut Op, dir: Direction) -> Change {
        match op {
            Op::Add { clips } => {
                let ids: Vec<Uuid> = clips.iter().map(|c| c.id).collect();
                match dir {
                    Direction::Undo => {
                        *clips = self
                            .remove_clips(&ids)
                            .into_iter()
                            .map(|(_, c)| c)
                            .collect();
                        self.clear_selection_of(&ids);
                        Change::removed(ids)
                    }
                    Direction::Redo => {
                        self.project.clips.extend(clips.iter().cloned());
                        self.apply_orphan_records(&ids);
                        Change::upserted(ids)
                    }
                }
            }
            Op::Delete { removed } => {
                let ids: Vec<Uuid> = removed.iter().map(|(_, c)| c.id).collect();
                match dir {
                    Direction::Undo => {
                        self.insert_clips(removed);
                        self.apply_orphan_records(&ids);
                        Change::upserted(ids)
                    }
                    Direction::Redo => {
                        *removed = self.remove_clips(&ids);
                        self.clear_selection_of(&ids);
                        Change::removed(ids)
                    }
                }
            }
            Op::Edit { changes } => {
                for change in changes.iter() {
                    if let Some(clip) = self.project.clip_mut(change.id) {
                        clip.set_fields(if dir == Direction::Undo {
                            &change.before
                        } else {
                            &change.after
                        });
                    }
                }
                Change::upserted(changes.iter().map(|c| c.id).collect())
            }
            Op::Renumber { changes } => {
                for (id, old, new) in changes.iter() {
                    if let Some(clip) = self.project.clip_mut(*id) {
                        clip.index = if dir == Direction::Undo { *old } else { *new };
                    }
                }
                Change::upserted(changes.iter().map(|(id, ..)| *id).collect())
            }
            Op::Tags {
                before,
                after,
                cleared,
            } => {
                self.project.tags = if dir == Direction::Undo {
                    before.clone()
                } else {
                    after.clone()
                };
                self.set_clip_tags(cleared, dir);
                Change {
                    upserted: cleared.iter().map(|(id, _)| *id).collect(),
                    tags: true,
                    ..Change::default()
                }
            }
        }
    }
}

/// "Label" or "Label (N clips)".
fn edit_label(base: &str, n: usize) -> String {
    if n == 1 {
        base.to_string()
    } else {
        format!("{base} ({n} clips)")
    }
}

fn apply_record(clip: &mut Clip, record: &ExportRecord, previous: Option<ExportStatus>) {
    match &record.outcome {
        ClipOutcome::Done { output_path, .. } => {
            clip.last_export = Some(LastExport {
                output_path: Some(output_path.clone()),
                exported_at: record.exported_at,
                settings_hash: record.settings_hash.clone(),
                error: None,
                preset: Some(record.preset),
                content_hash: Some(record.content_hash.clone()),
                exit_code: None,
                log_tail: Vec::new(),
                log_path: None,
            });
            clip.export_status = if clip.content_hash(record.preset) == record.content_hash {
                ExportStatus::Completed
            } else {
                ExportStatus::Outdated
            };
        }
        ClipOutcome::Failed {
            exit_code,
            message,
            log_tail,
            log_path,
        } => {
            let output_path = clip
                .last_export
                .as_ref()
                .and_then(|le| le.output_path.clone());
            clip.last_export = Some(LastExport {
                output_path,
                exported_at: record.exported_at,
                settings_hash: record.settings_hash.clone(),
                error: Some(message.clone()),
                preset: Some(record.preset),
                content_hash: Some(record.content_hash.clone()),
                exit_code: *exit_code,
                log_tail: log_tail.clone(),
                log_path: log_path.clone(),
            });
            clip.export_status = ExportStatus::Failed;
        }
        ClipOutcome::Cancelled | ClipOutcome::Skipped { .. } => {
            let restored = previous.unwrap_or(clip.export_status);
            clip.export_status = if restored == ExportStatus::Exporting {
                ExportStatus::Pending
            } else {
                restored
            };
            clip.refresh_status();
        }
    }
}

fn new_clip(index: u32, name: &str, start_sec: f64, end_sec: f64, tag_id: Option<String>) -> Clip {
    Clip {
        id: Uuid::now_v7(),
        index,
        name: name.trim().to_string(),
        start_sec,
        end_sec,
        tag_id,
        crop_x_offset: 0.0,
        selected_for_export: true,
        export_status: ExportStatus::Pending,
        last_export: None,
    }
}

/// Clamp to `[-1, 1]` with 1e-4 precision (the precision of `content_hash`).
fn normalize_crop(value: f64) -> Result<f64, StoreError> {
    if !value.is_finite() {
        return Err(StoreError::InvalidInput(
            "The crop position must be a number between -1 (left edge) and 1 (right edge).".into(),
        ));
    }
    Ok((value.clamp(-1.0, 1.0) * 10_000.0).round() / 10_000.0 + 0.0)
}

fn normalize_tags(tags: Vec<Tag>) -> Result<Vec<Tag>, StoreError> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(tags.len());
    for tag in tags {
        let id = tag.id.trim().to_string();
        if id.is_empty() {
            return Err(StoreError::InvalidInput(
                "Every tag needs a non-empty id.".into(),
            ));
        }
        let label = tag.label.trim().to_string();
        if label.is_empty() {
            return Err(StoreError::InvalidInput(format!(
                "Tag \"{id}\" needs a label: tags always show a label, never colour alone."
            )));
        }
        let color = tag.color.trim();
        if !is_hex_color(color) {
            return Err(StoreError::InvalidInput(format!(
                "Tag \"{label}\" has an invalid colour \"{color}\". Use the #RRGGBB form, e.g. #EF4444."
            )));
        }
        if !seen.insert(id.clone()) {
            return Err(StoreError::InvalidInput(format!(
                "The tag id \"{id}\" is used twice; tag ids must be unique."
            )));
        }
        out.push(Tag {
            id,
            label,
            color: color.to_ascii_uppercase(),
        });
    }
    Ok(out)
}

/// `#RRGGBB`.
pub(crate) fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

fn dedupe(ids: &[Uuid]) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    ids.iter().copied().filter(|id| seen.insert(*id)).collect()
}

fn too_many_clips() -> StoreError {
    StoreError::InvalidInput(
        "This project has run out of clip numbers. Renumber the clips chronologically first."
            .into(),
    )
}

fn clips(n: usize) -> String {
    if n == 1 {
        "1 clip".into()
    } else {
        format!("{n} clips")
    }
}

fn were(n: usize) -> &'static str {
    if n == 1 { "was" } else { "were" }
}
