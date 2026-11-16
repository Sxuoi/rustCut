// Player preview of the blurred background (FRM-05, FRM-09). The layout's
// background source is drawn from the player's own <video> into a small
// canvas (no second video element: playback memory, docs/PERFORMANCE.md) and
// blurred with CSS so it looks like FFmpeg's quarter-resolution
// `boxblur=10:2`, scaled up to the output.

import type { CropWindow, FrameLayout } from "$lib/bindings";

/**
 * Standard deviation of `boxblur=r:2` in blur-working pixels: two passes of
 * a (2r + 1)-tap box, each with variance ((2r + 1)² − 1) / 12.
 */
export function boxblurSigma(radius: number): number {
  const taps = 2 * Math.max(0, radius) + 1;
  return Math.sqrt((2 * (taps * taps - 1)) / 12);
}

/** `boxblur=10:2`, the export's blur at every regular output size. */
export const BOXBLUR_SIGMA = boxblurSigma(10);

/** Canvas size and CSS that reproduce the export's background in an output box. */
export interface BlurPlan {
  /** Canvas pixels that hold the background source (half the FFmpeg blur size). */
  contentW: number;
  contentH: number;
  /** Edge-replicated border (canvas px) so the CSS blur never fades into transparency. */
  margin: number;
  /** Canvas element box relative to the output box (CSS px); the margin lies outside it. */
  left: number;
  top: number;
  width: number;
  height: number;
  /** CSS `blur()` radius (a Gaussian standard deviation, CSS px). */
  blurPx: number;
}

/** `null` when the layout has no background or the box is empty. */
export function blurPlan(layout: FrameLayout, boxW: number, boxH: number): BlurPlan | null {
  const b = layout.blur;
  if (!b || !(boxW > 0 && boxH > 0)) return null;
  const contentW = Math.max(8, Math.round(b.w / 2));
  const contentH = Math.max(8, Math.round(b.h / 2));
  // The blur in canvas pixels (the layout's radius: 10 unless the output is
  // tiny), and a border wide enough for three deviations.
  const sigma = (boxblurSigma(b.radius) * contentW) / b.w;
  const margin = Math.ceil(sigma * 3);
  const sx = boxW / contentW;
  const sy = boxH / contentH;
  return {
    contentW,
    contentH,
    margin,
    left: -margin * sx,
    top: -margin * sy,
    width: (contentW + 2 * margin) * sx,
    height: (contentH + 2 * margin) * sy,
    blurPx: sigma * sx,
  };
}

/** The 2D-context calls painting needs (a fake in tests). */
export interface PaintContext {
  drawImage(
    image: CanvasImageSource,
    sx: number,
    sy: number,
    sw: number,
    sh: number,
    dx: number,
    dy: number,
    dw: number,
    dh: number,
  ): void;
  setTransform(a: number, b: number, c: number, d: number, e: number, f: number): void;
}

/**
 * Draw `source` (display-frame px) from the video into the canvas content
 * area, then mirror the content's outer rows and columns into the margin, as
 * FFmpeg's boxblur mirrors the picture at its edges: the CSS blur then sees
 * the same neighbours as the export instead of fading into a dark rim. A
 * margin wider than the content (tiny outputs only) is first filled by
 * stretching the edge rows and columns. Returns false when the video has no
 * frame yet.
 */
export function paintBlurSource(
  ctx: PaintContext,
  canvas: CanvasImageSource,
  video: CanvasImageSource & { videoWidth: number; videoHeight: number },
  source: CropWindow,
  plan: BlurPlan,
  displayW: number,
  displayH: number,
): boolean {
  if (!(video.videoWidth > 0 && video.videoHeight > 0 && displayW > 0 && displayH > 0)) return false;
  if (!(source.w > 0 && source.h > 0)) return false;
  // The element may decode a proxy of another size: map display px to video px.
  const kx = video.videoWidth / displayW;
  const ky = video.videoHeight / displayH;
  const { contentW: cw, contentH: ch, margin: m } = plan;
  ctx.drawImage(video, source.x * kx, source.y * ky, source.w * kx, source.h * ky, m, m, cw, ch);
  if (m > 0) {
    const fullH = ch + 2 * m;
    if (m > ch || m > cw) {
      ctx.drawImage(canvas, m, m, cw, 1, m, 0, cw, m);
      ctx.drawImage(canvas, m, m + ch - 1, cw, 1, m, m + ch, cw, m);
      ctx.drawImage(canvas, m, 0, 1, fullH, 0, 0, m, fullH);
      ctx.drawImage(canvas, m + cw - 1, 0, 1, fullH, m + cw, 0, m, fullH);
    }
    const my = Math.min(m, ch);
    const mx = Math.min(m, cw);
    // Each band is drawn onto itself under a flip about the content edge.
    ctx.setTransform(1, 0, 0, -1, 0, 2 * m);
    ctx.drawImage(canvas, m, m, cw, my, m, m, cw, my);
    ctx.setTransform(1, 0, 0, -1, 0, 2 * (m + ch));
    ctx.drawImage(canvas, m, m + ch - my, cw, my, m, m + ch - my, cw, my);
    // Left and right over the full height, so the corners mirror both ways.
    ctx.setTransform(-1, 0, 0, 1, 2 * m, 0);
    ctx.drawImage(canvas, m, 0, mx, fullH, m, 0, mx, fullH);
    ctx.setTransform(-1, 0, 0, 1, 2 * (m + cw), 0);
    ctx.drawImage(canvas, m + cw - mx, 0, mx, fullH, m + cw - mx, 0, mx, fullH);
    ctx.setTransform(1, 0, 0, 1, 0, 0);
  }
  return true;
}

/** Player-box rectangles of the output-only view (CSS px). */
export interface OutputView {
  /** The whole output frame, contained in the player box. */
  out: { left: number; top: number; width: number; height: number };
  /** Where the sharp window is drawn (the clipping box of the video). */
  fg: { left: number; top: number; width: number; height: number };
  /** CSS px per display-frame px of the video, horizontally and vertically. */
  vx: number;
  vy: number;
}

/**
 * Geometry of the output-only view for a player box of `boxW`×`boxH`. A
 * layout without background (Vertical Crop) keeps the window's own aspect, as
 * the 9:16-only view always did; blur layouts show the whole output frame
 * with the window at its place. The video stretches exactly like the export's
 * `scale` (the window fills `fg`).
 */
export function outputView(layout: FrameLayout, boxW: number, boxH: number): OutputView | null {
  const win = layout.window;
  if (!(boxW > 0 && boxH > 0 && win.w > 0 && win.h > 0 && layout.out_w > 0 && layout.out_h > 0)) return null;
  const contain = (w: number, h: number) => {
    const s = Math.min(boxW / w, boxH / h);
    return { left: (boxW - w * s) / 2, top: (boxH - h * s) / 2, width: w * s, height: h * s };
  };
  if (!layout.blur) {
    const out = contain(win.w, win.h);
    return { out, fg: out, vx: out.width / win.w, vy: out.height / win.h };
  }
  const out = contain(layout.out_w, layout.out_h);
  const k = out.width / layout.out_w;
  const fg = {
    left: out.left + layout.fg.x * k,
    top: out.top + layout.fg.y * k,
    width: layout.fg.w * k,
    height: layout.fg.h * k,
  };
  return { out, fg, vx: fg.width / win.w, vy: fg.height / win.h };
}
