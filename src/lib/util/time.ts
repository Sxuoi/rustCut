// Mirrors crates/rustcut-model/src/time.rs. Keep the two in sync.

/** `HH:MM:SS.mmm` (PLY-04). */
export function formatTimecode(sec: number): string {
  const msTotal = Number.isFinite(sec) && sec > 0 ? Math.round(sec * 1000) : 0;
  const ms = msTotal % 1000;
  const s = Math.floor(msTotal / 1000);
  return `${pad(Math.floor(s / 3600))}:${pad(Math.floor(s / 60) % 60)}:${pad(s % 60)}.${String(ms).padStart(3, "0")}`;
}

/** Placeholder shown when a mark is not set. */
export const EMPTY_TIMECODE = "--:--:--.---";

/** Queue duration `m:ss.s` (QUE-01). */
export function formatDuration(sec: number): string {
  const tenths = Number.isFinite(sec) && sec > 0 ? Math.round(sec * 10) : 0;
  const m = Math.floor(tenths / 600);
  const rem = tenths % 600;
  return `${m}:${pad(Math.floor(rem / 10))}.${rem % 10}`;
}

/** Short label for timeline rulers: `0:00`, `12:30`, `1:02:03`. */
export function formatRuler(sec: number): string {
  const s = Math.max(0, Math.floor(sec));
  const h = Math.floor(s / 3600);
  const m = Math.floor(s / 60) % 60;
  const ss = s % 60;
  return h > 0 ? `${h}:${pad(m)}:${pad(ss)}` : `${m}:${pad(ss)}`;
}

/** Round to millisecond precision (stored precision). */
export function roundMs(sec: number): number {
  return Number.isFinite(sec) ? Math.round(sec * 1000) / 1000 : 0;
}

/** Clip number label: `#01`, `#123` (padding like `{index}`, min 2). */
export function formatIndex(index: number, width = 2): string {
  return `#${String(index).padStart(width, "0")}`;
}

function pad(n: number): string {
  return String(n).padStart(2, "0");
}
