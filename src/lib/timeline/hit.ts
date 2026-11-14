// Hit testing for clip bars, trim handles (MRK-05) and the playhead.

import type { Span } from "./lanes";
import { timeToX, type Viewport } from "./scale";

/** Vertical layout of the lane stack. */
export interface LaneGeom {
  /** y of the top of lane 0. */
  top: number;
  /** Height of one lane (bar + gap). */
  laneHeight: number;
  /** Gap between stacked bars. */
  gap: number;
}

/** Trim handle grab distance (px) either side of a clip edge. */
export const HANDLE_PX = 6;
/** Playhead grab distance (px). */
export const PLAYHEAD_PX = 3;

export function laneY(lane: number, g: LaneGeom): number {
  return g.top + lane * g.laneHeight;
}

export type Hit =
  | { kind: "handle-start"; index: number }
  | { kind: "handle-end"; index: number }
  | { kind: "clip"; index: number }
  | { kind: "playhead" }
  | { kind: "none" };

export interface HitInput {
  spans: readonly Span[];
  lanes: ArrayLike<number>;
  maxLanes: number;
  geom: LaneGeom;
  vp: Viewport;
  playheadX: number;
  /** Trim handles are active. */
  handles: boolean;
  /** Overview exposes only the selected clip's handles; omitted means all clips. */
  handleIndex?: number;
}

/** Handle -> clip -> playhead -> none, for a pointer at (x, y). */
export function hitTest(x: number, y: number, h: HitInput): Hit {
  const lane = Math.floor((y - h.geom.top) / h.geom.laneHeight);
  if (lane >= 0 && lane < h.maxLanes && y - laneY(lane, h.geom) <= h.geom.laneHeight - h.geom.gap) {
    let handle: Hit = { kind: "none" };
    let handleDist = Infinity;
    let body: Hit = { kind: "none" };
    for (let i = 0; i < h.spans.length; i++) {
      if (h.lanes[i] !== lane) continue;
      const xs = timeToX(h.spans[i].start, h.vp);
      const xe = Math.max(timeToX(h.spans[i].end, h.vp), xs + 2);
      if (h.handles && (h.handleIndex === undefined || h.handleIndex === i)) {
        const ds = Math.abs(x - xs);
        const de = Math.abs(x - xe);
        const d = Math.min(ds, de);
        if (d <= HANDLE_PX && d < handleDist) {
          handleDist = d;
          handle = { kind: ds <= de ? "handle-start" : "handle-end", index: i };
        }
      }
      if (body.kind === "none" && x >= xs && x <= xe) body = { kind: "clip", index: i };
    }
    if (handle.kind !== "none") return handle;
    if (body.kind !== "none") return body;
  }
  if (Math.abs(x - h.playheadX) <= PLAYHEAD_PX) return { kind: "playhead" };
  return { kind: "none" };
}
