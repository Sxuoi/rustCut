// UI-only state: app phase, dialogs, toasts, focus requests.
// OWNER: fe-shell. Public API is shared by every component; extend, don't break.

import type { AppInfo, AppSettings, Notice, NoticeLevel, RecentProject } from "$lib/bindings";
import { detectPlatform, toPlatform, type Platform } from "$lib/keyboard/shortcuts";
import { DEFAULT_MIN_CLIP_SEC, normalizeMinDuration } from "$lib/util/marks";

export type AppPhase =
  | "empty" // drop zone, Open video, recent projects
  | "probing" // spinner with file name
  | "ready"; // player, timelines, queue (Preparing preview is player.playback.preparing)

export interface Toast {
  id: number;
  level: NoticeLevel;
  message: string;
  code: string | null;
}

/** PLY-06 zoom window bounds (seconds) and the default for new projects. */
export const ZOOM_WINDOW_MIN_SEC = 2;
export const ZOOM_WINDOW_MAX_SEC = 600;
export const DEFAULT_ZOOM_WINDOW_SEC = 20;

let toastSeq = 0;

export class UiState {
  phase = $state<AppPhase>("empty");
  /** File name shown while probing. */
  probingName = $state<string | null>(null);

  exportDialogOpen = $state(false);
  settingsOpen = $state(false);
  cheatSheetOpen = $state(false);
  tagEditorOpen = $state(false);
  timestampsOpen = $state(false);
  locateMediaOpen = $state(false);

  /** Rename request for the clip queue (`N` key); nonce re-triggers. */
  renameRequest = $state<{ clipId: string; nonce: number } | null>(null);

  toasts = $state<Toast[]>([]);

  /** App settings as normalized by the backend; null until loaded. */
  settings = $state.raw<AppSettings | null>(null);
  /** Versions, folders, platform; null until loaded. */
  appInfo = $state.raw<AppInfo | null>(null);
  /** Start-screen list (PRJ-07). */
  recent = $state.raw<RecentProject[]>([]);
  /** Decides Ctrl vs Cmd (§5) and platform-only bindings. */
  platform = $state<Platform>(detectPlatform(typeof navigator === "undefined" ? undefined : navigator));
  /** Files are dragged over the window (drop-zone highlight). */
  dragOver = $state(false);
  /**
   * Zoom timeline window in seconds (PLY-06, 2 s – 10 min). Restored from and
   * saved to ViewState; the zoom timeline reads and writes it.
   */
  zoomWindowSec = $state(DEFAULT_ZOOM_WINDOW_SEC);
  /**
   * Clip ids in the queue's visible order (sort + search). The queue sets it
   * so `↑`/`↓` follow what the user sees; null = by clip number.
   */
  queueOrder = $state.raw<string[] | null>(null);

  private playerFocusEl: HTMLElement | null = null;

  /** True while any modal dialog is open (global shortcuts pause). */
  get anyDialogOpen(): boolean {
    return (
      this.exportDialogOpen ||
      this.settingsOpen ||
      this.cheatSheetOpen ||
      this.tagEditorOpen ||
      this.timestampsOpen ||
      this.locateMediaOpen
    );
  }

  /** MRK-01 minimum clip duration (setting 0.1–5 s, default 0.5 s). */
  get minClipDuration(): number {
    const v = this.settings?.min_clip_duration_sec;
    return v === undefined ? DEFAULT_MIN_CLIP_SEC : normalizeMinDuration(v);
  }

  closeAllDialogs(): void {
    this.exportDialogOpen = false;
    this.settingsOpen = false;
    this.cheatSheetOpen = false;
    this.tagEditorOpen = false;
    this.timestampsOpen = false;
    this.locateMediaOpen = false;
  }

  setAppInfo(info: AppInfo): void {
    this.appInfo = info;
    this.platform = toPlatform(info.platform, this.platform);
  }

  setZoomWindow(sec: number): void {
    const v = Number.isFinite(sec) ? sec : DEFAULT_ZOOM_WINDOW_SEC;
    this.zoomWindowSec = Math.min(ZOOM_WINDOW_MAX_SEC, Math.max(ZOOM_WINDOW_MIN_SEC, v));
  }

  /**
   * Show a toast. Errors stay 8 s, others 4 s; `timeoutMs = 0` keeps it.
   * A toast identical to a visible one replaces it (no stacking repeats).
   */
  toast(n: Pick<Notice, "level" | "message"> & { code?: string | null }, timeoutMs?: number): number {
    const id = ++toastSeq;
    const rest = this.toasts.filter((x) => !(x.level === n.level && x.message === n.message));
    this.toasts = [...rest.slice(-4), { id, level: n.level, message: n.message, code: n.code ?? null }];
    const ms = timeoutMs ?? (n.level === "error" ? 8000 : 4000);
    if (ms > 0) setTimeout(() => this.dismissToast(id), ms);
    return id;
  }

  info(message: string, code?: string): number {
    return this.toast({ level: "info", message, code });
  }

  error(message: string, code?: string): number {
    return this.toast({ level: "error", message, code });
  }

  dismissToast(id: number): void {
    this.toasts = this.toasts.filter((t) => t.id !== id);
  }

  requestRename(clipId: string): void {
    this.renameRequest = { clipId, nonce: (this.renameRequest?.nonce ?? 0) + 1 };
  }

  /** Player.svelte registers its focusable container. */
  registerPlayerFocus(el: HTMLElement | null): void {
    this.playerFocusEl = el;
  }

  /**
   * Return keyboard focus to the player (§5 rule 1: after a rename is
   * committed or a text field is left with Esc).
   */
  focusPlayer(): void {
    const el = this.playerFocusEl;
    if (el && el.isConnected) el.focus({ preventScroll: true });
    else if (typeof document !== "undefined" && document.activeElement instanceof HTMLElement) document.activeElement.blur();
  }
}

export const ui = new UiState();
