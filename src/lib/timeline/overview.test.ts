import { describe, expect, it } from "vitest";
import { hitTest, type HitInput } from "./hit";
import timelineMessages from "$lib/i18n/en/timeline";
import { timelineMessageKeys } from "./messages";
import { overviewViewport, timeToX } from "./scale";
import { clipEdges } from "./snap";
import { trimAtPointer } from "./trim";

it("all shared timeline option labels are translated", () => {
  for (const key of Object.values(timelineMessageKeys)) expect(timelineMessages[key]).toBeTruthy();
});

describe("MRK-05 overview trim", () => {
  const vp = overviewViewport(100, 1000);
  const spans = [{ start: 10, end: 20 }, { start: 25, end: 35 }];
  const hit: HitInput = { spans, lanes: [0, 0], maxLanes: 4, geom: { top: 16, laneHeight: 18, gap: 2 },
    vp, playheadX: 100, handles: true, handleIndex: 0 };

  it("selected edges beat the playhead; unselected clips remain selectable", () => {
    expect(hitTest(100, 20, hit)).toEqual({ kind: "handle-start", index: 0 });
    expect(hitTest(200, 20, hit)).toEqual({ kind: "handle-end", index: 0 });
    expect(hitTest(250, 20, hit)).toEqual({ kind: "clip", index: 1 });
    expect(hitTest(100, 20, { ...hit, handles: false })).toEqual({ kind: "clip", index: 0 });
    expect(hitTest(100, 20, { ...hit, handleIndex: -1 })).toEqual({ kind: "clip", index: 0 });
    expect(hitTest(200, 50, hit)).toEqual({ kind: "none" });
  });

  it("maps overview pixels to time and enforces minimum duration and source bounds", () => {
    const trim = (edge: "start" | "end", time: number) => trimAtPointer(timeToX(time, vp), vp, edge, spans[0], 100, 0.1, null).time;
    expect(trim("start", 15.123)).toBe(15.123);
    expect(trim("start", 30)).toBe(19.9);
    expect(trim("start", -5)).toBe(0);
    expect(trim("end", 0)).toBe(10.1);
    expect(trim("end", 110)).toBe(100);
  });

  it("Shift snaps to playhead, keyframes and other clips within eight overview pixels", () => {
    const context = { playhead: 15, keyframes: [12, 18], edges: clipEdges(spans, 0) };
    const trim = (time: number) => trimAtPointer(timeToX(time, vp), vp, "end", spans[0], 100, 0.1, context);
    expect(trim(15.6)).toEqual({ time: 15, kind: "playhead" });
    expect(trim(18.6)).toEqual({ time: 18, kind: "keyframe" });
    expect(trim(25.6)).toEqual({ time: 25, kind: "edge" });
    expect(trim(26)).toEqual({ time: 26, kind: null });
    expect(trimAtPointer(156, vp, "end", spans[0], 100, 0.1, null)).toEqual({ time: 15.6, kind: null });
    expect(trimAtPointer(199, vp, "start", spans[0], 100, 0.1, { ...context, playhead: 20 })).toEqual({ time: 19.9, kind: null });
  });
});
