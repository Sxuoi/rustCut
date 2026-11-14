import { describe, expect, it } from "vitest";
import { revealScrollTop, virtualWindow } from "./virtual";

describe("QUE-08 virtualWindow", () => {
  it("renders only the visible rows plus overscan at 500 clips", () => {
    const w = virtualWindow({ scrollTop: 0, viewportHeight: 520, rowHeight: 52, count: 500, overscan: 4 });
    expect(w).toEqual({ start: 0, end: 14, offsetTop: 0, totalHeight: 26000 });
    expect(w.end - w.start).toBeLessThan(20);
  });

  it("windows the middle of the list", () => {
    const w = virtualWindow({ scrollTop: 5200, viewportHeight: 520, rowHeight: 52, count: 500, overscan: 4 });
    expect(w.start).toBe(96);
    expect(w.end).toBe(114);
    expect(w.offsetTop).toBe(96 * 52);
  });

  it("covers a partially visible last row", () => {
    const w = virtualWindow({ scrollTop: 26, viewportHeight: 100, rowHeight: 52, count: 10, overscan: 0 });
    // rows 0 (half), 1, 2 (partial) are visible
    expect([w.start, w.end]).toEqual([0, 3]);
  });

  it("clamps overscroll and a list that shrank under the scroll position", () => {
    const w = virtualWindow({ scrollTop: 99999, viewportHeight: 520, rowHeight: 52, count: 20, overscan: 2 });
    expect(w.end).toBe(20);
    expect(w.start).toBe(8);
    const neg = virtualWindow({ scrollTop: -40, viewportHeight: 104, rowHeight: 52, count: 20, overscan: 0 });
    expect([neg.start, neg.end]).toEqual([0, 2]);
  });

  it("handles empty lists, unmeasured viewports and bad input", () => {
    expect(virtualWindow({ scrollTop: 0, viewportHeight: 500, rowHeight: 52, count: 0 })).toEqual({
      start: 0,
      end: 0,
      offsetTop: 0,
      totalHeight: 0,
    });
    const unmeasured = virtualWindow({ scrollTop: 0, viewportHeight: 0, rowHeight: 52, count: 100, overscan: 3 });
    expect([unmeasured.start, unmeasured.end]).toEqual([0, 4]);
    const nan = virtualWindow({ scrollTop: Number.NaN, viewportHeight: 104, rowHeight: 52, count: 5, overscan: 0 });
    expect([nan.start, nan.end]).toEqual([0, 2]);
  });

  it("never renders past the end with a short list", () => {
    const w = virtualWindow({ scrollTop: 0, viewportHeight: 1000, rowHeight: 52, count: 3 });
    expect([w.start, w.end, w.totalHeight]).toEqual([0, 3, 156]);
  });
});

describe("QUE-03 revealScrollTop", () => {
  const base = { rowHeight: 52, viewportHeight: 520 };

  it("returns null when the row is fully visible", () => {
    expect(revealScrollTop({ ...base, index: 3, scrollTop: 0 })).toBeNull();
    expect(revealScrollTop({ ...base, index: 9, scrollTop: 0 })).toBeNull();
  });

  it("scrolls up to align a row above the viewport to the top", () => {
    expect(revealScrollTop({ ...base, index: 2, scrollTop: 300 })).toBe(104);
  });

  it("scrolls down just enough for a row below the viewport", () => {
    expect(revealScrollTop({ ...base, index: 10, scrollTop: 0 })).toBe(52);
    expect(revealScrollTop({ ...base, index: 20, scrollTop: 0 })).toBe(21 * 52 - 520);
  });

  it("aligns a row taller than the viewport to its top and ignores missing rows", () => {
    expect(revealScrollTop({ index: 4, rowHeight: 52, scrollTop: 0, viewportHeight: 30 })).toBe(208);
    expect(revealScrollTop({ ...base, index: -1, scrollTop: 0 })).toBeNull();
  });
});
