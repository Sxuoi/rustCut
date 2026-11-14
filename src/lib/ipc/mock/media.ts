// Synthetic media data: waveform peaks (PLY-05) and the media index (PLY-06).

import type { MediaIndexInfo, SourceInfo, WaveformChunk } from "$lib/bindings";

export const PEAKS_PER_SEC = 100;
/** Distance between keyframes of the sample (and of the fake source). */
export const KEYFRAME_INTERVAL_SEC = 2;

/** Deterministic pseudo-random in [0, 1) from two integers. */
function noise(a: number, b: number): number {
  let h = Math.imul(a ^ 0x9e3779b9, 0x85ebca6b) ^ Math.imul(b + 0x7f4a7c15, 0xc2b2ae35);
  h ^= h >>> 15;
  h = Math.imul(h, 0x2c1b3c6d);
  h ^= h >>> 12;
  h = Math.imul(h, 0x297a2d39);
  h ^= h >>> 15;
  return (h >>> 0) / 0x100000000;
}

/**
 * Peak `i` of audio track `track` as `[min, max]` in -127..=127. A slow
 * envelope, a beep every 5 s (like the sample's tone) and per-peak jitter, so
 * the waveform has visible shape. Pure and deterministic.
 */
export function waveformPeak(track: number, i: number): [number, number] {
  const t = i / PEAKS_PER_SEC;
  const envelope = 0.3 + 0.25 * Math.sin((2 * Math.PI * t) / 7.3 + track) + 0.15 * Math.sin((2 * Math.PI * t) / 1.9);
  const beep = t % 5 < 0.35 ? 0.35 : 0;
  const amp = Math.min(1, Math.max(0.03, envelope + beep)) * 127;
  const max = Math.round(amp * (0.7 + 0.3 * noise(track, i)));
  const min = -Math.round(amp * (0.6 + 0.4 * noise(track + 101, i)));
  return [Math.max(-127, min), Math.min(127, max)];
}

export function totalPeaks(durationSec: number): number {
  return Math.max(0, Math.ceil(durationSec * PEAKS_PER_SEC));
}

/** The chunk covering peaks `[start, start + count)`. */
export function waveformChunk(track: number, start: number, count: number, total: number): WaveformChunk {
  const end = Math.min(total, start + count);
  const peaks: number[] = [];
  for (let i = start; i < end; i++) {
    const [min, max] = waveformPeak(track, i);
    peaks.push(min, max);
  }
  return {
    track,
    peaks_per_sec: PEAKS_PER_SEC,
    start_peak: start,
    peaks,
    total_peaks: total,
    done: end >= total,
  };
}

function parseRational(s: string): number {
  const [n, d] = s.split("/").map(Number);
  if (n === undefined || !Number.isFinite(n)) return 30;
  return d && Number.isFinite(d) && d !== 0 ? n / d : n;
}

/** Frame rate of the source as a number (30/1 -> 30). */
export function frameRate(source: SourceInfo): number {
  return parseRational(source.video.frame_rate);
}

/** Keyframes every 2 s of a CFR source. */
export function keyframesFor(source: SourceInfo): number[] {
  const out: number[] = [];
  for (let t = 0; t < source.duration_sec; t += KEYFRAME_INTERVAL_SEC) out.push(t);
  return out;
}

/** PLY-06 media index of the synthetic CFR sources. */
export function mediaIndexFor(source: SourceInfo): MediaIndexInfo {
  return {
    keyframes: keyframesFor(source),
    frame_duration_sec: 1 / frameRate(source),
    vfr: false,
    frame_times: null,
    source: "sample_table",
    complete: true,
  };
}

/** Seconds of footage before In that a lossless cut includes (MRK-07). */
export function leadInFor(startSec: number): number {
  const kf = Math.floor(startSec / KEYFRAME_INTERVAL_SEC) * KEYFRAME_INTERVAL_SEC;
  return Math.round((startSec - kf) * 1000) / 1000;
}
