// Ruler, frame and keyframe tick helpers (PLY-06). Callback style so a draw
// pass allocates nothing.

const STEPS = [
  0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200,
  14400, 28800,
];

/** Smallest "nice" step (1-2-5, minute and hour multiples) that is >= `minSec`. */
export function niceStep(minSec: number): number {
  for (const s of STEPS) if (s >= minSec - 1e-12) return s;
  return STEPS[STEPS.length - 1];
}

/** Ruler step so labels are at least `minLabelPx` apart. */
export function rulerStep(spanSec: number, widthPx: number, minLabelPx = 72): number {
  if (!(widthPx > 0) || !(spanSec > 0)) return 1;
  return niceStep((minLabelPx * spanSec) / widthPx);
}

/** Calls `cb(t, index)` for every multiple of `step` inside [start, end]. */
export function forEachTick(start: number, end: number, step: number, cb: (t: number, index: number) => void): void {
  if (!(step > 0) || end < start) return;
  const first = Math.ceil(start / step - 1e-9);
  const last = Math.floor(end / step + 1e-9);
  for (let k = first; k <= last; k++) cb(k * step, k);
}

/** Ruler label: `m:ss`/`h:mm:ss`, with fraction digits for sub-second steps. */
export function formatTick(t: number, step: number): string {
  const digits = step >= 1 ? 0 : step >= 0.1 ? 1 : step >= 0.01 ? 2 : 3;
  const scale = 10 ** digits;
  const total = Math.max(0, Math.round(t * scale));
  const whole = Math.floor(total / scale);
  const h = Math.floor(whole / 3600);
  const m = Math.floor(whole / 60) % 60;
  const ss = String(whole % 60).padStart(2, "0");
  const base = h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
  return digits === 0 ? base : `${base}.${String(total % scale).padStart(digits, "0")}`;
}

/**
 * Frame ticks are drawn once a frame is at least `minPx` wide; returns the
 * frame stride (1 = every frame) or 0 when too dense.
 */
export function frameStride(frameDur: number, pxPerSecond: number, minPx = 4): number {
  if (!(frameDur > 0) || !(pxPerSecond > 0)) return 0;
  return frameDur * pxPerSecond >= minPx ? 1 : 0;
}

/** Calls `cb(t)` for each nominal frame boundary in [start, end] (CFR). */
export function forEachFrameTick(start: number, end: number, frameDur: number, cb: (t: number) => void): void {
  forEachTick(Math.max(0, start), end, frameDur, cb);
}

/** First index with value >= x in a sorted array. */
export function lowerBound(sorted: ArrayLike<number>, x: number): number {
  let lo = 0;
  let hi = sorted.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (sorted[mid] < x) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** [lo, hi) index range of a sorted array (keyframes, frame times) inside [start, end]. */
export function sortedRange(sorted: ArrayLike<number>, start: number, end: number): [number, number] {
  return [lowerBound(sorted, start), lowerBound(sorted, end + 1e-9)];
}

/** Nearest value of a sorted array to `x`, or null when empty. */
export function nearestSorted(sorted: ArrayLike<number>, x: number): number | null {
  const n = sorted.length;
  if (n === 0) return null;
  const i = lowerBound(sorted, x);
  if (i === 0) return sorted[0];
  if (i === n) return sorted[n - 1];
  return x - sorted[i - 1] <= sorted[i] - x ? sorted[i - 1] : sorted[i];
}
