import { describe, expect, it } from "vitest";
import { assignLanes, duplicateFlags, type Span } from "./lanes";

const s = (start: number, end: number): Span => ({ start, end });

describe("assignLanes (MRK-02)", () => {
  it("puts disjoint and touching clips in lane 0", () => {
    const r = assignLanes([s(0, 10), s(10, 20), s(30, 40)]);
    expect([...r.lane]).toEqual([0, 0, 0]);
    expect(r.laneCount).toBe(1);
    expect(r.overflow).toEqual([]);
  });

  it("stacks overlaps greedily by start, independent of input order", () => {
    const r = assignLanes([s(5, 15), s(0, 10), s(12, 20)]);
    // sorted: [0,10)->0, [5,15)->1, [12,20)->0 (lane 0 free at 10)
    expect([...r.lane]).toEqual([1, 0, 0]);
    expect(r.laneCount).toBe(2);
  });

  it("is stable for identical ranges (input order decides)", () => {
    const r = assignLanes([s(0, 5), s(0, 5), s(0, 5)]);
    expect([...r.lane]).toEqual([0, 1, 2]);
  });

  it("shows at most 4 lanes and counts hidden clips per region as +N", () => {
    const spans = [s(0, 100), s(0, 100), s(0, 100), s(0, 100), s(10, 20), s(15, 30), s(200, 210)];
    const r = assignLanes(spans);
    expect(r.laneCount).toBe(4);
    expect([...r.lane].slice(4, 6)).toEqual([4, 5]);
    expect(r.overflow).toEqual([{ start: 10, end: 30, count: 2 }]);
  });

  it("splits overflow into separate regions when hidden clips do not overlap", () => {
    const base = [s(0, 100), s(0, 100), s(0, 100), s(0, 100)];
    const spans = [...base, s(10, 20), s(50, 60), s(300, 400), s(300, 400), s(300, 400), s(300, 400), s(310, 320)];
    const r = assignLanes(spans);
    expect(r.overflow.map((o) => o.count)).toEqual([1, 1, 1]);
  });

  it("handles an empty list", () => {
    const r = assignLanes([]);
    expect(r.laneCount).toBe(0);
    expect(r.overflow).toEqual([]);
  });

  it("micro-benchmark: 500 overlapping clips in < 5 ms", () => {
    const spans: Span[] = [];
    for (let i = 0; i < 500; i++) spans.push(s(i * 7, i * 7 + 120 + (i % 13) * 10));
    assignLanes(spans); // warm up
    const t0 = performance.now();
    const r = assignLanes(spans);
    const dt = performance.now() - t0;
    expect(r.lane.length).toBe(500);
    expect(dt).toBeLessThan(5);
  });
});

describe("duplicateFlags (MRK-03)", () => {
  it("flags every clip sharing an identical range", () => {
    const f = duplicateFlags([s(1, 2), s(3, 4), s(1.0004, 2.0004), s(1, 2), s(1, 2.5)]);
    expect([...f]).toEqual([1, 0, 1, 1, 0]);
  });
});
