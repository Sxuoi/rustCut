// Min/max column reduction of waveform peaks (PLY-05). Progressive: columns
// without any arrived peak are flagged empty and not drawn.

export interface PeakSource {
  peaksPerSec: number;
  totalPeaks: number;
  /** Interleaved [min, max] pairs, -127..127. */
  peaks: Int8Array;
  filled: Uint8Array;
}

/** Reusable output buffers (no allocation per draw once sized). */
export class WaveColumns {
  min = new Int8Array(0);
  max = new Int8Array(0);
  has = new Uint8Array(0);
  width = 0;

  ensure(width: number): void {
    if (this.min.length < width) {
      this.min = new Int8Array(width);
      this.max = new Int8Array(width);
      this.has = new Uint8Array(width);
    }
    this.width = width;
  }
}

/** Reduce the peaks of [t0, t1] into `width` pixel columns. */
export function computeColumns(w: PeakSource, t0: number, t1: number, width: number, out: WaveColumns): void {
  const cols = Math.max(0, Math.floor(width));
  out.ensure(cols);
  const span = t1 - t0;
  for (let x = 0; x < cols; x++) {
    const a = t0 + (x / cols) * span;
    const b = t0 + ((x + 1) / cols) * span;
    const p0 = Math.max(0, Math.floor(a * w.peaksPerSec));
    if (b <= 0 || p0 >= w.totalPeaks) {
      out.has[x] = 0;
      continue;
    }
    const p1 = Math.min(w.totalPeaks, Math.max(p0 + 1, Math.ceil(b * w.peaksPerSec)));
    let lo = 127;
    let hi = -127;
    let any = false;
    for (let p = p0; p < p1; p++) {
      if (!w.filled[p]) continue;
      any = true;
      const mn = w.peaks[p * 2];
      const mx = w.peaks[p * 2 + 1];
      if (mn < lo) lo = mn;
      if (mx > hi) hi = mx;
    }
    out.has[x] = any ? 1 : 0;
    out.min[x] = any ? lo : 0;
    out.max[x] = any ? hi : 0;
  }
}
