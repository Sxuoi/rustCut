import { afterEach, describe, expect, it, vi } from "vitest";
import type { MediaIndexInfo } from "$lib/bindings";
import { media, stepFrameTime } from "./media.svelte";
import { player } from "./player.svelte";

const info = (over: Partial<MediaIndexInfo>): MediaIndexInfo => ({
  keyframes: [0, 2],
  frame_duration_sec: 0.04,
  vfr: false,
  frame_times: null,
  source: "packet_scan",
  complete: true,
  ...over,
});

afterEach(() => {
  media.reset();
  vi.restoreAllMocks();
});

describe("stepFrameTime (PLY-04 VFR)", () => {
  const times = [0, 0.04, 0.1, 0.15, 0.4];
  it("steps to the next and previous real frame", () => {
    expect(stepFrameTime(times, 0.04, 1)).toBe(0.1);
    expect(stepFrameTime(times, 0.041, 1)).toBe(0.1);
    expect(stepFrameTime(times, 0.1, -1)).toBe(0.04);
    expect(stepFrameTime(times, 0.12, -1)).toBe(0.04);
  });
  it("clamps at the ends and handles empty input", () => {
    expect(stepFrameTime(times, 0, -1)).toBe(0);
    expect(stepFrameTime(times, 0.4, 1)).toBe(0.4);
    expect(stepFrameTime([], 1, 1)).toBeNull();
  });
});

describe("media index frame-step hook", () => {
  it("registers for VFR sources with frame times and unregisters on reset", () => {
    const spy = vi.spyOn(player, "setFrameStepHook");
    media.handleIndex(info({ vfr: true, frame_times: [0, 0.04, 0.1] }));
    const hook = spy.mock.calls.at(-1)?.[0];
    expect(typeof hook).toBe("function");
    expect(hook?.(0.04, 1)).toBe(0.1);
    expect(hook?.(0.04, -1)).toBe(0);
    media.reset();
    expect(spy.mock.calls.at(-1)?.[0]).toBeNull();
    expect(hook?.(0.04, 1)).toBeNull();
  });

  it("does not register for CFR sources", () => {
    const spy = vi.spyOn(player, "setFrameStepHook");
    media.handleIndex(info({ vfr: false, frame_times: null }));
    expect(spy.mock.calls.at(-1)?.[0]).toBeNull();
  });

  it("exposes keyframes for lossless awareness (MRK-07)", () => {
    media.handleIndex(info({ keyframes: [0, 2, 4] }));
    expect(media.keyframeAtOrBefore(3.4)).toBe(2);
    expect(media.leadIn(3.4)).toBeCloseTo(1.4, 9);
  });
});
