import { describe, expect, it } from "vitest";
import { contrastText, parseHex, tagLetter } from "./colors";
import { HANDLE_PX, hitTest, type HitInput } from "./hit";
import { assignLanes } from "./lanes";
import { clipEdges, snapTime } from "./snap";
import { clampWindow, overviewViewport, pxPerSec, timeToX, xToTime, zoomAround, zoomViewport } from "./scale";
import { forEachFrameTick, forEachTick, formatTick, frameStride, nearestSorted, niceStep, rulerStep, sortedRange } from "./ticks";
import { clampTrim } from "./trim";
import { WaveColumns, computeColumns } from "./waveform";

describe("scale (PLY-06)", () => {
  it("overview maps the whole duration", () => {
    const vp = overviewViewport(100, 500);
    expect(timeToX(50, vp)).toBe(250);
    expect(xToTime(500, vp)).toBe(100);
  });

  it("zoom window is centred on the playhead", () => {
    const vp = zoomViewport(30, 10, 1000);
    expect([vp.start, vp.end]).toEqual([25, 35]);
    expect(timeToX(30, vp)).toBe(500);
    expect(pxPerSec(vp)).toBe(100);
  });

  it("clamps the window to 2 s .. 10 min", () => {
    expect(clampWindow(0.1)).toBe(2);
    expect(clampWindow(99999)).toBe(600);
    expect(clampWindow(Number.NaN)).toBeGreaterThanOrEqual(2);
  });

  it("wheel zoom keeps the time under the cursor fixed", () => {
    const view = { center: 100, window: 20 };
    for (const frac of [0, 0.25, 0.8, 1]) {
      const before = zoomViewport(view.center, view.window, 800);
      const t = xToTime(frac * 800, before);
      const z = zoomAround(view, frac, 0.5);
      const after = zoomViewport(z.center, z.window, 800);
      expect(z.window).toBe(10);
      expect(xToTime(frac * 800, after)).toBeCloseTo(t, 9);
    }
  });

  it("wheel zoom stops at the limits", () => {
    expect(zoomAround({ center: 0, window: 2 }, 0.5, 0.1).window).toBe(2);
    expect(zoomAround({ center: 0, window: 600 }, 0.5, 10).window).toBe(600);
  });
});

describe("ticks", () => {
  it("picks nice steps", () => {
    expect(niceStep(0.003)).toBe(0.005);
    expect(niceStep(7)).toBe(10);
    expect(niceStep(61)).toBe(120);
    expect(rulerStep(10800, 1000)).toBe(900);
  });

  it("enumerates ticks inside the window", () => {
    const got: number[] = [];
    forEachTick(0.5, 3.2, 1, (t) => got.push(t));
    expect(got).toEqual([1, 2, 3]);
  });

  it("formats labels with sub-second precision when needed", () => {
    expect(formatTick(75, 5)).toBe("1:15");
    expect(formatTick(3725, 5)).toBe("1:02:05");
    expect(formatTick(1.5, 0.5)).toBe("0:01.5");
    expect(formatTick(1.04, 0.02)).toBe("0:01.04");
  });

  it("draws frame ticks only when a frame is wide enough", () => {
    expect(frameStride(1 / 30, 100)).toBe(0);
    expect(frameStride(1 / 30, 300)).toBe(1);
    const got: number[] = [];
    forEachFrameTick(0, 0.1, 0.04, (t) => got.push(t));
    expect(got.length).toBe(3);
  });

  it("finds keyframe ranges and nearest values by binary search", () => {
    const k = Float64Array.from([0, 2, 4, 6, 8]);
    expect(sortedRange(k, 2, 6)).toEqual([1, 4]);
    expect(sortedRange(k, 2.1, 3.9)).toEqual([2, 2]);
    expect(nearestSorted(k, 4.9)).toBe(4);
    expect(nearestSorted(k, 5.1)).toBe(6);
    expect(nearestSorted(new Float64Array(), 1)).toBeNull();
  });
});

describe("hitTest (MRK-02, MRK-05)", () => {
  const spans = [
    { start: 10, end: 20 },
    { start: 15, end: 30 },
  ];
  const layout = assignLanes(spans);
  const vp = overviewViewport(100, 1000); // 10 px/s
  const input = (handles: boolean): HitInput => ({
    spans,
    lanes: layout.lane,
    maxLanes: 4,
    geom: { top: 10, laneHeight: 12, gap: 2 },
    vp,
    playheadX: 800,
    handles,
  });

  it("hits the clip body in the right lane", () => {
    expect(hitTest(120, 12, input(false))).toEqual({ kind: "clip", index: 0 });
    expect(hitTest(250, 24, input(false))).toEqual({ kind: "clip", index: 1 });
    expect(hitTest(250, 12, input(false))).toEqual({ kind: "none" });
  });

  it("detects trim handles within 6 px when enabled", () => {
    expect(hitTest(100 - HANDLE_PX, 12, input(true))).toEqual({ kind: "handle-start", index: 0 });
    expect(hitTest(200 + 5, 12, input(true))).toEqual({ kind: "handle-end", index: 0 });
    expect(hitTest(200 + 7, 12, input(true))).toEqual({ kind: "none" });
    expect(hitTest(100 - HANDLE_PX, 12, input(false))).toEqual({ kind: "none" });
  });

  it("detects the playhead", () => {
    expect(hitTest(802, 40, input(true))).toEqual({ kind: "playhead" });
  });
});

describe("snapTime (MRK-05)", () => {
  const ctx = { playhead: 10, keyframes: Float64Array.from([0, 4, 8, 12]), edges: [20, 30] };
  it("snaps to the nearest candidate within the threshold", () => {
    expect(snapTime(10.2, ctx, 0.5)).toEqual({ time: 10, kind: "playhead" });
    expect(snapTime(7.8, ctx, 0.5)).toEqual({ time: 8, kind: "keyframe" });
    expect(snapTime(20.3, ctx, 0.5)).toEqual({ time: 20, kind: "edge" });
  });
  it("does not snap outside the threshold", () => {
    expect(snapTime(15, ctx, 0.5)).toEqual({ time: 15, kind: null });
  });
  it("prefers the playhead over a keyframe on ties", () => {
    const c = { playhead: 8, keyframes: Float64Array.from([8]), edges: [8] };
    expect(snapTime(8.1, c, 0.5).kind).toBe("playhead");
  });
  it("collects other clips' edges", () => {
    expect(clipEdges([{ start: 1, end: 2 }, { start: 3, end: 4 }], 0)).toEqual([3, 4]);
  });
});

describe("clampTrim", () => {
  const clip = { start: 10, end: 20 };
  it("keeps the minimum duration and the source bounds", () => {
    expect(clampTrim("start", 19.9, clip, 100, 0.5)).toBe(19.5);
    expect(clampTrim("start", -5, clip, 100, 0.5)).toBe(0);
    expect(clampTrim("end", 10.1, clip, 100, 0.5)).toBe(10.5);
    expect(clampTrim("end", 500, clip, 100, 0.5)).toBe(100);
    expect(clampTrim("end", 12.34567, clip, 100, 0.5)).toBe(12.346);
  });
});

describe("colors (MRK-06)", () => {
  it("parses hex and chooses readable text", () => {
    expect(parseHex("#ff0000")).toEqual([255, 0, 0]);
    expect(parseHex("red")).toBeNull();
    expect(contrastText("#ffff00")).toBe("#000000");
    expect(contrastText("#1d4ed8")).toBe("#ffffff");
  });
  it("derives a tag letter", () => {
    expect(tagLetter("funny")).toBe("F");
    expect(tagLetter("  ")).toBe("");
    expect(tagLetter(null)).toBe("");
  });
});

describe("waveform columns (PLY-05)", () => {
  const peaks = new Int8Array([-10, 10, -20, 20, -30, 30, -40, 40]);
  const src = { peaksPerSec: 1, totalPeaks: 4, peaks, filled: Uint8Array.from([1, 1, 0, 0]) };

  it("reduces to min/max per column and skips unfilled peaks", () => {
    const out = new WaveColumns();
    computeColumns(src, 0, 4, 2, out);
    expect([out.has[0], out.min[0], out.max[0]]).toEqual([1, -20, 20]);
    expect(out.has[1]).toBe(0);
    computeColumns(src, 0, 4, 4, out);
    expect([...out.has.slice(0, 4)]).toEqual([1, 1, 0, 0]);
  });

  it("merges several peaks into one column", () => {
    const out = new WaveColumns();
    computeColumns({ ...src, filled: Uint8Array.from([1, 1, 1, 1]) }, 0, 4, 1, out);
    expect([out.min[0], out.max[0]]).toEqual([-40, 40]);
  });
});
