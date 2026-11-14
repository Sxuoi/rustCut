import { afterEach, describe, expect, it, vi } from "vitest";
import { get } from "svelte/store";
import type { PlatformPreset as BackendPlatformPreset } from "$lib/bindings";
import { MockBackend } from "$lib/ipc/mock";
import type { HwEncoderInfo } from "$lib/bindings";
import {
  applyPlatform,
  applyPreset,
  clampJobs,
  CUSTOM_PLATFORM,
  defaultParallelJobs,
  encoderOptions,
  FPS_CAPS,
  insertToken,
  isEncoderChoice,
  isHardwareEncoder,
  isReencodePreset,
  isVerticalPreset,
  leadInSummary,
  fromBackendPresets,
  loadPlatformPresets,
  PLATFORM_PRESETS,
  platformPresets,
  PRESETS,
  presetOf,
  previewNames,
  resolveAutoEncoder,
  RESOLUTIONS,
  setMaxDuration,
  templateHasExtension,
  TEMPLATE_VARIABLES,
  unknownVariables,
} from "./exportOptions";
import { defaultSettings, makeOutput } from "./testClips";

afterEach(() => {
  platformPresets.set(PLATFORM_PRESETS);
  vi.restoreAllMocks();
});

const bp = (id: string, max: number | null, label = id): BackendPlatformPreset => ({
  id, label, framing: "vertical_crop", output_resolution: "1080x1920", max_duration_sec: max, fps_cap: null,
});

describe("FRM-08 platform presets from the backend", () => {
  it("the mock backend serves the bundled defaults, which match the recovery list", async () => {
    const list = await new MockBackend().getPlatformPresets();
    expect(fromBackendPresets(list)).toEqual(PLATFORM_PRESETS);
  });

  it("uses backend limits and labels when choosing or editing a platform", async () => {
    await loadPlatformPresets({ getPlatformPresets: async () => [bp("youtube_shorts", 210, "Shorts"), bp("x", null, "X")] });
    expect(get(platformPresets)).toEqual([
      { id: "youtube_shorts", label: "Shorts", maxDurationSec: 210, fpsCap: null, outputResolution: "1080x1920" },
      { id: "x", label: "X", maxDurationSec: null, fpsCap: null, outputResolution: "1080x1920" },
    ]);
    const settings = applyPlatform(defaultSettings(), "youtube_shorts");
    expect(settings.max_duration_sec).toBe(210);
    expect(setMaxDuration(settings, 210).platform_preset).toBe("youtube_shorts");
    expect(setMaxDuration(settings, 180).platform_preset).toBe("custom");
    expect(applyPlatform(settings, "x")).toMatchObject({ platform_preset: "x", max_duration_sec: null });
    expect(setMaxDuration(applyPlatform(settings, "x"), 60).platform_preset).toBe("custom");
  });

  it("choosing a platform applies its frame-rate cap and output size; missing values keep the user's", async () => {
    await loadPlatformPresets({
      getPlatformPresets: async () => [
        { ...bp("tiktok", 600), fps_cap: 30, output_resolution: "720x1280" },
        { ...bp("loose", null), fps_cap: null, output_resolution: "bogus" as BackendPlatformPreset["output_resolution"] },
      ],
    });
    const mine = { ...defaultSettings(), fps_cap: 120, output_resolution: "native" as const };
    expect(applyPlatform(mine, "tiktok")).toMatchObject({ platform_preset: "tiktok", max_duration_sec: 600, fps_cap: 30, output_resolution: "720x1280" });
    expect(applyPlatform(mine, "loose")).toMatchObject({ platform_preset: "loose", fps_cap: 120, output_resolution: "native" });
  });

  it("drops malformed entries", () => {
    const list = [bp("a", 10), bp("a", 20), bp("", 5), bp("b", 0), bp("c", -1), bp("d", NaN), bp("custom", 5), { ...bp("e", 5), label: "" }];
    expect(fromBackendPresets(list).map((p) => p.id)).toEqual(["a"]);
  });

  it.each(["rejects", "empty"])("falls back to built-in defaults when the backend %s", async (failure) => {
    platformPresets.set([]);
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await loadPlatformPresets({ getPlatformPresets: async () => { if (failure === "rejects") throw new Error("offline"); return []; } });
    expect(get(platformPresets)).toEqual(PLATFORM_PRESETS);
    expect(warn).toHaveBeenCalledTimes(failure === "rejects" ? 1 : 0);
  });
});

const amf: HwEncoderInfo = { name: "h264_amf", label: "AMD AMF H.264", available: true, error: null };
const nvenc: HwEncoderInfo = { name: "h264_nvenc", label: "NVIDIA NVENC H.264", available: false, error: "No device" };
const qsv: HwEncoderInfo = { name: "h264_qsv", label: "Intel Quick Sync H.264", available: false, error: "x" };

describe("§6.2 presets", () => {
  it("lists the four presets", () => {
    expect(PRESETS).toEqual(["lossless", "accurate", "vcrop", "vblur"]);
  });

  it("mirrors ExportSettings::preset (Lossless ignores framing)", () => {
    expect(presetOf({ cut_mode: "lossless", framing: "vertical_crop" })).toBe("lossless");
    expect(presetOf({ cut_mode: "accurate", framing: "original" })).toBe("accurate");
    expect(presetOf({ cut_mode: "accurate", framing: "vertical_crop" })).toBe("vcrop");
    expect(presetOf({ cut_mode: "accurate", framing: "vertical_blur" })).toBe("vblur");
  });

  it("a vertical preset switches the cut mode to Frame-Accurate", () => {
    const s = applyPreset(defaultSettings(), "vcrop");
    expect([s.cut_mode, s.framing]).toEqual(["accurate", "vertical_crop"]);
    const back = applyPreset(s, "lossless");
    expect([back.cut_mode, back.framing]).toEqual(["lossless", "original"]);
    for (const p of PRESETS) expect(presetOf(applyPreset(defaultSettings(), p))).toBe(p);
  });

  it("classifies presets", () => {
    expect(isVerticalPreset("vcrop") && isVerticalPreset("vblur")).toBe(true);
    expect(isVerticalPreset("accurate") || isVerticalPreset(null)).toBe(false);
    expect(isReencodePreset("accurate") && !isReencodePreset("lossless") && !isReencodePreset(null)).toBe(true);
  });
});

describe("FRM-06 resolution and frame-rate options", () => {
  it("offers 1080x1920 (default first), 720x1280 and Native", () => {
    expect(RESOLUTIONS).toEqual(["1080x1920", "720x1280", "native"]);
  });

  it("offers 30/60/120 caps and uncapped source rate", () => {
    expect(FPS_CAPS).toEqual([30, 60, 120, null]);
    expect(defaultSettings().fps_cap).toBe(60);
  });
});

describe("EXP-13 encoders", () => {
  it("recognises encoder names", () => {
    expect(isHardwareEncoder("h264_amf")).toBe(true);
    expect(isHardwareEncoder("libx264")).toBe(false);
    expect(isEncoderChoice("auto") && isEncoderChoice("h264_vaapi")).toBe(true);
    expect(isEncoderChoice("hevc_nvenc")).toBe(false);
  });

  it("Auto resolves to the first available hardware encoder, else libx264", () => {
    expect(resolveAutoEncoder([nvenc, amf])).toBe("h264_amf");
    expect(resolveAutoEncoder([nvenc])).toBe("libx264");
    expect(resolveAutoEncoder([])).toBe("libx264");
  });

  it("lists Auto, libx264 and detected encoders with unavailable ones disabled", () => {
    const opts = encoderOptions([nvenc, amf, { ...amf }, { name: "hevc_amf", label: "x", available: true, error: null }], "auto");
    expect(opts.map((o) => o.value)).toEqual(["auto", "libx264", "h264_nvenc", "h264_amf"]);
    expect(opts.find((o) => o.value === "h264_nvenc")).toMatchObject({ available: false, reason: "No device" });
    expect(opts.find((o) => o.value === "h264_amf")).toMatchObject({ available: true, label: "AMD AMF H.264" });
  });

  it("keeps a chosen encoder that is no longer detected, disabled", () => {
    const opts = encoderOptions([amf], "h264_qsv");
    expect(opts.at(-1)).toMatchObject({ value: "h264_qsv", available: false });
  });
});

describe("EXP-12 parallel job defaults", () => {
  it("Lossless defaults to 1", () => {
    expect(defaultParallelJobs({ preset: "lossless", encoder: "auto", hw: [amf], cpuCount: 16 })).toBe(1);
  });

  it("hardware encode defaults to 2 (explicit or via Auto)", () => {
    expect(defaultParallelJobs({ preset: "vcrop", encoder: "h264_amf", hw: [amf], cpuCount: 4 })).toBe(2);
    expect(defaultParallelJobs({ preset: "accurate", encoder: "auto", hw: [amf], cpuCount: 4 })).toBe(2);
  });

  it("software re-encode defaults to 2 on ≥ 8 cores, else 1", () => {
    expect(defaultParallelJobs({ preset: "vblur", encoder: "libx264", hw: [amf], cpuCount: 8 })).toBe(2);
    expect(defaultParallelJobs({ preset: "vblur", encoder: "libx264", hw: [], cpuCount: 6 })).toBe(1);
    expect(defaultParallelJobs({ preset: "accurate", encoder: "auto", hw: [qsv], cpuCount: 12 })).toBe(2);
    expect(defaultParallelJobs({ preset: "accurate", encoder: "auto", hw: [qsv], cpuCount: 0 })).toBe(1);
  });

  it("clamps the user range to 1–4", () => {
    expect([clampJobs(0), clampJobs(2.4), clampJobs(9), clampJobs(Number.NaN)]).toEqual([1, 2, 4, 1]);
  });
});

describe("FRM-08 platform presets", () => {
  it("ships editable defaults for Shorts, TikTok and Reels", () => {
    expect(PLATFORM_PRESETS.map((p) => [p.id, p.maxDurationSec])).toEqual([
      ["youtube_shorts", 180],
      ["tiktok", 600],
      ["instagram_reels", 180],
    ]);
  });

  it("choosing a platform sets its maximum; None removes it", () => {
    const s = applyPlatform(defaultSettings(), "youtube_shorts");
    expect([s.platform_preset, s.max_duration_sec]).toEqual(["youtube_shorts", 180]);
    const none = applyPlatform(s, null);
    expect([none.platform_preset, none.max_duration_sec]).toEqual([null, null]);
    const custom = applyPlatform(s, CUSTOM_PLATFORM);
    expect([custom.platform_preset, custom.max_duration_sec]).toEqual([CUSTOM_PLATFORM, 180]);
  });

  it("editing the maximum keeps the platform only when it matches its default", () => {
    const shorts = applyPlatform(defaultSettings(), "youtube_shorts");
    expect(setMaxDuration(shorts, 180).platform_preset).toBe("youtube_shorts");
    const edited = setMaxDuration(shorts, 60);
    expect([edited.platform_preset, edited.max_duration_sec]).toEqual([CUSTOM_PLATFORM, 60]);
    expect(setMaxDuration(defaultSettings(), 90).platform_preset).toBe(CUSTOM_PLATFORM);
    const cleared = setMaxDuration(shorts, null);
    expect([cleared.platform_preset, cleared.max_duration_sec]).toEqual([null, null]);
    expect(setMaxDuration(shorts, 0).max_duration_sec).toBeNull();
  });
});

describe("EXP-05 template editor", () => {
  it("offers every template variable", () => {
    expect(TEMPLATE_VARIABLES).toEqual(["index", "name", "source_name", "start", "end", "duration", "date", "tag", "preset"]);
  });

  it("inserts a variable at the caret or over the selection", () => {
    expect(insertToken("{index}_", 8, 8, "{name}")).toEqual({ text: "{index}_{name}", caret: 14 });
    expect(insertToken("{index}_X", 8, 9, "{preset}")).toEqual({ text: "{index}_{preset}", caret: 16 });
    expect(insertToken("ab", 99, 99, "{tag}")).toEqual({ text: "ab{tag}", caret: 7 });
    expect(insertToken("ab", 2, 0, "{tag}")).toEqual({ text: "ab{tag}", caret: 7 });
  });

  it("reports unknown variables once, in order", () => {
    expect(unknownVariables("{index}_{nmae}_{nmae}_{foo}{name}")).toEqual(["nmae", "foo"]);
    expect(unknownVariables("{source_name}_{start}_{name}")).toEqual([]);
  });

  it("detects a file extension in the template", () => {
    expect(templateHasExtension("{index}_{name}.mp4")).toBe(true);
    expect(templateHasExtension("{index}_{name}.MKV ")).toBe(true);
    expect(templateHasExtension("{index}_v2.0")).toBe(false);
  });

  it("previews the first three names by clip number", () => {
    const outs = [5, 1, 3, 2].map((index) => makeOutput({ index }));
    const p = previewNames(outs);
    expect(p.shown.map((o) => o.index)).toEqual([1, 2, 3]);
    expect(p.more).toBe(1);
    expect(previewNames([]).more).toBe(0);
  });
});

describe("EXP-02 lossless lead-in summary", () => {
  it("reports the largest known lead-in and unknown count, ignoring skipped clips", () => {
    const outs = [
      makeOutput({ index: 1, lead_in_sec: 1.5 }),
      makeOutput({ index: 2, lead_in_sec: 4.2 }),
      makeOutput({ index: 3, lead_in_sec: null }),
      makeOutput({ index: 4, lead_in_sec: 9, action: "skip" }),
    ];
    expect(leadInSummary(outs)).toEqual({ maxSec: 4.2, known: 2, unknown: 1 });
    expect(leadInSummary([])).toEqual({ maxSec: null, known: 0, unknown: 0 });
  });
});
