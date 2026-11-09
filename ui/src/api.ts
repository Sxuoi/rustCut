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

export interface PreviewInfo {
  /** File the player should load (not yet converted to a webview URL). */
  preview_path: string;
  /** Loopback HTTP URL to assign to the <video> element. */
  preview_url: string;
  transcoded: boolean;
  source_codec: string;
  source_format: string;
  reason: string | null;
  cached: boolean;
  /** Encoder that produced the proxy (`qsv`, `software`); null when cached. */
  encoder: string | null;
  /** Resolution of this proxy: `720p` or `1080p`. */
  quality: string;
}

/**
 * Asks the backend for a playable preview of `path`. HEVC, 10-bit and
 * Matroska sources are transcoded once into a cached H.264 proxy; everything
 * the embedded player can decode is returned unchanged.
 */
export async function preparePreview(
  path: string,
  quality: "fast" | "high" = "fast",
): Promise<PreviewInfo> {
  if (!isTauri) {
    throw new Error("prepare_preview requires the Tauri runtime");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<PreviewInfo>("prepare_preview", { path, quality });
}

/** Transcode progress for the preview proxy (0.0..=1.0). */
export async function onPreviewProgress(
  onProgress: (fraction: number, encoder: string) => void,
): Promise<() => void> {
  const { listen } = await import("@tauri-apps/api/event");
  return listen<{ fraction: number; encoder: string }>(
    "preview://progress",
    (e) => onProgress(e.payload.fraction, e.payload.encoder),
  );
}

/** Renders a naming template for one clip (same code path as the exporter). */
export async function previewFilename(params: {
  pattern: string;
  index: number;
  clipName: string;
  sourceName: string;
  startSec: number;
  endSec: number;
}): Promise<string> {
  if (!isTauri) {
    throw new Error("preview_filename requires the Tauri runtime");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<string>("preview_filename", params);
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
  encoder: string = "auto",
  workers: number = 1,
): Promise<number> {
  if (!isTauri) {
    throw new Error("start_export requires the Tauri runtime");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<number>("start_export", { state, source, encoder, workers });
}

export interface EncoderOption {
  id: string;
  label: string;
  available: boolean;
  hardware: boolean;
}

/** Which H.264 encoders actually work here (probed by encoding a frame). */
export async function detectEncoders(): Promise<EncoderOption[]> {
  if (!isTauri) {
    return [];
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<EncoderOption[]>("detect_encoders");
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
