import { describe, expect, it } from "vitest";
import vectors from "../../../crates/rustcut-framing/tests/crop_vectors.json";
import { cropWindow, dragWindowX, offsetForX, offsetHasEffect } from "./crop";

describe("cropWindow reproduces PRD §7.7 vectors (AC-18)", () => {
  for (const c of vectors.cases) {
    it(c.label, () => {
      vectors.offsets.forEach((off, i) => {
        const r = cropWindow(c.dw, c.dh, off);
        expect(r).toEqual({ w: c.w, h: c.h, x: c.x[i], y: c.y });
      });
    });
  }

  it("drag inverse and full-width detection", () => {
    const r = cropWindow(1920, 1080, 0.3);
    expect(offsetForX(1920, 1080, r.x)).toBeCloseTo(0.3, 2);
    expect(offsetHasEffect(1080, 1920)).toBe(false);
    expect(offsetHasEffect(1920, 1080)).toBe(true);
  });
});

describe("dragWindowX", () => {
  it("full-frame: window follows the pointer", () => {
    expect(dragWindowX(100, 50, 0.5, false)).toBe(200);
  });
  it("9:16-only: content follows the pointer (offset moves the other way)", () => {
    expect(dragWindowX(100, 50, 0.5, true)).toBe(0);
    // dragging right reveals content to the left => smaller window x => smaller offset
    const start = cropWindow(1920, 1080, 0.3).x;
    expect(offsetForX(1920, 1080, dragWindowX(start, 40, 1, true))).toBeLessThan(0.3);
  });
});
