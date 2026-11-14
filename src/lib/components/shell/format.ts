// Pure helpers of the shell (header, banners, dialogs). OWNER: fe-shell.

import type { ExportSettings, Preset, SaveState } from "$lib/bindings";
import { t } from "$lib/i18n";

/** The four presets in Export-dialog order (§3.1 step 6). */
export const PRESETS: readonly Preset[] = ["lossless", "accurate", "vcrop", "vblur"];

/** i18n key of a preset's name ("Lossless (Original)", …). */
export function presetLabelKey(p: Preset): string {
  return `preset.${p}`;
}

/**
 * The preset an ExportSettings value represents. Lossless always means the
 * original framing; the vertical framings only exist in Frame-Accurate mode.
 */
export function presetOf(s: Pick<ExportSettings, "cut_mode" | "framing">): Preset {
  if (s.cut_mode === "lossless") return "lossless";
  if (s.framing === "vertical_crop") return "vcrop";
  if (s.framing === "vertical_blur") return "vblur";
  return "accurate";
}

/**
 * Apply a preset with its semantics: Lossless forces the original framing;
 * the vertical presets switch the cut mode to Frame-Accurate. Every other
 * setting (folder, template, encoder, …) is kept.
 */
export function applyPreset(s: ExportSettings, p: Preset): ExportSettings {
  switch (p) {
    case "lossless":
      return { ...s, cut_mode: "lossless", framing: "original" };
    case "accurate":
      return { ...s, cut_mode: "accurate", framing: "original" };
    case "vcrop":
      return { ...s, cut_mode: "accurate", framing: "vertical_crop" };
    case "vblur":
      return { ...s, cut_mode: "accurate", framing: "vertical_blur" };
  }
}

/** Vertical Crop 9:16 is active (crop overlay, `Alt+←/→` nudges). */
export function isVerticalCrop(s: Pick<ExportSettings, "cut_mode" | "framing"> | null | undefined): boolean {
  return !!s && presetOf(s) === "vcrop";
}

/** Last path segment (either separator): `C:\v\a b.mp4` -> `a b.mp4`. */
export function basename(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** File name without its last extension: `stream.2026.mp4` -> `stream.2026`. */
export function stem(path: string): string {
  const name = basename(path);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}

/** Human-readable size: `512 B`, `1.5 KB`, `2.3 GB` (1024-based). */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 1024) return `${Math.max(0, Math.round(bytes || 0))} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

export type SaveTone = "muted" | "busy" | "error";

/** What the header shows about saving (§6.1 "saved 2s ago"). */
export interface SaveDisplay {
  text: string;
  tone: SaveTone;
  /** "stored in app library" when the project fell back there (PRJ-02). */
  location: string | null;
  /** Tooltip: the error detail or the project file path. */
  title: string | null;
}

/** Local `HH:MM` of a timestamp. */
function clockTime(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** Header save state at wall-clock time `nowMs`. */
export function formatSaveState(save: SaveState | null, nowMs: number): SaveDisplay {
  if (!save) return { text: "", tone: "muted", location: null, title: null };
  const location = save.location === "app_library" ? t("save.inLibrary") : null;
  if (save.error) {
    return { text: t("save.error"), tone: "error", location, title: save.error };
  }
  if (save.pending) return { text: t("save.saving"), tone: "busy", location, title: save.project_path };
  const savedMs = save.saved_at ? Date.parse(save.saved_at) : Number.NaN;
  if (!Number.isFinite(savedMs)) return { text: t("save.notYet"), tone: "muted", location, title: save.project_path };
  const age = Math.max(0, Math.floor((nowMs - savedMs) / 1000));
  let text: string;
  if (age < 60) text = t("save.secondsAgo", { n: age });
  else if (age < 3600) text = t("save.minutesAgo", { n: Math.floor(age / 60) });
  else text = t("save.at", { time: clockTime(savedMs) });
  return { text, tone: "muted", location, title: save.project_path };
}
