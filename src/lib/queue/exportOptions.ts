// Export dialog option logic (§6.2): preset switching (EXP-02, §4.5), encoder
// choices (EXP-13), parallel-job defaults (EXP-12), output resolution and
// frame-rate caps (FRM-06), platform maximum durations (FRM-08) and the
// file-name template editor (EXP-05). No IPC.

import { get, writable } from "svelte/store";

import type {
  ConflictPolicy,
  CutMode,
  EncoderChoice,
  ExportSettings,
  Framing,
  HwEncoderInfo,
  OutputResolution,
  PlannedOutput,
  PlatformPreset as BackendPlatformPreset,
  Preset,
} from "$lib/bindings";

/** The four presets of the Export dialog, in display order. */
export const PRESETS: readonly Preset[] = ["lossless", "accurate", "vcrop", "vblur"];

const PRESET_MODES: Record<Preset, { cut_mode: CutMode; framing: Framing }> = {
  lossless: { cut_mode: "lossless", framing: "original" },
  accurate: { cut_mode: "accurate", framing: "original" },
  vcrop: { cut_mode: "accurate", framing: "vertical_crop" },
  vblur: { cut_mode: "accurate", framing: "vertical_blur" },
};

/** Mirror of `ExportSettings::preset`: Lossless ignores framing. */
export function presetOf(s: Pick<ExportSettings, "cut_mode" | "framing">): Preset {
  if (s.cut_mode === "lossless") return "lossless";
  if (s.framing === "vertical_crop") return "vcrop";
  if (s.framing === "vertical_blur") return "vblur";
  return "accurate";
}

/** Mirror of `ExportSettings::apply_preset`: a vertical preset switches the cut mode (§4.5). */
export function applyPreset(s: ExportSettings, preset: Preset): ExportSettings {
  return { ...s, ...PRESET_MODES[preset] };
}

/** Vertical Crop / Vertical Blur (9:16). */
export function isVerticalPreset(p: Preset | null): boolean {
  return p === "vcrop" || p === "vblur";
}

/** Presets that re-encode (everything but Lossless). */
export function isReencodePreset(p: Preset | null): boolean {
  return p !== null && p !== "lossless";
}

/** FRM-06 output sizes of the vertical presets. */
export const RESOLUTIONS: readonly OutputResolution[] = ["1080x1920", "720x1280", "native"];

/** FRM-06 frame-rate caps; `null` = source rate, uncapped. 60 is the default. */
export const FPS_CAPS: readonly (number | null)[] = [30, 60, 120, null];

/** EXP-07 policies for existing files, Auto-rename first (default). */
export const CONFLICT_POLICIES: readonly ConflictPolicy[] = ["rename", "overwrite", "skip"];

/** EXP-12 user range. */
export const PARALLEL_JOBS: readonly number[] = [1, 2, 3, 4];

const HARDWARE_ENCODERS: readonly EncoderChoice[] = [
  "h264_nvenc",
  "h264_amf",
  "h264_qsv",
  "h264_videotoolbox",
  "h264_vaapi",
];

/** True for the FFmpeg hardware encoders RustCut can use (EXP-13). */
export function isHardwareEncoder(name: string): boolean {
  return (HARDWARE_ENCODERS as readonly string[]).includes(name);
}

/** Narrow an FFmpeg encoder name to the settings enum. */
export function isEncoderChoice(name: string): name is EncoderChoice {
  return name === "auto" || name === "libx264" || isHardwareEncoder(name);
}

/** What "Auto" encodes with: the first available hardware encoder, else libx264 (EXP-13). */
export function resolveAutoEncoder(hw: readonly HwEncoderInfo[]): EncoderChoice {
  for (const e of hw) if (e.available && isHardwareEncoder(e.name) && isEncoderChoice(e.name)) return e.name;
  return "libx264";
}

/** One entry of the encoder selector. */
export interface EncoderOption {
  value: EncoderChoice;
  /** Detected label for hardware encoders; `null` = the UI's own label (Auto, libx264). */
  label: string | null;
  hardware: boolean;
  /** Unavailable encoders are listed but disabled. */
  available: boolean;
  /** Why a hardware encoder is unavailable (test-encode error). */
  reason: string | null;
}

/**
 * Encoder selector entries (EXP-13): Auto, libx264, then every detected
 * hardware encoder (unavailable ones disabled). A previously chosen encoder
 * that is no longer detected stays listed (disabled) so the choice is visible.
 */
export function encoderOptions(hw: readonly HwEncoderInfo[], current: EncoderChoice): EncoderOption[] {
  const out: EncoderOption[] = [
    { value: "auto", label: null, hardware: false, available: true, reason: null },
    { value: "libx264", label: null, hardware: false, available: true, reason: null },
  ];
  const seen = new Set<string>(["auto", "libx264"]);
  for (const e of hw) {
    if (!isEncoderChoice(e.name) || seen.has(e.name)) continue;
    seen.add(e.name);
    out.push({ value: e.name, label: e.label, hardware: true, available: e.available, reason: e.error });
  }
  if (!seen.has(current)) out.push({ value: current, label: current, hardware: true, available: false, reason: null });
  return out;
}

/**
 * EXP-12 default number of concurrent FFmpeg processes: Lossless 1
 * (disk-bound), hardware encode 2, software re-encode 2 on ≥ 8 cores else 1.
 */
export function defaultParallelJobs(opts: {
  preset: Preset;
  encoder: EncoderChoice;
  hw: readonly HwEncoderInfo[];
  cpuCount: number;
}): number {
  if (opts.preset === "lossless") return 1;
  const effective = opts.encoder === "auto" ? resolveAutoEncoder(opts.hw) : opts.encoder;
  if (isHardwareEncoder(effective)) return 2;
  return opts.cpuCount >= 8 ? 2 : 1;
}

/** Clamp to the EXP-12 range 1–4. */
export function clampJobs(n: number): number {
  const v = Number.isFinite(n) ? Math.round(n) : 1;
  return Math.min(4, Math.max(1, v));
}

/** A platform with an optional maximum clip duration (FRM-08). */
export interface PlatformPreset {
  id: string;
  /** Display name supplied by the backend (platform-presets.json). */
  label: string;
  /** Longest clip the platform accepts; `null` = no limit. */
  maxDurationSec: number | null;
  /** Frame-rate cap the platform wants (FRM-06); `null` = keep the user's setting. */
  fpsCap: number | null;
  /** Output size the platform wants; `null` = keep the user's setting. */
  outputResolution: OutputResolution | null;
}

/** Recovery defaults when the backend cannot be asked (same values as the bundled file). */
export const PLATFORM_PRESETS: readonly PlatformPreset[] = [
  { id: "youtube_shorts", label: "YouTube Shorts", maxDurationSec: 180, fpsCap: 60, outputResolution: "1080x1920" },
  { id: "tiktok", label: "TikTok", maxDurationSec: 600, fpsCap: 60, outputResolution: "1080x1920" },
  { id: "instagram_reels", label: "Instagram Reels", maxDurationSec: 180, fpsCap: 60, outputResolution: "1080x1920" },
];

/** Presets shown in the Export dialog; replaced once the backend answers. */
export const platformPresets = writable<readonly PlatformPreset[]>(PLATFORM_PRESETS);

/** Map backend presets, dropping malformed entries (empty id/label, non-positive limit). */
export function fromBackendPresets(list: readonly BackendPlatformPreset[]): PlatformPreset[] {
  const seen = new Set<string>();
  const out: PlatformPreset[] = [];
  for (const p of list) {
    if (!p.id || !p.label || seen.has(p.id) || p.id === CUSTOM_PLATFORM) continue;
    const max = p.max_duration_sec;
    if (max !== null && !(Number.isFinite(max) && max > 0)) continue;
    seen.add(p.id);
    const fps = p.fps_cap;
    out.push({
      id: p.id,
      label: p.label,
      maxDurationSec: max,
      // An unusable cap or size is ignored (the user's setting stays), not a reason to drop the platform.
      fpsCap: fps !== null && Number.isFinite(fps) && fps > 0 ? fps : null,
      outputResolution: RESOLUTIONS.includes(p.output_resolution) ? p.output_resolution : null,
    });
  }
  return out;
}

/** FRM-08: ask the backend (it merges the user's platform-presets.json with the bundled list). */
export async function loadPlatformPresets(source: { getPlatformPresets(): Promise<BackendPlatformPreset[]> }): Promise<void> {
  try {
    const list = fromBackendPresets(await source.getPlatformPresets());
    platformPresets.set(list.length > 0 ? list : PLATFORM_PRESETS);
  } catch (error) {
    console.warn("Could not load platform presets; using built-in defaults.", error);
    platformPresets.set(PLATFORM_PRESETS);
  }
}

/** `platform_preset` value once the user edits the maximum by hand. */
export const CUSTOM_PLATFORM = "custom";

export function platformById(id: string | null): PlatformPreset | undefined {
  return id === null ? undefined : get(platformPresets).find((p) => p.id === id);
}

/** Choose a platform: its default maximum, `custom` keeps the current one, `null` removes the limit. */
export function applyPlatform(s: ExportSettings, id: string | null): ExportSettings {
  if (id === null) return { ...s, platform_preset: null, max_duration_sec: null };
  const p = platformById(id);
  if (p) {
    return {
      ...s,
      platform_preset: p.id,
      max_duration_sec: p.maxDurationSec,
      ...(p.fpsCap !== null ? { fps_cap: p.fpsCap } : {}),
      ...(p.outputResolution !== null ? { output_resolution: p.outputResolution } : {}),
    };
  }
  return { ...s, platform_preset: CUSTOM_PLATFORM, max_duration_sec: s.max_duration_sec };
}

/**
 * Edit the maximum duration by hand. A value equal to the chosen platform's
 * default keeps the platform; any other value becomes `custom`; an empty or
 * non-positive value removes the limit.
 */
export function setMaxDuration(s: ExportSettings, sec: number | null): ExportSettings {
  if (sec === null || !Number.isFinite(sec) || sec <= 0) return { ...s, platform_preset: null, max_duration_sec: null };
  const p = platformById(s.platform_preset);
  const same = p !== undefined && p.maxDurationSec !== null && Math.round(p.maxDurationSec * 1000) === Math.round(sec * 1000);
  return { ...s, platform_preset: same && p ? p.id : CUSTOM_PLATFORM, max_duration_sec: sec };
}

/** EXP-05 template variables, in chip order. */
export const TEMPLATE_VARIABLES: readonly string[] = [
  "index",
  "name",
  "source_name",
  "start",
  "end",
  "duration",
  "date",
  "tag",
  "preset",
];

/** Replace the selection `[start, end)` of `text` with `token`; returns the new text and caret. */
export function insertToken(text: string, start: number, end: number, token: string): { text: string; caret: number } {
  const len = text.length;
  const a = Math.min(Math.max(0, Number.isFinite(start) ? start : len), len);
  const b = Math.min(Math.max(a, Number.isFinite(end) ? end : a), len);
  return { text: text.slice(0, a) + token + text.slice(b), caret: a + token.length };
}

/** `{names}` in a template that are not EXP-05 variables, in order, without repeats. */
export function unknownVariables(template: string): string[] {
  const out: string[] = [];
  for (const m of template.matchAll(/\{([^{}]*)\}/g)) {
    const name = m[1];
    if (!TEMPLATE_VARIABLES.includes(name) && !out.includes(name)) out.push(name);
  }
  return out;
}

/** EXP-05: templates never contain the extension (it is appended automatically). */
export function templateHasExtension(template: string): boolean {
  return /\.(mp4|mkv|mov|m4v|webm|avi|ts|mts|m2ts|flv|wmv)$/i.test(template.trim());
}

/** The dialog's name preview: the first `n` planned outputs by clip number, plus how many more. */
export function previewNames(outputs: readonly PlannedOutput[], n = 3): { shown: PlannedOutput[]; more: number } {
  const sorted = [...outputs].sort((a, b) => a.index - b.index);
  return { shown: sorted.slice(0, n), more: Math.max(0, sorted.length - n) };
}

/** Lossless lead-in summary (EXP-02, MRK-07): largest known lead-in and how many are unknown. */
export function leadInSummary(outputs: readonly PlannedOutput[]): { maxSec: number | null; known: number; unknown: number } {
  let maxSec: number | null = null;
  let known = 0;
  let unknown = 0;
  for (const o of outputs) {
    if (o.action === "skip") continue;
    if (o.lead_in_sec === null || !Number.isFinite(o.lead_in_sec)) {
      unknown++;
      continue;
    }
    known++;
    maxSec = maxSec === null ? o.lead_in_sec : Math.max(maxSec, o.lead_in_sec);
  }
  return { maxSec, known, unknown };
}
