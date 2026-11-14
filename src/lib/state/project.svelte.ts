// Mirror of the Rust-owned project state (PRD §7.2: Rust owns state; the UI
// sends commands and renders the patches it gets back).
// OWNER: fe-shell. Public API is shared by every component; extend, don't break.
//
// Patch rule (docs/ARCHITECTURE.md §IPC): every change has revision = prev + 1
// and arrives both as the command's return value and as AppEvent::Patch.
// Apply iff revision === current + 1; ignore older; on a gap, refetch.

import { SvelteSet } from "svelte/reactivity";
import type {
  AddClipRequest,
  AddClipResult,
  Clip,
  ClipEdit,
  EditClipsRequest,
  ExportSettings,
  HistoryState,
  ProjectSnapshot,
  SaveState,
  StatePatch,
  Tag,
  ViewState,
} from "$lib/bindings";
import { t } from "$lib/i18n";
import { backend, BackendError } from "$lib/ipc";
import { formatIndex } from "$lib/util/time";
import { ui } from "./ui.svelte";

const EMPTY_HISTORY: HistoryState = { can_undo: false, can_redo: false, undo_label: null, redo_label: null };
const VIEW_STATE_THROTTLE_MS = 1000;

function sameView(a: ViewState, b: ViewState | null): boolean {
  return (
    b !== null &&
    Math.abs(a.playhead_sec - b.playhead_sec) < 0.0005 &&
    a.zoom_window_sec === b.zoom_window_sec &&
    a.selected_clip_id === b.selected_clip_id
  );
}

function byIndex(a: Clip, b: Clip): number {
  return a.index - b.index;
}

export class ProjectState {
  /** Whole state; replaced (never mutated) on every change. */
  snapshot = $state.raw<ProjectSnapshot | null>(null);

  /** Multi-selection of clip ids (QUE-04, QUE-07). */
  selection = new SvelteSet<string>();
  /** Primary selected clip: target of N, P, Alt+I/O, crop nudges. */
  primaryId = $state<string | null>(null);

  readonly loaded = $derived(this.snapshot !== null);
  readonly project = $derived(this.snapshot?.project ?? null);
  /** Clips sorted by stable index. */
  readonly clips = $derived<Clip[]>(this.snapshot?.project.clips ?? []);
  readonly tags = $derived<Tag[]>(this.snapshot?.project.tags ?? []);
  readonly settings = $derived<ExportSettings | null>(this.snapshot?.project.export_settings ?? null);
  readonly history = $derived<HistoryState>(this.snapshot?.history ?? EMPTY_HISTORY);
  readonly save = $derived<SaveState | null>(this.snapshot?.save ?? null);
  readonly readOnly = $derived(this.snapshot?.read_only ?? null);
  readonly source = $derived(this.snapshot?.project.source ?? null);
  readonly primary = $derived<Clip | null>(
    this.primaryId ? (this.snapshot?.project.clips.find((c) => c.id === this.primaryId) ?? null) : null,
  );

  private refetching = false;

  get revision(): number {
    return this.snapshot?.revision ?? 0;
  }

  clip(id: string): Clip | undefined {
    return this.snapshot?.project.clips.find((c) => c.id === id);
  }

  tag(id: string | null): Tag | undefined {
    return id ? this.tags.find((t) => t.id === id) : undefined;
  }

  load(s: ProjectSnapshot): void {
    const clips = [...s.project.clips].sort(byIndex);
    this.snapshot = { ...s, project: { ...s.project, clips } };
    this.selection.clear();
    const sel = s.project.view_state.selected_clip_id;
    this.primaryId = sel && clips.some((c) => c.id === sel) ? sel : null;
    if (this.primaryId) this.selection.add(this.primaryId);
  }

  clear(): void {
    this.snapshot = null;
    this.selection.clear();
    this.primaryId = null;
  }

  /** Refresh backend data without restoring stale persisted selection (PRJ-03/05). */
  refresh(s: ProjectSnapshot): void {
    const selection = [...this.selection];
    const primary = this.primaryId;
    this.load(s);
    this.selection.clear();
    for (const id of selection) if (this.clip(id)) this.selection.add(id);
    this.primaryId = primary && this.clip(primary) ? primary : null;
  }

  /** Apply a patch from a command result or the event stream. */
  applyPatch(p: StatePatch): void {
    const s = this.snapshot;
    if (!s) return;
    if (p.revision <= s.revision) return;
    if (p.revision !== s.revision + 1) {
      void this.refetch();
      return;
    }
    const removed = new Set(p.removed);
    const up = new Map(p.upserted.map((c) => [c.id, c]));
    const clips = s.project.clips.filter((c) => !removed.has(c.id) && !up.has(c.id));
    clips.push(...up.values());
    clips.sort(byIndex);
    this.snapshot = {
      ...s,
      revision: p.revision,
      history: p.history,
      project: {
        ...s.project,
        clips,
        next_index: p.next_index,
        tags: p.tags ?? s.project.tags,
        export_settings: p.export_settings ?? s.project.export_settings,
      },
    };
    for (const id of p.removed) this.selection.delete(id);
    if (this.primaryId && removed.has(this.primaryId)) this.primaryId = null;
  }

  setSaveState(save: SaveState): void {
    if (this.snapshot) this.snapshot = { ...this.snapshot, save };
  }

  async refetch(): Promise<void> {
    if (this.refetching) return;
    this.refetching = true;
    try {
      const snap = await backend.getSnapshot();
      if (snap) {
        const keepPrimary = this.primaryId;
        const keepSel = [...this.selection];
        this.load(snap);
        for (const id of keepSel) if (this.clip(id)) this.selection.add(id);
        if (keepPrimary && this.clip(keepPrimary)) this.primaryId = keepPrimary;
      }
    } finally {
      this.refetching = false;
    }
  }

  // ───────────── selection ─────────────

  select(id: string, mode: "replace" | "toggle" | "range" = "replace"): void {
    if (mode === "replace") {
      this.selection.clear();
      this.selection.add(id);
    } else if (mode === "toggle") {
      if (this.selection.has(id)) this.selection.delete(id);
      else this.selection.add(id);
    } else {
      const ids = this.clips.map((c) => c.id);
      const a = ids.indexOf(this.primaryId ?? id);
      const b = ids.indexOf(id);
      const [lo, hi] = a < b ? [a, b] : [b, a];
      this.selection.clear();
      for (const cid of ids.slice(lo, hi + 1)) this.selection.add(cid);
    }
    this.primaryId = id;
  }

  clearSelection(): void {
    this.selection.clear();
    this.primaryId = null;
  }

  /** Ids targeted by bulk commands: the selection, else the primary clip. */
  targetIds(): string[] {
    if (this.selection.size > 0) return [...this.selection];
    return this.primaryId ? [this.primaryId] : [];
  }

  // ───────────── commands (errors become toasts) ─────────────

  private async run<T>(fn: () => Promise<T>): Promise<T | null> {
    try {
      return await fn();
    } catch (e) {
      const err = BackendError.from(e);
      ui.toast({ level: "error", message: err.message, code: err.code });
      return null;
    }
  }

  async addClip(req: AddClipRequest): Promise<AddClipResult | null> {
    const r = await this.run(() => backend.addClip(req));
    if (r) {
      this.applyPatch(r.patch);
      this.select(r.clip.id);
      if (r.duplicate_of !== null) {
        // MRK-03: identical ranges are allowed but flagged.
        ui.toast({ level: "info", message: t("project.duplicate", { index: formatIndex(r.duplicate_of) }), code: "duplicate_range" });
      }
    }
    return r;
  }

  async editClips(ids: string[], edit: ClipEdit): Promise<void> {
    if (ids.length === 0) return;
    const p = await this.run(() => backend.editClips({ ids, edit }));
    if (p) this.applyPatch(p);
  }

  async editClipsBatch(batch: EditClipsRequest[]): Promise<void> {
    // Edits without ids are dropped; the rest go out as ONE call = one undo step (QUE-04).
    const edits = batch.filter((e) => e.ids.length > 0);
    if (edits.length === 0) return;
    const p = await this.run(() => backend.editClipsBatch(edits));
    if (p) this.applyPatch(p);
  }

  async deleteClips(ids: string[]): Promise<void> {
    if (ids.length === 0) return;
    const p = await this.run(() => backend.deleteClips(ids));
    if (p) this.applyPatch(p);
  }

  async undo(): Promise<void> {
    const p = await this.run(() => backend.undo());
    if (p) {
      this.applyPatch(p);
      if (p.label) ui.info(t("project.undone", { label: p.label }), "undo");
    } else if (p === null && !this.history.can_undo) {
      ui.info(t("project.nothingToUndo"), "undo");
    }
  }

  async redo(): Promise<void> {
    const p = await this.run(() => backend.redo());
    if (p) {
      this.applyPatch(p);
      if (p.label) ui.info(t("project.redone", { label: p.label }), "redo");
    } else if (p === null && !this.history.can_redo) {
      ui.info(t("project.nothingToRedo"), "redo");
    }
  }

  async renumber(): Promise<void> {
    const p = await this.run(() => backend.renumberClips());
    if (p) this.applyPatch(p);
  }

  async setTags(tags: Tag[]): Promise<void> {
    const p = await this.run(() => backend.setTags(tags));
    if (p) this.applyPatch(p);
  }

  async setExportSettings(settings: ExportSettings): Promise<void> {
    const p = await this.run(() => backend.setExportSettings(settings));
    if (p) this.applyPatch(p);
  }

  private viewTimer: ReturnType<typeof setTimeout> | null = null;
  private viewLatest: ViewState | null = null;
  private viewSent: ViewState | null = null;

  /**
   * PRJ-03: playhead/zoom/selection are saved, not undoable. Throttled with a
   * trailing send (at most one IPC per second, also during playback);
   * unchanged views are not resent.
   */
  setViewState(view: ViewState): void {
    this.viewLatest = view;
    if (this.viewTimer) return;
    this.viewTimer = setTimeout(() => {
      this.viewTimer = null;
      const v = this.viewLatest;
      if (!v || !this.snapshot || sameView(v, this.viewSent)) return;
      this.viewSent = v;
      void backend.setViewState(v).catch(() => {});
    }, VIEW_STATE_THROTTLE_MS);
  }

  /** Forget the last sent view (a new project was opened). */
  resetViewState(view: ViewState | null): void {
    this.viewLatest = null;
    this.viewSent = view;
  }

  async saveNow(): Promise<void> {
    const s = await this.run(() => backend.saveNow());
    if (s) this.setSaveState(s);
  }
}

export const project = new ProjectState();
