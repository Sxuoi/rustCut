// Clip Queue row logic: include counts (QUE-01, EXP-01), export status badges
// (QUE-05), row flags (duplicate range, out of range, FRM-08 too long), bulk
// include (QUE-04) and click selection (QUE-07). Pure: no Svelte, no IPC.

import type { Clip, ClipProgress, Preset } from "$lib/bindings";
import { toMs } from "./timecode";

/** Clip length in seconds (Out − In, never negative). */
export function clipDuration(c: Pick<Clip, "start_sec" | "end_sec">): number {
  return Math.max(0, c.end_sec - c.start_sec);
}

/** Clips whose include checkbox is ticked, in the given order (EXP-01). */
export function includedClips(clips: readonly Clip[]): Clip[] {
  return clips.filter((c) => c.selected_for_export);
}

/** Number of included clips (header "M included", export button). */
export function countIncluded(clips: readonly Clip[]): number {
  let n = 0;
  for (const c of clips) if (c.selected_for_export) n++;
  return n;
}

/** Badge kinds of QUE-05. */
export type RowStatus = "pending" | "exporting" | "done" | "failed" | "outdated";

/** What a row's status badge shows. */
export interface RowStatusInfo {
  kind: RowStatus;
  /** 0..1 while exporting and progress is known, else `null`. */
  fraction: number | null;
}

function clamp01(v: number): number {
  return Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 0;
}

/**
 * QUE-05 badge for a clip. A clip the running batch reports progress for
 * shows Exporting (with %) even before its status patch arrives.
 */
export function rowStatus(clip: Pick<Clip, "export_status">, progress?: ClipProgress | null): RowStatusInfo {
  if (progress) return { kind: "exporting", fraction: clamp01(progress.fraction) };
  switch (clip.export_status) {
    case "exporting":
      return { kind: "exporting", fraction: null };
    case "completed":
      return { kind: "done", fraction: null };
    case "failed":
      return { kind: "failed", fraction: null };
    case "outdated":
      return { kind: "outdated", fraction: null };
    default:
      return { kind: "pending", fraction: null };
  }
}

/** Failure details of a Failed clip (EXP-10): reason, exit code, log tail. */
export interface FailureInfo {
  reason: string | null;
  exitCode: number | null;
  logTail: string[];
  logPath: string | null;
}

/** EXP-10 details for the Failed badge, or `null` when the clip did not fail. */
export function failureInfo(clip: Pick<Clip, "export_status" | "last_export">): FailureInfo | null {
  if (clip.export_status !== "failed") return null;
  const last = clip.last_export;
  return {
    reason: last?.error ?? null,
    exitCode: last?.exit_code ?? null,
    logTail: last?.log_tail ?? [],
    logPath: last?.log_path ?? null,
  };
}

/** Readable one-line reason of a failed clip (AC-09), `fallback` when none was recorded; `null` if not failed. */
export function failureReasonText(clip: Pick<Clip, "export_status" | "last_export">, fallback: string): string | null {
  const info = failureInfo(clip);
  if (!info) return null;
  const reason = info.reason?.trim();
  return reason ? reason : fallback;
}

/** Output file of a Done clip for *Show in folder* (QUE-05), else `null`. */
export function revealablePath(clip: Pick<Clip, "export_status" | "last_export">): string | null {
  if (clip.export_status !== "completed") return null;
  const path = clip.last_export?.output_path ?? null;
  return path !== null && path !== "" ? path : null;
}

/** Row warnings shown next to the times. */
export type ClipFlag =
  | { kind: "duplicate"; of: number }
  | { kind: "out_of_range" }
  | { kind: "too_long"; maxSec: number };

/** What the flags depend on besides the clips. */
export interface FlagContext {
  /** Probed source duration; `null`/0 = unknown (no out-of-range flag). */
  sourceDurationSec: number | null;
  /** FRM-08 maximum clip duration of the chosen platform preset. */
  maxDurationSec: number | null;
  /** Current export preset; the maximum applies to vertical presets only. */
  preset: Preset | null;
}

/**
 * Flags per clip id (only clips with at least one flag are present):
 * * `duplicate`: another clip has exactly the same In and Out (ms precision);
 *   `of` is the lowest other clip number with that range;
 * * `out_of_range`: In at or after the end of the source (e.g. after a relink
 *   to a shorter file);
 * * `too_long`: longer than the vertical preset's maximum (FRM-08).
 */
export function clipFlags(clips: readonly Clip[], ctx: FlagContext): Map<string, ClipFlag[]> {
  const byRange = new Map<string, number[]>();
  for (const c of clips) {
    const key = `${toMs(c.start_sec)}:${toMs(c.end_sec)}`;
    const list = byRange.get(key);
    if (list) list.push(c.index);
    else byRange.set(key, [c.index]);
  }
  const sourceMs = ctx.sourceDurationSec !== null && ctx.sourceDurationSec > 0 ? toMs(ctx.sourceDurationSec) : null;
  const vertical = ctx.preset === "vcrop" || ctx.preset === "vblur";
  const maxMs = vertical && ctx.maxDurationSec !== null && ctx.maxDurationSec > 0 ? toMs(ctx.maxDurationSec) : null;
  const out = new Map<string, ClipFlag[]>();
  for (const c of clips) {
    const flags: ClipFlag[] = [];
    const same = byRange.get(`${toMs(c.start_sec)}:${toMs(c.end_sec)}`) ?? [];
    const others = same.filter((i) => i !== c.index);
    if (others.length > 0) flags.push({ kind: "duplicate", of: Math.min(...others) });
    if (sourceMs !== null && toMs(c.start_sec) >= sourceMs) flags.push({ kind: "out_of_range" });
    if (maxMs !== null && ctx.maxDurationSec !== null && toMs(c.end_sec) - toMs(c.start_sec) > maxMs) {
      flags.push({ kind: "too_long", maxSec: ctx.maxDurationSec });
    }
    if (flags.length > 0) out.set(c.id, flags);
  }
  return out;
}

/** Bulk include choices of QUE-04. */
export type IncludeMode = "all" | "none" | "not_exported" | "failed";

export const INCLUDE_MODES: readonly IncludeMode[] = ["all", "none", "not_exported", "failed"];

/** Should `clip` be included after choosing `mode`? */
export function wantsInclude(clip: Pick<Clip, "export_status">, mode: IncludeMode): boolean {
  switch (mode) {
    case "all":
      return true;
    case "none":
      return false;
    case "not_exported":
      // Outdated clips were edited after their export: not exported as they are now.
      return clip.export_status !== "completed";
    case "failed":
      return clip.export_status === "failed";
  }
}

/**
 * QUE-04 bulk include: the clips whose include flag must change so that
 * exactly the clips matching `mode` are included. Unchanged clips are left
 * out so the edit (and its undo step) only touches what changes.
 */
export function includeChanges(clips: readonly Clip[], mode: IncludeMode): { include: string[]; exclude: string[] } {
  const include: string[] = [];
  const exclude: string[] = [];
  for (const c of clips) {
    const want = wantsInclude(c, mode);
    if (want && !c.selected_for_export) include.push(c.id);
    else if (!want && c.selected_for_export) exclude.push(c.id);
  }
  return { include, exclude };
}

/** How a click changes the row selection. */
export type ClickMode = "replace" | "toggle" | "range";

/** Shift = range, Ctrl/Cmd = toggle, plain = replace (QUE-07 multi-selection). */
export function clickMode(e: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }): ClickMode {
  if (e.shiftKey) return "range";
  if (e.ctrlKey || e.metaKey) return "toggle";
  return "replace";
}

/**
 * Ids from `anchor` to `target` inclusive, in the order the queue shows them
 * (sorted/filtered view). Without a visible anchor the range is the target.
 */
export function rangeIds(ordered: readonly string[], anchor: string | null, target: string): string[] {
  const b = ordered.indexOf(target);
  if (b < 0) return [];
  const a = anchor === null ? -1 : ordered.indexOf(anchor);
  if (a < 0) return [target];
  const [lo, hi] = a <= b ? [a, b] : [b, a];
  return ordered.slice(lo, hi + 1);
}

/** The selection after a click on `target` (pure version of the queue's click handling). */
export function nextSelection(
  current: ReadonlySet<string>,
  mode: ClickMode,
  ordered: readonly string[],
  anchor: string | null,
  target: string,
): Set<string> {
  if (mode === "replace") return new Set([target]);
  if (mode === "toggle") {
    const next = new Set(current);
    if (next.has(target)) next.delete(target);
    else next.add(target);
    return next;
  }
  return new Set(rangeIds(ordered, anchor, target));
}
