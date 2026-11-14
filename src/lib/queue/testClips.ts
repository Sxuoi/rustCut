// Test fixtures for the queue helpers (imported by *.test.ts only).

import type { Clip, ExportSettings, PlannedOutput } from "$lib/bindings";

let seq = 0;

/** A clip with sensible defaults; `index` also names the id unless given. */
export function makeClip(p: Partial<Clip> = {}): Clip {
  const index = p.index ?? ++seq;
  return {
    id: p.id ?? `clip-${index}`,
    index,
    name: `Clip ${index}`,
    start_sec: 0,
    end_sec: 10,
    tag_id: null,
    crop_x_offset: 0,
    selected_for_export: true,
    export_status: "pending",
    last_export: null,
    ...p,
  };
}

/** The Rust `ExportSettings::default()`. */
export function defaultSettings(p: Partial<ExportSettings> = {}): ExportSettings {
  return {
    output_dir: null,
    naming_template: "{index}_{name}",
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
    ...p,
  };
}

export function makeOutput(p: Partial<PlannedOutput> & { index: number }): PlannedOutput {
  return {
    clip_id: `clip-${p.index}`,
    file_name: `${String(p.index).padStart(2, "0")}_Clip.mp4`,
    output_path: `D:/out/${String(p.index).padStart(2, "0")}_Clip.mp4`,
    action: "write",
    lead_in_sec: null,
    ...p,
  };
}
