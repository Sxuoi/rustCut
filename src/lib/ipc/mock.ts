// Browser-only emulation of the Rust core (`npm run dev` without Tauri, and
// in-browser UI tests). OWNER: fe-mock. Implementation lives in ./mock/*:
//
//   store.ts      port of rustcut-project's ProjectStore (validation, stable
//                 indices, undo/redo, patches)
//   exporter.ts   timer-driven export engine simulation (EXP-08..EXP-10)
//   preflight.ts  names, checks and estimates (EXP-05..EXP-11)
//   naming.ts     TS port of rustcut-naming
//   timestamps.ts timestamp export/import (EXP-16/17)
//   media.ts      synthetic waveform and media index
//   sources.ts    the sample source and fake sources
//
// Behaviour switches (by path / clip name) so every UI state can be reached:
//   open "/sample/sample.mp4"   the generated 3-minute sample
//   any other path              a fake 2:30:00 1080p60 livestream
//   path contains "missing"     source missing, project read-only
//   path contains "readonly"    project open elsewhere, read-only
//   relink path contains "mismatch"  fingerprint mismatch prompt (PRJ-04)
//   clip name contains "fail"      that clip's export fails (EXP-10)
//   clip name contains "diskfull"  the batch pauses until resume_export
//   output folder contains "nospace"  pre-flight free-space error
//
// Every state change produces exactly one StatePatch with revision + 1; it is
// returned from the command AND emitted as {type:"patch"} (UI dedupes by
// revision). Changes are "saved" to localStorage ~300 ms later.

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
  Notice,
  NoticeLevel,
  OpenResult,
  PlaybackInfo,
  PlaybackTier,
  PlatformPreset,
  PreflightReport,
  Project,
  ProjectSnapshot,
  ReadOnlyReason,
  RecentProject,
  RelinkResult,
  SaveState,
  SourceInfo,
  SourceStatus,
  StatePatch,
  Tag,
  TimestampExport,
  TimestampFormat,
  ViewState,
} from "$lib/bindings";
import { cropWindow as tsCropWindow } from "$lib/util/crop";
import type { Backend, EventHandler } from "./backend";
import { browserDownload, type MockOptions } from "./mock/options";
import { DEFAULT_EXPORT_TIMING, MockExporter } from "./mock/exporter";
import { mediaIndexFor, totalPeaks, waveformChunk } from "./mock/media";
import { APP_VERSION, SCHEMA_VERSION, defaultAppSettings, normalizeAppSettings } from "./mock/model";
import { DEFAULT_FREE_BYTES, planExport } from "./mock/preflight";
import {
  FAKE_OUTPUT_DIR,
  SAMPLE_PATH,
  defaultOutputDir,
  newProject,
  playbackInfo,
  projectPathFor,
  sourceFor,
} from "./mock/sources";
import { MockProjectStore, normalizeOnLoad, outOfRangeIds, readOnlyMessage } from "./mock/store";
import { exportTimestamps, parseTimestampText } from "./mock/timestamps";
import {
  clone,
  defaultStorage,
  fail,
  hash32,
  baseName,
  readJson,
  writeJson,
  type KeyValueStore,
} from "./mock/util";

export type { MockOptions } from "./mock/options";

const SETTINGS_KEY = "rustcut.mock.settings";
const RECENT_KEY = "rustcut.mock.recent";
const projectKey = (sourcePath: string) => `rustcut.mock.project:${sourcePath}`;
const RECENT_LIMIT = 10;

interface StoredProject {
  project: Project;
  saved_at: string;
}

/** Mirrors crates/rustcut-export/data/platform-presets.json (FRM-08). */
export const MOCK_PLATFORM_PRESETS: readonly PlatformPreset[] = [
  { id: "youtube_shorts", label: "YouTube Shorts", framing: "vertical_crop", output_resolution: "1080x1920", max_duration_sec: 180, fps_cap: 60 },
  { id: "tiktok", label: "TikTok", framing: "vertical_crop", output_resolution: "1080x1920", max_duration_sec: 600, fps_cap: 60 },
  { id: "instagram_reels", label: "Instagram Reels", framing: "vertical_crop", output_resolution: "1080x1920", max_duration_sec: 180, fps_cap: 60 },
];

const HW_ENCODERS: HwEncoderInfo[] = [
  { name: "h264_amf", label: "AMD AMF", available: true, error: null },
  { name: "h264_nvenc", label: "NVIDIA NVENC", available: false, error: "No NVENC-capable GPU was found (mock)." },
  { name: "h264_qsv", label: "Intel Quick Sync", available: false, error: "No Quick Sync device was found (mock)." },
];

export class MockBackend implements Backend {
  readonly kind = "mock" as const;

  private handler: EventHandler | null = null;
  private readonly storage: KeyValueStore;
  private readonly saveDelayMs: number;
  private readonly waveformMs: number;
  private readonly clock: () => Date;
  private readonly download: (name: string, text: string) => void;
  private readonly freeBytesOpt: number | null;
  private readonly exporter: MockExporter;

  private settings: AppSettings | null = null;
  private store: MockProjectStore | null = null;
  private sourceStatus: SourceStatus = { kind: "ok" };
  private save: SaveState = emptySave();
  private saveTimer: ReturnType<typeof setTimeout> | null = null;
  private waveformTimers = new Set<ReturnType<typeof setInterval>>();
  private currentKey: string | null = null;

  constructor(options: MockOptions = {}) {
    this.storage = options.storage ?? defaultStorage();
    this.saveDelayMs = options.saveDelayMs ?? 300;
    this.waveformMs = options.waveformMs ?? 2000;
    this.clock = options.now ?? (() => new Date());
    this.download = options.download ?? browserDownload;
    this.freeBytesOpt = options.freeBytes === undefined ? DEFAULT_FREE_BYTES : options.freeBytes;
    this.exporter = new MockExporter(
      {
        store: () => this.store,
        sourceStatus: () => this.sourceStatus,
        emit: (e) => this.emit(e),
        commitPatch: (p) => this.publish(p),
        now: () => this.clock(),
        freeBytes: (dir) => this.freeBytes(dir),
      },
      { ...DEFAULT_EXPORT_TIMING, ...options.exportTiming },
    );
  }

  // ---------------------------------------------------------------- events

  async subscribe(handler: EventHandler): Promise<void> {
    this.handler = handler;
  }

  /** Deliver in order, asynchronously (like the Tauri channel). */
  private emit(event: AppEvent): void {
    queueMicrotask(() => this.handler?.(event));
  }

  private toast(level: NoticeLevel, code: string, message: string): void {
    this.emit({ type: "toast", notice: { level, code, message } });
  }

  /** A change happened: emit the patch (a copy) and schedule the save. */
  private publish(patch: StatePatch): StatePatch {
    this.emit({ type: "patch", patch: clone(patch) });
    this.markDirty();
    return patch;
  }

  /** Run a store command; publish its patch only when it changed something. */
  private change(fn: (s: MockProjectStore) => StatePatch): StatePatch {
    const s = this.requireStore();
    const before = s.revision;
    const patch = fn(s);
    return s.revision === before ? patch : this.publish(patch);
  }

  private requireStore(): MockProjectStore {
    if (!this.store) throw fail("no_project", "No video is open. Open a video first.");
    return this.store;
  }

  // ------------------------------------------------------------ persistence

  private markDirty(): void {
    if (!this.store || this.store.readOnly) return;
    this.save = { ...this.save, pending: true };
    if (this.saveTimer !== null) clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => {
      this.saveTimer = null;
      this.persist();
    }, this.saveDelayMs);
  }

  /** Write the project to localStorage and announce it. */
  private persist(): SaveState {
    if (this.saveTimer !== null) {
      clearTimeout(this.saveTimer);
      this.saveTimer = null;
    }
    const s = this.store;
    if (!s || s.readOnly || !this.currentKey) return this.save;
    const savedAt = this.clock().toISOString();
    const ok = writeJson(this.storage, this.currentKey, { project: s.project, saved_at: savedAt } satisfies StoredProject);
    this.save = ok
      ? { ...this.save, saved_at: savedAt, pending: false, error: null }
      : {
          ...this.save,
          pending: true,
          error: "Could not save: the browser's storage is full or blocked. Your changes are only kept until you close this tab.",
        };
    if (ok) this.touchRecent(s.project);
    this.emit({ type: "saved", save: clone(this.save) });
    return this.save;
  }

  private loadSettings(): AppSettings {
    if (!this.settings) {
      const stored = readJson<AppSettings>(this.storage, SETTINGS_KEY);
      this.settings = normalizeAppSettings(stored ?? defaultAppSettings());
    }
    return this.settings;
  }

  private recent(): RecentProject[] {
    return readJson<RecentProject[]>(this.storage, RECENT_KEY) ?? [];
  }

  private touchRecent(project: Project): void {
    const entry: RecentProject = {
      project_path: this.save.project_path ?? projectPathFor(project.source.path),
      source_path: project.source.path,
      source_name: baseName(project.source.path),
      opened_at: this.clock().toISOString(),
      clip_count: project.clips.length,
      location: this.save.location,
    };
    const rest = this.recent().filter((r) => r.project_path !== entry.project_path);
    writeJson(this.storage, RECENT_KEY, [entry, ...rest].slice(0, RECENT_LIMIT));
  }

  private freeBytes(dir: string): number | null {
    return /nospace/i.test(dir) ? 50_000 : this.freeBytesOpt;
  }

  // ------------------------------------------------------------------ app

  /** Browser mode has no command line: nothing to open at launch. */
  async getLaunchPath(): Promise<string | null> {
    return null;
  }

  async getAppInfo(): Promise<AppInfo> {
    const nav = (typeof navigator === "undefined" ? undefined : navigator) as
      | (Navigator & { userAgentData?: { platform?: string } })
      | undefined;
    const raw = (nav?.userAgentData?.platform ?? nav?.platform ?? "").toLowerCase();
    return {
      app_version: APP_VERSION,
      schema_version: SCHEMA_VERSION,
      ffmpeg_version: "9.0.2-full_build (mock)",
      ffmpeg_found: true,
      ffprobe_found: true,
      data_dir: "C:/Users/Demo/AppData/Roaming/RustCut",
      log_dir: "C:/Users/Demo/AppData/Roaming/RustCut/logs",
      cache_dir: "C:/Users/Demo/AppData/Local/RustCut/cache",
      platform: raw.includes("mac") ? "macos" : raw.includes("win") ? "windows" : raw === "" ? "windows" : "linux",
      cpu_count: nav?.hardwareConcurrency || 8,
      hw_encoders: clone(HW_ENCODERS),
    };
  }

  async getSettings(): Promise<AppSettings> {
    return clone(this.loadSettings());
  }

  async setSettings(settings: AppSettings): Promise<AppSettings> {
    this.settings = normalizeAppSettings(settings);
    writeJson(this.storage, SETTINGS_KEY, this.settings);
    return clone(this.settings);
  }

  async listRecentProjects(): Promise<RecentProject[]> {
    return this.recent();
  }

  async pickMediaFile(): Promise<string | null> {
    return SAMPLE_PATH;
  }

  async pickOutputDir(_current: string | null): Promise<string | null> {
    return FAKE_OUTPUT_DIR;
  }

  // ---------------------------------------------------------------- project

  async openPath(path: string): Promise<OpenResult> {
    let sourcePath = path;
    if (path.toLowerCase().endsWith(".rustcut")) {
      const hit = this.recent().find((r) => r.project_path === path);
      if (!hit) {
        throw fail("not_found", `The project file "${baseName(path)}" was not found. Open the video it belongs to instead.`);
      }
      sourcePath = hit.source_path;
    }
    await this.closeProject();

    const settings = this.loadSettings();
    const key = projectKey(sourcePath);
    const stored = readJson<StoredProject>(this.storage, key);
    const notices: Notice[] = [];
    const missing = /missing/i.test(sourcePath);
    const locked = /readonly/i.test(sourcePath);

    let project: Project;
    if (stored?.project) {
      project = stored.project;
      normalizeOnLoad(project);
      notices.push({
        level: "info",
        code: "restored",
        message: `Restored your project: ${project.clips.length} clip${project.clips.length === 1 ? "" : "s"} from your last session.`,
      });
    } else {
      project = newProject(sourceFor(sourcePath), settings.default_spaces_to_underscores);
    }

    let readOnly: ReadOnlyReason | null = null;
    if (missing) {
      readOnly = { kind: "source_missing" };
      this.sourceStatus = { kind: "missing", suggested_path: null };
      notices.push({
        level: "warning",
        code: "source_missing",
        message: `The source video was not found at ${sourcePath}. Use Locate… to relink it; the queue is read-only until then.`,
      });
    } else {
      this.sourceStatus = { kind: "ok" };
    }
    if (locked && !readOnly) {
      readOnly = { kind: "open_elsewhere", holder: "RustCut (pid 4242) on DEMO-PC" };
      notices.push({ level: "warning", code: "read_only_locked", message: readOnlyMessage(readOnly) });
    }
    if (project.source.video.hdr) {
      notices.push({
        level: "info",
        code: "hdr_source",
        message: "This video is HDR. Previews may look washed out; re-encoded exports convert to SDR.",
      });
    }

    const location = settings.project_location;
    this.currentKey = key;
    this.save = {
      saved_at: stored?.saved_at ?? null,
      pending: false,
      error: null,
      project_path:
        location === "app_library"
          ? `C:/Users/Demo/AppData/Roaming/RustCut/library/${hash32(sourcePath)}.rustcut`
          : projectPathFor(sourcePath),
      location,
    };
    this.store = new MockProjectStore(project, { minClipSec: () => this.loadSettings().min_clip_duration_sec }, readOnly, 1);
    const stray = outOfRangeIds(project).length;
    if (stray > 0) {
      notices.push({
        level: "warning",
        code: "out_of_range_clips",
        message: `${stray} clip${stray === 1 ? " starts" : "s start"} after the end of this video and will be skipped on export.`,
      });
    }
    if (!readOnly) {
      this.persistQuiet();
    }
    return {
      snapshot: this.store.snapshot(this.save, this.sourceStatus),
      playback: missing ? null : playbackInfo(project.source, "direct"),
      notices,
    };
  }

  /** Persist without announcing (used at open so the project shows up in "recent"). */
  private persistQuiet(): void {
    const s = this.store;
    if (!s || !this.currentKey) return;
    const savedAt = this.save.saved_at ?? this.clock().toISOString();
    if (writeJson(this.storage, this.currentKey, { project: s.project, saved_at: savedAt } satisfies StoredProject)) {
      this.touchRecent(s.project);
    }
  }

  async relinkSource(path: string, acceptMismatch: boolean): Promise<RelinkResult> {
    const s = this.requireStore();
    const found: SourceInfo = sourceFor(path);
    if (/mismatch/i.test(path) && !acceptMismatch) {
      const foundDuration = 120;
      return {
        kind: "mismatch",
        expected: clone(s.project.source.fingerprint),
        found: { size_bytes: 1_234_567, xxh3_head_tail: hash32(path) + hash32(`${path}!`) },
        found_duration_sec: foundDuration,
        clips_out_of_range: s.project.clips.filter((c) => c.start_sec >= foundDuration).length,
      };
    }
    s.project.source = /mismatch/i.test(path) ? { ...found, duration_sec: 120 } : found;
    this.sourceStatus = { kind: "ok" };
    if (s.readOnly?.kind === "source_missing") s.readOnly = null;
    s.bumpForSnapshot();
    this.markDirty();
    const notices: Notice[] = [];
    const stray = outOfRangeIds(s.project).length;
    if (stray > 0) {
      notices.push({
        level: "warning",
        code: "out_of_range_clips",
        message: `${stray} clip${stray === 1 ? " starts" : "s start"} after the end of the relinked video and will be skipped on export.`,
      });
    }
    const snapshot = s.snapshot(this.save, this.sourceStatus);
    this.emit({ type: "snapshot", snapshot: clone(snapshot) });
    return { kind: "relinked", result: { snapshot, playback: playbackInfo(s.project.source, "direct"), notices } };
  }

  async closeProject(): Promise<void> {
    this.exporter.dispose();
    for (const t of this.waveformTimers) clearInterval(t);
    this.waveformTimers.clear();
    if (this.store && this.save.pending) this.persist();
    if (this.saveTimer !== null) clearTimeout(this.saveTimer);
    this.saveTimer = null;
    this.store = null;
    this.currentKey = null;
    this.sourceStatus = { kind: "ok" };
    this.save = emptySave();
  }

  async getSnapshot(): Promise<ProjectSnapshot | null> {
    return this.store ? this.store.snapshot(this.save, this.sourceStatus) : null;
  }

  // ------------------------------------------------------------------ queue

  async addClip(req: AddClipRequest): Promise<AddClipResult> {
    const s = this.requireStore();
    const result = s.addClip(req);
    this.publish(result.patch);
    return result;
  }

  async editClips(req: EditClipsRequest): Promise<StatePatch> {
    return this.change((s) => s.editClips(req.ids, req.edit));
  }

  async editClipsBatch(batch: EditClipsRequest[]): Promise<StatePatch> {
    return this.change((s) => s.editClipsBatch(batch));
  }

  async deleteClips(ids: string[]): Promise<StatePatch> {
    return this.change((s) => s.deleteClips(ids));
  }

  async undo(): Promise<StatePatch | null> {
    const s = this.requireStore();
    const patch = s.undo();
    return patch ? this.publish(patch) : null;
  }

  async redo(): Promise<StatePatch | null> {
    const s = this.requireStore();
    const patch = s.redo();
    return patch ? this.publish(patch) : null;
  }

  async renumberClips(): Promise<StatePatch> {
    return this.change((s) => s.renumberClips());
  }

  async setTags(tags: Tag[]): Promise<StatePatch> {
    return this.change((s) => s.setTags(tags));
  }

  async setExportSettings(settings: ExportSettings): Promise<StatePatch> {
    return this.change((s) => s.setExportSettings(settings));
  }

  async setViewState(view: ViewState): Promise<void> {
    if (this.store?.setViewState(view)) this.markDirty();
  }

  async saveNow(): Promise<SaveState> {
    this.requireStore();
    return clone(this.persist());
  }

  async cropWindow(displayWidth: number, displayHeight: number, offset: number): Promise<CropWindow> {
    return tsCropWindow(displayWidth, displayHeight, offset);
  }

  // ----------------------------------------------------------------- export

  private planEnv() {
    const s = this.requireStore();
    return {
      project: s.project,
      sourceStatus: this.sourceStatus,
      exists: (p: string) =>
        this.exporter.exists(p) || s.project.clips.some((c) => c.last_export?.output_path?.toLowerCase() === p.toLowerCase()),
      now: this.clock(),
      freeBytes: (dir: string) => this.freeBytes(dir),
    };
  }

  async previewExport(clipIds: string[], settings: ExportSettings): Promise<PreflightReport> {
    return planExport(this.planEnv(), clipIds, settings).report;
  }

  async startExport(clipIds: string[], settings: ExportSettings): Promise<BatchInfo> {
    return this.exporter.start(clipIds, settings);
  }

  async cancelExport(clipId: string | null): Promise<void> {
    this.exporter.cancel(clipId);
  }

  async resumeExport(): Promise<void> {
    this.exporter.resume();
  }

  async retryFailed(): Promise<BatchInfo | null> {
    return this.exporter.retryFailed();
  }

  // ------------------------------------------------------------------ media

  async requestPlayback(tier: PlaybackTier): Promise<PlaybackInfo> {
    const s = this.requireStore();
    if (this.sourceStatus.kind === "missing") {
      throw fail("source_missing", "The source video is missing, so it can't be played. Use Locate… to relink it.");
    }
    return playbackInfo(s.project.source, tier);
  }

  async startMediaIndex(): Promise<void> {
    this.requireStore(); // the mock's index is complete at once (getMediaIndex)
  }

  async startWaveform(track: number): Promise<void> {
    const s = this.requireStore();
    const count = s.project.source.audio_tracks.length;
    if (!Number.isInteger(track) || track < 1 || track > count) {
      throw fail(
        "invalid_input",
        count === 0
          ? "This video has no audio track, so there is no waveform."
          : `Audio track ${track} does not exist; this video has ${count} track${count === 1 ? "" : "s"}.`,
      );
    }
    const total = totalPeaks(s.project.source.duration_sec);
    const chunks = 10;
    const size = Math.max(1, Math.ceil(total / chunks));
    let sent = 0;
    const timer = setInterval(() => {
      const chunk = waveformChunk(track, sent, size, total);
      sent += size;
      this.emit({ type: "waveform", chunk });
      if (chunk.done) {
        clearInterval(timer);
        this.waveformTimers.delete(timer);
      }
    }, Math.max(1, Math.round(this.waveformMs / chunks)));
    this.waveformTimers.add(timer);
  }

  async getMediaIndex(): Promise<MediaIndexInfo | null> {
    if (!this.store) return null;
    const info = mediaIndexFor(this.store.project.source);
    this.emit({ type: "media_index", info: clone(info) });
    return info;
  }

  // ------------------------------------------------------------- shell stuff

  async revealPath(path: string): Promise<void> {
    const s = this.requireStore();
    const key = path.toLowerCase();
    const known =
      this.exporter.exists(path) ||
      s.project.clips.some((c) => c.last_export?.output_path?.toLowerCase() === key) ||
      key === this.outputDir(s).toLowerCase();
    if (!known) {
      throw fail("invalid_input", "RustCut can only show files this project exported, or its output folder.");
    }
    this.toast("info", "reveal_path", `Would show ${path} in your file manager (mock backend).`);
  }

  async openOutputFolder(): Promise<void> {
    const dir = this.outputDir(this.requireStore());
    this.toast("info", "open_output_folder", `Would open ${dir} in your file manager (mock backend).`);
  }

  private outputDir(s: MockProjectStore): string {
    return s.project.export_settings.output_dir ?? defaultOutputDir(s.project.source);
  }

  async clearCache(): Promise<number> {
    return 0;
  }

  async detectHwEncoders(_force: boolean): Promise<HwEncoderInfo[]> {
    return clone(HW_ENCODERS);
  }

  async getPlatformPresets(): Promise<PlatformPreset[]> {
    return clone([...MOCK_PLATFORM_PRESETS]);
  }

  // ------------------------------------------------------------- timestamps

  async exportTimestamps(format: TimestampFormat): Promise<TimestampExport> {
    return exportTimestamps(this.requireStore().project, format);
  }

  async saveTextFile(defaultName: string, text: string): Promise<string | null> {
    this.download(defaultName, text);
    return defaultName;
  }

  async parseTimestamps(text: string): Promise<ImportPreview> {
    const s = this.requireStore();
    return parseTimestampText(text, s.project.source.duration_sec, this.loadSettings().min_clip_duration_sec);
  }

  async importTimestamps(items: ImportedClip[]): Promise<StatePatch> {
    return this.change((s) => s.importClips(items).patch);
  }

  mediaUrl(path: string): string {
    return path.startsWith("/sample/") ? path : SAMPLE_PATH;
  }
}

function emptySave(): SaveState {
  return { saved_at: null, pending: false, error: null, project_path: null, location: "next_to_video" };
}

