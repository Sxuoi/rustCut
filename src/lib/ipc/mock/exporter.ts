// Timer-driven simulation of the export engine (PRD §4.4 EXP-08..EXP-10).
//
// Event order for a batch (all via `host.emit`, i.e. microtask-ordered):
//   export_batch{running} -> patch{exporting} -> export_progress ticks
//   -> per clip: patch{completed|failed|restored} then export_clip_finished
//   -> export_batch{finished}.
//
// Mock behaviours: a clip whose name contains "fail" fails half way with a
// fake 20-line FFmpeg tail; "diskfull" pauses the whole batch (disk_full)
// until resume_export; cancel works per clip or for all; retry_failed re-runs
// the failures (renaming the clip first makes it succeed).

import type {
  AppEvent,
  BatchInfo,
  Clip,
  ClipOutcome,
  ClipProgress,
  ExportProgress,
  ExportRecord,
  ExportSettings,
  PauseReason,
  PlannedOutput,
  Preset,
  SourceStatus,
  StatePatch,
} from "$lib/bindings";
import { outputSignature } from "./model";
import { AUDIO_BITRATE_BPS, planExport, videoMaxrateBps } from "./preflight";
import { contentHash, type MockProjectStore } from "./store";
import { fail, hash32, newId } from "./util";

/** Pacing of the simulation. Tests pass tiny values; the defaults finish a demo in seconds. */
export interface ExportTiming {
  /** Simulated wall time per second of clip footage. */
  msPerClipSec: number;
  minClipMs: number;
  maxClipMs: number;
  tickMs: number;
}

export const DEFAULT_EXPORT_TIMING: ExportTiming = { msPerClipSec: 30, minClipMs: 400, maxClipMs: 2500, tickMs: 100 };

/** What the exporter needs from the backend that owns it. */
export interface ExporterHost {
  store(): MockProjectStore | null;
  sourceStatus(): SourceStatus;
  /** Queue an event for the subscriber (ordered). */
  emit(event: AppEvent): void;
  /** A background store change: emit the patch and schedule a save. */
  commitPatch(patch: StatePatch): void;
  now(): Date;
  freeBytes(dir: string): number | null;
}

type JobState = "queued" | "running" | "done" | "failed" | "cancelled";

interface Job {
  clip: Clip;
  output: PlannedOutput;
  durSec: number;
  totalMs: number;
  elapsedMs: number;
  state: JobState;
  /** Fraction at which this job fails (name contains "fail"). */
  failAt: number | null;
  /** Fraction at which the batch pauses (name contains "diskfull"); cleared after it fired. */
  pauseAt: number | null;
  /** Hashes of the clip as it was when the job started (for Outdated detection). */
  contentHash: string;
  settingsHash: string;
}

interface Batch {
  info: BatchInfo;
  settings: ExportSettings;
  preset: Preset;
  jobs: Job[];
  skipped: string[];
  paused: { reason: PauseReason; message: string; job: Job } | null;
  timer: ReturnType<typeof setInterval> | null;
  elapsedMs: number;
  source: { bitRate: number; height: number };
}

const FAIL_WORD = /fail/i;
const DISK_WORD = /diskfull/i;

/** The simulated export engine. */
export class MockExporter {
  /** Simulated output folder contents (lower-case paths). */
  readonly files = new Set<string>();
  private batch: Batch | null = null;
  private last: { failed: string[]; settings: ExportSettings } | null = null;

  constructor(
    private readonly host: ExporterHost,
    private readonly timing: ExportTiming = DEFAULT_EXPORT_TIMING,
  ) {}

  /** Does `path` exist in the simulated output folders? */
  exists(path: string): boolean {
    return this.files.has(path.toLowerCase());
  }

  get running(): boolean {
    return this.batch !== null;
  }

  /** EXP-01: start a batch; `busy` while one runs or is paused, `invalid_input` on pre-flight errors. */
  start(clipIds: string[], settings: ExportSettings): BatchInfo {
    const store = this.host.store();
    if (!store) throw fail("no_project", "Open a video before exporting.");
    if (this.batch) {
      throw fail("busy", "An export is already running. Wait for it to finish or cancel it, then start again.");
    }
    const plan = planExport(
      {
        project: store.project,
        sourceStatus: this.host.sourceStatus(),
        exists: (p) => this.pathExists(store, p),
        now: this.host.now(),
        freeBytes: (d) => this.host.freeBytes(d),
      },
      clipIds,
      settings,
    );
    const firstError = plan.report.items.find((i) => i.level === "error");
    if (firstError) throw fail("invalid_input", firstError.message);

    const outputs = new Map(plan.report.outputs.map((o) => [o.clip_id, o]));
    const jobs: Job[] = plan.runnable.map((clip) => {
      const durSec = clip.end_sec - clip.start_sec;
      const totalMs = Math.min(
        this.timing.maxClipMs,
        Math.max(this.timing.minClipMs, Math.round(durSec * this.timing.msPerClipSec)),
      );
      const ch = contentHash(clip, plan.preset);
      return {
        clip: structuredClone(clip),
        output: outputs.get(clip.id) as PlannedOutput,
        durSec,
        totalMs,
        elapsedMs: 0,
        state: "queued",
        failAt: FAIL_WORD.test(clip.name) ? 0.5 : null,
        pauseAt: DISK_WORD.test(clip.name) ? 0.3 : null,
        contentHash: ch,
        settingsHash: hash32(`${ch}#${outputSignature(plan.settings)}`),
      };
    });
    const info: BatchInfo = {
      batch_id: newId(),
      clip_ids: jobs.map((j) => j.clip.id),
      total_duration_sec: jobs.reduce((s, j) => s + j.durSec, 0),
      output_dir: plan.report.output_dir,
    };
    const batch: Batch = {
      info,
      settings: plan.settings,
      preset: plan.preset,
      jobs,
      skipped: plan.report.excluded_clip_ids,
      paused: null,
      timer: null,
      elapsedMs: 0,
      source: { bitRate: store.project.source.bit_rate ?? 8_000_000, height: store.project.source.video.display_height },
    };
    this.batch = batch;

    this.host.emit({ type: "export_batch", state: { state: "running", batch: info } });
    if (jobs.length > 0) this.host.commitPatch(store.markExporting(info.clip_ids));
    for (const id of batch.skipped) {
      const reason = plan.policySkipped.includes(id)
        ? "A file with this name already exists and the conflict policy is Skip."
        : "The clip starts after the end of the source.";
      this.host.emit({ type: "export_clip_finished", finished: { batch_id: info.batch_id, clip_id: id, outcome: { outcome: "skipped", reason } } });
    }
    batch.timer = setInterval(() => this.tick(batch), this.timing.tickMs);
    return info;
  }

  /** EXP-09: cancel one clip, or the whole batch with `null`. */
  cancel(clipId: string | null): void {
    const batch = this.batch;
    if (!batch) return;
    const targets = batch.jobs.filter((j) => (clipId === null || j.clip.id === clipId) && this.isOpen(j));
    for (const job of targets) {
      this.finishJob(batch, job, { outcome: "cancelled" });
      if (batch.paused?.job === job) {
        batch.paused = null;
        this.host.emit({ type: "export_batch", state: { state: "running", batch: batch.info } });
      }
    }
    if (targets.length > 0) this.emitProgress(batch);
    this.finishIfDone(batch);
  }

  /** EXP-10: continue after a pause (disk full / folder missing). */
  resume(): void {
    const batch = this.batch;
    if (!batch?.paused) return;
    batch.paused = null;
    this.host.emit({ type: "export_batch", state: { state: "running", batch: batch.info } });
  }

  /** EXP-10: re-queue only the clips that failed in the last batch. */
  retryFailed(): BatchInfo | null {
    if (this.batch) {
      throw fail("busy", "An export is already running. Wait for it to finish before retrying failed clips.");
    }
    const store = this.host.store();
    if (!store || !this.last) return null;
    const ids = this.last.failed.filter((id) => store.clip(id)?.export_status === "failed");
    if (ids.length === 0) return null;
    return this.start(ids, this.last.settings);
  }

  /** Stop timers (project closed / backend disposed). Clips return to their previous status. */
  dispose(): void {
    const batch = this.batch;
    if (!batch) return;
    this.cancel(null);
    if (this.batch) this.endBatch(batch);
  }

  // ------------------------------------------------------------- internals

  private pathExists(store: MockProjectStore, path: string): boolean {
    const key = path.toLowerCase();
    return this.files.has(key) || store.project.clips.some((c) => c.last_export?.output_path?.toLowerCase() === key);
  }

  private isOpen(j: Job): boolean {
    return j.state === "queued" || j.state === "running";
  }

  private tick(batch: Batch): void {
    if (this.batch !== batch) return;
    if (batch.paused) {
      this.emitProgress(batch);
      return;
    }
    const slots = batch.settings.parallel_jobs;
    for (const job of batch.jobs) {
      if (job.state === "queued" && batch.jobs.filter((j) => j.state === "running").length < slots) job.state = "running";
    }
    batch.elapsedMs += this.timing.tickMs;
    for (const job of batch.jobs.filter((j) => j.state === "running")) {
      job.elapsedMs = Math.min(job.totalMs, job.elapsedMs + this.timing.tickMs);
      const frac = job.elapsedMs / job.totalMs;
      if (job.pauseAt !== null && frac >= job.pauseAt) {
        job.pauseAt = null;
        const message = `The output drive is full while writing "${job.output.file_name}". Free up space on ${batch.info.output_dir}, then choose Resume.`;
        batch.paused = { reason: "disk_full", message, job };
        this.host.emit({ type: "export_batch", state: { state: "paused", batch_id: batch.info.batch_id, reason: "disk_full", message } });
        break;
      }
      if (job.failAt !== null && frac >= job.failAt) {
        this.finishJob(batch, job, this.failure(job));
      } else if (job.elapsedMs >= job.totalMs) {
        this.finishJob(batch, job, this.success(batch, job));
      }
    }
    this.emitProgress(batch);
    this.finishIfDone(batch);
  }

  private success(batch: Batch, job: Job): ClipOutcome {
    const bytes =
      batch.preset === "lossless"
        ? (batch.source.bitRate / 8) * (job.durSec + (job.output.lead_in_sec ?? 0))
        : ((videoMaxrateBps(batch.preset, batch.settings, batch.source.height) * 0.5 + AUDIO_BITRATE_BPS) * job.durSec) / 8;
    return { outcome: "done", output_path: job.output.output_path, size_bytes: Math.max(1, Math.round(bytes)) };
  }

  private failure(job: Job): ClipOutcome {
    const name = job.output.file_name;
    return {
      outcome: "failed",
      exit_code: 1,
      message: "Invalid data found when processing input (simulated failure: the clip name contains \"fail\").",
      log_tail: fakeFfmpegTail(name),
      log_path: `C:/Users/Demo/AppData/Roaming/RustCut/logs/${job.clip.id}.log`,
    };
  }

  private finishJob(batch: Batch, job: Job, outcome: ClipOutcome): void {
    job.state = outcome.outcome === "done" ? "done" : outcome.outcome === "failed" ? "failed" : "cancelled";
    if (outcome.outcome === "done") this.files.add(outcome.output_path.toLowerCase());
    const record: ExportRecord = {
      clip_id: job.clip.id,
      outcome,
      exported_at: this.host.now().toISOString(),
      preset: batch.preset,
      settings_hash: job.settingsHash,
      content_hash: job.contentHash,
    };
    const store = this.host.store();
    const patch = store?.applyExportRecord(record) ?? null;
    if (patch) this.host.commitPatch(patch);
    this.host.emit({ type: "export_clip_finished", finished: { batch_id: batch.info.batch_id, clip_id: job.clip.id, outcome } });
  }

  private emitProgress(batch: Batch): void {
    const total = batch.jobs.reduce((s, j) => s + j.durSec, 0);
    const frac = (j: Job) => (this.isOpen(j) ? j.elapsedMs / j.totalMs : 1);
    const overall = total > 0 ? batch.jobs.reduce((s, j) => s + j.durSec * frac(j), 0) / total : 1;
    const running: ClipProgress[] = batch.jobs
      .filter((j) => j.state === "running")
      .map((j) => ({
        clip_id: j.clip.id,
        fraction: j.elapsedMs / j.totalMs,
        out_time_sec: (j.elapsedMs / j.totalMs) * j.durSec,
        speed: batch.preset === "lossless" ? 40 : 2.4,
      }));
    const elapsedSec = batch.elapsedMs / 1000;
    const progress: ExportProgress = {
      batch_id: batch.info.batch_id,
      overall_fraction: Math.min(1, overall),
      eta_sec: overall > 0.02 && overall < 1 ? Math.round((elapsedSec * (1 - overall)) / overall) : null,
      done: batch.jobs.filter((j) => j.state === "done").length,
      failed: batch.jobs.filter((j) => j.state === "failed").length,
      total: batch.jobs.length,
      running,
    };
    this.host.emit({ type: "export_progress", progress });
  }

  private finishIfDone(batch: Batch): void {
    if (this.batch !== batch || batch.jobs.some((j) => this.isOpen(j))) return;
    const ids = (s: JobState) => batch.jobs.filter((j) => j.state === s).map((j) => j.clip.id);
    this.last = { failed: ids("failed"), settings: batch.settings };
    this.endBatch(batch);
    this.host.emit({
      type: "export_batch",
      state: {
        state: "finished",
        summary: {
          batch_id: batch.info.batch_id,
          done: ids("done"),
          failed: ids("failed"),
          skipped: batch.skipped,
          cancelled: ids("cancelled"),
          output_dir: batch.info.output_dir,
          elapsed_sec: batch.elapsedMs / 1000,
        },
      },
    });
  }

  private endBatch(batch: Batch): void {
    if (batch.timer !== null) clearInterval(batch.timer);
    batch.timer = null;
    if (this.batch === batch) this.batch = null;
  }
}

/** EXP-10: a believable last-20-lines FFmpeg log for a failed clip. */
export function fakeFfmpegTail(outputName: string): string[] {
  return [
    "ffmpeg version 9.0.2-full_build Copyright (c) 2000-2026 the FFmpeg developers",
    "  built with gcc 14.2.0 (Rev1, Built by MSYS2 project)",
    "Input #0, matroska,webm, from 'source.mkv':",
    "  Duration: 02:30:00.00, start: 0.000000, bitrate: 6000 kb/s",
    "  Stream #0:0: Video: h264 (High), yuv420p(progressive), 1920x1080, 60 fps",
    "  Stream #0:1(eng): Audio: aac (LC), 48000 Hz, stereo, fltp (default)",
    "Stream mapping:",
    "  Stream #0:0 -> #0:0 (copy)",
    "  Stream #0:1 -> #0:1 (copy)",
    `Output #0, mp4, to '${outputName}.part':`,
    "[matroska,webm @ 000001f2a3c4d500] Read error at pos. 1843021824 (0x6de1c000)",
    "[matroska,webm @ 000001f2a3c4d500] Invalid data found when processing input",
    "[mov,mp4,m4a,3gp,3g2,mj2 @ 000001f2a3c8e100] Packet corrupt (stream = 0, dts = 28341).",
    "av_interleaved_write_frame(): Invalid data found when processing input",
    "frame= 1530 fps=612 q=-1.0 size=   18432KiB time=00:00:25.50 bitrate=5920.3kbits/s speed=10.2x",
    "[out#0/mp4 @ 000001f2a3c8e100] Error writing trailer: Invalid data found when processing input",
    "Error closing file " + outputName + ".part: Invalid data found when processing input",
    "video:18102KiB audio:402KiB subtitle:0KiB other streams:0KiB global headers:0KiB muxing overhead: unknown",
    "Conversion failed!",
    "Exiting normally, received signal 2.",
  ].slice(-20);
}
