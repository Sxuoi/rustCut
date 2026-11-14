// Media index (keyframes, VFR frame stepping) and waveform peaks (PLY-05/06).
// OWNER: fe-timeline. Public API is shared; extend, don't break.

import type { MediaIndexInfo, WaveformChunk } from "$lib/bindings";
import { player } from "./player.svelte";

export interface WaveformData {
  track: number;
  peaksPerSec: number;
  /** Interleaved [min, max] pairs, -127..127; length = 2 * totalPeaks. */
  peaks: Int8Array;
  /** Which peaks have arrived (chunks may arrive out of order). */
  filled: Uint8Array;
  totalPeaks: number;
  done: boolean;
}

/** Index of the last element <= t in a sorted array, or -1. */
export function lastAtOrBefore(sorted: ArrayLike<number>, t: number): number {
  let lo = 0;
  let hi = sorted.length - 1;
  let ans = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (sorted[mid] <= t + 1e-9) {
      ans = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return ans;
}

/**
 * PLY-04 VFR stepping: the presentation time of the frame after (dir = 1) or
 * before (dir = -1) the frame shown at `t`. `t` may lie anywhere inside the
 * current frame. Clamps at the first/last frame; null without frame times.
 */
export function stepFrameTime(times: ArrayLike<number>, t: number, dir: 1 | -1): number | null {
  const n = times.length;
  if (n === 0) return null;
  const cur = lastAtOrBefore(times, t);
  if (dir === 1) return times[Math.min(n - 1, cur + 1)];
  return times[Math.max(0, cur - 1)];
}

export class MediaState {
  index = $state.raw<MediaIndexInfo | null>(null);
  keyframes = $state.raw<Float64Array>(new Float64Array());
  waveform = $state.raw<WaveformData | null>(null);
  /** Bumped on every waveform chunk so canvases redraw. */
  waveformVersion = $state(0);

  /** Real frame timestamps of a VFR source (PLY-04); empty otherwise. */
  private frameTimes: Float64Array | null = null;

  private readonly stepHook = (t: number, dir: 1 | -1): number | null =>
    this.frameTimes ? stepFrameTime(this.frameTimes, t, dir) : null;

  reset(): void {
    this.frameTimes = null;
    player.setFrameStepHook(null);
    this.index = null;
    this.keyframes = new Float64Array();
    this.waveform = null;
    this.waveformVersion = 0;
  }

  handleIndex(info: MediaIndexInfo): void {
    this.index = info;
    this.keyframes = Float64Array.from(info.keyframes);
    if (info.vfr && info.frame_times && info.frame_times.length > 0) {
      this.frameTimes = Float64Array.from(info.frame_times);
      player.setFrameStepHook(this.stepHook);
    } else {
      this.frameTimes = null;
      player.setFrameStepHook(null);
    }
  }

  handleWaveform(chunk: WaveformChunk): void {
    let w = this.waveform;
    if (!w || w.track !== chunk.track || w.totalPeaks !== chunk.total_peaks || w.peaksPerSec !== chunk.peaks_per_sec) {
      w = {
        track: chunk.track,
        peaksPerSec: chunk.peaks_per_sec,
        peaks: new Int8Array(chunk.total_peaks * 2),
        filled: new Uint8Array(chunk.total_peaks),
        totalPeaks: chunk.total_peaks,
        done: false,
      };
    }
    const n = chunk.peaks.length >> 1;
    const start = chunk.start_peak;
    for (let i = 0; i < n && start + i < w.totalPeaks; i++) {
      w.peaks[(start + i) * 2] = chunk.peaks[i * 2];
      w.peaks[(start + i) * 2 + 1] = chunk.peaks[i * 2 + 1];
      w.filled[start + i] = 1;
    }
    w.done = chunk.done;
    this.waveform = w;
    this.waveformVersion++;
  }

  /** MRK-07: keyframe at or before `t` (effective lossless start). */
  keyframeAtOrBefore(t: number): number | null {
    const i = lastAtOrBefore(this.keyframes, t);
    return i >= 0 ? this.keyframes[i] : null;
  }

  /** MRK-07 lead-in for a lossless cut starting at `t`, if the index is known. */
  leadIn(t: number): number | null {
    const k = this.keyframeAtOrBefore(t);
    return k === null ? null : Math.max(0, t - k);
  }
}

export const media = new MediaState();
