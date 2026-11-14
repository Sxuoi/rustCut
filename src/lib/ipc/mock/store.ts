// In-memory port of crates/rustcut-project `ProjectStore` (PRD §4.2 MRK, §4.3
// QUE, ARCHITECTURE §5 rustcut-project). Pure and synchronous: it validates,
// mutates, records undo steps and returns `StatePatch`es; the MockBackend
// decides when to emit them and when to save.
//
// Contract (also exercised by store.test.ts):
//  * every change bumps `revision` by exactly one and yields one StatePatch;
//  * a command that changes nothing returns a patch with the CURRENT revision
//    and no upserts;
//  * clip numbers (`index`) come from the monotonic `next_index` and are never
//    reused, not even after undo (QUE-02);
//  * undo/redo restore user fields (ClipFields) or whole clips (add/delete);
//    export status is system state and is recomputed (Clip::after_user_edit).

import type {
  AddClipRequest,
  AddClipResult,
  Clip,
  ClipEdit,
  ClipFields,
  ClipOutcome,
  EditClipsRequest,
  ExportRecord,
  ExportSettings,
  ExportStatus,
  HistoryState,
  ImportedClip,
  Preset,
  Project,
  ProjectSnapshot,
  ReadOnlyReason,
  SaveState,
  SourceStatus,
  StatePatch,
  Tag,
  ViewState,
} from "$lib/bindings";
import { formatTimecode, roundMs } from "$lib/util/time";
import {
  UNDO_LIMIT,
  ZOOM_WINDOW_RANGE_SEC,
  defaultViewState,
  normalizeExportSettings,
} from "./model";
import { clamp, clone, fail, formatSeconds, hash32, newId, roundAway, toMs } from "./util";

// ---------------------------------------------------------------- clip logic

/** `Clip::fields`. */
export function clipFields(c: Clip): ClipFields {
  return {
    name: c.name,
    start_sec: c.start_sec,
    end_sec: c.end_sec,
    tag_id: c.tag_id,
    crop_x_offset: c.crop_x_offset,
    selected_for_export: c.selected_for_export,
  };
}

/** `Clip::content_hash`: what determines an export's content and file name. */
export function contentHash(c: Clip, preset: Preset): string {
  const crop = preset === "vcrop" ? roundAway(c.crop_x_offset * 10_000) : 0;
  return hash32(`${toMs(c.start_sec)}|${toMs(c.end_sec)}|${crop}|${c.name}|${preset}`);
}

/** `Clip::refresh_status`: Completed <-> Outdated from the stored hash. */
export function refreshStatus(c: Clip): void {
  const le = c.last_export;
  if (!le || !le.preset || !le.content_hash) return;
  const current = contentHash(c, le.preset);
  if (c.export_status === "completed" && current !== le.content_hash) c.export_status = "outdated";
  else if (c.export_status === "outdated" && current === le.content_hash) c.export_status = "completed";
}

/** `Clip::after_user_edit` (EXP-01 / QUE-05). */
export function afterUserEdit(c: Clip, before: ClipFields): void {
  const le = c.last_export;
  if (le && le.preset && le.content_hash) {
    refreshStatus(c);
  } else if (c.export_status === "completed") {
    const changed =
      toMs(before.start_sec) !== toMs(c.start_sec) ||
      toMs(before.end_sec) !== toMs(c.end_sec) ||
      before.name !== c.name ||
      Math.abs(before.crop_x_offset - c.crop_x_offset) > 1e-9;
    if (changed) c.export_status = "outdated";
  }
}

/** `Clip::set_fields`. */
export function setFields(c: Clip, f: ClipFields): void {
  const before = clipFields(c);
  c.name = f.name;
  c.start_sec = f.start_sec;
  c.end_sec = f.end_sec;
  c.tag_id = f.tag_id;
  c.crop_x_offset = f.crop_x_offset;
  c.selected_for_export = f.selected_for_export;
  afterUserEdit(c, before);
}

function sameFields(a: ClipFields, b: ClipFields): boolean {
  return (
    a.name === b.name &&
    a.start_sec === b.start_sec &&
    a.end_sec === b.end_sec &&
    a.tag_id === b.tag_id &&
    a.crop_x_offset === b.crop_x_offset &&
    a.selected_for_export === b.selected_for_export
  );
}

/** `Project::index_width`: padding of `{index}`. */
export function indexWidth(p: Project): number {
  const max = p.clips.reduce((m, c) => Math.max(m, c.index), 1);
  return Math.max(2, String(max).length);
}

/** `Project::duplicate_of` (MRK-03). */
export function duplicateOf(p: Project, start: number, end: number, excludeId: string | null = null): number | null {
  const [s, e] = [toMs(start), toMs(end)];
  const hit = p.clips.find((c) => c.id !== excludeId && toMs(c.start_sec) === s && toMs(c.end_sec) === e);
  return hit ? hit.index : null;
}

/** `Project::out_of_range_ids`: clips starting at or after the source end. */
export function outOfRangeIds(p: Project): string[] {
  const end = toMs(p.source.duration_sec);
  return p.clips.filter((c) => toMs(c.start_sec) >= end).map((c) => c.id);
}

/** PRJ-03 load normalisation: Exporting -> Pending, statuses recomputed, numbers repaired. */
export function normalizeOnLoad(p: Project): void {
  const seenIds = new Set<string>();
  const seenIdx = new Set<number>();
  let max = 0;
  for (const c of p.clips) max = Math.max(max, c.index);
  for (const c of p.clips) {
    if (seenIds.has(c.id)) c.id = newId();
    seenIds.add(c.id);
    if (seenIdx.has(c.index)) c.index = ++max;
    seenIdx.add(c.index);
    if (c.export_status === "exporting") c.export_status = "pending";
    if (c.tag_id !== null && !p.tags.some((t) => t.id === c.tag_id)) c.tag_id = null;
    refreshStatus(c);
  }
  p.next_index = Math.max(p.next_index, max + 1, 1);
}

// ---------------------------------------------------------------- undo model

interface PresenceItem {
  pos: number;
  clip: Clip;
}

type Op =
  /** `present`: whether the clips are in the project now. Undo and redo both toggle. */
  | { kind: "presence"; items: PresenceItem[]; present: boolean }
  | { kind: "fields"; changes: { id: string; before: ClipFields; after: ClipFields }[] }
  | { kind: "renumber"; changes: { id: string; before: number; after: number }[] }
  | { kind: "tags"; before: Tag[]; after: Tag[]; cleared: { id: string; tag: string }[] };

interface Entry {
  label: string;
  op: Op;
}

interface Change {
  upserted: string[];
  removed: string[];
  tags: boolean;
  settings: boolean;
}

const noChange = (): Change => ({ upserted: [], removed: [], tags: false, settings: false });

export interface StoreOptions {
  /** Current minimum clip duration setting (MRK-01), seconds. */
  minClipSec: () => number;
  /** Monotonic milliseconds for crop undo coalescing; injectable without sleeping in tests. */
  now?: () => number;
}

/** Result of `importClips` (EXP-17). */
export interface ImportResult {
  patch: StatePatch;
  added: string[];
  skipped: ImportedClip[];
}

export class MockProjectStore {
  project: Project;
  revision: number;
  readOnly: ReadOnlyReason | null;
  private undoStack: Entry[] = [];
  private redoStack: Entry[] = [];
  private cropChain: { ids: string[]; at: number } | null = null;
  private readonly exportingPrev = new Map<string, ExportStatus>();

  constructor(
    project: Project,
    private readonly opts: StoreOptions,
    readOnly: ReadOnlyReason | null = null,
    revision = 1,
  ) {
    this.project = project;
    this.readOnly = readOnly;
    this.revision = revision;
  }

  // ------------------------------------------------------------- read side

  /** Undo/redo availability and labels ("Undo Add clip"). */
  history(): HistoryState {
    const u = this.undoStack[this.undoStack.length - 1];
    const r = this.redoStack[this.redoStack.length - 1];
    return {
      can_undo: !!u,
      can_redo: !!r,
      undo_label: u ? `Undo ${u.label}` : null,
      redo_label: r ? `Redo ${r.label}` : null,
    };
  }

  undoDepth(): number {
    return this.undoStack.length;
  }

  clip(id: string): Clip | undefined {
    return this.project.clips.find((c) => c.id === id);
  }

  snapshot(save: SaveState, sourceStatus: SourceStatus): ProjectSnapshot {
    return {
      revision: this.revision,
      project: clone(this.project),
      history: this.history(),
      save: clone(save),
      read_only: this.readOnly ? clone(this.readOnly) : null,
      source_status: clone(sourceStatus),
    };
  }

  // ------------------------------------------------------------- commands

  /** MRK-01 / MRK-03 / QUE-02. */
  addClip(req: AddClipRequest): AddClipResult {
    this.ensureWritable();
    const [start, end] = this.validateRange(req.start_sec, req.end_sec);
    const tagId = this.validateTag(req.tag_id);
    const index = this.firstFreeIndex();
    const dup = duplicateOf(this.project, start, end);
    const clip = newClip(index, req.name ?? "", start, end, tagId);
    this.project.next_index = index + 1;
    this.project.clips.push(clip);
    this.record({
      label: "Add clip",
      op: { kind: "presence", items: [{ pos: this.project.clips.length - 1, clip: clone(clip) }], present: true },
    });
    const patch = this.commit({ ...noChange(), upserted: [clip.id] });
    return { clip: clone(clip), patch, duplicate_of: dup };
  }

  /** QUE-07: one undo step for all clips; all validated before any change. */
  editClips(ids: string[], edit: ClipEdit): StatePatch {
    return this.editBatch([{ ids, edit }], editLabel(edit), edit.kind === "set_crop" ? ids : undefined);
  }

  /** QUE-04: validate the complete action before mutating; one history entry and patch. */
  editClipsBatch(batch: EditClipsRequest[]): StatePatch {
    return this.editBatch(batch);
  }

  private editBatch(batch: EditClipsRequest[], labelOverride?: string, cropIds?: string[]): StatePatch {
    this.ensureWritable();
    const working = new Map<string, { id: string; before: ClipFields; after: ClipFields }>();
    for (const { ids, edit } of batch) {
      for (const id of dedupe(ids)) {
        let change = working.get(id);
        if (!change) {
          const clip = this.clip(id);
          if (!clip) throw clipNotFound();
          const before = clipFields(clip);
          change = { id, before, after: before };
          working.set(id, change);
        }
        change.after = this.editedFields(change.after, edit);
      }
    }
    const changes = [...working.values()].filter((c) => !sameFields(c.before, c.after));
    if (changes.length === 0) return this.noopPatch();
    for (const ch of changes) {
      const clip = this.clip(ch.id);
      if (clip) setFields(clip, ch.after);
    }
    if (cropIds && this.coalesceCrop(cropIds, changes)) {
      return this.commit({ ...noChange(), upserted: changes.map((c) => c.id) });
    }
    const first = batch[0] ? editLabel(batch[0].edit) : "Edit clips";
    const base = labelOverride ?? (batch.every((r) => r.edit.kind === "set_included")
      ? "Change include"
      : batch.every((r) => editLabel(r.edit) === first) ? first : "Edit clips");
    this.record({
      label: changes.length === 1 ? base : `${base} (${changes.length} clips)`,
      op: { kind: "fields", changes },
    });
    if (cropIds) this.cropChain = { ids: dedupe(cropIds).sort(), at: this.now() };
    return this.commit({ ...noChange(), upserted: changes.map((c) => c.id) });
  }

  deleteClips(ids: string[]): StatePatch {
    this.ensureWritable();
    const unique = dedupe(ids);
    if (unique.some((id) => !this.clip(id))) throw clipNotFound();
    if (unique.length === 0) return this.noopPatch();
    const items = this.removeClips(unique);
    this.clearSelectionOf(unique);
    this.record({
      label: unique.length === 1 ? "Delete clip" : `Delete ${unique.length} clips`,
      op: { kind: "presence", items, present: false },
    });
    return this.commit({ ...noChange(), removed: unique });
  }

  /** QUE-02 (P1): renumber 1..n by In, then Out, then old number; `next_index` is kept. */
  renumberClips(): StatePatch {
    this.ensureWritable();
    const order = [...this.project.clips].sort(
      (a, b) =>
        toMs(a.start_sec) - toMs(b.start_sec) || toMs(a.end_sec) - toMs(b.end_sec) || a.index - b.index,
    );
    const changes: { id: string; before: number; after: number }[] = [];
    order.forEach((c, rank) => {
      if (c.index !== rank + 1) changes.push({ id: c.id, before: c.index, after: rank + 1 });
    });
    if (changes.length === 0) return this.noopPatch();
    for (const ch of changes) {
      const c = this.clip(ch.id);
      if (c) c.index = ch.after;
    }
    this.project.next_index = Math.max(this.project.next_index, this.project.clips.length + 1);
    this.record({ label: "Renumber clips", op: { kind: "renumber", changes } });
    return this.commit({ ...noChange(), upserted: changes.map((c) => c.id) });
  }

  /** MRK-06: validated palette; removed tags are cleared from clips in the same step. */
  setTags(tags: Tag[]): StatePatch {
    this.ensureWritable();
    const next = normalizeTags(tags);
    if (JSON.stringify(next) === JSON.stringify(this.project.tags)) return this.noopPatch();
    const kept = new Set(next.map((t) => t.id));
    const cleared = this.project.clips
      .filter((c) => c.tag_id !== null && !kept.has(c.tag_id))
      .map((c) => ({ id: c.id, tag: c.tag_id as string }));
    const before = this.project.tags;
    this.project.tags = next;
    for (const c of cleared) {
      const clip = this.clip(c.id);
      if (clip) clip.tag_id = null;
    }
    this.record({ label: "Edit tags", op: { kind: "tags", before, after: clone(next), cleared } });
    return this.commit({ ...noChange(), upserted: cleared.map((c) => c.id), tags: true });
  }

  /** Normalised; not undoable. */
  setExportSettings(settings: ExportSettings): StatePatch {
    this.ensureWritable();
    const next = normalizeExportSettings(settings);
    if (JSON.stringify(next) === JSON.stringify(this.project.export_settings)) return this.noopPatch();
    this.project.export_settings = next;
    return this.commit({ ...noChange(), settings: true });
  }

  /** PRJ-03: clamped, not undoable, no patch, no revision. Returns false when ignored. */
  setViewState(view: ViewState): boolean {
    if (this.readOnly) return false;
    const duration = this.sourceDuration();
    const [lo, hi] = ZOOM_WINDOW_RANGE_SEC;
    const playhead = Number.isFinite(view.playhead_sec) ? roundMs(clamp(view.playhead_sec, 0, duration)) : 0;
    const zoom = Number.isFinite(view.zoom_window_sec)
      ? roundMs(clamp(view.zoom_window_sec, lo, hi))
      : defaultViewState().zoom_window_sec;
    const selected = view.selected_clip_id && this.clip(view.selected_clip_id) ? view.selected_clip_id : null;
    this.project.view_state = { playhead_sec: playhead, zoom_window_sec: zoom, selected_clip_id: selected };
    return true;
  }

  /** EXP-17: valid items become clips in one undo step; the rest are returned as skipped. */
  importClips(items: ImportedClip[]): ImportResult {
    this.ensureWritable();
    const valid: { name: string; start: number; end: number }[] = [];
    const skipped: ImportedClip[] = [];
    for (const item of items) {
      if (item.error !== null) {
        skipped.push(item);
        continue;
      }
      try {
        const [start, end] = this.validateRange(item.start_sec, item.end_sec);
        valid.push({ name: item.name, start, end });
      } catch (e) {
        skipped.push({ ...item, error: e instanceof Error ? e.message : String(e) });
      }
    }
    if (valid.length === 0) return { patch: this.noopPatch(), added: [], skipped };
    const first = this.firstFreeIndex();
    const clips = valid.map((v, i) => newClip(first + i, v.name, v.start, v.end, null));
    this.project.next_index = first + clips.length;
    const base = this.project.clips.length;
    this.project.clips.push(...clips);
    this.record({
      label: clips.length === 1 ? "Import clip" : `Import ${clips.length} clips`,
      op: { kind: "presence", items: clips.map((c, i) => ({ pos: base + i, clip: clone(c) })), present: true },
    });
    const added = clips.map((c) => c.id);
    return { patch: this.commit({ ...noChange(), upserted: added }), added, skipped };
  }

  /** MRK-04. `null` when there is nothing to undo. */
  undo(): StatePatch | null {
    if (this.readOnly) return null;
    this.cropChain = null;
    const entry = this.undoStack.pop();
    if (!entry) return null;
    const change = this.apply(entry.op, "undo");
    this.redoStack.push(entry);
    return this.commit(change, `Undo ${entry.label}`);
  }

  redo(): StatePatch | null {
    if (this.readOnly) return null;
    this.cropChain = null;
    const entry = this.redoStack.pop();
    if (!entry) return null;
    const change = this.apply(entry.op, "redo");
    this.undoStack.push(entry);
    return this.commit(change, `Redo ${entry.label}`);
  }

  // --------------------------------------------------- system (not undoable)

  /** Engine marks the clips of a batch as Exporting. */
  markExporting(ids: string[]): StatePatch {
    const changed: string[] = [];
    for (const id of dedupe(ids)) {
      const clip = this.clip(id);
      if (clip && clip.export_status !== "exporting") {
        this.exportingPrev.set(id, clip.export_status);
        clip.export_status = "exporting";
        changed.push(id);
      }
    }
    return changed.length === 0 ? this.noopPatch() : this.commit({ ...noChange(), upserted: changed });
  }

  /** QUE-05 / EXP-10: apply a finished clip's result. `null` when the clip was deleted meanwhile. */
  applyExportRecord(record: ExportRecord): StatePatch | null {
    const previous = this.exportingPrev.get(record.clip_id);
    this.exportingPrev.delete(record.clip_id);
    const clip = this.clip(record.clip_id);
    if (!clip) return null;
    applyRecord(clip, record, previous);
    return this.commit({ ...noChange(), upserted: [clip.id] });
  }

  /** Replace the source after a relink (PRJ-04); statuses are recomputed by the caller's snapshot. */
  bumpForSnapshot(): void {
    this.revision += 1;
  }

  // -------------------------------------------------------------- internals

  private ensureWritable(): void {
    if (this.readOnly) throw fail("read_only", readOnlyMessage(this.readOnly));
  }

  private sourceDuration(): number {
    const d = this.project.source.duration_sec;
    return Number.isFinite(d) && d > 0 ? roundMs(d) : 0;
  }

  /** MRK-01 range rules; returns the clamped, ms-rounded `[start, end]`. */
  private validateRange(start: number, end: number): [number, number] {
    if (!Number.isFinite(start) || !Number.isFinite(end)) {
      throw fail("invalid_range", "Clip times must be numbers of seconds.");
    }
    const duration = this.sourceDuration();
    const s = roundMs(clamp(start, 0, duration + 1));
    if (toMs(s) >= toMs(duration)) {
      throw fail(
        "invalid_range",
        `In (${formatTimecode(start)}) is at or past the end of the source (${formatTimecode(duration)}). Set In before the end of the video.`,
      );
    }
    const e = roundMs(clamp(end, 0, duration));
    if (toMs(e) <= toMs(s)) {
      throw fail("invalid_range", `Out (${formatTimecode(e)}) must be after In (${formatTimecode(s)}).`);
    }
    const min = this.minDuration();
    if (toMs(e) - toMs(s) < toMs(min)) {
      throw fail("clip_too_short", `Clips must be at least ${formatSeconds(min)} s long. Move In and Out further apart.`);
    }
    return [s, e];
  }

  private minDuration(): number {
    const m = this.opts.minClipSec();
    return Number.isFinite(m) ? clamp(m, 0.1, 5) : 0.5;
  }

  private validateTag(tagId: string | null | undefined): string | null {
    const id = tagId?.trim();
    if (!id) return null;
    if (this.project.tags.some((t) => t.id === id)) return id;
    throw fail(
      "invalid_input",
      `The tag "${id}" is not in this project's palette. Choose an existing tag or add it to the palette first.`,
    );
  }

  private editedFields(before: ClipFields, edit: ClipEdit): ClipFields {
    const f = { ...before };
    switch (edit.kind) {
      case "rename":
        f.name = edit.name.trim();
        break;
      case "retime":
        if (edit.start_sec !== null || edit.end_sec !== null) {
          const s = edit.start_sec ?? before.start_sec;
          const e = edit.end_sec ?? before.end_sec;
          [f.start_sec, f.end_sec] = this.validateRange(s, e);
        }
        break;
      case "set_tag":
        f.tag_id = this.validateTag(edit.tag_id);
        break;
      case "set_crop":
        f.crop_x_offset = normalizeCrop(edit.crop_x_offset);
        break;
      case "set_included":
        f.selected_for_export = edit.included;
        break;
    }
    return f;
  }

  /** Lowest number >= `next_index` and above every existing number. */
  private firstFreeIndex(): number {
    const max = this.project.clips.reduce((m, c) => Math.max(m, c.index), 0);
    return Math.max(this.project.next_index, max + 1, 1);
  }

  private record(entry: Entry): void {
    this.cropChain = null;
    this.undoStack.push(entry);
    this.redoStack = [];
    while (this.undoStack.length > UNDO_LIMIT) this.undoStack.shift();
  }

  private now(): number {
    return this.opts.now ? this.opts.now() : performance.now();
  }

  /** MRK-04: match Rust's sliding 1.5 s window, first before and latest after. */
  private coalesceCrop(requested: string[], changes: Extract<Op, { kind: "fields" }>["changes"]): boolean {
    const at = this.now();
    const ids = dedupe(requested).sort();
    const chain = this.cropChain;
    if (!chain || Math.max(0, at - chain.at) > 1500 ||
      ids.length !== chain.ids.length || ids.some((id, i) => id !== chain.ids[i])) return false;
    const op = this.undoStack.at(-1)?.op;
    if (op?.kind !== "fields") return false;
    for (const change of changes) {
      const previous = op.changes.find((c) => c.id === change.id);
      if (previous) previous.after = change.after;
      else op.changes.push(change);
    }
    this.cropChain = { ids, at };
    return true;
  }

  private removeClips(ids: string[]): PresenceItem[] {
    const set = new Set(ids);
    const removed: PresenceItem[] = [];
    this.project.clips.forEach((clip, pos) => {
      if (set.has(clip.id)) removed.push({ pos, clip: clone(clip) });
    });
    this.project.clips = this.project.clips.filter((c) => !set.has(c.id));
    return removed;
  }

  private insertClips(items: PresenceItem[]): void {
    for (const { pos, clip } of [...items].sort((a, b) => a.pos - b.pos)) {
      const c = clone(clip);
      refreshStatus(c);
      this.project.clips.splice(Math.min(pos, this.project.clips.length), 0, c);
      this.project.next_index = Math.max(this.project.next_index, c.index + 1);
    }
  }

  private clearSelectionOf(ids: string[]): void {
    const sel = this.project.view_state.selected_clip_id;
    if (sel && ids.includes(sel)) this.project.view_state.selected_clip_id = null;
  }

  private apply(op: Op, dir: "undo" | "redo"): Change {
    const change = noChange();
    switch (op.kind) {
      case "presence": {
        const ids = op.items.map((i) => i.clip.id);
        if (op.present) {
          // Capture the clips as they are now so a later redo restores them exactly.
          op.items = this.removeClips(ids);
          op.present = false;
          this.clearSelectionOf(ids);
          change.removed = ids;
        } else {
          this.insertClips(op.items);
          op.present = true;
          change.upserted = ids;
        }
        break;
      }
      case "fields":
        for (const ch of op.changes) {
          const clip = this.clip(ch.id);
          if (!clip) continue;
          setFields(clip, dir === "undo" ? ch.before : ch.after);
          change.upserted.push(ch.id);
        }
        break;
      case "renumber":
        for (const ch of op.changes) {
          const clip = this.clip(ch.id);
          if (!clip) continue;
          clip.index = dir === "undo" ? ch.before : ch.after;
          change.upserted.push(ch.id);
        }
        break;
      case "tags":
        this.project.tags = clone(dir === "undo" ? op.before : op.after);
        for (const c of op.cleared) {
          const clip = this.clip(c.id);
          if (!clip) continue;
          clip.tag_id = dir === "undo" ? c.tag : null;
          change.upserted.push(c.id);
        }
        change.tags = true;
        break;
    }
    return change;
  }

  private noopPatch(): StatePatch {
    return this.buildPatch(noChange(), null);
  }

  private commit(change: Change, label: string | null = null): StatePatch {
    this.revision += 1;
    this.project.updated_at = new Date().toISOString();
    return this.buildPatch(change, label);
  }

  private buildPatch(change: Change, label: string | null): StatePatch {
    const upsert = new Set(change.upserted);
    return {
      revision: this.revision,
      upserted: this.project.clips.filter((c) => upsert.has(c.id)).map((c) => clone(c)),
      removed: [...change.removed],
      next_index: this.project.next_index,
      tags: change.tags ? clone(this.project.tags) : null,
      export_settings: change.settings ? clone(this.project.export_settings) : null,
      history: this.history(),
      label,
    };
  }
}

// ------------------------------------------------------------------ helpers

function newClip(index: number, name: string, start: number, end: number, tagId: string | null): Clip {
  return {
    id: newId(),
    index,
    name: name.trim(),
    start_sec: start,
    end_sec: end,
    tag_id: tagId,
    crop_x_offset: 0,
    selected_for_export: true,
    export_status: "pending",
    last_export: null,
  };
}

function dedupe(ids: string[]): string[] {
  return [...new Set(ids)];
}

function clipNotFound() {
  return fail(
    "not_found",
    "That clip no longer exists (it may have been deleted or undone). Refresh the queue and try again.",
  );
}

/** Clamp to [-1, 1] with 1e-4 precision (the precision of `content_hash`). */
function normalizeCrop(v: number): number {
  if (!Number.isFinite(v)) {
    throw fail("invalid_input", "The crop position must be a number between -1 (left edge) and 1 (right edge).");
  }
  return roundAway(clamp(v, -1, 1) * 10_000) / 10_000 + 0;
}

function editLabel(edit: ClipEdit): string {
  switch (edit.kind) {
    case "rename":
      return "Rename clip";
    case "retime":
      return "Change clip times";
    case "set_tag":
      return "Change tag";
    case "set_crop":
      return "Change crop position";
    case "set_included":
      return "Toggle include";
  }
}

/** Human message for each read-only reason (PRJ-05, PRJ-06, source missing). */
export function readOnlyMessage(r: ReadOnlyReason): string {
  switch (r.kind) {
    case "unwritable":
      return `RustCut cannot write to the project folder (${r.path}). Copy the project to a writable folder to edit it.`;
    case "open_elsewhere":
      return `This project is open in another RustCut window${r.holder ? ` (${r.holder})` : ""}. Close it there to edit here; until then it is read-only.`;
    case "newer_schema":
      return `This project was saved by a newer RustCut (format ${r.found}; this version supports ${r.supported}). Update RustCut to edit it; until then it is read-only.`;
    case "source_missing":
      return "The source video is missing, so the queue is read-only. Use Locate… to find the file and relink it.";
  }
}

const HEX_COLOR = /^#[0-9a-fA-F]{6}$/;

function normalizeTags(tags: Tag[]): Tag[] {
  const seen = new Set<string>();
  return tags.map((tag) => {
    const id = tag.id.trim();
    if (!id) throw fail("invalid_input", "Every tag needs a non-empty id.");
    const label = tag.label.trim();
    if (!label) {
      throw fail("invalid_input", `Tag "${id}" needs a label: tags always show a label, never colour alone.`);
    }
    const color = tag.color.trim();
    if (!HEX_COLOR.test(color)) {
      throw fail("invalid_input", `Tag "${label}" has an invalid colour "${color}". Use the #RRGGBB form, e.g. #EF4444.`);
    }
    if (seen.has(id)) throw fail("invalid_input", `The tag id "${id}" is used twice; tag ids must be unique.`);
    seen.add(id);
    return { id, label, color: color.toUpperCase() };
  });
}

function applyRecord(clip: Clip, record: ExportRecord, previous: ExportStatus | undefined): void {
  const o: ClipOutcome = record.outcome;
  if (o.outcome === "done") {
    clip.last_export = {
      output_path: o.output_path,
      exported_at: record.exported_at,
      settings_hash: record.settings_hash,
      error: null,
      preset: record.preset,
      content_hash: record.content_hash,
      exit_code: null,
      log_tail: [],
      log_path: null,
    };
    clip.export_status = contentHash(clip, record.preset) === record.content_hash ? "completed" : "outdated";
  } else if (o.outcome === "failed") {
    clip.last_export = {
      output_path: clip.last_export?.output_path ?? null,
      exported_at: record.exported_at,
      settings_hash: record.settings_hash,
      error: o.message,
      preset: record.preset,
      content_hash: record.content_hash,
      exit_code: o.exit_code,
      log_tail: [...o.log_tail],
      log_path: o.log_path,
    };
    clip.export_status = "failed";
  } else {
    const restored = previous ?? clip.export_status;
    clip.export_status = restored === "exporting" ? "pending" : restored;
    refreshStatus(clip);
  }
}
