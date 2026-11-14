// §7.3 time origin and messages, PLY-04 stepping, FRM-04 overlay geometry.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PlaybackInfo, PlaybackTier } from "$lib/bindings";
import { defaultSettings } from "$lib/queue/testClips";
import {
  codecAdvice,
  computeTimeOffset,
  confirmFirstFrame,
  containRect,
  DEFAULT_FRAME_DURATION,
  formatEta,
  frameDurationOf,
  ignorePlaybackInfo,
  loopPreviewRange,
  mediaErrorReason,
  nextPlaybackTier,
  parseFrameRate,
  PRESENT_GRACE_MS,
  tierLabel,
  loopAsExportedAvailable,
  unsupportedMessage,
} from "./playback";

describe("MRK-07 loop as exported", () => {
  it("is offered only for the Lossless preset", () => {
    expect(loopAsExportedAvailable(defaultSettings())).toBe(true);
    expect(loopAsExportedAvailable(defaultSettings({ cut_mode: "accurate" }))).toBe(false);
    expect(loopAsExportedAvailable(null)).toBe(false);
  });

  const range = { start: 5.4, end: 10 };
  const keyframe = (time: number) => Math.floor(time / 2) * 2;

  it("includes the effective lossless keyframe and follows retimes", () => {
    expect(loopPreviewRange(range, true, defaultSettings(), keyframe)).toEqual({ start: 4, end: 10 });
    expect(loopPreviewRange({ start: 7.5, end: 12 }, true, defaultSettings(), keyframe)).toEqual({ start: 6, end: 12 });
  });

  it("includes the lossless lead-in even when Snap In is enabled", () => {
    expect(loopPreviewRange(range, true, defaultSettings({ snap_in_to_keyframe: true }), keyframe)).toEqual({ start: 4, end: 10 });
  });

  it("keeps marked edges when disabled, re-encoding, or missing keyframes", () => {
    expect(loopPreviewRange(range, false, defaultSettings(), keyframe)).toEqual(range);
    expect(loopPreviewRange(range, true, defaultSettings({ cut_mode: "accurate" }), keyframe)).toEqual(range);
    expect(loopPreviewRange(range, true, null, keyframe)).toEqual(range);
    expect(loopPreviewRange(range, true, defaultSettings(), () => null)).toEqual(range);
    expect(loopPreviewRange({ start: 0.5, end: 1 }, true, defaultSettings(), keyframe)).toEqual({ start: 0, end: 1 });
  });
});

describe("§7.3 preparation fallback decisions", () => {
  const failed: PlaybackInfo = { tier: "remux", next_tier: "proxy", media_path: null, mime_type: "video/mp4",
    preparing: false, progress: null, eta_sec: null, unsupported_reason: "Remux failed" };

  it("a failed Remux advances to Proxy once", () => {
    const requested = new Set<PlaybackTier>(["remux"]);
    expect(ignorePlaybackInfo(failed, "remux")).toBe(false);
    expect(nextPlaybackTier(failed, new Set(["remux"]), requested)).toBe("proxy");
    requested.add("proxy");
    expect(nextPlaybackTier(failed, new Set(["remux"]), requested)).toBeNull();
    expect(nextPlaybackTier(failed, new Set(["proxy"]), new Set())).toBeNull();
    expect(nextPlaybackTier({ ...failed, tier: "proxy", next_tier: null }, new Set(), new Set())).toBeNull();
  });

  it("ignores superseded tier progress, failures and completions, and all cancellations", () => {
    expect(ignorePlaybackInfo(failed, "proxy")).toBe(true);
    expect(ignorePlaybackInfo({ ...failed, preparing: true, unsupported_reason: null }, "proxy")).toBe(true);
    expect(ignorePlaybackInfo({ ...failed, media_path: "/old.mp4", unsupported_reason: null }, "proxy")).toBe(true);
    expect(ignorePlaybackInfo({ ...failed, unsupported_reason: "Preparing the preview was cancelled." }, "remux")).toBe(true);
  });

  it("accepts initial Remux preparation and the requested Proxy result", () => {
    expect(ignorePlaybackInfo({ ...failed, preparing: true, unsupported_reason: null }, null)).toBe(false);
    expect(ignorePlaybackInfo({ ...failed, tier: "proxy", media_path: "/proxy.mp4", unsupported_reason: null }, "proxy")).toBe(false);
  });
});

/** A `<video>` stand-in: decode() fires loadeddata, present() fires the frame callback. */
class FakeVideo extends EventTarget {
  readyState = 0;
  videoWidth = 0;
  videoHeight = 0;
  currentTime = 0;
  error: { code: number } | null = null;
  private frameCallback: ((now: number, meta: { mediaTime: number }) => void) | null = null;
  requestVideoFrameCallback(cb: (now: number, meta: { mediaTime: number }) => void): number {
    this.frameCallback = cb;
    return 1;
  }
  decode(w = 640, h = 360): void {
    this.videoWidth = w;
    this.videoHeight = h;
    this.readyState = 4;
    this.dispatchEvent(new Event("loadeddata"));
  }
  present(mediaTime: number): void {
    this.frameCallback?.(0, { mediaTime });
  }
  fail(code: number): void {
    this.error = { code };
    this.dispatchEvent(new Event("error"));
  }
}

describe("§7.3 first-frame confirmation", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("a picture decoded inside the budget succeeds even when it is painted after the budget", async () => {
    const v = new FakeVideo();
    const result = confirmFirstFrame(v, 3000);
    vi.advanceTimersByTime(2700); // busy machine: the HEVC decoder needs 2.7 s
    v.decode();
    vi.advanceTimersByTime(400); // past the 3 s budget, the frame is not painted yet
    v.present(0.023);
    await expect(result).resolves.toEqual({ ok: true, mediaTime: 0.023 });
  });

  it("uses the loaded frame's time when no new frame is painted", async () => {
    const v = new FakeVideo();
    v.currentTime = 1.5;
    const result = confirmFirstFrame(v, 3000);
    v.decode();
    vi.advanceTimersByTime(PRESENT_GRACE_MS);
    await expect(result).resolves.toEqual({ ok: true, mediaTime: 1.5 });
  });

  it("an element that already has data is confirmed at once", async () => {
    const v = new FakeVideo();
    v.videoWidth = 320;
    v.videoHeight = 240;
    v.readyState = 2;
    const result = confirmFirstFrame(v, 3000);
    v.present(0);
    await expect(result).resolves.toEqual({ ok: true, mediaTime: 0 });
  });

  it("fails after the budget when nothing is decoded", async () => {
    const result = confirmFirstFrame(new FakeVideo(), 3000);
    vi.advanceTimersByTime(3000);
    const r = await result;
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.reason).toMatch(/3/);
  });

  it("a decode without a picture (audio only) and a media error fail at once", async () => {
    const audioOnly = new FakeVideo();
    const a = confirmFirstFrame(audioOnly, 3000);
    audioOnly.decode(0, 0);
    expect((await a).ok).toBe(false);
    const broken = new FakeVideo();
    const b = confirmFirstFrame(broken, 3000);
    broken.fail(4);
    await expect(b).resolves.toEqual({ ok: false, reason: mediaErrorReason(4) });
  });
});

describe("PLY-04 frame rate", () => {
  it("parses FFprobe rationals, never floats from the probe", () => {
    expect(parseFrameRate("30/1")).toBe(30);
    expect(parseFrameRate("60000/1001")).toBeCloseTo(59.94, 2);
    expect(parseFrameRate("25")).toBe(25);
    expect(parseFrameRate("0/0")).toBeNull();
    expect(parseFrameRate("abc")).toBeNull();
    expect(parseFrameRate(null)).toBeNull();
  });

  it("frame duration falls back to 30 fps", () => {
    expect(frameDurationOf("50/1")).toBe(0.02);
    expect(frameDurationOf("0/0")).toBe(DEFAULT_FRAME_DURATION);
  });
});


