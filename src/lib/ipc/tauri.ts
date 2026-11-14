// Tauri implementation of Backend. Every command is
// `#[tauri::command(rename_all = "snake_case")]`, so argument keys are snake_case.

import { Channel, convertFileSrc, invoke } from "@tauri-apps/api/core";
import type {
  AddClipRequest,
  AddClipResult,
  AppEvent,
  AppInfo,
  AppSettings,
  BatchInfo,
  CropWindow,
  EditClipsRequest,
  ExportSettings,
  HwEncoderInfo,
  ImportedClip,
  ImportPreview,
  MediaIndexInfo,
  OpenResult,
  PlaybackInfo,
  PlaybackTier,
  PlatformPreset,
  PreflightReport,
  ProjectSnapshot,
  RecentProject,
  RelinkResult,
  SaveState,
  StatePatch,
  Tag,
  TimestampExport,
  TimestampFormat,
  ViewState,
} from "$lib/bindings";
import { BackendError, type Backend, type EventHandler } from "./backend";

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw BackendError.from(e);
  }
}

export class TauriBackend implements Backend {
  readonly kind = "tauri" as const;
  private channel: Channel<AppEvent> | null = null;

  async subscribe(handler: EventHandler): Promise<void> {
    const ch = new Channel<AppEvent>();
    ch.onmessage = handler;
    this.channel = ch;
    await call<void>("subscribe_events", { on_event: ch });
  }

  getAppInfo = () => call<AppInfo>("get_app_info");
  getSettings = () => call<AppSettings>("get_settings");
  setSettings = (settings: AppSettings) => call<AppSettings>("set_settings", { settings });
  listRecentProjects = () => call<RecentProject[]>("list_recent_projects");

  getLaunchPath = () => call<string | null>("get_launch_path");

  pickMediaFile = () => call<string | null>("pick_media_file");
  pickOutputDir = (current: string | null) => call<string | null>("pick_output_dir", { current });

  openPath = (path: string) => call<OpenResult>("open_path", { path });
  relinkSource = (path: string, acceptMismatch: boolean) =>
    call<RelinkResult>("relink_source", { path, accept_mismatch: acceptMismatch });
  closeProject = () => call<void>("close_project");
  getSnapshot = () => call<ProjectSnapshot | null>("get_snapshot");

  addClip = (req: AddClipRequest) => call<AddClipResult>("add_clip", { req });
  editClips = (req: EditClipsRequest) => call<StatePatch>("edit_clips", { req });
  editClipsBatch = (batch: EditClipsRequest[]) => call<StatePatch>("edit_clips_batch", { batch });
  deleteClips = (ids: string[]) => call<StatePatch>("delete_clips", { ids });
  undo = () => call<StatePatch | null>("undo");
  redo = () => call<StatePatch | null>("redo");
  renumberClips = () => call<StatePatch>("renumber_clips");
  setTags = (tags: Tag[]) => call<StatePatch>("set_tags", { tags });
  setExportSettings = (settings: ExportSettings) => call<StatePatch>("set_export_settings", { settings });
  setViewState = (view: ViewState) => call<void>("set_view_state", { view });
  saveNow = () => call<SaveState>("save_now");

  cropWindow = (displayWidth: number, displayHeight: number, offset: number) =>
    call<CropWindow>("crop_window", { display_width: displayWidth, display_height: displayHeight, offset });

  previewExport = (clipIds: string[], settings: ExportSettings) =>
    call<PreflightReport>("preview_export", { clip_ids: clipIds, settings });
  startExport = (clipIds: string[], settings: ExportSettings) =>
    call<BatchInfo>("start_export", { clip_ids: clipIds, settings });
  cancelExport = (clipId: string | null) => call<void>("cancel_export", { clip_id: clipId });
  resumeExport = () => call<void>("resume_export");
  retryFailed = () => call<BatchInfo | null>("retry_failed");

  requestPlayback = (tier: PlaybackTier) => call<PlaybackInfo>("request_playback", { tier });
  startWaveform = (track: number) => call<void>("start_waveform", { track });
  getMediaIndex = () => call<MediaIndexInfo | null>("get_media_index");
  startMediaIndex = () => call<void>("start_media_index");

  revealPath = (path: string) => call<void>("reveal_path", { path });
  openOutputFolder = () => call<void>("open_output_folder");
  clearCache = () => call<number>("clear_cache");
  detectHwEncoders = (force: boolean) => call<HwEncoderInfo[]>("detect_hw_encoders", { force });

  getPlatformPresets = () => call<PlatformPreset[]>("get_platform_presets");

  exportTimestamps = (format: TimestampFormat) => call<TimestampExport>("export_timestamps", { format });
  saveTextFile = (defaultName: string, text: string) =>
    call<string | null>("save_text_file", { default_name: defaultName, text });
  parseTimestamps = (text: string) => call<ImportPreview>("parse_timestamps", { text });
  importTimestamps = (items: ImportedClip[]) => call<StatePatch>("import_timestamps", { items });

  mediaUrl(path: string): string {
    return convertFileSrc(path);
  }
}
