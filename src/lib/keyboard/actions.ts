// What every shortcut action does (PRD §5 table). OWNER: fe-shell.
//
// One implementation shared by the key handler, the Transport buttons and the
// header menus. Store dependencies are injected (`createActions`) so the
// orchestration is unit-testable; `$lib/keyboard` wires the real stores.
//
// Mark actions receive `now`, the playhead captured synchronously by the
// caller before anything else ran (§5 rule 4): no IPC happens before it.

import type { AddClipRequest, AddClipResult, Clip, ClipEdit, ExportSettings, NoticeLevel, ReadOnlyReason } from "$lib/bindings";
import { t } from "$lib/i18n";
import { timelineMessageKeys } from "$lib/timeline/messages";
import { isVerticalCrop } from "$lib/components/shell/format";
import { markTarget, resolveAdd, resolveRetime, setMark, type MarkSide, type PendingMarks } from "$lib/util/marks";
import type { ActionId } from "./shortcuts";

/** PLY-02 seek steps (seconds). */
export const SEEK_SMALL_SEC = 1;
export const SEEK_LARGE_SEC = 5;
/** §5 / FRM-03 crop nudges (offset units, range [-1, +1]). */
export const CROP_NUDGE = 0.05;
export const CROP_NUDGE_LARGE = 0.25;
/** Successive nudges within this window build on each other, not on the store. */
const NUDGE_CHAIN_MS = 1500;

/** The player surface actions use (PlayerState satisfies it). */
export interface PlayerPort {
  readonly ready: boolean;
  readonly duration: number;
  readonly marks: PendingMarks;
  loopClipId: string | null;
  setMarks(m: PendingMarks): void;
  clearPending(): void;
  now(): number;
  toggle(): void;
  seek(t: number): void;
  seekBy(dt: number): void;
  stepFrame(dir: 1 | -1): void;
  speedStep(dir: 1 | -1): void;
  setLoop(clipId: string | null, range: { start: number; end: number } | null): void;
}

/** The project surface actions use (ProjectState satisfies it). */
export interface ProjectPort {
  readonly loaded: boolean;
  readonly readOnly: ReadOnlyReason | null;
  readonly clips: Clip[];
  readonly primary: Clip | null;
  readonly primaryId: string | null;
  readonly settings: ExportSettings | null;
  select(id: string, mode?: "replace" | "toggle" | "range"): void;
  targetIds(): string[];
  clip(id: string): Clip | undefined;
  addClip(req: AddClipRequest): Promise<AddClipResult | null>;
  editClips(ids: string[], edit: ClipEdit): Promise<void>;
  deleteClips(ids: string[]): Promise<void>;
  undo(): Promise<void>;
  redo(): Promise<void>;
  saveNow(): Promise<void>;
  readonly save: { error: string | null } | null;
}

/** The UI surface actions use (UiState satisfies it). */
export interface UiPort {
  readonly minClipDuration: number;
  readonly queueOrder: string[] | null;
  exportDialogOpen: boolean;
  cheatSheetOpen: boolean;
  toast(n: { level: NoticeLevel; message: string; code?: string | null }, timeoutMs?: number): number;
  info(message: string, code?: string): number;
  requestRename(clipId: string): void;
}

export interface ActionDeps {
  player: PlayerPort;
  project: ProjectPort;
  ui: UiPort;
  /** Ctrl+O: native open dialog, then the open flow. */
  open: () => void | Promise<void>;
  /** Wall clock (ms), injectable for tests. */
  clock?: () => number;
}

export interface Actions {
  /**
   * Run a §5 action. `now` is the playhead captured before anything else
   * (required by mark actions; others ignore it). `repeat` = key auto-repeat.
   */
  run(action: ActionId, now?: number, repeat?: boolean): void;
  /** `I` / `O`: set a pending mark at `now` (MRK-01). */
  setMark(side: MarkSide, now: number): void;
  /** `Enter` / `A`: add the pending range (MRK-01); resolves when done. */
  addPending(now: number): Promise<void>;
}

/** Message for a read-only project (§6.4 banner reasons, shortened). */
export function readOnlyMessage(reason: ReadOnlyReason): string {
  switch (reason.kind) {
    case "source_missing":
      return t("readonly.blocked.sourceMissing");
    case "open_elsewhere":
      return t("readonly.blocked.openElsewhere");
    case "newer_schema":
      return t("readonly.blocked.newerSchema");
    case "unwritable":
      return t(timelineMessageKeys.readOnlyUnwritable);
  }
}

/** The most recently added clip (highest stable number), or null. */
export function mostRecentClip(clips: readonly Clip[]): Clip | null {
  let best: Clip | null = null;
  for (const c of clips) if (!best || c.index > best.index) best = c;
  return best;
}

/**
 * `↑` / `↓` target: the clip before/after `currentId` in `order`; with no
 * (or an unknown) current clip, `↓` picks the first and `↑` the last. Stops at
 * the ends (no wrap-around). `null` when the queue is empty.
 */
export function adjacentClipId(order: readonly string[], currentId: string | null, dir: 1 | -1): string | null {
  if (order.length === 0) return null;
  const i = currentId === null ? -1 : order.indexOf(currentId);
  if (i < 0) return dir > 0 ? order[0] : order[order.length - 1];
  return order[Math.min(order.length - 1, Math.max(0, i + dir))];
}

/** Crop offset after a nudge: clamped to [-1, +1], rounded to 0.01. */
export function nudgeOffset(current: number, delta: number): number {
  const base = Number.isFinite(current) ? current : 0;
  const next = Math.round((base + delta) * 100) / 100;
  return Math.min(1, Math.max(-1, next));
}

export function createActions(deps: ActionDeps): Actions {
  const { player, project, ui } = deps;
  const clock = deps.clock ?? (() => Date.now());
  let lastNudge: { clipId: string; value: number; at: number } | null = null;

  const warn = (message: string, code: string): void => {
    ui.toast({ level: "warning", message, code });
  };

  /** A source is loaded and its preview shows frames. */
  function playerReady(explain: boolean): boolean {
    if (!project.loaded) return false;
    if (player.ready) return true;
    // §6.4 Preparing preview: "marking starts once the preview plays".
    if (explain) ui.info(t("marks.previewNotReady"), "preview_not_ready");
    return false;
  }

  /** The queue may be edited (not read-only, §6.4). */
  function editable(): boolean {
    if (!project.loaded) return false;
    const ro = project.readOnly;
    if (!ro) return true;
    warn(readOnlyMessage(ro), "read_only");
    return false;
  }

  function setMarkAt(side: MarkSide, now: number): void {
    if (!playerReady(true)) return;
    const r = setMark(side, player.marks, now, player.duration);
    player.setMarks(r.marks);
    if (r.toast) ui.toast({ level: "info", message: r.toast, code: r.cleared === "out" ? "out_cleared" : "in_cleared" });
  }

  async function addPending(now: number): Promise<void> {
    if (!playerReady(true) || !editable()) return;
    const pending = player.marks;
    const res = resolveAdd(pending, now, ui.minClipDuration, player.duration);
    if (!res.ok) {
      warn(res.error.message, res.error.kind === "too_short" ? "clip_too_short" : res.error.kind);
      return;
    }
    // MRK-01: the pending marks clear after adding. Clearing first makes a
    // fast double Enter unable to add the same range twice.
    player.clearPending();
    const req: AddClipRequest = { start_sec: res.range.start, end_sec: res.range.end, name: null, tag_id: null };
    const added = await project.addClip(req); // selects the new clip
    if (!added) {
      // Failed (the error toast is shown by the store): give the marks back
      // unless new ones were set meanwhile.
      const cur = player.marks;
      if (cur.in === null && cur.out === null) player.setMarks(pending);
    }
  }

  function escape(): void {
    // Dialogs and text fields handle their own Esc (keymap.ts); here Esc
    // clears the pending marks (§5).
    player.clearPending();
  }

  function rename(): void {
    if (!editable()) return;
    const target = project.primary ?? mostRecentClip(project.clips);
    if (!target) {
      ui.info(t("kb.noClipToRename"), "no_clip");
      return;
    }
    if (project.primaryId !== target.id) project.select(target.id);
    ui.requestRename(target.id);
  }

  function selectAdjacent(dir: 1 | -1): void {
    if (!project.loaded) return;
    const order = ui.queueOrder ?? project.clips.map((c) => c.id);
    const id = adjacentClipId(order, project.primaryId, dir);
    if (id === null) return;
    project.select(id);
    const c = project.clip(id);
    if (c && player.ready) player.seek(c.start_sec);
  }

  function toggleLoop(): void {
    if (!playerReady(false)) return;
    const c = project.primary;
    if (!c) {
      ui.info(t("kb.selectClipToLoop"), "no_clip");
      return;
    }
    if (player.loopClipId === c.id) player.setLoop(null, null);
    else player.setLoop(c.id, { start: c.start_sec, end: c.end_sec });
  }

  async function deleteSelection(): Promise<void> {
    if (!editable()) return;
    const ids = project.targetIds();
    if (ids.length === 0) return;
    if (player.loopClipId !== null && ids.includes(player.loopClipId)) player.setLoop(null, null);
    await project.deleteClips(ids);
    // MRK-04: no confirmation; tell how to undo once the delete happened.
    if (ids.every((id) => project.clip(id) === undefined)) {
      ui.info(t("kb.deleted", { count: ids.length }), "deleted");
    }
  }

  async function saveNow(): Promise<void> {
    if (!project.loaded) return;
    await project.saveNow();
    if (project.save && !project.save.error) ui.info(t("kb.saved"), "saved");
  }

  function goTo(side: MarkSide): void {
    if (!playerReady(false)) return;
    const target = markTarget(side, player.marks, project.primary);
    if (target === null) {
      ui.info(t(side === "in" ? "kb.nothingToGoIn" : "kb.nothingToGoOut"), "no_mark");
      return;
    }
    player.seek(target);
  }

  function retime(side: MarkSide, now: number): void {
    if (!playerReady(true) || !editable()) return;
    const c = project.primary;
    if (!c) {
      ui.info(t("kb.selectClipFirst"), "no_clip");
      return;
    }
    const r = resolveRetime(c, side, now, ui.minClipDuration, player.duration);
    if (!r.ok) {
      warn(r.error.message, r.error.kind === "too_short" ? "clip_too_short" : r.error.kind);
      return;
    }
    const edit: ClipEdit =
      side === "in"
        ? { kind: "retime", start_sec: r.range.start, end_sec: null }
        : { kind: "retime", start_sec: null, end_sec: r.range.end };
    void project.editClips([c.id], edit);
  }

  function nudgeCrop(delta: number, repeat: boolean): void {
    if (!project.loaded) return;
    if (!isVerticalCrop(project.settings)) {
      if (!repeat) ui.info(t("kb.cropOnlyVertical"), "crop_not_vertical");
      return;
    }
    if (!editable()) return;
    const c = project.primary;
    if (!c) {
      if (!repeat) ui.info(t("kb.selectClipFirst"), "no_clip");
      return;
    }
    // Key repeat outruns the IPC round trip: chain on the last value sent.
    const at = clock();
    const chained = lastNudge && lastNudge.clipId === c.id && at - lastNudge.at < NUDGE_CHAIN_MS;
    const base = chained && lastNudge ? lastNudge.value : c.crop_x_offset;
    const next = nudgeOffset(base, delta);
    lastNudge = { clipId: c.id, value: next, at };
    if (next !== base) void project.editClips([c.id], { kind: "set_crop", crop_x_offset: next });
  }

  function run(action: ActionId, now: number = Number.NaN, repeat = false): void {
    const at = Number.isFinite(now) ? now : player.now();
    switch (action) {
      case "playPause":
        if (playerReady(false)) player.toggle();
        return;
      case "seekBack5":
        if (playerReady(false)) player.seekBy(-SEEK_LARGE_SEC);
        return;
      case "seekForward5":
        if (playerReady(false)) player.seekBy(SEEK_LARGE_SEC);
        return;
      case "seekBack1":
        if (playerReady(false)) player.seekBy(-SEEK_SMALL_SEC);
        return;
      case "seekForward1":
        if (playerReady(false)) player.seekBy(SEEK_SMALL_SEC);
        return;
      case "frameBack":
        if (playerReady(false)) player.stepFrame(-1);
        return;
      case "frameForward":
        if (playerReady(false)) player.stepFrame(1);
        return;
      case "speedDown":
        player.speedStep(-1);
        return;
      case "speedUp":
        player.speedStep(1);
        return;
      case "markIn":
        setMarkAt("in", at);
        return;
      case "markOut":
        setMarkAt("out", at);
        return;
      case "addClip":
        void addPending(at);
        return;
      case "escape":
        escape();
        return;
      case "rename":
        rename();
        return;
      case "selectPrevious":
        selectAdjacent(-1);
        return;
      case "selectNext":
        selectAdjacent(1);
        return;
      case "loopToggle":
        toggleLoop();
        return;
      case "deleteSelection":
        void deleteSelection();
        return;
      case "undo":
        if (editable()) void project.undo();
        return;
      case "redo":
        if (editable()) void project.redo();
        return;
      case "exportDialog":
        if (project.loaded) ui.exportDialogOpen = true;
        return;
      case "saveNow":
        void saveNow();
        return;
      case "open":
        void deps.open();
        return;
      case "goToIn":
        goTo("in");
        return;
      case "goToOut":
        goTo("out");
        return;
      case "retimeIn":
        retime("in", at);
        return;
      case "retimeOut":
        retime("out", at);
        return;
      case "cropLeft":
        nudgeCrop(-CROP_NUDGE, repeat);
        return;
      case "cropRight":
        nudgeCrop(CROP_NUDGE, repeat);
        return;
      case "cropLeftLarge":
        nudgeCrop(-CROP_NUDGE_LARGE, repeat);
        return;
      case "cropRightLarge":
        nudgeCrop(CROP_NUDGE_LARGE, repeat);
        return;
      case "cheatSheet":
        ui.cheatSheetOpen = !ui.cheatSheetOpen;
        return;
      default: {
        const never: never = action;
        throw new Error(`unhandled shortcut action: ${String(never)}`);
      }
    }
  }

  return { run, setMark: setMarkAt, addPending };
}
