// TypeScript ports of the pure helpers of crates/rustcut-model: default
// settings, normalisation, presets, output signature, tags, view state.

import type { AppSettings, ExportSettings, Preset, Tag, ViewState } from "$lib/bindings";
import { clamp } from "./util";

export const SCHEMA_VERSION = 2;
export const APP_VERSION = "0.1.0-mock";
export const DEFAULT_MIN_CLIP_SEC = 0.5;
export const MIN_CLIP_SEC_RANGE: readonly [number, number] = [0.1, 5];
export const ZOOM_WINDOW_RANGE_SEC: readonly [number, number] = [2, 600];
export const DEFAULT_NAMING_TEMPLATE = "{index}_{name}";
/** MRK-04: at least 200 undo steps. */
export const UNDO_LIMIT = 500;

export function defaultTags(): Tag[] {
  return [
    { id: "funny", label: "Funny", color: "#EF4444" },
    { id: "edu", label: "Educational", color: "#3B82F6" },
    { id: "highlight", label: "Highlight", color: "#22C55E" },
  ];
}

export function defaultViewState(): ViewState {
  return { playhead_sec: 0, zoom_window_sec: 20, selected_clip_id: null };
}

export function defaultExportSettings(): ExportSettings {
  return {
    output_dir: null,
    naming_template: DEFAULT_NAMING_TEMPLATE,
    spaces_to_underscores: true,
    on_conflict: "rename",
    cut_mode: "lossless",
    framing: "original",
    output_resolution: "1080x1920",
    fps_cap: 60,
    audio_track: 1,
    encoder: "auto",
    parallel_jobs: 1,
    lossless_audio_to_aac: false,
    snap_in_to_keyframe: false,
    mix_all_audio: false,
    max_duration_sec: null,
    platform_preset: null,
  };
}

export function defaultAppSettings(): AppSettings {
  return {
    project_location: "next_to_video",
    min_clip_duration_sec: DEFAULT_MIN_CLIP_SEC,
    cache_cap_gb: 50,
    telemetry_opt_in: false,
    default_spaces_to_underscores: true,
    reduced_motion: null,
    language: "en",
  };
}

/** `AppSettings::normalized`. */
export function normalizeAppSettings(s: AppSettings): AppSettings {
  const out = { ...defaultAppSettings(), ...s };
  if (!Number.isFinite(out.min_clip_duration_sec)) out.min_clip_duration_sec = DEFAULT_MIN_CLIP_SEC;
  out.min_clip_duration_sec = clamp(out.min_clip_duration_sec, MIN_CLIP_SEC_RANGE[0], MIN_CLIP_SEC_RANGE[1]);
  out.cache_cap_gb = clamp(Math.round(Number.isFinite(out.cache_cap_gb) ? out.cache_cap_gb : 50), 1, 10_000);
  if (!out.language || out.language.trim() === "") out.language = "en";
  return out;
}

/** `ExportSettings::preset`. */
export function presetOf(s: ExportSettings): Preset {
  if (s.cut_mode === "lossless") return "lossless";
  if (s.framing === "vertical_crop") return "vcrop";
  if (s.framing === "vertical_blur") return "vblur";
  return "accurate";
}

export const PRESET_LABELS: Record<Preset, string> = {
  lossless: "Lossless (Original)",
  accurate: "Frame-Accurate (Original)",
  vcrop: "Vertical Crop 9:16",
  vblur: "Vertical Blur 9:16",
};

export function isVertical(p: Preset): boolean {
  return p === "vcrop" || p === "vblur";
}

/** `ExportSettings::apply_preset`. */
export function applyPreset(s: ExportSettings, preset: Preset): ExportSettings {
  const out = { ...s };
  out.cut_mode = preset === "lossless" ? "lossless" : "accurate";
  out.framing = preset === "vcrop" ? "vertical_crop" : preset === "vblur" ? "vertical_blur" : "original";
  return out;
}

/** `ExportSettings::normalized`. */
export function normalizeExportSettings(input: ExportSettings): ExportSettings {
  const s: ExportSettings = { ...defaultExportSettings(), ...input };
  s.parallel_jobs = clamp(Math.trunc(Number.isFinite(s.parallel_jobs) ? s.parallel_jobs : 1), 1, 4);
  s.audio_track = Math.max(1, Math.trunc(Number.isFinite(s.audio_track) ? s.audio_track : 1));
  if (s.fps_cap !== null) {
    s.fps_cap = !Number.isFinite(s.fps_cap) || s.fps_cap <= 0 ? null : Math.min(Math.trunc(s.fps_cap), 240);
  }
  if (s.naming_template.trim() === "") s.naming_template = DEFAULT_NAMING_TEMPLATE;
  if (s.cut_mode === "lossless") s.framing = "original";
  if (s.max_duration_sec !== null) {
    s.max_duration_sec = Number.isFinite(s.max_duration_sec) && s.max_duration_sec > 0 ? s.max_duration_sec : null;
  }
  if (s.output_dir !== null && s.output_dir.trim() === "") s.output_dir = null;
  return s;
}

/** `ExportSettings::output_signature`: equal signatures mean equal output. */
export function outputSignature(s: ExportSettings): string {
  const p = presetOf(s);
  let sig = `p=${p};dir=${s.output_dir ?? ""};tpl=${s.naming_template};sp=${s.spaces_to_underscores}`;
  if (p === "lossless") {
    sig += `;aac=${s.lossless_audio_to_aac};snap=${s.snap_in_to_keyframe}`;
  } else if (p === "accurate") {
    sig += `;fps=${s.fps_cap};enc=${s.encoder}`;
  } else {
    sig += `;res=${s.output_resolution};fps=${s.fps_cap};enc=${s.encoder};a=${s.audio_track};mix=${s.mix_all_audio}`;
  }
  return sig;
}
