// Pending In/Out mark rules (PRD §4.2 MRK-01, MRK-05 keyboard retime, §9
// "Marking", AC-02, AC-03). OWNER: fe-shell.
//
// Pure functions. The keyboard controller captures the playhead synchronously
// (§5 rule 4: no IPC before the time is captured) and passes it in; results
// carry the toast text to show. Marks are UI state: only `add_clip` sends the
// final range to the backend (docs/ARCHITECTURE.md §4 "Mark capture").
//
// Invariants (AC-03): a range returned by `resolveAdd` / `resolveRetime` always
// has 0 <= start < end <= duration and end - start >= the minimum duration.
// Marks are never swapped: a conflicting mark clears the other one.

import { t } from "$lib/i18n";
import { roundMs } from "./time";

/** MRK-01 default minimum clip duration (seconds). */
export const DEFAULT_MIN_CLIP_SEC = 0.5;
/** MRK-01 setting range for the minimum clip duration (seconds). */
export const MIN_CLIP_SEC_RANGE: readonly [number, number] = [0.1, 5];

/** Tolerance for comparing ms-rounded times. */
const EPS = 1e-6;

/** Pending In/Out marks (MRK-01). UI state only; never persisted. */
export interface PendingMarks {
  readonly in: number | null;
  readonly out: number | null;
}

/** No pending marks. */
export const NO_MARKS: PendingMarks = Object.freeze({ in: null, out: null });

export type MarkSide = "in" | "out";

/** Result of setting a pending mark. */
export interface MarkResult {
  /** The new pending marks. */
  marks: PendingMarks;
  /** The clamped, millisecond-rounded time that was used. */
  time: number;
  /** The other mark, when it was cleared because the new one conflicted. */
  cleared: MarkSide | null;
  /** Toast text when a mark was cleared ("Out point cleared"). */
  toast: string | null;
}

/** A clip range on the source timeline (seconds, ms precision). */
export interface MarkRange {
  start: number;
  end: number;
}

/** Why a pending range could not be added (§9 "Marking"). */
export type AddErrorKind = "no_in" | "out_before_in" | "too_short";

export interface AddError {
  kind: AddErrorKind;
  /** Specific, user-facing sentence (the too-short one states the minimum). */
  message: string;
}

export type AddResolution =
  | { ok: true; range: MarkRange; /** Out came from the playhead (only In was set). */ outFromPlayhead: boolean }
  | { ok: false; error: AddError };

export type RetimeResolution = { ok: true; range: MarkRange } | { ok: false; error: AddError };

/** Seconds for messages: `0.5`, `2`, `0.25` (no trailing zeros). */
export function formatSeconds(sec: number): string {
  if (!Number.isFinite(sec)) return "0";
  return String(Number(sec.toFixed(3)));
}

/** Largest millisecond-aligned time that does not exceed the duration. */
function maxMarkTime(duration: number): number {
  if (!Number.isFinite(duration) || duration <= 0) return Number.POSITIVE_INFINITY;
  return Math.floor(duration * 1000 + EPS) / 1000;
}

/**
 * MRK-01 "Marks clamp to [0, source duration]", rounded to the stored
 * millisecond precision. A non-positive or unknown duration only clamps at 0.
 */
export function clampToSource(time: number, duration: number): number {
  const x = Number.isFinite(time) ? time : 0;
  return Math.min(roundMs(Math.max(0, x)), maxMarkTime(duration));
}

/** Clamp the minimum-duration setting to its range (MRK-01: 0.1–5 s). */
export function normalizeMinDuration(minDuration: number): number {
  if (!Number.isFinite(minDuration)) return DEFAULT_MIN_CLIP_SEC;
  return Math.min(MIN_CLIP_SEC_RANGE[1], Math.max(MIN_CLIP_SEC_RANGE[0], minDuration));
}

/**
 * Set the pending In at `time` (key `I` or `[`). If an Out is pending and the
 * new In is at or after it (In >= Out), the Out is cleared, never swapped.
 */
export function setIn(marks: PendingMarks, time: number, duration: number): MarkResult {
  const tIn = clampToSource(time, duration);
  if (marks.out !== null && tIn >= marks.out - EPS) {
    return { marks: { in: tIn, out: null }, time: tIn, cleared: "out", toast: t("marks.outCleared") };
  }
  return { marks: { in: tIn, out: marks.out }, time: tIn, cleared: null, toast: null };
}

/**
 * Set the pending Out at `time` (key `O` or `]`). If an In is pending and the
 * new Out is at or before it (In >= Out), the In is cleared, never swapped.
 */
export function setOut(marks: PendingMarks, time: number, duration: number): MarkResult {
  const tOut = clampToSource(time, duration);
  if (marks.in !== null && marks.in >= tOut - EPS) {
    return { marks: { in: null, out: tOut }, time: tOut, cleared: "in", toast: t("marks.inCleared") };
  }
  return { marks: { in: marks.in, out: tOut }, time: tOut, cleared: null, toast: null };
}

/** Set either mark. */
export function setMark(side: MarkSide, marks: PendingMarks, time: number, duration: number): MarkResult {
  return side === "in" ? setIn(marks, time, duration) : setOut(marks, time, duration);
}

function tooShort(start: number, end: number, minDuration: number): AddError {
  return {
    kind: "too_short",
    message: t("marks.tooShort", { length: formatSeconds(Math.max(0, end - start)), min: formatSeconds(minDuration) }),
  };
}

/** Validate a candidate range against MRK-01 (Out > In, minimum duration). */
function checkRange(start: number, end: number, minDuration: number, outError: AddError): RetimeResolution {
  if (end < start - EPS) return { ok: false, error: outError };
  if (end - start < minDuration - EPS) return { ok: false, error: tooShort(start, end, minDuration) };
  return { ok: true, range: { start, end } };
}

/**
 * MRK-01 `Enter` / `A`: the range to add from the pending marks.
 *
 * - Both marks set: that range.
 * - Only In set: Out = playhead, when the playhead is after In.
 * - No In: error (an Out alone is not a clip).
 * - Shorter than `minDuration`: error whose message states the minimum.
 *
 * The caller clears the pending marks after a successful add; the new clip is
 * selected by `project.addClip`.
 */
export function resolveAdd(
  pending: PendingMarks,
  playhead: number,
  minDuration: number,
  duration: number,
): AddResolution {
  const min = normalizeMinDuration(minDuration);
  if (pending.in === null) {
    return { ok: false, error: { kind: "no_in", message: t("marks.noIn") } };
  }
  const start = clampToSource(pending.in, duration);
  const outFromPlayhead = pending.out === null;
  const end = clampToSource(outFromPlayhead ? playhead : (pending.out as number), duration);
  const outError: AddError = {
    kind: "out_before_in",
    message: outFromPlayhead ? t("marks.playheadBeforeIn") : t("marks.outBeforeIn"),
  };
  const r = checkRange(start, end, min, outError);
  return r.ok ? { ok: true, range: r.range, outFromPlayhead } : r;
}

/**
 * MRK-05 keyboard retime (`Alt+I` / `Alt+O`): move the selected clip's In or
 * Out to `time`. The other edge stays; the result must still be a valid clip.
 */
export function resolveRetime(
  clip: { start_sec: number; end_sec: number },
  side: MarkSide,
  time: number,
  minDuration: number,
  duration: number,
): RetimeResolution {
  const min = normalizeMinDuration(minDuration);
  const tm = clampToSource(time, duration);
  const start = side === "in" ? tm : clip.start_sec;
  const end = side === "out" ? tm : clip.end_sec;
  const outError: AddError = {
    kind: "out_before_in",
    message: side === "in" ? t("marks.retimeInAfterOut") : t("marks.retimeOutBeforeIn"),
  };
  return checkRange(start, end, min, outError);
}

/**
 * `Shift+I` / `Shift+O`: the time to go to: the pending mark, else the
 * selected clip's edge, else `null` (nothing to go to).
 */
export function markTarget(
  side: MarkSide,
  pending: PendingMarks,
  selected: { start_sec: number; end_sec: number } | null,
): number | null {
  const p = side === "in" ? pending.in : pending.out;
  if (p !== null) return p;
  if (!selected) return null;
  return side === "in" ? selected.start_sec : selected.end_sec;
}
