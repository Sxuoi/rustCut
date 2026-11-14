// The UI's only door to the Rust core (PRD §7.2, §7.9). Two implementations:
//   * TauriBackend (ipc/tauri.ts): invoke() + one Channel<AppEvent>.
//   * MockBackend  (ipc/mock.ts):  in-memory emulation for browser-only dev
//                                   and in-browser UI tests.
// Command names/argument names are the contract in docs/ARCHITECTURE.md §IPC.

import type {
  AddClipRequest,
  AddClipResult,
  AppEvent,
  AppInfo,
  AppSettings,
  BatchInfo,
  CommandError,
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

export type EventHandler = (event: AppEvent) => void;

export interface Backend {
  readonly kind: "tauri" | "mock";

  /** Register the single ordered event stream. Call once at startup. */
  subscribe(handler: EventHandler): Promise<void>;

  getAppInfo(): Promise<AppInfo>;
  getSettings(): Promise<AppSettings>;
  setSettings(settings: AppSettings): Promise<AppSettings>;
  listRecentProjects(): Promise<RecentProject[]>;

  /** File passed on the command line at first launch (open-with / CLI). */
  getLaunchPath(): Promise<string | null>;

  /** Native open dialog (videos + .rustcut). `null` when cancelled. */
  pickMediaFile(): Promise<string | null>;
  pickOutputDir(current: string | null): Promise<string | null>;

  /** Open a video or a `.rustcut` file (PRJ-03, PRJ-04). */
  openPath(path: string): Promise<OpenResult>;
  relinkSource(path: string, acceptMismatch: boolean): Promise<RelinkResult>;
  closeProject(): Promise<void>;
  getSnapshot(): Promise<ProjectSnapshot | null>;

  addClip(req: AddClipRequest): Promise<AddClipResult>;
  editClips(req: EditClipsRequest): Promise<StatePatch>;
  editClipsBatch(batch: EditClipsRequest[]): Promise<StatePatch>;
  deleteClips(ids: string[]): Promise<StatePatch>;
  undo(): Promise<StatePatch | null>;
  redo(): Promise<StatePatch | null>;
  renumberClips(): Promise<StatePatch>;
  setTags(tags: Tag[]): Promise<StatePatch>;
  setExportSettings(settings: ExportSettings): Promise<StatePatch>;
  setViewState(view: ViewState): Promise<void>;
  saveNow(): Promise<SaveState>;

  /** FRM-04: the same Rust function the export uses. */
  cropWindow(displayWidth: number, displayHeight: number, offset: number): Promise<CropWindow>;

  previewExport(clipIds: string[], settings: ExportSettings): Promise<PreflightReport>;
  startExport(clipIds: string[], settings: ExportSettings): Promise<BatchInfo>;
  /** `null` = cancel all. */
  cancelExport(clipId: string | null): Promise<void>;
  resumeExport(): Promise<void>;
  retryFailed(): Promise<BatchInfo | null>;

  requestPlayback(tier: PlaybackTier): Promise<PlaybackInfo>;
  startWaveform(track: number): Promise<void>;
  getMediaIndex(): Promise<MediaIndexInfo | null>;

  /** Show a file this project exported in the OS file manager. */
  revealPath(path: string): Promise<void>;
  openOutputFolder(): Promise<void>;
  clearCache(): Promise<number>;
  detectHwEncoders(force: boolean): Promise<HwEncoderInfo[]>;
  /** FRM-08: platform presets (the user's platform-presets.json, else the bundled list). */
  getPlatformPresets(): Promise<PlatformPreset[]>;

  exportTimestamps(format: TimestampFormat): Promise<TimestampExport>;
  /** Native save dialog + write; returns the chosen path or `null`. */
  saveTextFile(defaultName: string, text: string): Promise<string | null>;
  parseTimestamps(text: string): Promise<ImportPreview>;
  importTimestamps(items: ImportedClip[]): Promise<StatePatch>;

  /** URL the <video> element can load for a local media path. */
  mediaUrl(path: string): string;
}

/** Error thrown by every Backend method (mirrors Rust `CommandError`). */
export class BackendError extends Error {
  readonly code: CommandError["code"];
  readonly detail: string | null;

  constructor(err: CommandError) {
    super(err.message);
    this.name = "BackendError";
    this.code = err.code;
    this.detail = err.detail;
  }

  static from(e: unknown): BackendError {
    if (e instanceof BackendError) return e;
    if (e && typeof e === "object" && "code" in e && "message" in e) {
      return new BackendError(e as CommandError);
    }
    const message = e instanceof Error ? e.message : String(e);
    return new BackendError({ code: "internal", message, detail: null });
  }
}
