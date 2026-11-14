// Routes the backend's ordered AppEvent stream into the stores.
// OWNER: fe-shell.

import type { AppEvent, Notice } from "$lib/bindings";
import { t } from "$lib/i18n";
import { backend } from "$lib/ipc";
import { exporter } from "./exporter.svelte";
import { media } from "./media.svelte";
import { player } from "./player.svelte";
import { project } from "./project.svelte";
import { ui } from "./ui.svelte";

/** PRD 8.5: known backend notices use localised copy; future codes retain their fallback. */
export function localizeNotice(notice: Notice): Notice {
  switch (notice.code) {
    case "ffmpeg_too_old":
      return { ...notice, message: t("notice.ffmpegTooOld") };
    default:
      return notice;
  }
}

export function dispatchEvent(e: AppEvent): void {
  switch (e.type) {
    case "patch":
      project.applyPatch(e.patch);
      break;
    case "snapshot":
      // Background replacement (relink, refetch): keep the player's duration
      // in step with the (possibly different) source.
      project.load(e.snapshot);
      if (e.snapshot.project.source.duration_sec > 0) player.duration = e.snapshot.project.source.duration_sec;
      break;
    case "saved":
      project.setSaveState(e.save);
      break;
    case "export_batch":
      exporter.handleBatch(e.state);
      break;
    case "export_progress":
      exporter.handleProgress(e.progress);
      break;
    case "export_clip_finished":
      exporter.handleClipFinished(e.finished);
      break;
    case "playback":
      player.playback = e.info;
      break;
    case "media_index":
      media.handleIndex(e.info);
      break;
    case "waveform":
      media.handleWaveform(e.chunk);
      break;
    case "toast":
      ui.toast(localizeNotice(e.notice));
      break;
  }
}

let started = false;

/** Subscribe once at startup. */
export async function startEventLoop(): Promise<void> {
  if (started) return;
  started = true;
  await backend.subscribe(dispatchEvent);
}
