// TypeScript port of rustcut-framing::crop_window (PRD §7.7). The overlay's
// authoritative rectangle comes from the Rust function over IPC (FRM-04);
// this port gives instant feedback while dragging and must reproduce the
// same vectors (crates/rustcut-framing/tests/crop_vectors.json).

export interface CropRect {
  w: number;
  h: number;
  x: number;
  y: number;
}

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

/** Offset placing the window's left edge at `x` (drag), clamped to [-1, 1]. */
export function offsetForX(dw: number, dh: number, x: number): number {
  const w = cropWindow(dw, dh, 0);
  const range = dw - w.w;
  if (range <= 0) return 0;
  return Math.min(1, Math.max(-1, (x / range) * 2 - 1));
}

/** FRM-03: false when the window already spans the full width. */
export function offsetHasEffect(dw: number, dh: number): boolean {
  return cropWindow(dw, dh, 0).w < (dw & ~1);
}

/**
 * Window x (source px) for a horizontal drag of `dxPx` CSS pixels. In full-frame
 * mode the window moves with the pointer; in 9:16-only mode the window is fixed
 * on screen and the video slides under it, so the content must follow the
 * pointer (x moves the opposite way).
 */
export function dragWindowX(startX: number, dxPx: number, scale: number, contentFollows: boolean): number {
  const dx = scale > 0 ? dxPx / scale : 0;
  return contentFollows ? startX - dx : startX + dx;
}
