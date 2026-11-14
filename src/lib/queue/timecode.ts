// Time parsing for the queue search (QUE-06) and the Export dialog's maximum
// duration field (FRM-08). Mirrors `rustcut_model::time::{to_ms, parse_timecode}`
// so the UI compares times exactly like the Rust core does.

/** Whole milliseconds, for exact comparisons (mirror of `time::to_ms`). */
export function toMs(sec: number): number {
  return Number.isFinite(sec) ? Math.round(sec * 1000) : 0;
}

/**
 * Parse `SS`, `MM:SS` or `H:MM:SS`, each optionally with `.fff`, into seconds
 * rounded to milliseconds. Returns `null` for anything else (mirror of
 * `time::parse_timecode`): minutes and seconds after a `:` must be < 60.
 */
export function parseTimecode(input: string): number | null {
  const s = input.trim();
  if (s === "") return null;
  const parts = s.split(":");
  if (parts.length > 3) return null;
  let total = 0;
  for (let i = 0; i < parts.length; i++) {
    const part = parts[i];
    const last = i === parts.length - 1;
    if (part === "") return null;
    if (last) {
      if (!/^[0-9.]+$/.test(part) || (part.match(/\./g) ?? []).length > 1 || part === ".") return null;
      const v = Number(part);
      if (!Number.isFinite(v)) return null;
      if (parts.length > 1 && v >= 60) return null;
      total = total * 60 + v;
    } else {
      if (!/^[0-9]+$/.test(part)) return null;
      const v = Number(part);
      if (i > 0 && v >= 60) return null;
      total = total * 60 + v;
    }
  }
  return toMs(total) / 1000;
}

/** `m:ss` (or `h:mm:ss`) for whole-second durations such as platform limits: 180 → `3:00`. */
export function formatClock(sec: number): string {
  const s = Number.isFinite(sec) && sec > 0 ? Math.round(sec) : 0;
  const h = Math.floor(s / 3600);
  const m = Math.floor(s / 60) % 60;
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}
