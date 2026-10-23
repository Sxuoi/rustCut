// Mirrors the `.rustcut` schema in rustcut-core (PRD 7.3). Keep in sync with
// crates/rustcut-core/src/project.rs and clip.rs.

export type ExportStatus = "pending" | "in_progress" | "completed" | "failed";

export type ExportMode = "lossless_copy" | "smart_reencode";

export type CropStyle = "center_crop" | "blurred_background";

export interface Clip {
  id: string;
  index: number;
  name: string;
  start_sec: number;
  end_sec: number;
  color: string;
  crop_x_offset: number;
  selected_for_export: boolean;
  export_status: ExportStatus;
}

export interface SourceVideo {
  path: string;
  duration_sec: number;
  resolution: string;
  fps: number;
}

export interface ExportSettings {
  output_dir: string;
  naming_pattern: string;
  mode: ExportMode;
  aspect_ratio_override: string | null;
  crop_style: CropStyle;
  crop_x_offset: number;
  parallel_jobs: number;
}

export interface ProjectState {
  project_version: string;
  source_video: SourceVideo;
  export_settings: ExportSettings;
  clips: Clip[];
}

/** Decoded ffprobe facts (rustcut-core `probe::MediaInfo`). */
export interface MediaInfo {
  duration_sec: number;
  width: number;
  height: number;
  fps: number;
  bitrate_bps: number;
  codec_name: string;
  format_name: string;
}

/** Payload of the `export://progress` Tauri event. */
export interface ExportProgressEvent {
  clip_id: string;
  index: number;
  fraction: number;
  phase: "running" | "done" | "failed" | "skipped";
  message: string | null;
}

/** Payload of the `export://finished` Tauri event. */
export interface ExportFinishedEvent {
  completed: string[];
  failed: [string, string][];
  skipped: [string, string][];
}
