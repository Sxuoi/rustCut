import { describe, expect, it } from "vitest";
import type { Framing, OutputResolution } from "$lib/bindings";
import vectors from "../../../crates/rustcut-framing/tests/crop_vectors.json";
import layouts from "../../../crates/rustcut-framing/tests/layout_vectors.json";
import squares from "../../../crates/rustcut-framing/tests/square_vectors.json";
import {
  blurRadii,
  cropWindow,
  dragWindowX,
  frameLayout,
  offsetForX,
  offsetHasEffect,
  shapeWindow,
  squareWindow,
  windowShape,
} from "./crop";

describe("squareWindow reproduces the §7.7 square vectors (FRM-09)", () => {
  for (const c of squares.cases) {
    it(c.label, () => {
      squares.offsets.forEach((off, i) => {
        expect(squareWindow(c.dw, c.dh, off)).toEqual({ w: c.w, h: c.h, x: c.x[i], y: c.y });
      });
    });
  }

  it("slides only in landscape frames; drag inverse per shape", () => {
    expect(offsetHasEffect(1920, 1080, "1:1")).toBe(true);
    expect(offsetHasEffect(1080, 1080, "1:1")).toBe(false);
    expect(offsetHasEffect(1080, 1920, "1:1")).toBe(false);
    // A 1:1 source still has room for a 9:16 window to slide.
    expect(offsetHasEffect(1080, 1080, "9:16")).toBe(true);
    const r = squareWindow(1920, 1080, 0.3);
    expect(offsetForX(1920, 1080, r.x, "1:1")).toBeCloseTo(0.3, 2);
    expect(shapeWindow("1:1", 1920, 1080, -1)).toEqual({ w: 1080, h: 1080, x: 0, y: 0 });
    expect(shapeWindow("9:16", 1920, 1080, 0)).toEqual(cropWindow(1920, 1080, 0));
    expect(windowShape("vertical_crop")).toBe("9:16");
    expect(windowShape("vertical_square")).toBe("1:1");
    expect(windowShape("square_blur")).toBeNull();
    expect(windowShape("vertical_blur")).toBeNull();
  });
});

describe("frameLayout reproduces rustcut-framing's golden layouts (FRM-04)", () => {
  it(`all ${layouts.cases.length} cases`, () => {
    expect(layouts.cases.length).toBe(270);
    for (const c of layouts.cases) {
      const got = frameLayout(c.framing as Framing, c.dw, c.dh, c.offset, c.output_resolution as OutputResolution);
      expect(got, `${c.framing} ${c.dw}x${c.dh} @${c.offset} ${c.output_resolution}`).toEqual(c.layout);
    }
  });

  it("original framing has no layout", () => {
    expect(frameLayout("original", 1920, 1080, 0, "1080x1920")).toBeNull();
  });

  it("blur radii follow FFmpeg's limit (checked against FFmpeg 9.0.2)", () => {
    expect(blurRadii(270, 480)).toEqual([10, 10]);
    expect(blurRadii(40, 40)).toEqual([10, 9]);
    expect(blurRadii(18, 32)).toEqual([8, 4]);
    expect(blurRadii(2, 2)).toEqual([0, 0]);
  });
});

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
