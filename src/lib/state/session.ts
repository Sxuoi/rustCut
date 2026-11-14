// Session flows: startup, open (PRD §3.1 step 1, §6.4 Probing -> Ready),
// relink (PRJ-04), close. OWNER: fe-shell.
//
// One implementation used by Ctrl+O, the drop zone, recent projects, the
// File menu and the Locate dialog.

import { tick } from "svelte";
import type { AppSettings, OpenResult, PlaybackInfo, ProjectSnapshot, RelinkResult } from "$lib/bindings";
import { basename } from "$lib/components/shell/format";
import { t } from "$lib/i18n";
import { backend, BackendError } from "$lib/ipc";
import { loadPlatformPresets } from "$lib/queue/exportOptions";
import { exporter } from "./exporter.svelte";
import { localizeNotice } from "./events";
import { media } from "./media.svelte";
import { player } from "./player.svelte";
import { project } from "./project.svelte";
import { ui } from "./ui.svelte";

/** Notices shown as banners (from the snapshot), not as toasts. */
const BANNER_NOTICES = new Set(["source_missing", "read_only_locked", "read_only_newer_schema"]);

function toastError(e: unknown): void {
  const err = BackendError.from(e);
  ui.toast({ level: "error", message: err.message, code: err.code });
}

/**
 * Load settings, platform presets and recent projects (startup). Never throws. Only quick
 * calls: app info waits for the backend's FFmpeg probe, so `loadAppInfo()` fetches it later.
 */
export async function loadAppContext(): Promise<void> {
  const [settings] = await Promise.allSettled([backend.getSettings(), loadPlatformPresets(backend)]);
  if (settings.status === "fulfilled") ui.settings = settings.value;
  else ui.toast({ level: "warning", message: t("settings.loadFailed", { reason: BackendError.from(settings.reason).message }) });
  await refreshRecent();
}

/** App info (versions, folders, platform). Waits for the backend's FFmpeg probe. Never throws. */
export async function loadAppInfo(): Promise<void> {
  try {
    ui.setAppInfo(await backend.getAppInfo());
  } catch {
    // The Settings dialog asks again when it opens.
  }
}

/**
 * Startup once the event loop runs (PRD §3.1 step 1): settings and recent projects, a project
 * still open in the core (WebView reload), then the file given on the command line through the
 * normal open flow. App info follows the first frame (or comes at once without a video): it
 * waits for the backend's FFmpeg probe, which would otherwise hold up the launch file.
 */
export async function startUp(): Promise<void> {
  await Promise.all([loadAppContext(), restoreSession()]);
  try {
    const launch = await backend.getLaunchPath();
    if (launch && !project.loaded) await openPath(launch);
  } catch (e) {
    ui.error(BackendError.from(e).message, "launch");
  }
  if (project.loaded) await player.firstFrame(MEDIA_JOBS_START_TIMEOUT_MS);
  await loadAppInfo();
}

/** Refresh the start screen's recent projects (PRJ-07). */
export async function refreshRecent(): Promise<void> {
  try {
    ui.recent = await backend.listRecentProjects();
  } catch {
    ui.recent = [];
  }
}

/** Save app settings; returns the normalized value or throws BackendError. */
export async function saveSettings(next: AppSettings): Promise<AppSettings> {
  try {
    const saved = await backend.setSettings(next);
    ui.settings = saved;
    return saved;
  } catch (e) {
    throw BackendError.from(e);
  }
}

/**
 * Longest wait for the first presented frame before the jobs that read the whole file start
 * anyway, so the first waveform view still arrives within PRD 8.1's 5 s when no frame is shown.
 */
export const MEDIA_JOBS_START_TIMEOUT_MS = 3000;

/**
 * Start the background media jobs of an available source (PLY-05/06). The media index is
 * requested at once: the open builds it from the MP4/MOV sample tables, and Player.svelte reads
 * its first keyframe for the time origin (§7.3) when the first frame arrives. The jobs that read
 * the whole file (the waveform, and the packet-scan index of other containers) wait for that
 * first frame (AC-14: < 50 MB read before it) or `MEDIA_JOBS_START_TIMEOUT_MS`, whichever
 * comes first.
 */
function startMediaJobs(s: ProjectSnapshot): void {
  const tracks = s.project.source.audio_tracks.length;
  const wanted = s.project.export_settings.audio_track;
  const track = wanted >= 1 && wanted <= tracks ? wanted : 1;
  const generation = player.generation;
  void player.firstFrame(MEDIA_JOBS_START_TIMEOUT_MS).then(() => {
    // Another project was opened, or this one closed, before its first frame.
    if (player.generation !== generation) return;
    backend.startMediaIndex().catch(() => {
      // Only fails without an open source; a failed scan arrives as an `index_failed` toast.
    });
    if (tracks === 0) return;
    backend.startWaveform(track).catch((e: unknown) => {
      ui.toast({ level: "warning", message: t("session.waveformFailed", { reason: BackendError.from(e).message }) });
    });
  });
  backend
    .getMediaIndex()
    .then((info) => {
      if (info) media.handleIndex(info);
    })
    .catch(() => {
      // The index also arrives as `media_index` events; stepping falls back
      // to the nominal frame duration meanwhile.
    });
}

/** Put the stores in the state of a freshly opened project (§6.4 Ready). */
function loadIntoStores(snapshot: ProjectSnapshot, playback: PlaybackInfo | null): void {
  const src = snapshot.project.source;
  const view = snapshot.project.view_state;
  player.reset();
  media.reset();
  project.load(snapshot);
  project.resetViewState(view);
  player.duration = src.duration_sec;
  player.setFrameRate(src.video.frame_rate);
  // PRJ-03: restore the playhead once the first frame is decoded.
  player.currentTime = Math.min(Math.max(0, view.playhead_sec), src.duration_sec || view.playhead_sec);
  player.restoreTime = view.playhead_sec > 0 ? view.playhead_sec : null;
  ui.setZoomWindow(view.zoom_window_sec);
  ui.queueOrder = null;
  player.playback = playback;
  ui.phase = "ready";
  ui.probingName = null;
  if (playback) startMediaJobs(snapshot);
}

/**
 * §5 rule 1: a freshly opened project starts with the keyboard on the player,
 * so I / O / Enter work at once. Focus left on a toolbar button or menu (the
 * Preset menu, File > Open) would otherwise take Enter and open that control
 * instead of adding the clip. Runs after the Player has mounted; a focused
 * dialog (Locate media) keeps its focus.
 */
function focusPlayerAfterOpen(): void {
  if (typeof document === "undefined") return;
  void tick().then(() => {
    if (document.activeElement?.closest("[role=dialog]")) return;
    ui.focusPlayer();
  });
}

/** Apply an `open_path` / relink result (notices become toasts or banners). */
export function applyOpenResult(r: OpenResult): void {
  loadIntoStores(r.snapshot, r.playback);
  for (const n of r.notices) {
    if (!BANNER_NOTICES.has(n.code)) ui.toast(localizeNotice(n));
  }
  void refreshRecent();
  focusPlayerAfterOpen();
}

let opening = false;

/** View state is throttled, so only persistent identity can identify a session (PRJ-03/05). */
function sameSession(a: ProjectSnapshot, b: ProjectSnapshot | null): boolean {
  if (!b) return false;
  if (a.save.project_path && b.save.project_path) return a.save.project_path === b.save.project_path;
  // The project schema has no UUID; source plus creation time identifies unsaved projects.
  return a.project.source.path === b.project.source.path && a.project.created_at === b.project.created_at;
}

/**
 * Open a video or `.rustcut` file: Probing (spinner with the file name),
 * then Ready. On failure resync with the backend, which may already have
 * closed the previous project. Returns whether the file opened.
 */
export async function openPath(path: string, displayName: string = basename(path)): Promise<boolean> {
  if (opening) {
    ui.info(t("session.alreadyOpening"), "busy");
    return false;
  }
  // Opening another file closes the session and cancels a running batch (EXP-09): refuse.
  if (exporter.running) {
    ui.info(t("session.exportRunning"), "export_running");
    return false;
  }
  opening = true;
  const previous = ui.phase === "probing" ? "empty" : ui.phase;
  ui.phase = "probing";
  ui.probingName = displayName;
  try {
    const r = await backend.openPath(path);
    applyOpenResult(r);
    return true;
  } catch (e) {
    let snapshot: ProjectSnapshot | null = null;
    try {
      snapshot = await backend.getSnapshot();
    } catch {
      // get_snapshot throws when the failed open left no project in the core.
    }
    if (!snapshot) {
      player.reset();
      media.reset();
      project.clear();
      ui.phase = "empty";
    } else if (!sameSession(snapshot, project.snapshot)) {
      let playback: PlaybackInfo | null = null;
      if (snapshot.source_status.kind === "ok") {
        try {
          playback = await backend.requestPlayback("direct");
        } catch {
          // An unavailable preview must not hide the project that is actually open.
        }
      }
      loadIntoStores(snapshot, playback);
    } else {
      project.refresh(snapshot);
      // The Player remounts with a fresh element: retain the live playhead (PRJ-03).
      if (previous === "ready" && player.currentTime > 0) player.restoreTime = player.currentTime;
      ui.phase = "ready";
    }
    ui.probingName = null;
    toastError(e);
    return false;
  } finally {
    opening = false;
  }
}

/** Ctrl+O / Open video: native dialog (videos + .rustcut), then open. */
export async function openWithDialog(): Promise<void> {
  if (opening) return;
  let path: string | null;
  try {
    path = await backend.pickMediaFile();
  } catch (e) {
    toastError(e);
    return;
  }
  if (path) await openPath(path);
}

/**
 * Browser-only fallback (no Tauri): a dropped or chosen File is served to the
 * <video> as an object URL; the in-browser mock backend opens that URL.
 */
export function openBrowserFile(file: File): Promise<boolean> {
  return openPath(URL.createObjectURL(file), file.name);
}

/** Open a dropped path list: the first file (one source per project). */
export function openDropped(paths: readonly string[]): void {
  if (paths.length === 0) return;
  if (paths.length > 1) ui.info(t("session.oneFileOnly"), "multiple_files");
  void openPath(paths[0]);
}

/**
 * PRJ-04 relink. Returns the backend result: `mismatch` asks the user before
 * calling again with `acceptMismatch = true`; `relinked` is applied here.
 * `null` after an error (already shown as a toast unless `quiet`).
 */
export async function relinkSource(path: string, acceptMismatch: boolean, quiet = false): Promise<RelinkResult | null> {
  try {
    const r = await backend.relinkSource(path, acceptMismatch);
    if (r.kind === "relinked") {
      applyOpenResult(r.result);
      ui.info(t("session.relinked"), "relinked");
    }
    return r;
  } catch (e) {
    if (quiet) throw BackendError.from(e);
    toastError(e);
    return null;
  }
}

/** File > Close project: flush, release the lock, back to the start screen. */
export async function closeProject(): Promise<void> {
  try {
    await backend.closeProject();
  } catch (e) {
    toastError(e);
    return;
  }
  player.reset();
  media.reset();
  project.clear();
  ui.closeAllDialogs();
  ui.phase = "empty";
  await refreshRecent();
}

/**
 * Startup: if the core already has a project open (WebView reload), show it
 * again instead of the start screen.
 */
export async function restoreSession(): Promise<void> {
  let snap: ProjectSnapshot | null;
  try {
    snap = await backend.getSnapshot();
  } catch {
    return;
  }
  if (!snap || project.loaded || ui.phase !== "empty") return;
  let playback: PlaybackInfo | null = null;
  if (snap.source_status.kind === "ok") {
    try {
      playback = await backend.requestPlayback("direct");
    } catch {
      playback = null;
    }
  }
  loadIntoStores(snap, playback);
}
