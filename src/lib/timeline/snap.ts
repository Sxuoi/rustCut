// Snapping candidates for Shift-drag trimming (MRK-05).

import type { Span } from "./lanes";
import { nearestSorted } from "./ticks";

export type SnapKind = "playhead" | "keyframe" | "edge";

export interface SnapContext {
  playhead: number;
  /** Ascending keyframe times. */
  keyframes: ArrayLike<number>;
  /** Edges (start and end) of the other clips, any order. */
  edges: ArrayLike<number>;
}

export interface SnapResult {
  time: number;
  kind: SnapKind | null;
}

/** Start and end of every clip except `exclude`. */
export function clipEdges(spans: readonly Span[], exclude: number): number[] {
  const out: number[] = [];
  for (let i = 0; i < spans.length; i++) {
    if (i === exclude) continue;
    out.push(spans[i].start, spans[i].end);
  }
  return out;
}

/**
 * Snap `t` to the closest candidate within `threshold` seconds. On equal
 * distance the playhead beats keyframes, which beat clip edges.
 */
export function snapTime(t: number, ctx: SnapContext, threshold: number): SnapResult {
  let best: SnapResult = { time: t, kind: null };
  let bestDist = threshold + 1e-12;
  const consider = (cand: number | null, kind: SnapKind) => {
    if (cand === null) return;
    const d = Math.abs(cand - t);
    if (d < bestDist - 1e-12) {
      bestDist = d;
      best = { time: cand, kind };
    }
  };
  consider(ctx.playhead, "playhead");
  consider(nearestSorted(ctx.keyframes, t), "keyframe");
  let nearEdge: number | null = null;
  for (let i = 0; i < ctx.edges.length; i++) {
    if (nearEdge === null || Math.abs(ctx.edges[i] - t) < Math.abs(nearEdge - t)) nearEdge = ctx.edges[i];
  }
  consider(nearEdge, "edge");
  return best;
}
