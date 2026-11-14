// Helpers of the timestamps dialog (EXP-16 export, EXP-17 import). The
// formats themselves are produced and parsed by the Rust core.

import type { ImportedClip, TimestampFormat } from "$lib/bindings";

/** Formats offered by the export tab, in display order (EXP-16). */
export const TIMESTAMP_FORMATS: readonly TimestampFormat[] = ["youtube_chapters", "csv", "edl", "fcpxml"];

/** File name without folder and extension, for either path separator. */
export function sourceStem(path: string): string {
  const base = path.split(/[\\/]/).pop() ?? "";
  const dot = base.lastIndexOf(".");
  const stem = dot > 0 ? base.slice(0, dot) : base;
  return stem === "" ? "timestamps" : stem;
}

const SUFFIX: Record<TimestampFormat, string> = {
  youtube_chapters: "chapters",
  csv: "clips",
  edl: "clips",
  fcpxml: "clips",
};

/** Default name in the save dialog: `livestream_chapters.txt`, `livestream_clips.edl`. */
export function timestampFileName(stem: string, format: TimestampFormat, extension: string): string {
  const ext = extension.replace(/^\.+/, "");
  return `${stem}_${SUFFIX[format]}${ext === "" ? "" : `.${ext}`}`;
}

/** Rows of an import preview that become clips (no per-line error). */
export function importableItems(items: readonly ImportedClip[]): ImportedClip[] {
  return items.filter((i) => i.error === null);
}
