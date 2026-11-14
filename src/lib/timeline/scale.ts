// Time <-> pixel mapping for the overview and zoom timelines (PLY-06).

export const ZOOM_MIN_SEC = 2;
export const ZOOM_MAX_SEC = 600;

/** A visible time window mapped onto `width` CSS pixels. */
export interface Viewport {
  start: number;
  end: number;
  width: number;
}

export function timeToX(t: number, vp: Viewport): number {
  const span = vp.end - vp.start;
  return span > 0 ? ((t - vp.start) / span) * vp.width : 0;
}

export function xToTime(x: number, vp: Viewport): number {
  return vp.width > 0 ? vp.start + (x / vp.width) * (vp.end - vp.start) : vp.start;
}

export function pxPerSec(vp: Viewport): number {
  const span = vp.end - vp.start;
  return span > 0 ? vp.width / span : 0;
}

/** Overview: the whole source duration. */
export function overviewViewport(duration: number, width: number): Viewport {
  return { start: 0, end: Math.max(duration, 0), width };
}

/** Clamp a zoom window length to 2 s .. 10 min (PLY-06). */
export function clampWindow(sec: number): number {
  if (!Number.isFinite(sec)) return ZOOM_MIN_SEC * 10;
  return Math.min(ZOOM_MAX_SEC, Math.max(ZOOM_MIN_SEC, sec));
}

/** Zoom: a window of `windowSec` centred on `center`. */
export function zoomViewport(center: number, windowSec: number, width: number): Viewport {
  const w = clampWindow(windowSec);
  return { start: center - w / 2, end: center + w / 2, width };
}

export interface ZoomView {
  center: number;
  window: number;
}

/**
 * Wheel zoom: scale the window by `factor` (>1 zooms out) while the time under
 * the cursor (`cursorFrac` = x / width in 0..1) stays at the same pixel.
 */
export function zoomAround(view: ZoomView, cursorFrac: number, factor: number): ZoomView {
  const frac = Math.min(1, Math.max(0, cursorFrac));
  const tCursor = view.center - view.window / 2 + frac * view.window;
  const window = clampWindow(view.window * factor);
  const start = tCursor - frac * window;
  return { center: start + window / 2, window };
}
