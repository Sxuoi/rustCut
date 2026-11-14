// Batch progress maths for the export panel (EXP-09, EXP-10): ETA fallback and
// smoothing, summary counts. Pure: the exporter store feeds it events.

import type { BatchSummary, ExportProgress } from "$lib/bindings";

/**
 * Remaining seconds extrapolated from elapsed time and the duration-weighted
 * fraction, when the backend sends no ETA. `null` until 1 % is done (too
 * noisy before that); `0` once finished.
 */
export function estimateRemaining(fraction: number, elapsedSec: number): number | null {
  if (!Number.isFinite(fraction) || !Number.isFinite(elapsedSec) || elapsedSec <= 0) return null;
  if (fraction >= 1) return 0;
  if (fraction < 0.01) return null;
  return (elapsedSec * (1 - fraction)) / fraction;
}

/** Default weight of a new ETA sample. */
export const ETA_ALPHA = 0.3;

/**
 * Exponential smoothing of ETA samples so the label does not jump with
 * every FFmpeg speed wobble. Invalid samples keep the previous value; the
 * first valid sample is taken as is.
 */
export function smoothEta(prev: number | null, sample: number | null, alpha = ETA_ALPHA): number | null {
  if (sample === null || !Number.isFinite(sample) || sample < 0) return prev;
  if (prev === null || !Number.isFinite(prev)) return sample;
  const a = Math.min(1, Math.max(0, alpha));
  return prev + a * (sample - prev);
}

/** Overall bar value, clamped to 0..1 (already weighted by clip duration, EXP-09). */
export function overallFraction(p: Pick<ExportProgress, "overall_fraction"> | null): number {
  const f = p?.overall_fraction ?? 0;
  return Number.isFinite(f) ? Math.min(1, Math.max(0, f)) : 0;
}

/** Clips finished so far (done or failed) for the `1 / 2` label. */
export function finishedCount(p: Pick<ExportProgress, "done" | "failed">): number {
  return Math.max(0, p.done) + Math.max(0, p.failed);
}

/** Counts shown in the end-of-batch summary (EXP-10). */
export interface SummaryCounts {
  done: number;
  failed: number;
  skipped: number;
  cancelled: number;
  total: number;
}

export function summaryCounts(s: Pick<BatchSummary, "done" | "failed" | "skipped" | "cancelled">): SummaryCounts {
  const done = s.done.length;
  const failed = s.failed.length;
  const skipped = s.skipped.length;
  const cancelled = s.cancelled.length;
  return { done, failed, skipped, cancelled, total: done + failed + skipped + cancelled };
}
