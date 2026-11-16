import { describe, expect, it } from "vitest";
import type { FrameLayout } from "$lib/bindings";
import { BOXBLUR_SIGMA, blurPlan, boxblurSigma, outputView, paintBlurSource, type PaintContext } from "./blurPreview";
import { frameLayout } from "./crop";

const square = frameLayout("vertical_square", 1920, 1080, 0, "1080x1920") as FrameLayout;
const flat = frameLayout("square_blur", 1920, 1080, 0, "1080x1920") as FrameLayout;
const crop = frameLayout("vertical_crop", 1920, 1080, 0.3, "1080x1920") as FrameLayout;

describe("blurPlan matches FFmpeg's quarter-resolution boxblur=10:2", () => {
  it("blur deviation of two 21-tap boxes", () => {
    expect(BOXBLUR_SIGMA).toBeCloseTo(8.563, 3);
    expect(boxblurSigma(0)).toBe(0);
    expect(boxblurSigma(5)).toBeCloseTo(Math.sqrt(20), 9);
  });

  it("tiny outputs blur with the layout's smaller radius", () => {
    const tiny = frameLayout("vertical_square", 128, 128, 0, "native") as FrameLayout;
    expect(tiny.blur).toMatchObject({ w: 32, h: 56, radius: 10 });
    const e = frameLayout("vertical_blur", 160, 90, 0, "native") as FrameLayout;
    expect(e.blur).toMatchObject({ w: 12, h: 22, radius: 5 });
    const p = blurPlan(e, 50, 90);
    // σ = boxblurSigma(5) blur px, shown at 50 / 12 CSS px per blur px.
    expect(p?.blurPx).toBeCloseTo((boxblurSigma(5) * 50) / 12, 6);
  });

  it("scales the deviation to the shown box: σ_out = 8.56 × out_w / blur_w", () => {
    // A 270-px-wide on-screen box shows the 1080-wide output at 1/4 scale.
    const p = blurPlan(square, 270, 480);
    expect(p).not.toBeNull();
    if (!p) return;
    expect([p.contentW, p.contentH]).toEqual([135, 240]);
    // 8.56 × (1080 / 270) output px = 34.25 output px = 8.56 CSS px at 1/4 scale.
    expect(p.blurPx).toBeCloseTo(BOXBLUR_SIGMA * (270 / 270), 6);
    expect(p.margin).toBeGreaterThanOrEqual(Math.ceil((BOXBLUR_SIGMA / 2) * 3));
    // The margin lies outside the box, symmetrically, on the same scale as the content.
    expect(p.left).toBeCloseTo(-p.margin * 2, 6);
    expect(p.width).toBeCloseTo(270 + 4 * p.margin, 6);
    expect(p.top).toBeCloseTo(-p.margin * 2, 6);
    expect(p.height).toBeCloseTo(480 + 4 * p.margin, 6);
  });

  it("square output gets a square canvas; no background, no plan", () => {
    const p = blurPlan(flat, 300, 300);
    expect(p && [p.contentW, p.contentH]).toEqual([135, 135]);
    expect(blurPlan(crop, 300, 500)).toBeNull();
    expect(blurPlan(square, 0, 100)).toBeNull();
  });
});

describe("paintBlurSource", () => {
  function fake(): { ctx: PaintContext; calls: unknown[][] } {
    const calls: unknown[][] = [];
    return {
      ctx: {
        drawImage: (...args: unknown[]) => void calls.push(["draw", ...args]),
        setTransform: (...args: number[]) => void calls.push(["transform", ...args]),
      },
      calls,
    };
  }
  const canvas = { id: "canvas" } as unknown as CanvasImageSource;

  it("maps display px to the decoded video size and mirrors the edges into the margin", () => {
    const plan = blurPlan(square, 270, 480);
    if (!plan || !square.blur) throw new Error("plan");
    // A half-size proxy: 960x540 decoded for a 1920x1080 display frame.
    const video = { videoWidth: 960, videoHeight: 540 } as unknown as CanvasImageSource & {
      videoWidth: number;
      videoHeight: number;
    };
    const { ctx, calls } = fake();
    expect(paintBlurSource(ctx, canvas, video, square.blur.source, plan, 1920, 1080)).toBe(true);
    const [m, cw, ch] = [plan.margin, plan.contentW, plan.contentH];
    const s = square.blur.source; // 608x1080 at 656
    const fullH = ch + 2 * m;
    expect(calls).toEqual([
      ["draw", video, s.x / 2, s.y / 2, s.w / 2, s.h / 2, m, m, cw, ch],
      // The margin is narrower than the content: mirror only, no stretching.
      ["transform", 1, 0, 0, -1, 0, 2 * m],
      ["draw", canvas, m, m, cw, m, m, m, cw, m],
      ["transform", 1, 0, 0, -1, 0, 2 * (m + ch)],
      ["draw", canvas, m, m + ch - m, cw, m, m, m + ch - m, cw, m],
      ["transform", -1, 0, 0, 1, 2 * m, 0],
      ["draw", canvas, m, 0, m, fullH, m, 0, m, fullH],
      ["transform", -1, 0, 0, 1, 2 * (m + cw), 0],
      ["draw", canvas, m + cw - m, 0, m, fullH, m + cw - m, 0, m, fullH],
      ["transform", 1, 0, 0, 1, 0, 0],
    ]);
  });

  it("a margin wider than a tiny content is stretched first, then mirrored as far as the content goes", () => {
    // 160x90 at Native: a 90x90 square blurred at 22x22, an 11 px canvas with a 13 px margin.
    const tiny = frameLayout("square_blur", 160, 90, 0, "native") as FrameLayout;
    const plan = blurPlan(tiny, 90, 90);
    if (!plan || !tiny.blur) throw new Error("plan");
    expect(plan.margin).toBeGreaterThan(plan.contentH);
    const video = { videoWidth: 160, videoHeight: 90 } as unknown as CanvasImageSource & { videoWidth: number; videoHeight: number };
    const { ctx, calls } = fake();
    paintBlurSource(ctx, canvas, video, tiny.blur.source, plan, 160, 90);
    const draws = calls.filter((c) => c[0] === "draw");
    expect(draws).toHaveLength(1 + 4 + 4);
    // The mirrored top band is only as tall as the content.
    expect(draws[5]).toEqual(["draw", canvas, plan.margin, plan.margin, plan.contentW, plan.contentH, plan.margin, plan.margin, plan.contentW, plan.contentH]);
  });

  it("does nothing before the first decoded frame", () => {
    const plan = blurPlan(square, 270, 480);
    if (!plan || !square.blur) throw new Error("plan");
    const { ctx, calls } = fake();
    const empty = { videoWidth: 0, videoHeight: 0 } as unknown as CanvasImageSource & { videoWidth: number; videoHeight: number };
    expect(paintBlurSource(ctx, canvas, empty, square.blur.source, plan, 1920, 1080)).toBe(false);
    expect(calls).toHaveLength(0);
  });
});

describe("outputView (the 9:16 only / 1:1 only geometry)", () => {
  it("Square on Blur: the whole 9:16 frame, the square at y = 420/1920 of it", () => {
    const v = outputView(square, 1000, 960);
    if (!v) throw new Error("view");
    // 960 tall player: the 9:16 frame is 540 x 960, centred.
    expect(v.out).toEqual({ left: 230, top: 0, width: 540, height: 960 });
    expect(v.fg.left).toBeCloseTo(230, 9);
    expect(v.fg.top).toBeCloseTo(210, 9);
    expect(v.fg.width).toBeCloseTo(540, 9);
    expect(v.fg.height).toBeCloseTo(540, 9);
    // The 1080 px window fills the 540 px square: half scale both ways.
    expect(v.vx).toBeCloseTo(0.5, 9);
    expect(v.vy).toBeCloseTo(0.5, 9);
  });

  it("Square Blur: the frame fitted inside the square output", () => {
    const v = outputView(flat, 600, 600);
    if (!v) throw new Error("view");
    expect(v.out).toEqual({ left: 0, top: 0, width: 600, height: 600 });
    // 1080x608 at y=236 of 1080, scaled by 600/1080.
    expect(v.fg.top).toBeCloseTo((236 * 600) / 1080, 9);
    expect(v.fg.height).toBeCloseTo((608 * 600) / 1080, 9);
    // The export stretches 1920x1080 to 1080x608: the preview stretches the same way.
    expect(v.vx).toBeCloseTo(600 / 1920, 9);
    expect(v.vy).toBeCloseTo((608 * 600) / 1080 / 1080, 9);
  });

  it("Vertical Crop keeps the window's own aspect (as the 9:16 only view always did)", () => {
    const v = outputView(crop, 1000, 960);
    if (!v) throw new Error("view");
    expect(v.out).toBe(v.fg);
    expect(v.out.height).toBe(960);
    expect(v.out.width).toBeCloseTo((608 * 960) / 1080, 9);
    expect(v.vx).toBeCloseTo(v.vy, 12);
  });

  it("empty boxes have no view", () => {
    expect(outputView(square, 0, 100)).toBeNull();
  });
});
