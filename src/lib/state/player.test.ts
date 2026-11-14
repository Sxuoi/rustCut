// PLY-02: a rapid second step builds on the first seek target, not the stale frame.
import { afterEach, describe, expect, it, vi } from "vitest";
import { defaultSettings } from "$lib/queue/testClips";
import { media } from "./media.svelte";
import { project } from "./project.svelte";
import { PlayerState } from "./player.svelte";

afterEach(() => {
  vi.restoreAllMocks();
  media.reset();
});

function fakeVideo() {
  const el = { paused: true, seeking: false, currentTime: 1, pause() {}, play: () => Promise.resolve() };
  return el as unknown as HTMLVideoElement;
}

describe("stepFrame after a finished seek, before the frame is presented", () => {
  it("does not repeat the previous step", () => {
    const p = new PlayerState();
    const el = fakeVideo();
    p.el = el;
    p.duration = 10;
    p.frameDuration = 0.1;
    (p as unknown as { presentedMediaTime: number }).presentedMediaTime = 1; // stale frame
    p.stepFrame(1);
    expect(el.currentTime).toBeCloseTo(1.101, 3);
    // seek finished (seeking false) but rVFC has not fired yet
    p.stepFrame(1);
    expect(el.currentTime).toBeCloseTo(1.202, 3);
  });
});

describe("firstFrame (AC-14: background jobs wait for the first frame)", () => {
  it("resolves true when the first frame is presented, at once if it already is", async () => {
    const p = new PlayerState();
    const waiting = p.firstFrame(10_000);
    p.ready = true;
    await expect(waiting).resolves.toBe(true);
    await expect(p.firstFrame(10_000)).resolves.toBe(true);
  });

  it("resolves false after the timeout or when reset() starts another project, and clears its timer", async () => {
    vi.useFakeTimers();
    try {
      const p = new PlayerState();
      const timedOut = p.firstFrame(500);
      await vi.advanceTimersByTimeAsync(499);
      p.ready = false; // a tier switch is not a presented frame
      await vi.advanceTimersByTimeAsync(1);
      await expect(timedOut).resolves.toBe(false);
      const generation = p.generation;
      const superseded = p.firstFrame(500);
      p.reset();
      await expect(superseded).resolves.toBe(false);
      expect(p.generation).toBe(generation + 1);
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("QUE-03 loop wrap", () => {
  it("does not wrap again while the previous wrap's seek is still in flight", () => {
    const p = new PlayerState();
    const el = fakeVideo() as unknown as { paused: boolean; seeking: boolean; currentTime: number };
    p.el = el as unknown as HTMLVideoElement;
    p.duration = 20;
    p.updateLoopRange({ start: 5, end: 10 });
    el.paused = false;
    el.currentTime = 10; // a frame queued before the wrap still reports the end
    el.seeking = true;
    p.syncFromElement();
    expect(el.currentTime).toBe(10);
    el.seeking = false;
    p.syncFromElement();
    expect(el.currentTime).toBe(5);
  });
});

describe("MRK-07 live loop range", () => {
  it("starts at the effective keyframe and follows the marked retime updates from App", () => {
    vi.spyOn(project, "settings", "get").mockReturnValue(defaultSettings());
    media.keyframes = new Float64Array([0, 2, 4, 6, 8]);
    const p = new PlayerState();
    p.el = fakeVideo();
    p.duration = 100;
    p.loopAsExported = true;
    p.setLoop("clip", { start: 5.4, end: 10 });
    expect(p.el.currentTime).toBe(4);
    expect(p.loopRange).toEqual({ start: 4, end: 10 });
    p.updateLoopRange({ start: 7.5, end: 12 });
    expect(p.loopRange).toEqual({ start: 6, end: 12 });
    // App may repeat this marked update while comparing against the effective start.
    p.updateLoopRange({ start: 7.5, end: 12 });
    expect(p.loopRange).toEqual({ start: 6, end: 12 });
    p.loopAsExported = false;
    expect(p.loopRange).toEqual({ start: 7.5, end: 12 });
  });

  it("reacts to export settings and a newly available keyframe index", () => {
    const settings = vi.spyOn(project, "settings", "get").mockReturnValue(defaultSettings());
    const p = new PlayerState();
    p.loopAsExported = true;
    p.setLoop("clip", { start: 5.4, end: 10 });
    expect(p.loopRange?.start).toBe(5.4);
    media.keyframes = new Float64Array([0, 4]);
    expect(p.loopRange?.start).toBe(4);
    settings.mockReturnValue(defaultSettings({ snap_in_to_keyframe: true }));
    expect(p.loopRange?.start).toBe(4);
    p.setLoop(null, null);
    expect(p.loopRange).toBeNull();
  });
});
