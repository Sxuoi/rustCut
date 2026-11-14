// Header helpers: preset semantics, save state, names, sizes.
import { describe, expect, it } from "vitest";
import type { ExportSettings, SaveState } from "$lib/bindings";
import common from "$lib/i18n/en/common";
import { applyPreset, basename, formatBytes, formatSaveState, isVerticalCrop, PRESETS, presetLabelKey, presetOf, stem } from "./format";

const base = {
  output_dir: null,
  naming_template: "{index}_{name}",
  cut_mode: "accurate",
  framing: "original",
  fps_cap: 60,
} as unknown as ExportSettings;

describe("presets", () => {
  it("Lossless forces the original framing", () => {
    const s = applyPreset({ ...base, framing: "vertical_crop" }, "lossless");
    expect(s).toMatchObject({ cut_mode: "lossless", framing: "original" });
    expect(presetOf(s)).toBe("lossless");
  });

  it("vertical presets switch the cut mode to Frame-Accurate", () => {
    const lossless = applyPreset(base, "lossless");
    expect(applyPreset(lossless, "vcrop")).toMatchObject({ cut_mode: "accurate", framing: "vertical_crop" });
    expect(applyPreset(lossless, "vblur")).toMatchObject({ cut_mode: "accurate", framing: "vertical_blur" });
    expect(applyPreset(lossless, "accurate")).toMatchObject({ cut_mode: "accurate", framing: "original" });
  });

  it("keeps every other setting", () => {
    const s = applyPreset(base, "vcrop");
    expect(s.naming_template).toBe("{index}_{name}");
    expect(s.fps_cap).toBe(60);
  });

  it("round-trips and has labels", () => {
    for (const p of PRESETS) {
      expect(presetOf(applyPreset(base, p))).toBe(p);
      expect(common[presetLabelKey(p)]).toBeTruthy();
    }
    expect(isVerticalCrop(applyPreset(base, "vcrop"))).toBe(true);
    expect(isVerticalCrop(applyPreset(base, "vblur"))).toBe(false);
    expect(isVerticalCrop(null)).toBe(false);
  });

  it("a lossless cut mode wins over a stray vertical framing", () => {
    expect(presetOf({ cut_mode: "lossless", framing: "vertical_crop" })).toBe("lossless");
  });
});

describe("save state (§6.1 'saved 2s ago')", () => {
  const now = Date.parse("2026-11-13T12:00:00Z");
  const save = (over: Partial<SaveState>): SaveState => ({
    saved_at: new Date(now - 2000).toISOString(),
    pending: false,
    error: null,
    project_path: "D:/v/stream.rustcut",
    location: "next_to_video",
    ...over,
  });

  it("ticks in seconds, then minutes, then a clock time", () => {
    expect(formatSaveState(save({}), now).text).toBe("saved 2s ago");
    expect(formatSaveState(save({ saved_at: new Date(now - 150_000).toISOString() }), now).text).toBe("saved 2 min ago");
    expect(formatSaveState(save({ saved_at: new Date(now - 7_200_000).toISOString() }), now).text).toMatch(/^saved at \d\d:\d\d$/);
  });

  it("shows saving, errors and never-saved states", () => {
    expect(formatSaveState(save({ pending: true }), now)).toMatchObject({ text: "saving…", tone: "busy" });
    expect(formatSaveState(save({ error: "disk full" }), now)).toMatchObject({ tone: "error", title: "disk full" });
    expect(formatSaveState(save({ saved_at: null }), now).text).toBe("not saved yet");
    expect(formatSaveState(null, now).text).toBe("");
  });

  it("says when the project is stored in the app library (PRJ-02)", () => {
    expect(formatSaveState(save({ location: "app_library" }), now).location).toBe("stored in app library");
    expect(formatSaveState(save({}), now).location).toBeNull();
  });

  it("clock skew never shows a negative age", () => {
    expect(formatSaveState(save({ saved_at: new Date(now + 5000).toISOString() }), now).text).toBe("saved 0s ago");
  });
});

describe("names and sizes", () => {
  it("basename / stem with either separator", () => {
    expect(basename("C:\\videos\\live stream.mp4")).toBe("live stream.mp4");
    expect(basename("/home/u/a.mkv")).toBe("a.mkv");
    expect(stem("D:/v/livestream_2026_10_21.mp4")).toBe("livestream_2026_10_21");
    expect(stem("archive.2026.mkv")).toBe("archive.2026");
    expect(stem(".hidden")).toBe(".hidden");
  });

  it("formats bytes", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(5 * 1024 ** 3)).toBe("5.0 GB");
    expect(formatBytes(300 * 1024 ** 2)).toBe("300 MB");
  });
});
