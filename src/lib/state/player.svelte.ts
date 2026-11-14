// Player state and transport (PLY-02..04). OWNER: fe-shell.
// Public API is shared by every component; extend, don't break.
//
// Times exposed here are SOURCE-RELATIVE seconds (same origin as clip times
// and FFmpeg -ss). `timeOffset` converts from the <video> element's timeline
// (§7.3 "Time origin"): source = video.currentTime + timeOffset.

import type { PlaybackInfo } from "$lib/bindings";
import type { PendingMarks } from "$lib/util/marks";
import { DEFAULT_FRAME_DURATION, frameDurationOf, loopPreviewRange } from "./playback";
import { media } from "./media.svelte";
import { project } from "./project.svelte";

/** PLY-03 speed steps. */
export const SPEEDS = [0.5, 1, 1.25, 1.5, 2, 3] as const;

type FrameCallbackVideo = HTMLVideoElement & {
  requestVideoFrameCallback?: (cb: (now: number, meta: { mediaTime: number }) => void) => number;
  cancelVideoFrameCallback?: (handle: number) => void;
};

export class PlayerState {
  el: HTMLVideoElement | null = null;

  /**
   * Source time of the last seek until its frame is presented (rVFC). Between
   * the seek finishing and the paint, `presentedMediaTime` is still the old
   * frame, so navigation (steps, relative seeks) builds on this instead.
   */
  pendingSeekTarget: number | null = null;

  /** Source-relative time of the frame on screen. */
  currentTime = $state(0);
  /** Source duration (from probe; the element's duration as fallback). */
  duration = $state(0);
  playing = $state(false);
  rate = $state(1);

  /** Pending In/Out (MRK-01); UI state, not persisted. */
  pendingIn = $state<number | null>(null);
  pendingOut = $state<number | null>(null);

  /** Loop preview (QUE-03, `P`). */
  loopClipId = $state<string | null>(null);
  loopAsExported = $state(false);
  private markedLoopRange = $state.raw<{ start: number; end: number } | null>(null);

  /** Keep marked edges separate so App's retime follower cannot overwrite the lead-in. */
  get loopRange(): { start: number; end: number } | null {
    const range = this.markedLoopRange;
    return range ? loopPreviewRange(range, this.loopAsExported, project.settings, (time) => media.keyframeAtOrBefore(time)) : null;
  }

  playback = $state.raw<PlaybackInfo | null>(null);
  /** First frame decoded for the current playback tier. */
  ready = $state(false);
  error = $state<string | null>(null);

  timeOffset = $state(0);
  /** Nominal frame duration (PLY-04); VFR stepping uses the media index. */
  frameDuration = $state(DEFAULT_FRAME_DURATION);

  /**
   * Source time to show once the first frame of a (new) tier is decoded:
   * the restored playhead on open (PRJ-03) or the position before a tier
   * switch. Consumed by Player.svelte.
   */
  restoreTime: number | null = null;
  /** Bumped by `reset()` (a project was opened): tier attempts start over. */
  generation = 0;

  /** Media time (video timeline) of the last presented frame, via rVFC. */
  private presentedMediaTime: number | null = null;
  private rvfcHandle: number | null = null;
  private stepHook: ((t: number, dir: 1 | -1) => number | null) | null = null;

  attach(el: HTMLVideoElement): void {
    this.detach();
    this.el = el;
    el.preservesPitch = true;
    el.playbackRate = this.rate;
    this.scheduleFrameCallback();
  }

  detach(): void {
    const el = this.el as FrameCallbackVideo | null;
    if (el && this.rvfcHandle !== null) el.cancelVideoFrameCallback?.(this.rvfcHandle);
    this.rvfcHandle = null;
    this.el = null;
    this.presentedMediaTime = null;
  }

  /** VFR-aware frame stepping provided by the media index (fe-timeline). */
  setFrameStepHook(hook: ((t: number, dir: 1 | -1) => number | null) | null): void {
    this.stepHook = hook;
  }

  /** PLY-04: nominal frame duration from the probe's rational rate. */
  setFrameRate(rate: string | null | undefined): void {
    this.frameDuration = frameDurationOf(rate);
  }

  private scheduleFrameCallback(): void {
    const el = this.el as FrameCallbackVideo | null;
    if (!el?.requestVideoFrameCallback) return;
    this.rvfcHandle = el.requestVideoFrameCallback((_now, meta) => {
      this.presentedMediaTime = meta.mediaTime;
      this.pendingSeekTarget = null;
      this.currentTime = this.toSource(meta.mediaTime);
      this.enforceLoop();
      this.scheduleFrameCallback();
    });
  }

  toSource(videoTime: number): number {
    return videoTime + this.timeOffset;
  }

  toVideo(sourceTime: number): number {
    return sourceTime - this.timeOffset;
  }

  /** The element is still seeking (decoder busy; key repeat waits for it). */
  get seeking(): boolean {
    return this.el?.seeking ?? false;
  }

  /** Pending marks as one value (util/marks.ts). */
  get marks(): PendingMarks {
    return { in: this.pendingIn, out: this.pendingOut };
  }

  setMarks(m: PendingMarks): void {
    this.pendingIn = m.in;
    this.pendingOut = m.out;
  }

  /**
   * Synchronous time capture for marks (§5 rule 4): no IPC, no await.
   * While paused, returns the media time of the frame actually on screen.
   */
  now(): number {
    const el = this.el;
    if (!el) return this.currentTime;
    if (el.paused && this.presentedMediaTime !== null && Math.abs(this.presentedMediaTime - el.currentTime) < 0.5) {
      return this.toSource(this.presentedMediaTime);
    }
    return this.toSource(el.currentTime);
  }

  /** Called by the <video> timeupdate/seeked handlers when rVFC is unavailable. */
  syncFromElement(): void {
    const el = this.el;
    if (!el) return;
    if (!(el as FrameCallbackVideo).requestVideoFrameCallback) this.pendingSeekTarget = null;
    if (!(el as FrameCallbackVideo).requestVideoFrameCallback || el.paused) {
      this.currentTime = this.pendingSeekTarget ?? this.now();
    }
    this.playing = !el.paused && !el.ended;
    this.enforceLoop();
  }

  private enforceLoop(): void {
    const r = this.loopRange;
    const el = this.el;
    // While a wrap's seek is in flight, frames decoded before it can still report the end:
    // wrapping again would restart the same seek (each wrap used to cost two seeks).
    if (!r || !el || el.paused || el.seeking) return;
    if (this.currentTime >= r.end - 0.001 || this.currentTime < r.start - 0.25) {
      el.currentTime = Math.max(0, this.toVideo(r.start));
    }
  }

  clamp(t: number): number {
    const d = this.duration || this.el?.duration || 0;
    return Math.min(Math.max(0, t), Number.isFinite(d) && d > 0 ? d : t);
  }

  play(): void {
    this.pendingSeekTarget = null;
    void this.el?.play().catch(() => {});
  }

  pause(): void {
    this.el?.pause();
  }

  toggle(): void {
    if (!this.el) return;
    if (this.el.paused) this.play();
    else this.pause();
  }

  /** Seek to a source time. Seeking outside a loop range ends the loop. */
  seek(t: number): void {
    const el = this.el;
    if (!el) return;
    const target = this.clamp(t);
    const r = this.loopRange;
    if (r && (target < r.start - 0.001 || target > r.end + 0.001)) {
      this.loopClipId = null;
      this.markedLoopRange = null;
    }
    el.currentTime = this.toVideo(target);
    this.currentTime = target;
    this.pendingSeekTarget = target;
  }

  /** Base time for navigation: the pending seek target, else the frame on screen. */
  private navBase(): number {
    return this.pendingSeekTarget ?? this.now();
  }

  seekBy(dt: number): void {
    this.seek(this.navBase() + dt);
  }

  /** PLY-02: step ±1 frame; pauses playback. */
  stepFrame(dir: 1 | -1): void {
    const el = this.el;
    if (!el) return;
    el.pause();
    // While a previous step is still seeking, build on its target so fast
    // presses are not lost.
    const t = el.seeking ? this.currentTime : this.navBase();
    const hooked = this.stepHook?.(t, dir);
    // +1 ms lands inside the target frame (the frame shown at T is the last
    // frame with PTS <= T), so the next presented frame is the one we want.
    const target = hooked ?? t + dir * this.frameDuration;
    this.seek(target + 0.001);
  }

  setRate(r: number): void {
    this.rate = r;
    if (this.el) {
      this.el.playbackRate = r;
      this.el.preservesPitch = true;
    }
  }

  /** `<` / `>`: step through SPEEDS. */
  speedStep(dir: 1 | -1): void {
    const i = SPEEDS.findIndex((s) => s >= this.rate - 1e-9);
    const cur = i < 0 ? SPEEDS.length - 1 : i;
    const next = Math.min(SPEEDS.length - 1, Math.max(0, cur + dir));
    this.setRate(SPEEDS[next]);
  }

  setLoop(clipId: string | null, range: { start: number; end: number } | null): void {
    this.loopClipId = clipId;
    this.updateLoopRange(range);
    if (range) {
      this.seek(this.loopRange?.start ?? range.start);
      this.play();
    }
  }

  /** Follow a retimed loop clip without seeking (QUE-03). */
  updateLoopRange(range: { start: number; end: number } | null): void {
    if (this.markedLoopRange?.start === range?.start && this.markedLoopRange?.end === range?.end) return;
    this.markedLoopRange = range;
  }

  clearPending(): void {
    this.pendingIn = null;
    this.pendingOut = null;
  }

  reset(): void {
    this.pendingSeekTarget = null;
    this.pause();
    this.currentTime = 0;
    this.duration = 0;
    this.playing = false;
    this.clearPending();
    this.loopClipId = null;
    this.markedLoopRange = null;
    this.playback = null;
    this.ready = false;
    this.error = null;
    this.timeOffset = 0;
    this.frameDuration = DEFAULT_FRAME_DURATION;
    this.restoreTime = null;
    this.presentedMediaTime = null;
    this.generation++;
  }
}

export const player = new PlayerState();
