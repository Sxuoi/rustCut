// Thin IPC layer: Tauri commands when running in the desktop shell, a
// browser fallback when running `npm run dev` standalone.

import type {
  ExportFinishedEvent,
  ExportProgressEvent,
  MediaInfo,
  ProjectState,
} from "./types";

export const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export interface SourceSpec {
  width: number;
  height: number;
  bitrate_bps: number;
}

export interface OpenedVideo {
  path: string;
  url: string;
}

export async function loadProject(path: string): Promise<ProjectState> {
  if (!isTauri) {
    throw new Error("load_project requires the Tauri runtime");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<ProjectState>("load_project", { path });
}

export async function saveProject(
  path: string,
  state: ProjectState,
): Promise<void> {
  if (isTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("save_project", { path, state });
    return;
  }
  // Browser fallback: download the .rustcut file so the flow stays usable.
  const json = JSON.stringify(state, null, 2);
  const blob = new Blob([json], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = `${path.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, "") || "project"}.rustcut`;
  a.click();
  URL.revokeObjectURL(url);
}

/** Native file dialog (Tauri). Returns the real path plus a webview URL. */
export async function openVideoDialog(): Promise<OpenedVideo | null> {
  if (!isTauri) {
    return null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const { convertFileSrc } = await import("@tauri-apps/api/core");
  const selected = await open({
    multiple: false,
    filters: [{ name: "Video", extensions: ["mp4", "mkv", "mov", "webm", "flv"] }],
  });
  if (typeof selected !== "string") {
    return null;
  }
  return { path: selected, url: convertFileSrc(selected) };
}

/** Directory picker for the export destination (Tauri). */
export async function pickOutputFolder(): Promise<string | null> {
  if (!isTauri) {
    return null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const selected = await open({ directory: true, multiple: false });
  return typeof selected === "string" ? selected : null;
}

/** ffprobe-backed media facts (duration, size, fps, bitrate). */
export async function probeMedia(path: string): Promise<MediaInfo> {
  if (!isTauri) {
    throw new Error("probe_media requires the Tauri runtime");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<MediaInfo>("probe_media", { path });
}

/** Timeline waveform peaks in `0.0..=1.0`, one value per bucket. */
export async function waveformPeaks(
  path: string,
  buckets: number,
): Promise<number[]> {
  if (!isTauri) {
    throw new Error("waveform_peaks requires the Tauri runtime");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<number[]>("waveform_peaks", { path, buckets });
}

/** Launches the batch export; returns the number of jobs. */
export async function startExport(
  state: ProjectState,
  source: SourceSpec,
): Promise<number> {
  if (!isTauri) {
    throw new Error("start_export requires the Tauri runtime");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<number>("start_export", { state, source });
}

/** Subscribes to export events; returns an unsubscribe function. */
export async function onExportEvents(
  onProgress: (e: ExportProgressEvent) => void,
  onFinished: (e: ExportFinishedEvent) => void,
): Promise<() => void> {
  const { listen } = await import("@tauri-apps/api/event");
  const unProgress = await listen<ExportProgressEvent>("export://progress", (e) =>
    onProgress(e.payload),
  );
  const unFinished = await listen<ExportFinishedEvent>("export://finished", (e) =>
    onFinished(e.payload),
  );
  return () => {
    unProgress();
    unFinished();
  };
}
