import { describe, expect, it } from "vitest";
import type { LastExport } from "$lib/bindings";
import {
  clickMode,
  clipDuration,
  clipFlags,
  countIncluded,
  failureInfo,
  failureReasonText,
  includeChanges,
  includedClips,
  nextSelection,
  rangeIds,
  revealablePath,
  rowStatus,
  wantsInclude,
} from "./rows";
import { makeClip } from "./testClips";

const lastExport = (p: Partial<LastExport> = {}): LastExport => ({
  output_path: "D:/out/01_Clip.mp4",
  exported_at: "2026-11-13T10:00:00Z",
  settings_hash: "h",
  error: null,
  preset: "lossless",
  content_hash: "c",
  exit_code: null,
  log_tail: [],
  log_path: null,
  ...p,
});

describe("QUE-01 durations and EXP-01 include counts", () => {
  it("duration is Out − In and never negative", () => {
    expect(clipDuration({ start_sec: 920.12, end_sec: 955.8 })).toBeCloseTo(35.68, 6);
    expect(clipDuration({ start_sec: 5, end_sec: 3 })).toBe(0);
  });

  it("counts and lists included clips in order", () => {
    const clips = [
      makeClip({ index: 1 }),
      makeClip({ index: 2, selected_for_export: false }),
      makeClip({ index: 3 }),
    ];
    expect(countIncluded(clips)).toBe(2);
    expect(includedClips(clips).map((c) => c.index)).toEqual([1, 3]);
    expect(countIncluded([])).toBe(0);
  });
});

describe("QUE-05 rowStatus", () => {
  it("maps every export status to a badge", () => {
    expect(rowStatus(makeClip({ export_status: "pending" })).kind).toBe("pending");
    expect(rowStatus(makeClip({ export_status: "exporting" }))).toEqual({ kind: "exporting", fraction: null });
    expect(rowStatus(makeClip({ export_status: "completed" })).kind).toBe("done");
    expect(rowStatus(makeClip({ export_status: "failed" })).kind).toBe("failed");
    expect(rowStatus(makeClip({ export_status: "outdated" })).kind).toBe("outdated");
  });

  it("shows Exporting with % from running progress, clamped", () => {
    const c = makeClip({ export_status: "pending" });
    const p = { clip_id: c.id, fraction: 0.42, out_time_sec: 4.2, speed: 2 };
    expect(rowStatus(c, p)).toEqual({ kind: "exporting", fraction: 0.42 });
    expect(rowStatus(c, { ...p, fraction: 1.7 }).fraction).toBe(1);
    expect(rowStatus(c, { ...p, fraction: Number.NaN }).fraction).toBe(0);
  });
});

describe("EXP-10 failureInfo and QUE-05 revealablePath", () => {
  it("returns reason, exit code and log tail of failed clips only", () => {
    const failed = makeClip({
      export_status: "failed",
      last_export: lastExport({ error: "No space left", exit_code: 1, log_tail: ["a", "b"], log_path: "L" }),
    });
    expect(failureInfo(failed)).toEqual({ reason: "No space left", exitCode: 1, logTail: ["a", "b"], logPath: "L" });
    expect(failureInfo(makeClip({ export_status: "failed" }))).toEqual({
      reason: null,
      exitCode: null,
      logTail: [],
      logPath: null,
    });
    expect(failureInfo(makeClip({ export_status: "completed", last_export: lastExport() }))).toBeNull();
  });

  it("offers Show in folder only for Done clips with an output path", () => {
    expect(revealablePath(makeClip({ export_status: "completed", last_export: lastExport() }))).toBe(
      "D:/out/01_Clip.mp4",
    );
    expect(revealablePath(makeClip({ export_status: "outdated", last_export: lastExport() }))).toBeNull();
    expect(
      revealablePath(makeClip({ export_status: "completed", last_export: lastExport({ output_path: null }) })),
    ).toBeNull();
  });
});

describe("queue flags", () => {
  const ctx = { sourceDurationSec: 100, maxDurationSec: null, preset: "lossless" as const };

  it("flags duplicate ranges at ms precision, naming the other clip", () => {
    const clips = [
      makeClip({ id: "a", index: 1, start_sec: 10, end_sec: 20 }),
      makeClip({ id: "b", index: 2, start_sec: 10.0004, end_sec: 19.9996 }),
      makeClip({ id: "c", index: 5, start_sec: 10, end_sec: 20 }),
      makeClip({ id: "d", index: 3, start_sec: 10, end_sec: 21 }),
    ];
    const f = clipFlags(clips, ctx);
    expect(f.get("a")).toEqual([{ kind: "duplicate", of: 2 }]);
    expect(f.get("b")).toEqual([{ kind: "duplicate", of: 1 }]);
    expect(f.get("c")).toEqual([{ kind: "duplicate", of: 1 }]);
    expect(f.has("d")).toBe(false);
  });

  it("flags In at or after the source end as out of range", () => {
    const clips = [
      makeClip({ id: "in", index: 1, start_sec: 99.999, end_sec: 100 }),
      makeClip({ id: "at", index: 2, start_sec: 100, end_sec: 101 }),
      makeClip({ id: "after", index: 3, start_sec: 150, end_sec: 160 }),
    ];
    const f = clipFlags(clips, ctx);
    expect(f.has("in")).toBe(false);
    expect(f.get("at")).toEqual([{ kind: "out_of_range" }]);
    expect(f.get("after")).toEqual([{ kind: "out_of_range" }]);
    expect(clipFlags(clips, { ...ctx, sourceDurationSec: null }).size).toBe(0);
  });

  it("FRM-08 flags clips longer than a vertical preset's maximum", () => {
    const clips = [
      makeClip({ id: "ok", index: 1, start_sec: 0, end_sec: 180 }),
      makeClip({ id: "long", index: 2, start_sec: 0, end_sec: 180.001 }),
    ];
    const vertical = clipFlags(clips, { sourceDurationSec: 1000, maxDurationSec: 180, preset: "vcrop" });
    expect(vertical.has("ok")).toBe(false);
    expect(vertical.get("long")).toEqual([{ kind: "too_long", maxSec: 180 }]);
    expect(clipFlags(clips, { sourceDurationSec: 1000, maxDurationSec: 180, preset: "vblur" }).has("long")).toBe(true);
    expect(clipFlags(clips, { sourceDurationSec: 1000, maxDurationSec: 180, preset: "lossless" }).size).toBe(0);
    expect(clipFlags(clips, { sourceDurationSec: 1000, maxDurationSec: null, preset: "vcrop" }).size).toBe(0);
  });
});

describe("QUE-04 bulk include", () => {
  const clips = [
    makeClip({ id: "p", index: 1, export_status: "pending", selected_for_export: false }),
    makeClip({ id: "d", index: 2, export_status: "completed", selected_for_export: true }),
    makeClip({ id: "f", index: 3, export_status: "failed", selected_for_export: false }),
    makeClip({ id: "o", index: 4, export_status: "outdated", selected_for_export: true }),
  ];

  it("select all / none touch only clips that change", () => {
    expect(includeChanges(clips, "all")).toEqual({ include: ["p", "f"], exclude: [] });
    expect(includeChanges(clips, "none")).toEqual({ include: [], exclude: ["d", "o"] });
  });

  it("not exported includes pending, failed and outdated clips and excludes Done ones", () => {
    expect(includeChanges(clips, "not_exported")).toEqual({ include: ["p", "f"], exclude: ["d"] });
    expect(wantsInclude(makeClip({ export_status: "outdated" }), "not_exported")).toBe(true);
  });

  it("failed includes exactly the failed clips", () => {
    expect(includeChanges(clips, "failed")).toEqual({ include: ["f"], exclude: ["d", "o"] });
  });
});

describe("QUE-07 selection by click", () => {
  const ordered = ["c3", "c1", "c4", "c2"]; // a sorted view: never by clip number

  it("maps modifiers to modes", () => {
    expect(clickMode({ shiftKey: false, ctrlKey: false, metaKey: false })).toBe("replace");
    expect(clickMode({ shiftKey: false, ctrlKey: true, metaKey: false })).toBe("toggle");
    expect(clickMode({ shiftKey: false, ctrlKey: false, metaKey: true })).toBe("toggle");
    expect(clickMode({ shiftKey: true, ctrlKey: true, metaKey: false })).toBe("range");
  });

  it("ranges follow the visible order in both directions", () => {
    expect(rangeIds(ordered, "c1", "c2")).toEqual(["c1", "c4", "c2"]);
    expect(rangeIds(ordered, "c2", "c3")).toEqual(["c3", "c1", "c4", "c2"]);
    expect(rangeIds(ordered, null, "c4")).toEqual(["c4"]);
    expect(rangeIds(ordered, "hidden", "c4")).toEqual(["c4"]);
    expect(rangeIds(ordered, "c1", "missing")).toEqual([]);
  });

  it("computes the next selection", () => {
    const cur = new Set(["c1", "c4"]);
    expect([...nextSelection(cur, "replace", ordered, "c1", "c2")]).toEqual(["c2"]);
    expect([...nextSelection(cur, "toggle", ordered, "c1", "c4")]).toEqual(["c1"]);
    expect([...nextSelection(cur, "toggle", ordered, "c1", "c3")]).toEqual(["c1", "c4", "c3"]);
    expect([...nextSelection(cur, "range", ordered, "c3", "c4")]).toEqual(["c3", "c1", "c4"]);
    expect([...cur]).toEqual(["c1", "c4"]);
  });
});

describe("AC-09 failureReasonText", () => {
  it("shows the recorded reason, a fallback when none, and null for non-failed clips", () => {
    const failed = makeClip({ export_status: "failed", last_export: lastExport({ error: " Disk full ", exit_code: 1 }) });
    expect(failureReasonText(failed, "No reason")).toBe("Disk full");
    expect(failureReasonText(makeClip({ export_status: "failed" }), "No reason")).toBe("No reason");
    expect(failureReasonText(makeClip({ export_status: "failed", last_export: lastExport({ error: "  " }) }), "No reason")).toBe("No reason");
    expect(failureReasonText(makeClip({ export_status: "pending" }), "No reason")).toBeNull();
  });
});
