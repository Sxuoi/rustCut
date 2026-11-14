// Export batch state (EXP-01, EXP-09, EXP-10), the Export dialog's pre-flight
// (EXP-11), the export panel's estimate and the encoder/CPU facts behind the
// EXP-12/13 defaults.
// OWNER: fe-queue. Public API is shared; extend, don't break.

import type {
  BatchInfo,
  BatchState,
  BatchSummary,
  ClipFinished,
  ClipProgress,
  ExportProgress,
  ExportSettings,
  HwEncoderInfo,
  PauseReason,
  PreflightReport,
} from "$lib/bindings";
import { backend, BackendError } from "$lib/ipc";
import { estimateRemaining, smoothEta } from "$lib/queue/progress";
import { ui } from "./ui.svelte";

export type ExportPhase = "idle" | "running" | "paused" | "finished";

function toastError(e: unknown): void {
  const err = BackendError.from(e);
  ui.toast({ level: "error", message: err.message, code: err.code });
}

export class ExporterState {
  phase = $state<ExportPhase>("idle");
  batch = $state.raw<BatchInfo | null>(null);
  progress = $state.raw<ExportProgress | null>(null);
  pause = $state.raw<{ reason: PauseReason; message: string } | null>(null);
  summary = $state.raw<BatchSummary | null>(null);
  /** Last pre-flight report for the dialog. */
  report = $state.raw<PreflightReport | null>(null);
  reportLoading = $state(false);
  /** Why the last dialog pre-flight failed (shown in the dialog, not as a toast). */
  reportError = $state<string | null>(null);
  /** Silent pre-flight of the included clips for the export panel's estimate. */
  estimate = $state.raw<PreflightReport | null>(null);
  /** Smoothed seconds remaining for the running batch (EXP-09); `null` = unknown. */
  etaSec = $state<number | null>(null);
  /** Detected encoders (EXP-13); `null` until loaded. */
  hwEncoders = $state.raw<HwEncoderInfo[] | null>(null);
  hwLoading = $state(false);
  /** Logical CPUs for the EXP-12 defaults; 0 = unknown. */
  cpuCount = $state(0);
  /** A start/retry request is in flight (blocks double starts). */
  starting = $state(false);

  readonly running = $derived(this.phase === "running" || this.phase === "paused");

  /** Running clips by id, for O(1) row lookups. */
  readonly progressById = $derived<Map<string, ClipProgress>>(
    new Map((this.progress?.running ?? []).map((c) => [c.clip_id, c])),
  );

  private reportSeq = 0;
  private estimateSeq = 0;
  private trackedBatchId: string | null = null;
  private startedAt: number | null = null;
  private cpuRequested = false;

  /** `now` is injectable for tests (milliseconds). */
  constructor(private readonly now: () => number = () => Date.now()) {}

  /** Per-clip running progress (0..1), for queue rows. */
  clipProgress(clipId: string): ClipProgress | undefined {
    return this.progressById.get(clipId);
  }

  /** Remember when a batch started (ETA fallback) and reset smoothing for a new batch. */
  private track(batchId: string): void {
    if (this.trackedBatchId === batchId) return;
    this.trackedBatchId = batchId;
    this.startedAt = this.now();
    this.etaSec = null;
  }

  /** True once the summary of `batchId` is known (late events and replies are ignored). */
  private finishedBatch(batchId: string): boolean {
    return this.summary !== null && this.summary.batch_id === batchId;
  }

  private markRunning(b: BatchInfo): void {
    this.track(b.batch_id);
    this.batch = b;
    if (this.finishedBatch(b.batch_id)) return;
    this.phase = "running";
    this.pause = null;
    this.summary = null;
  }

  handleBatch(state: BatchState): void {
    switch (state.state) {
      case "running":
        this.markRunning(state.batch);
        break;
      case "paused":
        this.phase = "paused";
        this.pause = { reason: state.reason, message: state.message };
        this.etaSec = null;
        break;
      case "finished":
        this.phase = "finished";
        this.summary = state.summary;
        this.progress = null;
        this.pause = null;
        this.etaSec = null;
        break;
    }
  }

  handleProgress(p: ExportProgress): void {
    if (this.finishedBatch(p.batch_id)) return;
    this.track(p.batch_id);
    this.progress = p;
    const elapsed = this.startedAt === null ? 0 : (this.now() - this.startedAt) / 1000;
    this.etaSec = smoothEta(this.etaSec, p.eta_sec ?? estimateRemaining(p.overall_fraction, elapsed));
  }

  handleClipFinished(_f: ClipFinished): void {
    // Statuses (and failure reasons) arrive as project patches (Clip.last_export).
  }

  /** Dialog pre-flight (EXP-11). Out-of-order replies are dropped; errors land in `reportError`. */
  async preview(clipIds: string[], settings: ExportSettings): Promise<PreflightReport | null> {
    const seq = ++this.reportSeq;
    this.reportLoading = true;
    try {
      const r = await backend.previewExport(clipIds, settings);
      if (seq === this.reportSeq) {
        this.report = r;
        this.reportError = null;
      }
      return r;
    } catch (e) {
      if (seq === this.reportSeq) {
        this.report = null;
        this.reportError = BackendError.from(e).message;
      }
      return null;
    } finally {
      if (seq === this.reportSeq) this.reportLoading = false;
    }
  }

  /** Export panel estimate ("est. ~90 MB ~4 s"); silent on errors. */
  async refreshEstimate(clipIds: string[], settings: ExportSettings): Promise<void> {
    const seq = ++this.estimateSeq;
    if (clipIds.length === 0) {
      this.estimate = null;
      return;
    }
    try {
      const r = await backend.previewExport(clipIds, settings);
      if (seq === this.estimateSeq) this.estimate = r;
    } catch {
      if (seq === this.estimateSeq) this.estimate = null;
    }
  }

  async start(clipIds: string[], settings: ExportSettings): Promise<BatchInfo | null> {
    if (this.starting) return null;
    this.starting = true;
    try {
      const b = await backend.startExport(clipIds, settings);
      this.markRunning(b);
      return b;
    } catch (e) {
      toastError(e);
      return null;
    } finally {
      this.starting = false;
    }
  }

  /** `null` = Cancel all (EXP-09). */
  async cancel(clipId: string | null): Promise<void> {
    await backend.cancelExport(clipId).catch(toastError);
  }

  async resume(): Promise<void> {
    await backend.resumeExport().catch(toastError);
  }

  /** EXP-10 *Retry failed*: re-queues only the failures of the last batch. */
  async retryFailed(): Promise<void> {
    if (this.starting) return;
    this.starting = true;
    try {
      const b = await backend.retryFailed();
      if (b) this.markRunning(b);
    } catch (e) {
      toastError(e);
    } finally {
      this.starting = false;
    }
  }

  /** Retry one failed clip from its row: a one-clip batch with the current settings. */
  async retryClip(clipId: string, settings: ExportSettings): Promise<BatchInfo | null> {
    if (this.running) return null;
    return this.start([clipId], settings);
  }

  dismissSummary(): void {
    if (this.phase === "finished") this.phase = "idle";
    this.summary = null;
  }

  /** QUE-05 *Show in folder*. */
  async reveal(path: string): Promise<void> {
    await backend.revealPath(path).catch(toastError);
  }

  /** Summary *Open folder*. */
  async openOutputFolder(): Promise<void> {
    await backend.openOutputFolder().catch(toastError);
  }

  /** EXP-13 encoder list (cached by the backend; `force` re-runs the test encodes). */
  async loadEncoders(force = false): Promise<void> {
    if (this.hwLoading || (this.hwEncoders !== null && !force)) return;
    this.hwLoading = true;
    try {
      this.hwEncoders = await backend.detectHwEncoders(force);
    } catch (e) {
      this.hwEncoders = this.hwEncoders ?? [];
      if (force) toastError(e);
    } finally {
      this.hwLoading = false;
    }
  }

  /** CPU count for the EXP-12 defaults (AppInfo, else the WebView's estimate). */
  async loadCpuCount(): Promise<void> {
    if (this.cpuRequested) return;
    this.cpuRequested = true;
    try {
      this.cpuCount = (await backend.getAppInfo()).cpu_count;
    } catch {
      this.cpuCount = typeof navigator === "undefined" ? 0 : (navigator.hardwareConcurrency ?? 0);
    }
  }
}

export const exporter = new ExporterState();
