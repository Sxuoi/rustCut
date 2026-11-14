import { afterEach, describe, expect, it, vi } from "vitest";
import type { BatchInfo, ExportProgress, PreflightReport } from "$lib/bindings";
import { backend } from "$lib/ipc";
import { ExporterState } from "$lib/state/exporter.svelte";
import { defaultSettings } from "./testClips";

const batch = (id = "b1"): BatchInfo => ({ batch_id: id, clip_ids: ["c1", "c2"], total_duration_sec: 60, output_dir: "D:/out" });

const progress = (p: Partial<ExportProgress> = {}): ExportProgress => ({
  batch_id: "b1",
  overall_fraction: 0.5,
  eta_sec: 20,
  done: 1,
  failed: 0,
  total: 2,
  running: [{ clip_id: "c2", fraction: 0.25, out_time_sec: 5, speed: 2 }],
  ...p,
});

const report = (p: Partial<PreflightReport> = {}): PreflightReport => ({
  items: [],
  can_start: true,
  outputs: [],
  output_dir: "D:/out",
  extension: "mp4",
  preset: "lossless",
  estimate_bytes: 1000,
  free_bytes: null,
  estimate_seconds: 4,
  offer_aac_audio: false,
  excluded_clip_ids: [],
  ...p,
});

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("EXP-09 batch events", () => {
  it("tracks running, paused and finished batches", () => {
    const ex = new ExporterState(() => 0);
    ex.handleBatch({ state: "running", batch: batch() });
    expect([ex.phase, ex.running, ex.batch?.batch_id]).toEqual(["running", true, "b1"]);
    ex.handleBatch({ state: "paused", batch_id: "b1", reason: "disk_full", message: "No space" });
    expect([ex.phase, ex.running, ex.pause?.reason]).toEqual(["paused", true, "disk_full"]);
    ex.handleBatch({ state: "running", batch: batch() });
    expect([ex.phase, ex.pause]).toEqual(["running", null]);
    ex.handleProgress(progress());
    ex.handleBatch({
      state: "finished",
      summary: { batch_id: "b1", done: ["c1"], failed: ["c2"], skipped: [], cancelled: [], output_dir: "D:/out", elapsed_sec: 12 },
    });
    expect([ex.phase, ex.running, ex.progress, ex.etaSec]).toEqual(["finished", false, null, null]);
    ex.dismissSummary();
    expect([ex.phase, ex.summary]).toEqual(["idle", null]);
  });

  it("looks up per-clip progress for queue rows", () => {
    const ex = new ExporterState(() => 0);
    ex.handleProgress(progress());
    expect(ex.clipProgress("c2")?.fraction).toBe(0.25);
    expect(ex.clipProgress("c1")).toBeUndefined();
  });

  it("smooths the backend ETA", () => {
    const ex = new ExporterState(() => 0);
    ex.handleProgress(progress({ eta_sec: 20 }));
    expect(ex.etaSec).toBe(20);
    ex.handleProgress(progress({ eta_sec: 10 }));
    expect(ex.etaSec).toBeCloseTo(17, 6);
  });

  it("extrapolates the ETA from elapsed time when the backend sends none", () => {
    let now = 1_000;
    const ex = new ExporterState(() => now);
    ex.handleBatch({ state: "running", batch: batch() });
    now = 11_000;
    ex.handleProgress(progress({ eta_sec: null, overall_fraction: 0.5 }));
    expect(ex.etaSec).toBe(10);
  });

  it("resets the ETA for a new batch and ignores late events of a finished one", () => {
    const ex = new ExporterState(() => 0);
    ex.handleProgress(progress({ eta_sec: 50 }));
    ex.handleProgress(progress({ batch_id: "b2", eta_sec: 5 }));
    expect(ex.etaSec).toBe(5);
    ex.handleBatch({
      state: "finished",
      summary: { batch_id: "b2", done: [], failed: [], skipped: [], cancelled: [], output_dir: "D:/out", elapsed_sec: 1 },
    });
    ex.handleProgress(progress({ batch_id: "b2" }));
    expect(ex.progress).toBeNull();
    ex.handleBatch({ state: "running", batch: batch("b2") });
    expect(ex.phase).toBe("finished");
  });
});

describe("EXP-01 start / EXP-10 retry", () => {
  it("start marks the batch running", async () => {
    vi.spyOn(backend, "startExport").mockResolvedValue(batch());
    const ex = new ExporterState(() => 0);
    const b = await ex.start(["c1", "c2"], defaultSettings());
    expect(b?.batch_id).toBe("b1");
    expect([ex.phase, ex.starting]).toEqual(["running", false]);
  });

  it("a batch that finished before start() returned stays finished", async () => {
    const d = deferred<BatchInfo>();
    vi.spyOn(backend, "startExport").mockReturnValue(d.promise);
    const ex = new ExporterState(() => 0);
    const pending = ex.start(["c1"], defaultSettings());
    expect(ex.starting).toBe(true);
    ex.handleBatch({
      state: "finished",
      summary: { batch_id: "b1", done: ["c1"], failed: [], skipped: [], cancelled: [], output_dir: "D:/out", elapsed_sec: 1 },
    });
    d.resolve(batch());
    await pending;
    expect(ex.phase).toBe("finished");
  });

  it("refuses a second start while one is in flight", async () => {
    const d = deferred<BatchInfo>();
    const spy = vi.spyOn(backend, "startExport").mockReturnValue(d.promise);
    const ex = new ExporterState(() => 0);
    const first = ex.start(["c1"], defaultSettings());
    expect(await ex.start(["c1"], defaultSettings())).toBeNull();
    d.resolve(batch());
    await first;
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("a failed start returns null and keeps the exporter idle", async () => {
    vi.spyOn(backend, "startExport").mockRejectedValue({ code: "busy", message: "Busy", detail: null });
    const ex = new ExporterState(() => 0);
    expect(await ex.start(["c1"], defaultSettings())).toBeNull();
    expect(ex.phase).toBe("idle");
  });

  it("row retry is refused while a batch runs", async () => {
    const spy = vi.spyOn(backend, "startExport").mockResolvedValue(batch("b9"));
    const ex = new ExporterState(() => 0);
    ex.handleBatch({ state: "running", batch: batch() });
    expect(await ex.retryClip("c1", defaultSettings())).toBeNull();
    expect(spy).not.toHaveBeenCalled();
  });

  it("Retry failed starts the returned batch", async () => {
    vi.spyOn(backend, "retryFailed").mockResolvedValue(batch("b3"));
    const ex = new ExporterState(() => 0);
    ex.handleBatch({
      state: "finished",
      summary: { batch_id: "b1", done: [], failed: ["c2"], skipped: [], cancelled: [], output_dir: "D:/out", elapsed_sec: 1 },
    });
    await ex.retryFailed();
    expect([ex.phase, ex.batch?.batch_id, ex.summary]).toEqual(["running", "b3", null]);
  });
});

describe("EXP-11 pre-flight requests", () => {
  it("drops out-of-order replies", async () => {
    const slow = deferred<PreflightReport>();
    const fast = deferred<PreflightReport>();
    vi.spyOn(backend, "previewExport").mockReturnValueOnce(slow.promise).mockReturnValueOnce(fast.promise);
    const ex = new ExporterState(() => 0);
    const p1 = ex.preview(["c1"], defaultSettings());
    const p2 = ex.preview(["c1", "c2"], defaultSettings());
    fast.resolve(report({ estimate_bytes: 2 }));
    await p2;
    slow.resolve(report({ estimate_bytes: 1 }));
    await p1;
    expect(ex.report?.estimate_bytes).toBe(2);
    expect(ex.reportLoading).toBe(false);
  });

  it("puts errors in reportError and clears the stale report", async () => {
    vi.spyOn(backend, "previewExport")
      .mockResolvedValueOnce(report())
      .mockRejectedValueOnce({ code: "invalid_input", message: "Source missing", detail: null });
    const ex = new ExporterState(() => 0);
    await ex.preview(["c1"], defaultSettings());
    expect(ex.report).not.toBeNull();
    await ex.preview(["c1"], defaultSettings());
    expect([ex.report, ex.reportError]).toEqual([null, "Source missing"]);
  });

  it("the panel estimate is silent and empty without clips", async () => {
    const spy = vi.spyOn(backend, "previewExport").mockRejectedValue(new Error("boom"));
    const ex = new ExporterState(() => 0);
    await ex.refreshEstimate([], defaultSettings());
    expect(spy).not.toHaveBeenCalled();
    expect(ex.estimate).toBeNull();
    await ex.refreshEstimate(["c1"], defaultSettings());
    expect(ex.estimate).toBeNull();
    spy.mockResolvedValue(report({ estimate_bytes: 90 * 1024 * 1024 }));
    await ex.refreshEstimate(["c1"], defaultSettings());
    expect(ex.estimate?.estimate_bytes).toBe(90 * 1024 * 1024);
  });
});

describe("EXP-12/13 environment", () => {
  it("loads encoders once and falls back to the WebView CPU count", async () => {
    const hw = vi
      .spyOn(backend, "detectHwEncoders")
      .mockResolvedValue([{ name: "h264_amf", label: "AMD AMF H.264", available: true, error: null }]);
    vi.spyOn(backend, "getAppInfo").mockRejectedValue(new Error("no app info"));
    const ex = new ExporterState(() => 0);
    await ex.loadEncoders();
    await ex.loadEncoders();
    expect(hw).toHaveBeenCalledTimes(1);
    expect(ex.hwEncoders?.[0].name).toBe("h264_amf");
    await ex.loadEncoders(true);
    expect(hw).toHaveBeenLastCalledWith(true);
    await ex.loadCpuCount();
    expect(ex.cpuCount).toBeGreaterThanOrEqual(0);
  });
});
