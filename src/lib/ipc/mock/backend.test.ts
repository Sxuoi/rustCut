// MockBackend contract: open, patches + events, save/restore, export
// simulation, media, timestamps. Doubles as documentation of the Backend
// interface behaviour the UI can rely on.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppEvent, ExportSettings, StatePatch } from "$lib/bindings";
import { BackendError } from "../backend";
import { MockBackend } from "../mock";
import { MemoryStorage } from "./util";

const FAST = { msPerClipSec: 1, minClipMs: 200, maxClipMs: 200, tickMs: 50 };

function setup(opts: ConstructorParameters<typeof MockBackend>[0] = {}) {
  const storage = new MemoryStorage();
  const downloads: [string, string][] = [];
  const b = new MockBackend({
    storage,
    exportTiming: FAST,
    now: () => new Date(2026, 10, 13, 12, 0, 0),
    download: (n, t) => downloads.push([n, t]),
    ...opts,
  });
  const events: AppEvent[] = [];
  void b.subscribe((e) => events.push(e));
  return { b, storage, events, downloads };
}

describe("launch path", () => {
  it("the mock has no command line, so there is nothing to open at launch", async () => {
    expect(await setup().b.getLaunchPath()).toBeNull();
  });
});

async function code(p: Promise<unknown>): Promise<string> {
  try {
    await p;
  } catch (e) {
    if (e instanceof BackendError) return e.code;
    throw e;
  }
  return "no error";
}

const types = (events: AppEvent[]) => events.map((e) => e.type);

describe("edit_clips_batch (QUE-04)", () => {
  it("returns and emits one patch, and one undo restores include and exclude", async () => {
    const { b, events } = setup();
    await openSample(b);
    const [a, c] = await addClips(b, ["A", "B"]);
    await b.editClips({ ids: [c], edit: { kind: "set_included", included: false } });
    const before = (await b.getSnapshot())!;
    events.length = 0;
    const patch = await b.editClipsBatch([
      { ids: [a], edit: { kind: "set_included", included: false } },
      { ids: [c], edit: { kind: "set_included", included: true } },
    ]);
    expect(patch.revision).toBe(before.revision + 1);
    expect(events.filter((e) => e.type === "patch")).toEqual([{ type: "patch", patch }]);
    await b.undo();
    expect((await b.getSnapshot())!.project.clips).toEqual(before.project.clips);
    await b.redo();
    expect((await b.getSnapshot())!.project.clips.map((c) => c.selected_for_export)).toEqual([false, true]);
  });

  it("rejects the whole action when the second edit is invalid", async () => {
    const { b, events } = setup();
    await openSample(b);
    const [a] = await addClips(b, ["A"]);
    const before = await b.getSnapshot();
    events.length = 0;
    expect(await code(b.editClipsBatch([
      { ids: [a], edit: { kind: "set_included", included: false } },
      { ids: ["missing"], edit: { kind: "set_included", included: true } },
    ]))).toBe("not_found");
    expect(await b.getSnapshot()).toEqual(before);
    expect(events).toEqual([]);
  });
});

async function openSample(b: MockBackend) {
  return b.openPath("/sample/sample.mp4");
}

async function addClips(b: MockBackend, names: string[]) {
  const ids: string[] = [];
  let t = 10;
  for (const name of names) {
    const r = await b.addClip({ start_sec: t, end_sec: t + 20, name, tag_id: null });
    ids.push(r.clip.id);
    t += 30;
  }
  return ids;
}

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("open_path", () => {
  it("opens the sample: 180 s 1280x720 30/1 h264 + one AAC track 'Mix'", async () => {
    const { b } = setup();
    const r = await openSample(b);
    const src = r.snapshot.project.source;
    expect(src).toMatchObject({ path: "/sample/sample.mp4", duration_sec: 180 });
    expect(src.video).toMatchObject({ codec: "h264", display_width: 1280, display_height: 720, frame_rate: "30/1" });
    expect(src.audio_tracks).toHaveLength(1);
    expect(src.audio_tracks[0]).toMatchObject({ codec: "aac", title: "Mix" });
    expect(r.snapshot.project.tags.map((t) => t.id)).toEqual(["funny", "edu", "highlight"]);
    expect(r.snapshot.project.export_settings.naming_template).toBe("{index}_{name}");
    expect(r.snapshot.read_only).toBeNull();
    expect(r.snapshot.source_status).toEqual({ kind: "ok" });
    expect(r.playback).toMatchObject({
      tier: "direct",
      media_path: "/sample/sample.mp4",
      mime_type: 'video/mp4; codecs="avc1.64001f, mp4a.40.2"',
    });
    expect(r.notices).toEqual([]);
  });

  it("any other path is a 2:30:00 1080p60 livestream", async () => {
    const { b } = setup();
    const r = await b.openPath("D:/streams/livestream_2026_10_21.mkv");
    expect(r.snapshot.project.source).toMatchObject({ duration_sec: 9000 });
    expect(r.snapshot.project.source.video).toMatchObject({ display_width: 1920, display_height: 1080, frame_rate: "60/1" });
  });

  it("'missing' simulates a missing source (read-only, no playback); 'readonly' simulates open elsewhere", async () => {
    const { b } = setup();
    const m = await b.openPath("D:/gone/missing.mp4");
    expect(m.snapshot.source_status).toEqual({ kind: "missing", suggested_path: null });
    expect(m.snapshot.read_only).toEqual({ kind: "source_missing" });
    expect(m.playback).toBeNull();
    expect(m.notices.map((n) => n.code)).toContain("source_missing");
    expect(await code(b.addClip({ start_sec: 1, end_sec: 5, name: null, tag_id: null }))).toBe("read_only");
    expect(await code(b.requestPlayback("direct"))).toBe("source_missing");

    const r = await b.openPath("D:/shared/readonly.mp4");
    expect(r.snapshot.read_only).toMatchObject({ kind: "open_elsewhere" });
    expect(r.notices.map((n) => n.code)).toContain("read_only_locked");
    expect(r.playback).not.toBeNull();
  });

  it("restores the previous project (PRJ-03) with a notice, after the simulated save", async () => {
    const { b, storage } = setup();
    await openSample(b);
    const [id] = await addClips(b, ["Kept"]);
    await b.setViewState({ playhead_sec: 42, zoom_window_sec: 30, selected_clip_id: id as string });
    await vi.advanceTimersByTimeAsync(400);
    const again = new MockBackend({ storage });
    const r = await again.openPath("/sample/sample.mp4");
    expect(r.notices.map((n) => n.code)).toEqual(["restored"]);
    expect(r.snapshot.project.clips.map((c) => c.name)).toEqual(["Kept"]);
    expect(r.snapshot.project.view_state).toMatchObject({ playhead_sec: 42, zoom_window_sec: 30 });
    expect((await again.listRecentProjects())[0]).toMatchObject({ source_name: "sample.mp4", clip_count: 1 });
  });

  it("opens a .rustcut path from the recent list, rejects an unknown one", async () => {
    const { b } = setup();
    await openSample(b);
    const recent = (await b.listRecentProjects())[0];
    expect(recent?.project_path).toBe("/sample/sample.rustcut");
    const r = await b.openPath(recent?.project_path as string);
    expect(r.snapshot.project.source.path).toBe("/sample/sample.mp4");
    expect(await code(b.openPath("D:/nope.rustcut"))).toBe("not_found");
  });

  it("commands need a project; snapshot is null until one is open and after close", async () => {
    const { b } = setup();
    expect(await b.getSnapshot()).toBeNull();
    expect(await b.getMediaIndex()).toBeNull();
    expect(await code(b.addClip({ start_sec: 1, end_sec: 5, name: null, tag_id: null }))).toBe("no_project");
    await openSample(b);
    expect(await b.getSnapshot()).not.toBeNull();
    await b.closeProject();
    expect(await b.getSnapshot()).toBeNull();
  });

  it("relink: mismatch asks first, accepting relinks and emits a snapshot", async () => {
    const { b, events } = setup();
    await b.openPath("D:/gone/missing.mp4");
    const mm = await b.relinkSource("D:/new/mismatch.mp4", false);
    expect(mm.kind).toBe("mismatch");
    const ok = await b.relinkSource("/sample/sample.mp4", false);
    expect(ok.kind).toBe("relinked");
    expect((await b.getSnapshot())?.read_only).toBeNull();
    expect((await b.getSnapshot())?.source_status).toEqual({ kind: "ok" });
    expect(types(events)).toContain("snapshot");
  });
});

describe("patches and events", () => {
  it("returns each patch AND emits it, revision + 1, via microtask", async () => {
    const { b, events } = setup();
    const open = await openSample(b);
    const rev0 = open.snapshot.revision;
    const p = b.addClip({ start_sec: 5, end_sec: 15, name: "A", tag_id: null });
    expect(events).toHaveLength(0);
    const r = await p;
    expect(r.patch.revision).toBe(rev0 + 1);
    await Promise.resolve();
    const patches = events.filter((e): e is Extract<AppEvent, { type: "patch" }> => e.type === "patch");
    expect(patches.map((e) => e.patch.revision)).toEqual([rev0 + 1]);
    expect(patches[0]?.patch.upserted[0]?.id).toBe(r.clip.id);

    const e2 = await b.editClips({ ids: [r.clip.id], edit: { kind: "rename", name: "B" } });
    const u = (await b.undo()) as StatePatch;
    const rd = (await b.redo()) as StatePatch;
    expect([e2.revision, u.revision, rd.revision]).toEqual([rev0 + 2, rev0 + 3, rev0 + 4]);
    await Promise.resolve();
    expect(events.filter((e) => e.type === "patch").map((e) => (e as { patch: StatePatch }).patch.revision)).toEqual([
      rev0 + 1,
      rev0 + 2,
      rev0 + 3,
      rev0 + 4,
    ]);
  });

  it("a no-op returns the current revision and emits nothing", async () => {
    const { b, events } = setup();
    await openSample(b);
    const [id] = await addClips(b, ["x"]);
    await Promise.resolve();
    events.length = 0;
    const patch = await b.editClips({ ids: [id as string], edit: { kind: "rename", name: "x" } });
    expect(patch.upserted).toEqual([]);
    expect(patch.revision).toBe((await b.getSnapshot())?.revision);
    expect(events).toEqual([]);
    expect(await b.undo()).not.toBeNull();
    await b.undo();
    expect(await b.undo()).toBeNull();
  });

  it("errors carry code and a specific message", async () => {
    const { b } = setup();
    await openSample(b);
    const err = await b.addClip({ start_sec: 10, end_sec: 10.2, name: null, tag_id: null }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(BackendError);
    expect((err as BackendError).code).toBe("clip_too_short");
    expect((err as BackendError).message).toContain("0.5 s");
    expect(await code(b.setSettings({ ...(await b.getSettings()), min_clip_duration_sec: 2 }))).toBe("no error");
    expect(await code(b.addClip({ start_sec: 10, end_sec: 11, name: null, tag_id: null }))).toBe("clip_too_short");
  });

  it("emits 'saved' ~300 ms after changes and not before", async () => {
    const { b, events } = setup();
    await openSample(b);
    await addClips(b, ["x"]);
    await vi.advanceTimersByTimeAsync(299);
    expect(types(events)).not.toContain("saved");
    expect((await b.getSnapshot())?.save.pending).toBe(true);
    await vi.advanceTimersByTimeAsync(2);
    const saved = events.find((e) => e.type === "saved") as Extract<AppEvent, { type: "saved" }>;
    expect(saved.save).toMatchObject({ pending: false, project_path: "/sample/sample.rustcut" });
    expect(saved.save.saved_at).not.toBeNull();
    const now = await b.saveNow();
    expect(now.pending).toBe(false);
  });

  it("set_view_state saves but sends no patch", async () => {
    const { b, events } = setup();
    await openSample(b);
    await b.setViewState({ playhead_sec: 10, zoom_window_sec: 20, selected_clip_id: null });
    await vi.advanceTimersByTimeAsync(400);
    expect(types(events)).toEqual(["saved"]);
  });

  it("tags and export settings are normalised and returned in the patch", async () => {
    const { b } = setup();
    const open = await openSample(b);
    const t = await b.setTags([{ id: "x", label: "X", color: "#aabbcc" }]);
    expect(t.tags).toEqual([{ id: "x", label: "X", color: "#AABBCC" }]);
    const s = await b.setExportSettings({ ...open.snapshot.project.export_settings, parallel_jobs: 9, cut_mode: "lossless", framing: "vertical_crop" });
    expect(s.export_settings).toMatchObject({ parallel_jobs: 4, framing: "original" });
    expect(await code(b.setTags([{ id: "", label: "q", color: "#000000" }]))).toBe("invalid_input");
  });
});

describe("app, settings, dialogs", () => {
  it("app info, hw encoders (AMF only), settings normalised and persisted", async () => {
    const { b, storage } = setup();
    const info = await b.getAppInfo();
    expect(info).toMatchObject({ ffmpeg_found: true, schema_version: 2 });
    expect(["windows", "macos", "linux"]).toContain(info.platform);
    expect(info.hw_encoders.filter((e) => e.available).map((e) => e.name)).toEqual(["h264_amf"]);
    expect((await b.detectHwEncoders(true)).length).toBe(info.hw_encoders.length);
    const s = await b.setSettings({ ...(await b.getSettings()), min_clip_duration_sec: 99, cache_cap_gb: 0 });
    expect(s).toMatchObject({ min_clip_duration_sec: 5, cache_cap_gb: 1 });
    expect(await new MockBackend({ storage }).getSettings()).toMatchObject({ min_clip_duration_sec: 5 });
  });

  it("pickers, cache, reveal and output folder", async () => {
    const { b, events } = setup();
    expect(await b.pickMediaFile()).toBe("/sample/sample.mp4");
    expect(await b.pickOutputDir(null)).toBe("C:/Users/Demo/Videos/clips");
    expect(await b.clearCache()).toBe(0);
    await openSample(b);
    await b.openOutputFolder();
    await Promise.resolve();
    expect(events.find((e) => e.type === "toast")).toMatchObject({ notice: { level: "info" } });
    expect(await code(b.revealPath("C:/elsewhere/file.mp4"))).toBe("invalid_input");
  });

  it("crop_window uses the FRM-04 port", async () => {
    const { b } = setup();
    expect(await b.cropWindow(1920, 1080, 0)).toEqual({ w: 608, h: 1080, x: 656, y: 0 });
    expect(await b.cropWindow(1920, 1080, -1)).toMatchObject({ x: 0 });
    expect(await b.cropWindow(1920, 1080, 1)).toMatchObject({ x: 1312 });
  });

  it("media_url serves the sample for every path", () => {
    const { b } = setup();
    expect(b.mediaUrl("/sample/sample.mp4")).toBe("/sample/sample.mp4");
    expect(b.mediaUrl("D:/x/y.mkv")).toBe("/sample/sample.mp4");
  });
});

describe("media", () => {
  it("request_playback echoes the playback info with the next tier", async () => {
    const { b } = setup();
    await openSample(b);
    expect(await b.requestPlayback("direct")).toMatchObject({ tier: "direct", next_tier: "remux", preparing: false });
    expect(await b.requestPlayback("proxy")).toMatchObject({ tier: "proxy", next_tier: null });
  });

  it("media index: keyframes every 2 s (CFR), also emitted", async () => {
    const { b, events } = setup();
    await openSample(b);
    const info = (await b.getMediaIndex()) as NonNullable<Awaited<ReturnType<typeof b.getMediaIndex>>>;
    expect(info.keyframes).toHaveLength(90);
    expect(info.keyframes.slice(0, 3)).toEqual([0, 2, 4]);
    expect(info).toMatchObject({ vfr: false, frame_times: null, complete: true });
    expect(info.frame_duration_sec).toBeCloseTo(1 / 30, 6);
    await Promise.resolve();
    expect(events.filter((e) => e.type === "media_index")).toHaveLength(1);
  });

  it("waveform: 100 peaks/s, progressive chunks over ~2 s, deterministic, ends with done", async () => {
    const run = async () => {
      const { b, events } = setup();
      await openSample(b);
      await b.startWaveform(1);
      await vi.advanceTimersByTimeAsync(1000);
      const half = events.filter((e) => e.type === "waveform").length;
      await vi.advanceTimersByTimeAsync(1100);
      const chunks = events.flatMap((e) => (e.type === "waveform" ? [e.chunk] : []));
      return { half, chunks };
    };
    const { half, chunks } = await run();
    expect(half).toBeGreaterThan(2);
    expect(half).toBeLessThan(chunks.length);
    expect(chunks.length).toBeGreaterThan(5);
    expect(chunks.every((c) => c.peaks_per_sec === 100 && c.total_peaks === 18000 && c.track === 1)).toBe(true);
    expect(chunks.map((c) => c.done)).toEqual(chunks.map((_, i) => i === chunks.length - 1));
    let next = 0;
    for (const c of chunks) {
      expect(c.start_peak).toBe(next);
      next += c.peaks.length / 2;
      expect(c.peaks.every((v) => v >= -127 && v <= 127)).toBe(true);
    }
    expect(next).toBe(18000);
    const again = (await run()).chunks;
    expect(again[3]?.peaks).toEqual(chunks[3]?.peaks);
    const { b } = setup();
    await openSample(b);
    expect(await code(b.startWaveform(2))).toBe("invalid_input");
    expect(await code(b.startWaveform(0))).toBe("invalid_input");
  });
});

describe("timestamps (EXP-16/17)", () => {
  async function withClips() {
    const s = setup();
    await openSample(s.b);
    await s.b.addClip({ start_sec: 15, end_sec: 45, name: "Jump scare", tag_id: "funny" });
    await s.b.addClip({ start_sec: 60, end_sec: 90.5, name: 'Say "hi", ok', tag_id: null });
    await s.b.addClip({ start_sec: 100, end_sec: 130, name: "", tag_id: null });
    return s;
  }

  it("YouTube chapters insert 00:00 Intro and need >= 3 ascending chapters of >= 10 s", async () => {
    const { b } = await withClips();
    const x = await b.exportTimestamps("youtube_chapters");
    expect(x.text).toBe("00:00 Intro\n00:15 Jump scare\n01:00 Say \"hi\", ok\n01:40 Clip 03\n");
    expect(x.file_extension).toBe("txt");
    expect(x.warnings).toEqual([]);
  });

  it("YouTube chapters warn about too few and too short chapters", async () => {
    const { b } = setup();
    await openSample(b);
    await b.addClip({ start_sec: 0, end_sec: 5, name: "Short", tag_id: null });
    await b.addClip({ start_sec: 5, end_sec: 8, name: "Shorter", tag_id: null });
    const x = await b.exportTimestamps("youtube_chapters");
    expect(x.warnings.join(" ")).toMatch(/three chapters/);
    expect(x.warnings.join(" ")).toMatch(/Short.*at least 10 s/);
  });

  it("CSV quotes cells; EDL and FCPXML are well-formed", async () => {
    const { b } = await withClips();
    const csv = await b.exportTimestamps("csv");
    expect(csv.text.split("\r\n")[0]).toBe("index,name,start,end,duration_sec,tag");
    expect(csv.text).toContain('2,"Say ""hi"", ok",00:01:00.000,00:01:30.500,30.5,');
    expect(csv.text).toContain("1,Jump scare,00:00:15.000,00:00:45.000,30,Funny");
    const edl = await b.exportTimestamps("edl");
    expect(edl.text).toContain("TITLE: sample");
    expect(edl.text).toContain("001  AX       AA/V  C        00:00:15:00 00:00:45:00 00:00:00:00 00:00:30:00");
    expect(edl.text).toContain("* FROM CLIP NAME: Jump scare");
    const fcp = await b.exportTimestamps("fcpxml");
    expect(fcp.text).toContain('<asset-clip ref="r2" name="Jump scare" offset="0/30s" start="450/30s" duration="900/30s"');
    expect(fcp.text).toContain("Say &quot;hi&quot;, ok");
    expect(fcp.file_extension).toBe("fcpxml");
  });

  it("parses '12:10-13:05 Name', chapter lines, CSV and flags bad lines", async () => {
    const { b } = setup();
    await openSample(b);
    const range = await b.parseTimestamps("0:10-0:25 Jump scare\n\n01:00 - 01:30 Funny bit\nnonsense");
    expect(range.items.map((i) => [i.line, i.start_sec, i.end_sec, i.name, i.error === null])).toEqual([
      [1, 10, 25, "Jump scare", true],
      [3, 60, 90, "Funny bit", true],
      [4, 0, 0, "nonsense", false],
    ]);
    expect(range.valid_count).toBe(2);
    expect(range.items[2]?.error).toContain("no time");

    const chap = await b.parseTimestamps("00:00 Intro\n00:30 Middle\n02:00 End");
    expect(chap.items.map((i) => [i.start_sec, i.end_sec, i.name])).toEqual([
      [0, 30, "Intro"],
      [30, 120, "Middle"],
      [120, 180, "End"],
    ]);

    const csv = await b.parseTimestamps('name,start,end\n"Say ""hi""",0:05,0:20\nB,0:30,0:30.2');
    expect(csv.items[0]).toMatchObject({ name: 'Say "hi"', start_sec: 5, end_sec: 20, error: null });
    expect(csv.items[1]?.error).toContain("at least");
    expect(csv.items[1]?.line).toBe(3);

    const out = await b.parseTimestamps("2:59-5:00 Past end\n3:30-4:00 Way past\n1:00-0:30 Backwards");
    expect(out.items[0]).toMatchObject({ end_sec: 180, error: null });
    expect(out.items[1]?.error).toContain("past the end");
    expect(out.items[2]?.error).toContain("must be after");
    expect((await b.parseTimestamps("1:02:03.5-1:02:30 Hours")).items[0]).toMatchObject({ start_sec: 3723.5 });
  });

  it("import_timestamps is one undo step and skips invalid items", async () => {
    const { b } = setup();
    await openSample(b);
    const preview = await b.parseTimestamps("0:10-0:25 A\n0:40-0:55 B\nbad line");
    const patch = await b.importTimestamps(preview.items);
    expect(patch.upserted.map((c) => c.name)).toEqual(["A", "B"]);
    expect(patch.history.undo_label).toBe("Undo Import 2 clips");
    const undo = (await b.undo()) as StatePatch;
    expect(undo.removed).toHaveLength(2);
  });

  it("save_text_file downloads and returns the file name", async () => {
    const { b, downloads } = setup();
    expect(await b.saveTextFile("chapters.txt", "00:00 Intro")).toBe("chapters.txt");
    expect(downloads).toEqual([["chapters.txt", "00:00 Intro"]]);
  });
});

describe("pre-flight (EXP-05..EXP-11)", () => {
  const settings = async (b: MockBackend, over: Partial<ExportSettings> = {}): Promise<ExportSettings> => ({
    ...((await b.getSnapshot())?.project.export_settings as ExportSettings),
    ...over,
  });

  it("renders names with padding, sanitisation and in-batch duplicate suffixes", async () => {
    const { b } = setup();
    await openSample(b);
    const ids = await addClips(b, ["Jump Scare", "What?! Epic/Win: 100%", "Jump Scare", ""]);
    const r = await b.previewExport(ids, await settings(b));
    expect(r.outputs.map((o) => o.file_name)).toEqual([
      "01_Jump_Scare.mp4",
      "02_What_!_Epic_Win_100%.mp4",
      "03_Jump_Scare.mp4",
      "04_Clip_04.mp4",
    ]);
    expect(r.output_dir).toBe("C:/Users/Demo/Videos/sample_clips");
    const dup = await b.previewExport(ids, await settings(b, { naming_template: "{name}" }));
    expect(dup.outputs.map((o) => o.file_name)).toEqual(["Jump_Scare.mp4", "What_!_Epic_Win_100%.mp4", "Jump_Scare_2.mp4", "Clip_04.mp4"]);
    expect(r.can_start).toBe(true);
    expect(r.extension).toBe("mp4");
  });

  it("supports every template variable and reports a bad template as an error", async () => {
    const { b } = setup();
    await openSample(b);
    const ids = await addClips(b, ["Bit"]);
    const ok = await b.previewExport(ids, await settings(b, { naming_template: "{source_name}_{start}_{end}_{duration}_{date}_{tag}_{preset}_{name}" }));
    expect(ok.outputs[0]?.file_name).toBe("sample_00-00-10_00-00-30_20_2026-11-13_lossless_Bit.mp4");
    const bad = await b.previewExport(ids, await settings(b, { naming_template: "{oops}" }));
    expect(bad.can_start).toBe(false);
    expect(bad.items.find((i) => i.check === "names_resolved")).toMatchObject({ level: "error" });
    expect(bad.items.find((i) => i.check === "names_resolved")?.message).toContain("{oops}");
  });

  it("nothing_to_export blocks Start", async () => {
    const { b } = setup();
    await openSample(b);
    const r = await b.previewExport([], await settings(b));
    expect(r.can_start).toBe(false);
    expect(r.items.find((i) => i.check === "nothing_to_export")?.level).toBe("error");
    expect(await code(b.startExport([], await settings(b)))).toBe("invalid_input");
    expect(await code(b.previewExport(["ghost"], await settings(b)))).toBe("not_found");
  });

  it("EXP-11 estimates: lossless uses bit rate and lead-in; re-encode uses maxrate + bufsize", async () => {
    const { b } = setup();
    await openSample(b);
    const a = await b.addClip({ start_sec: 11, end_sec: 41, name: "A", tag_id: null }); // lead-in 1 s
    const lossless = await b.previewExport([a.clip.id], await settings(b));
    expect(lossless.outputs[0]?.lead_in_sec).toBe(1);
    expect(lossless.estimate_bytes).toBe(Math.ceil((192_155 / 8) * 31));
    const snap = await b.previewExport([a.clip.id], await settings(b, { snap_in_to_keyframe: true }));
    expect(snap.estimate_bytes).toBe(Math.ceil((192_155 / 8) * 30));
    const accurate = await b.previewExport([a.clip.id], await settings(b, { cut_mode: "accurate" }));
    expect(accurate.outputs[0]?.lead_in_sec).toBeNull();
    // 720p source -> 8 Mbps video + 192 kbps audio, bufsize = 2x maxrate, one clip.
    expect(accurate.estimate_bytes).toBe(Math.ceil(((8_000_000 + 192_000) * 30 + 16_000_000) / 8));
    const vertical = await b.previewExport([a.clip.id], await settings(b, { cut_mode: "accurate", framing: "vertical_crop" }));
    expect(vertical.preset).toBe("vcrop");
    expect(vertical.estimate_bytes).toBe(Math.ceil(((16_000_000 + 192_000) * 30 + 32_000_000) / 8));
  });

  it("free_space (>= 1.2 x estimate), out_of_range and max_duration (FRM-08) checks", async () => {
    const { b } = setup();
    await openSample(b);
    const [id] = await addClips(b, ["Long"]);
    const full = await b.previewExport([id as string], await settings(b, { output_dir: "D:/nospace/out" }));
    expect(full.can_start).toBe(false);
    expect(full.items.find((i) => i.check === "free_space")).toMatchObject({ level: "error" });
    expect(full.free_bytes).toBe(50_000);
    const ok = await b.previewExport([id as string], await settings(b));
    expect(ok.items.find((i) => i.check === "free_space")?.level).toBe("ok");
    const long = await b.previewExport([id as string], await settings(b, { max_duration_sec: 15 }));
    expect(long.items.find((i) => i.check === "max_duration")).toMatchObject({ level: "warning", clip_ids: [id] });
    expect(long.can_start).toBe(true);

  });

  it("out_of_range: clips starting after a (relinked, shorter) source end are excluded with a warning", async () => {
    const { b } = setup();
    await openSample(b);
    const early = await b.addClip({ start_sec: 10, end_sec: 20, name: "early", tag_id: null });
    const late = await b.addClip({ start_sec: 130, end_sec: 150, name: "late", tag_id: null });
    expect((await b.relinkSource("D:/new/mismatch.mp4", true)).kind).toBe("relinked"); // 120 s long
    const r = await b.previewExport([early.clip.id, late.clip.id], await settings(b));
    expect(r.items.find((i) => i.check === "out_of_range")).toMatchObject({ level: "warning", clip_ids: [late.clip.id] });
    expect(r.excluded_clip_ids).toEqual([late.clip.id]);
    expect(r.outputs.map((o) => o.clip_id)).toEqual([early.clip.id]);
    expect(r.can_start).toBe(true);
    const only = await b.previewExport([late.clip.id], await settings(b));
    expect(only.can_start).toBe(false);
    expect(only.items.find((i) => i.check === "nothing_to_export")?.level).toBe("error");
  });

  it("existing files follow the policy; a clip's own previous output is replaced", async () => {
    const { b } = setup({ exportTiming: FAST });
    await openSample(b);
    const [x] = await addClips(b, ["Bit"]);
    const s = await settings(b, { naming_template: "{name}" });
    const first = await b.startExport([x as string], s);
    await vi.advanceTimersByTimeAsync(600);
    expect(first.clip_ids).toEqual([x]);
    const again = await b.previewExport([x as string], s);
    expect(again.outputs[0]).toMatchObject({ file_name: "Bit.mp4", action: "replace_previous" });
    // Another clip with the same name collides with the file now on disk.
    const y = await b.addClip({ start_sec: 100, end_sec: 120, name: "Bit", tag_id: null });
    await b.editClips({ ids: [y.clip.id], edit: { kind: "retime", start_sec: 100, end_sec: 121 } });
    const tpl = await b.previewExport([y.clip.id], await settings(b, { naming_template: "{name}" }));
    expect(tpl.outputs[0]).toMatchObject({ file_name: "Bit_2.mp4", action: "write" });
    const over = await b.previewExport([y.clip.id], await settings(b, { naming_template: "{name}", on_conflict: "overwrite" }));
    expect(over.outputs[0]?.action).toBe("overwrite");
    const skip = await b.previewExport([y.clip.id], await settings(b, { naming_template: "{name}", on_conflict: "skip" }));
    expect(skip.outputs[0]?.action).toBe("skip");
    expect(skip.excluded_clip_ids).toEqual([y.clip.id]);
  });
});

describe("export simulation (EXP-08..EXP-10)", () => {
  const run = async (names: string[], over: Partial<ExportSettings> = {}) => {
    const s = setup();
    await openSample(s.b);
    const ids = await addClips(s.b, names);
    await Promise.resolve();
    s.events.length = 0;
    const settings = { ...((await s.b.getSnapshot())?.project.export_settings as ExportSettings), ...over };
    return { ...s, ids, settings };
  };
  const finishedEvent = (events: AppEvent[]) =>
    events.find((e) => e.type === "export_batch" && e.state.state === "finished") as Extract<AppEvent, { type: "export_batch" }> | undefined;

  it("emits batch running -> patch exporting -> progress -> patch + clip_finished per clip -> finished", async () => {
    const { b, events, ids, settings } = await run(["One", "Two"]);
    const info = await b.startExport(ids, settings);
    expect(info).toMatchObject({ clip_ids: ids, total_duration_sec: 40, output_dir: "C:/Users/Demo/Videos/sample_clips" });
    await vi.advanceTimersByTimeAsync(2000);

    const seq = events.map((e) => {
      if (e.type === "export_batch") return `batch:${e.state.state}`;
      if (e.type === "patch") return `patch:${e.patch.upserted.map((c) => c.export_status).join(",")}`;
      if (e.type === "export_clip_finished") return `finished:${e.finished.outcome.outcome}`;
      return e.type;
    });
    expect(seq[0]).toBe("batch:running");
    expect(seq[1]).toBe("patch:exporting,exporting");
    const firstProgress = seq.indexOf("export_progress");
    expect(firstProgress).toBe(2);
    const compact = seq.filter((s, i) => s !== "export_progress" || seq[i - 1] !== "export_progress");
    expect(compact.filter((s) => !s.startsWith("saved"))).toEqual([
      "batch:running",
      "patch:exporting,exporting",
      "export_progress",
      "patch:completed",
      "finished:done",
      "export_progress",
      "patch:completed",
      "finished:done",
      "export_progress",
      "batch:finished",
    ]);

    const fin = finishedEvent(events);
    expect(fin?.state.state === "finished" && fin.state.summary).toMatchObject({ done: ids, failed: [], cancelled: [], skipped: [] });
    const snap = await b.getSnapshot();
    expect(snap?.project.clips.map((c) => c.export_status)).toEqual(["completed", "completed"]);
    expect(snap?.project.clips[0]?.last_export).toMatchObject({
      output_path: "C:/Users/Demo/Videos/sample_clips/01_One.mp4",
      preset: "lossless",
      error: null,
    });
    // Revisions of all patches are consecutive.
    const revs = events.filter((e) => e.type === "patch").map((e) => (e as { patch: StatePatch }).patch.revision);
    expect(revs).toEqual(revs.map((_, i) => (revs[0] as number) + i));
  });

  it("progress is weighted by clip duration and ends at 1 with done == total", async () => {
    const { b, events, ids, settings } = await run(["A", "B"]);
    await b.startExport(ids, settings);
    await vi.advanceTimersByTimeAsync(2000);
    const prog = events.flatMap((e) => (e.type === "export_progress" ? [e.progress] : []));
    const fractions = prog.map((p) => p.overall_fraction);
    expect([...fractions].sort((a, c) => a - c)).toEqual(fractions);
    expect(prog.at(-1)).toMatchObject({ overall_fraction: 1, done: 2, failed: 0, total: 2, running: [] });
    expect(prog[0]?.running[0]).toMatchObject({ clip_id: ids[0] });
    expect(prog[0]?.running).toHaveLength(1);
  });

  it("parallel_jobs runs several clips at once", async () => {
    const { b, events, ids, settings } = await run(["A", "B", "C"], { parallel_jobs: 3 });
    await b.startExport(ids, settings);
    await vi.advanceTimersByTimeAsync(100);
    const first = events.find((e) => e.type === "export_progress") as Extract<AppEvent, { type: "export_progress" }>;
    expect(first.progress.running).toHaveLength(3);
  });

  it("a clip whose name contains 'fail' fails with a 20-line tail; the batch continues; retry re-runs only failures", async () => {
    const { b, events, ids, settings } = await run(["Good", "will fail", "Fine"]);
    await b.startExport(ids, settings);
    await vi.advanceTimersByTimeAsync(3000);
    const finished = events.flatMap((e) => (e.type === "export_clip_finished" ? [e.finished] : []));
    expect(finished.map((f) => f.outcome.outcome)).toEqual(["done", "failed", "done"]);
    const failed = finished[1]?.outcome;
    expect(failed?.outcome === "failed" && failed.log_tail).toHaveLength(20);
    expect(failed?.outcome === "failed" && failed.exit_code).toBe(1);
    const snap = await b.getSnapshot();
    expect(snap?.project.clips.map((c) => c.export_status)).toEqual(["completed", "failed", "completed"]);
    expect(snap?.project.clips[1]?.last_export).toMatchObject({ exit_code: 1, preset: "lossless" });
    const summary = finishedEvent(events);
    expect(summary?.state.state === "finished" && summary.state.summary.failed).toEqual([ids[1]]);

    // Retrying without a fix fails again; renaming the clip first makes it pass.
    events.length = 0;
    const retry = await b.retryFailed();
    expect(retry?.clip_ids).toEqual([ids[1]]);
    await vi.advanceTimersByTimeAsync(1000);
    expect((await b.getSnapshot())?.project.clips[1]?.export_status).toBe("failed");
    await b.editClips({ ids: [ids[1] as string], edit: { kind: "rename", name: "fixed" } });
    expect((await b.retryFailed())?.clip_ids).toEqual([ids[1]]);
    await vi.advanceTimersByTimeAsync(1000);
    const after = (await b.getSnapshot())?.project.clips;
    expect(after?.map((c) => c.export_status)).toEqual(["completed", "completed", "completed"]);
    expect(await b.retryFailed()).toBeNull();
  });

  it("start while running fails with busy; retry while running too", async () => {
    const { b, ids, settings } = await run(["A"]);
    await b.startExport(ids, settings);
    expect(await code(b.startExport(ids, settings))).toBe("busy");
    expect(await code(b.retryFailed())).toBe("busy");
    await vi.advanceTimersByTimeAsync(1000);
    expect(await code(b.startExport(ids, settings))).toBe("no error");
  });

  it("'diskfull' pauses the batch until resume_export", async () => {
    const { b, events, ids, settings } = await run(["Quick", "diskfull clip", "After"]);
    await b.startExport(ids, settings);
    await vi.advanceTimersByTimeAsync(1500);
    const paused = events.find((e) => e.type === "export_batch" && e.state.state === "paused") as Extract<AppEvent, { type: "export_batch" }>;
    expect(paused.state).toMatchObject({ state: "paused", reason: "disk_full" });
    expect(paused.state.state === "paused" && paused.state.message).toContain("full");
    const doneBefore = events.filter((e) => e.type === "export_clip_finished").length;
    await vi.advanceTimersByTimeAsync(2000);
    expect(events.filter((e) => e.type === "export_clip_finished")).toHaveLength(doneBefore); // frozen
    expect(finishedEvent(events)).toBeUndefined();
    expect(await code(b.startExport(ids, settings))).toBe("busy");

    await b.resumeExport();
    await vi.advanceTimersByTimeAsync(2000);
    expect(events.filter((e) => e.type === "export_batch").map((e) => (e as { state: { state: string } }).state.state)).toEqual([
      "running",
      "paused",
      "running",
      "finished",
    ]);
    expect((await b.getSnapshot())?.project.clips.map((c) => c.export_status)).toEqual(["completed", "completed", "completed"]);
  });

  it("cancel_export(id) cancels one clip and restores its status; cancel(null) cancels all", async () => {
    const one = await run(["A", "B", "C"]);
    await one.b.startExport(one.ids, one.settings);
    await vi.advanceTimersByTimeAsync(60);
    await one.b.cancelExport(one.ids[2] as string); // still queued
    await vi.advanceTimersByTimeAsync(2000);
    const s1 = finishedEvent(one.events)?.state;
    expect(s1?.state === "finished" && s1.summary).toMatchObject({ done: [one.ids[0], one.ids[1]], cancelled: [one.ids[2]] });
    expect((await one.b.getSnapshot())?.project.clips.map((c) => c.export_status)).toEqual(["completed", "completed", "pending"]);

    const all = await run(["A", "B"]);
    await all.b.startExport(all.ids, all.settings);
    await vi.advanceTimersByTimeAsync(100);
    await all.b.cancelExport(null);
    await vi.advanceTimersByTimeAsync(10);
    const s2 = finishedEvent(all.events)?.state;
    expect(s2?.state === "finished" && s2.summary).toMatchObject({ done: [], cancelled: all.ids });
    expect((await all.b.getSnapshot())?.project.clips.map((c) => c.export_status)).toEqual(["pending", "pending"]);
    await vi.advanceTimersByTimeAsync(1000);
    expect(all.events.filter((e) => e.type === "export_batch" && e.state.state === "finished")).toHaveLength(1);
    await all.b.cancelExport(null); // nothing running: no-op
  });

  it("editing a clip while it exports marks it Outdated when done (EXP-01)", async () => {
    const { b, ids, settings } = await run(["Edit me"]);
    await b.startExport(ids, settings);
    await vi.advanceTimersByTimeAsync(60);
    expect((await b.getSnapshot())?.project.clips[0]?.export_status).toBe("exporting");
    await b.editClips({ ids, edit: { kind: "rename", name: "Edited" } });
    expect((await b.getSnapshot())?.project.clips[0]?.export_status).toBe("exporting");
    await vi.advanceTimersByTimeAsync(1000);
    expect((await b.getSnapshot())?.project.clips[0]?.export_status).toBe("outdated");
  });

  it("a clip deleted while exporting does not break the batch", async () => {
    const { b, events, ids, settings } = await run(["A", "B"]);
    await b.startExport(ids, settings);
    await vi.advanceTimersByTimeAsync(60);
    await b.deleteClips([ids[0] as string]);
    await vi.advanceTimersByTimeAsync(2000);
    expect(finishedEvent(events)).toBeDefined();
    expect((await b.getSnapshot())?.project.clips).toHaveLength(1);
  });

  it("policy-skipped clips are reported as skipped, not exported", async () => {
    const { b, events, ids, settings } = await run(["Bit"]);
    const s = { ...settings, naming_template: "{name}", on_conflict: "skip" as const };
    await b.startExport(ids, s);
    await vi.advanceTimersByTimeAsync(1000);
    const y = await b.addClip({ start_sec: 100, end_sec: 120, name: "Bit", tag_id: null });
    events.length = 0;
    const info = await b.startExport([y.clip.id], s);
    expect(info.clip_ids).toEqual([]);
    await vi.advanceTimersByTimeAsync(300);
    const skipped = events.find((e) => e.type === "export_clip_finished") as Extract<AppEvent, { type: "export_clip_finished" }>;
    expect(skipped.finished).toMatchObject({ clip_id: y.clip.id, outcome: { outcome: "skipped" } });
    const fin = finishedEvent(events)?.state;
    expect(fin?.state === "finished" && fin.summary).toMatchObject({ done: [], skipped: [y.clip.id] });
    expect((await b.getSnapshot())?.project.clips[1]?.export_status).toBe("pending");
  });

  it("close_project stops a running export and leaves no timers behind", async () => {
    const { b, ids, settings } = await run(["A", "B"]);
    await b.startExport(ids, settings);
    await vi.advanceTimersByTimeAsync(60);
    await b.closeProject();
    expect(vi.getTimerCount()).toBe(0);
  });
});
