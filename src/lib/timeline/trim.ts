// Trim-handle clamping (MRK-05).

import { pxPerSec, xToTime, type Viewport } from "./scale";
import { snapTime, type SnapContext, type SnapResult } from "./snap";

export type TrimEdge = "start" | "end";

/** Both timelines use an eight-pixel Shift snap radius, then enforce valid clip bounds. */
export function trimAtPointer(
  x: number, vp: Viewport, edge: TrimEdge, clip: { start: number; end: number },
  duration: number, minDuration: number, snap: SnapContext | null,
): SnapResult {
  const time = xToTime(x, vp);
  const pps = pxPerSec(vp);
  const result = snap ? snapTime(time, snap, pps > 0 ? 8 / pps : 0) : { time, kind: null };
  const clamped = clampTrim(edge, result.time, clip, duration, minDuration);
  return { time: clamped, kind: Math.abs(clamped - result.time) < 0.0005 ? result.kind : null };
}

/**
 * Clamp a dragged edge so the clip stays inside [0, duration] and at least
 * `minDuration` long. Returns the clamped time (ms precision).
 */
export function clampTrim(
  edge: TrimEdge,
  t: number,
  clip: { start: number; end: number },
  duration: number,
  minDuration: number,
): number {
  const hi = duration > 0 ? duration : Infinity;
  const v =
    edge === "start"
      ? Math.max(0, Math.min(t, clip.end - minDuration))
      : Math.min(hi, Math.max(t, clip.start + minDuration));
  return Math.round(v * 1000) / 1000;
}
