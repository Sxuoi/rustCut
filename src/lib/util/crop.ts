// TypeScript port of rustcut-framing's crop_window, square_window and
// frame_layout (PRD §7.7, FRM-04, FRM-09). The overlay's authoritative layout
// comes from the Rust functions over IPC; this port gives instant feedback
// while dragging and must reproduce the same vectors
// (crates/rustcut-framing/tests/{crop,square,layout}_vectors.json).

import type { FrameLayout, Framing, OutputResolution } from "$lib/bindings";

export interface CropRect {
  w: number;
  h: number;
  x: number;
  y: number;
}

/** Aspect of the window a crop framing cuts from the frame. */
export type WindowShape = "9:16" | "1:1";

// Rust's f64::round rounds half away from zero; Math.round rounds half up.
// Inputs here are non-negative, where the two agree.
const evenRound = (v: number) => Math.max(0, Math.round(v / 2)) * 2;
const floorEven = (v: number) => Math.max(0, Math.floor(v / 2)) * 2;

export function cropWindow(dw: number, dh: number, offset: number): CropRect {
  const off = Number.isFinite(offset) ? Math.min(1, Math.max(-1, offset)) : 0;
  if (dw < 2 || dh < 2) return { w: dw & ~1, h: dh & ~1, x: 0, y: 0 };
  if (dw * 16 >= dh * 9) {
    const h = dh & ~1;
    const w = Math.min(evenRound((dh * 9) / 16), dw & ~1);
    const x = floorEven(((dw - w) * (1 + off)) / 2);
    return { w, h, x, y: 0 };
  }
  const w = dw & ~1;
  const h = Math.min(evenRound((dw * 16) / 9), dh & ~1);
  return { w, h, x: 0, y: floorEven((dh - h) / 2) };
}

/** Largest 1:1 window (FRM-09): slides on X in landscape frames, centred on Y otherwise. */
export function squareWindow(dw: number, dh: number, offset: number): CropRect {
  const off = Number.isFinite(offset) ? Math.min(1, Math.max(-1, offset)) : 0;
  const side = Math.min(dw, dh) & ~1;
  if (dw >= dh) return { w: side, h: side, x: floorEven(((dw - side) * (1 + off)) / 2), y: 0 };
  return { w: side, h: side, x: 0, y: floorEven((dh - side) / 2) };
}

/** The crop window of `shape` (9:16 for Vertical Crop, 1:1 for Square on Blur). */
export function shapeWindow(shape: WindowShape, dw: number, dh: number, offset: number): CropRect {
  return shape === "1:1" ? squareWindow(dw, dh, offset) : cropWindow(dw, dh, offset);
}

/** Offset placing the window's left edge at `x` (drag), clamped to [-1, 1]. */
export function offsetForX(dw: number, dh: number, x: number, shape: WindowShape = "9:16"): number {
  const w = shapeWindow(shape, dw, dh, 0);
  const range = dw - w.w;
  if (range <= 0) return 0;
  return Math.min(1, Math.max(-1, (x / range) * 2 - 1));
}

/** FRM-03: false when the window already spans the full width. */
export function offsetHasEffect(dw: number, dh: number, shape: WindowShape = "9:16"): boolean {
  return shapeWindow(shape, dw, dh, 0).w < (dw & ~1);
}

/**
 * Window x (source px) for a horizontal drag of `dxPx` CSS pixels. In full-frame
 * mode the window moves with the pointer; in output-only mode the window is fixed
 * on screen and the video slides under it, so the content must follow the
 * pointer (x moves the opposite way).
 */
export function dragWindowX(startX: number, dxPx: number, scale: number, contentFollows: boolean): number {
  const dx = scale > 0 ? dxPx / scale : 0;
  return contentFollows ? startX - dx : startX + dx;
}

/** The crop window shape of a framing, or `null` when it has no window to position. */
export function windowShape(framing: Framing): WindowShape | null {
  if (framing === "vertical_crop") return "9:16";
  if (framing === "vertical_square") return "1:1";
  return null;
}

const NINE_SIXTEEN: Record<OutputResolution, [number, number] | null> = {
  "1080x1920": [1080, 1920],
  "720x1280": [720, 1280],
  native: null,
};

/** Fit like FFmpeg's force_original_aspect_ratio=decrease:force_divisible_by=2 (chain E). */
function fitInside(iw: number, ih: number, ow: number, oh: number): [number, number] {
  const w0 = Math.max(1, iw);
  const h0 = Math.max(1, ih);
  const tmpW = Math.floor((oh * w0 + h0) / (2 * h0)) * 2;
  const tmpH = Math.floor((ow * h0 + w0) / (2 * w0)) * 2;
  const w = Math.floor(Math.min(tmpW, ow) / 2) * 2;
  const h = Math.floor(Math.min(tmpH, oh) / 2) * 2;
  return [Math.max(2, w), Math.max(2, h)];
}

/** Centred part of `src` with the output's aspect (cover-scale + centre crop), all even. */
function cover(src: CropRect, ow: number, oh: number): CropRect {
  const sw = src.w & ~1;
  const sh = src.h & ~1;
  const o1 = Math.max(1, ow);
  const o2 = Math.max(1, oh);
  if (sw * o2 >= sh * o1) {
    const w = Math.min(Math.max(Math.floor((sh * o1 + Math.floor(o2 / 2)) / o2) & ~1, 2), sw);
    return { w, h: sh, x: src.x + (Math.floor((sw - w) / 2) & ~1), y: src.y };
  }
  const h = Math.min(Math.max(Math.floor((sw * o2 + Math.floor(o1 / 2)) / o1) & ~1, 2), sh);
  return { w: sw, h, x: src.x, y: src.y + (Math.floor((sh - h) / 2) & ~1) };
}

const quarter = (v: number) => Math.max(2, Math.floor(v / 4) & ~1);

/**
 * `boxblur` radii (luma, chroma) FFmpeg accepts on a `w`×`h` 4:2:0 picture:
 * the PRD's 10, at most (shorter side of the plane − 1) / 2.
 */
export function blurRadii(w: number, h: number): [number, number] {
  const max = (side: number) => Math.floor(Math.max(0, side - 1) / 2);
  return [Math.min(10, max(Math.min(w, h))), Math.min(10, max(Math.min(Math.ceil(w / 2), Math.ceil(h / 2))))];
}

/**
 * What chain E's runtime `scale=BW:BH:force_original_aspect_ratio=increase,crop=BW:BH`
 * resolves to: `[sw, sh, cx, cy]` (port of `vertical_blur_background_crop`).
 */
function verticalBlurBackgroundCrop(dw: number, dh: number, bw: number, bh: number): [number, number, number, number] {
  const w0 = Math.max(1, dw);
  const h0 = Math.max(1, dh);
  const rescale = (a: number, b: number, c: number) => Math.floor((a * b + Math.floor(c / 2)) / c);
  const sw = Math.max(rescale(bh, w0, h0), bw);
  const sh = Math.max(rescale(bw, h0, w0), bh);
  // lrint((in - out) / 2) ties to even, then the 4:2:0 alignment.
  const centre = (d: number) => {
    const half = Math.floor(d / 2);
    return (d % 2 === 1 && half % 2 === 1 ? half + 1 : half) & ~1;
  };
  return [sw, sh, centre(sw - bw), centre(sh - bh)];
}

/** The display-frame region chain E's background samples, to the nearest pixel. */
function verticalBlurSource(dw: number, dh: number, bw: number, bh: number): CropRect {
  const [sw, sh, cx, cy] = verticalBlurBackgroundCrop(dw, dh, bw, bh);
  const toDisplay = (v: number, full: number, scaled: number) =>
    Math.floor((v * full + Math.floor(scaled / 2)) / Math.max(1, scaled));
  const x = Math.min(toDisplay(cx, dw, sw), dw);
  const y = Math.min(toDisplay(cy, dh, sh), dh);
  return { w: Math.min(toDisplay(bw, dw, sw), dw - x), h: Math.min(toDisplay(bh, dh, sh), dh - y), x, y };
}

function background(source: CropRect, ow: number, oh: number) {
  const w = quarter(ow);
  const h = quarter(oh);
  return { source, w, h, radius: blurRadii(w, h)[0] };
}

function fitOnBlur(dw: number, dh: number, ow: number, oh: number): FrameLayout {
  const window = { w: dw, h: dh, x: 0, y: 0 };
  const [fw, fh] = fitInside(dw, dh, ow, oh);
  return {
    out_w: ow,
    out_h: oh,
    window,
    fg: { w: fw, h: fh, x: Math.floor(Math.max(0, ow - fw) / 2) & ~1, y: Math.floor(Math.max(0, oh - fh) / 2) & ~1 },
    blur: background(cover(window, ow, oh), ow, oh),
  };
}

/**
 * Port of `rustcut_framing::frame_layout`: what a framed export shows and
 * where (output size, sharp window and its place, blurred background source).
 * `null` for the original framing.
 */
export function frameLayout(
  framing: Framing,
  dw: number,
  dh: number,
  offset: number,
  output: OutputResolution,
): FrameLayout | null {
  const fixed = NINE_SIXTEEN[output];
  switch (framing) {
    case "original":
      return null;
    case "vertical_crop": {
      const window = cropWindow(dw, dh, offset);
      const [ow, oh] = fixed ?? [window.w, window.h];
      return { out_w: ow, out_h: oh, window, fg: { w: ow, h: oh, x: 0, y: 0 }, blur: null };
    }
    case "vertical_blur": {
      const win = cropWindow(dw, dh, 0);
      const [ow, oh] = fixed ?? [win.w, win.h];
      const layout = fitOnBlur(dw, dh, ow, oh);
      // Chain E keeps its runtime cover crop: name the region it really samples.
      if (layout.blur) layout.blur.source = verticalBlurSource(dw, dh, layout.blur.w, layout.blur.h);
      return layout;
    }
    case "square_blur": {
      const side = fixed ? fixed[0] : squareWindow(dw, dh, 0).w;
      return fitOnBlur(dw, dh, side, side);
    }
    case "vertical_square": {
      const window = squareWindow(dw, dh, offset);
      const [ow, oh] = fixed ?? [window.w, evenRound((window.w * 16) / 9)];
      return {
        out_w: ow,
        out_h: oh,
        window,
        fg: { w: ow, h: ow, x: 0, y: floorEven(Math.max(0, oh - ow) / 2) },
        blur: background(cover(window, ow, oh), ow, oh),
      };
    }
  }
}
