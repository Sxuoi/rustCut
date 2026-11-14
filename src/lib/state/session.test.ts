// PRJ-03 / EXP-09: opening another file must not cancel a batch or lose the playhead.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { backend } from "$lib/ipc";
import { t } from "$lib/i18n";
import { dispatchEvent } from "./events";
import { exporter } from "./exporter.svelte";
import { player } from "./player.svelte";
import { project } from "./project.svelte";
import { applyOpenResult, loadAppContext, MEDIA_JOBS_START_TIMEOUT_MS, openPath, startUp } from "./session";
import { ui } from "./ui.svelte";

beforeEach(() => {
  player.reset();
  project.clear();
  vi.spyOn(backend, "startWaveform").mockResolvedValue();
  vi.spyOn(backend, "getMediaIndex").mockResolvedValue(null);
  vi.spyOn(backend, "startMediaIndex").mockResolvedValue();
});

async function previousProject() {
  const result = await backend.openPath("/previous.mp4");
  await backend.addClip({ start_sec: 41, end_sec: 45, name: "selected", tag_id: null });
  result.snapshot = (await backend.getSnapshot())!;
  project.load(result.snapshot);
  player.playback = result.playback;
  player.duration = result.snapshot.project.source.duration_sec;
  ui.phase = "ready";
  return result;
}

afterEach(() => {
  player.reset(); // settles a waveform start still waiting for a first frame
  vi.restoreAllMocks();
  exporter.phase = "idle";
  ui.phase = "empty";
});

describe("openPath", () => {
  it("is refused while an export runs (no silent cancel)", async () => {
    const spy = vi.spyOn(backend, "openPath");
    exporter.phase = "running";
    expect(await openPath("/sample/sample.mp4")).toBe(false);
    expect(spy).not.toHaveBeenCalled();
    expect(ui.phase).not.toBe("probing");
  });

  it("keeps live view, pending In/Out, loop and selection when opening another file fails", async () => {
    const previous = await previousProject();
    const actual = structuredClone(previous.snapshot);
    actual.project.view_state.playhead_sec = 40;
    actual.project.view_state.zoom_window_sec = 60;
    const snapshot = vi.spyOn(backend, "getSnapshot").mockResolvedValue(actual);
    vi.spyOn(backend, "openPath").mockRejectedValue(new Error("nope"));
    ui.phase = "ready";
    player.currentTime = 42;
    player.restoreTime = null;
    player.pendingIn = 41;
    player.pendingOut = 45;
    player.setLoop("live-loop", { start: 41, end: 45 });
    project.select(previous.snapshot.project.clips[0].id);
    const selection = [...project.selection];
    ui.setZoomWindow(30);
    expect(await openPath("/bad.mp4")).toBe(false);
    expect(ui.phase).toBe("ready");
    expect(player.restoreTime).toBe(42);
    expect(player.currentTime).toBe(42);
    expect(player.marks).toEqual({ in: 41, out: 45 });
    expect(player.loopClipId).toBe("live-loop");
    expect(player.loopRange).toEqual({ start: 41, end: 45 });
    expect([...project.selection]).toEqual(selection);
    expect(ui.zoomWindowSec).toBe(30);
    expect(snapshot).toHaveBeenCalledOnce();
    expect(player.playback).toEqual(previous.playback);
  });

  it.each(["throws", "null"])("clears the stale project when getSnapshot %s after an open failure", async (mode) => {
    await previousProject();
    player.currentTime = 42;
    player.loopClipId = "old";
    vi.spyOn(backend, "openPath").mockRejectedValue(new Error("open failed"));
    const snapshot = vi.spyOn(backend, "getSnapshot");
    if (mode === "throws") snapshot.mockRejectedValue(new Error("No project open"));
    else snapshot.mockResolvedValue(null);
    expect(await openPath("/bad.mp4")).toBe(false);
    expect(ui.phase).toBe("empty");
    expect(ui.probingName).toBeNull();
    expect(project.loaded).toBe(false);
    expect(player.playback).toBeNull();
    expect(player.restoreTime).toBeNull();
    expect(player.loopClipId).toBeNull();
  });

  it("loads the backend's different project even at the same revision", async () => {
    await previousProject();
    const actual = await backend.openPath("/actual.mp4");
    actual.snapshot.project.view_state.playhead_sec = 17;
    vi.spyOn(backend, "getSnapshot").mockResolvedValue(actual.snapshot);
    vi.spyOn(backend, "openPath").mockRejectedValue(new Error("late failure"));
    const playback = vi.spyOn(backend, "requestPlayback").mockResolvedValue(actual.playback!);
    expect(await openPath("/bad.mp4")).toBe(false);
    expect(project.source?.path).toBe("/actual.mp4");
    expect(player.restoreTime).toBe(17);
    expect(ui.phase).toBe("ready");
    expect(playback).toHaveBeenCalledWith("direct");
  });

  it.each([false, true])("identifies an unsaved source session by creation time (different: %s)", async (different) => {
    const previous = await previousProject();
    previous.snapshot.save.project_path = null;
    project.load(previous.snapshot);
    const actual = structuredClone(previous.snapshot);
    actual.project.view_state.playhead_sec = 17;
    if (different) actual.project.created_at = "2020-01-01T00:00:00Z";
    player.currentTime = 42;
    player.pendingIn = 41;
    vi.spyOn(backend, "getSnapshot").mockResolvedValue(actual);
    vi.spyOn(backend, "openPath").mockRejectedValue(new Error("failed"));
    const playback = vi.spyOn(backend, "requestPlayback").mockResolvedValue(previous.playback!);
    await openPath("/bad.mp4");
    expect(player.currentTime).toBe(different ? 17 : 42);
    expect(player.pendingIn).toBe(different ? null : 41);
    expect(playback).toHaveBeenCalledTimes(different ? 1 : 0);
  });

  it("resyncs changed state in the same project", async () => {
    const previous = await previousProject();
    const actual = structuredClone(previous.snapshot);
    actual.revision++;
    actual.project.export_settings.naming_template = "changed_{index}";
    actual.read_only = { kind: "open_elsewhere", holder: "another window" };
    actual.history = { can_undo: true, can_redo: false, undo_label: "Undo crop", redo_label: null };
    actual.project.clips[0].name = "backend edit";
    player.currentTime = 42;
    player.pendingIn = 41;
    project.select(actual.project.clips[0].id);
    vi.spyOn(backend, "getSnapshot").mockResolvedValue(actual);
    vi.spyOn(backend, "openPath").mockRejectedValue(new Error("failed"));
    vi.spyOn(backend, "requestPlayback").mockResolvedValue(previous.playback!);
    expect(await openPath("/bad.mp4")).toBe(false);
    expect(project.revision).toBe(actual.revision);
    expect(project.readOnly).toEqual(actual.read_only);
    expect(project.settings?.naming_template).toBe("changed_{index}");
    expect(project.history).toEqual(actual.history);
    expect(project.clips[0].name).toBe("backend edit");
    expect(project.primaryId).toBe(actual.project.clips[0].id);
    expect(player.currentTime).toBe(42);
    expect(player.pendingIn).toBe(41);
  });

  it("loads a missing-source project without requesting playback", async () => {
    await previousProject();
    const actual = await backend.openPath("/missing.mp4");
    vi.spyOn(backend, "getSnapshot").mockResolvedValue(actual.snapshot);
    vi.spyOn(backend, "openPath").mockRejectedValue(new Error("failed"));
    const playback = vi.spyOn(backend, "requestPlayback");
    expect(await openPath("/bad.mp4")).toBe(false);
    expect(project.source?.path).toBe("/missing.mp4");
    expect(project.readOnly?.kind).toBe("source_missing");
    expect(player.playback).toBeNull();
    expect(playback).not.toHaveBeenCalled();
  });
});

describe("background media jobs after an open (AC-14)", () => {
  async function opened() {
    const result = await backend.openPath("/media/first-frame.mp4");
    expect(result.playback).not.toBeNull();
    expect(result.snapshot.project.source.audio_tracks.length).toBeGreaterThan(0);
    return result;
  }

  it("requests the media index at once but starts the whole-file jobs at the first frame", async () => {
    applyOpenResult(await opened());
    expect(backend.getMediaIndex).toHaveBeenCalledOnce(); // Player.svelte needs keyframes for the first frame
    await Promise.resolve();
    expect(backend.startWaveform).not.toHaveBeenCalled();
    expect(backend.startMediaIndex).not.toHaveBeenCalled();
    player.ready = true; // Player.svelte: first frame presented
    await vi.waitFor(() => expect(backend.startWaveform).toHaveBeenCalledWith(1));
    expect(backend.startWaveform).toHaveBeenCalledOnce();
    expect(backend.startMediaIndex).toHaveBeenCalledOnce();
  });

  it("starts the packet-scan index but no waveform for a video without audio", async () => {
    const result = await opened();
    result.snapshot.project.source.audio_tracks = [];
    applyOpenResult(result);
    player.ready = true;
    await vi.waitFor(() => expect(backend.startMediaIndex).toHaveBeenCalledOnce());
    expect(backend.startWaveform).not.toHaveBeenCalled();
  });

  it("starts the whole-file jobs after MEDIA_JOBS_START_TIMEOUT_MS when no frame is presented", async () => {
    const result = await opened();
    vi.useFakeTimers();
    try {
      applyOpenResult(result);
      await vi.advanceTimersByTimeAsync(MEDIA_JOBS_START_TIMEOUT_MS - 1);
      expect(backend.startWaveform).not.toHaveBeenCalled();
      expect(backend.startMediaIndex).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1);
      expect(backend.startWaveform).toHaveBeenCalledOnce();
      expect(backend.startMediaIndex).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

  it("never starts the whole-file jobs of a project closed or replaced before its first frame", async () => {
    const result = await opened();
    vi.useFakeTimers();
    try {
      applyOpenResult(result);
      player.reset(); // closeProject(), or loadIntoStores() for the next file
      player.ready = true;
      await vi.advanceTimersByTimeAsync(MEDIA_JOBS_START_TIMEOUT_MS);
      expect(backend.startWaveform).not.toHaveBeenCalled();
      expect(backend.startMediaIndex).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("startUp: app info waits for the backend's FFmpeg probe (cold start)", () => {
  it("loads settings and recent projects without asking for app info", async () => {
    const info = vi.spyOn(backend, "getAppInfo");
    await loadAppContext();
    expect(info).not.toHaveBeenCalled();
    expect(ui.settings).not.toBeNull();
  });

  it("opens the launch file first and asks for app info after its first frame", async () => {
    const sample = await backend.getAppInfo();
    let answer!: (value: typeof sample) => void;
    const info = vi.spyOn(backend, "getAppInfo").mockReturnValue(new Promise((resolve) => { answer = resolve; }));
    vi.spyOn(backend, "getSnapshot").mockResolvedValue(null);
    vi.spyOn(backend, "getLaunchPath").mockResolvedValue("/media/launch.mp4");
    const open = vi.spyOn(backend, "openPath");
    ui.appInfo = null;
    const started = startUp();
    await vi.waitFor(() => expect(project.loaded).toBe(true));
    expect(open).toHaveBeenCalledWith("/media/launch.mp4");
    expect(info).not.toHaveBeenCalled();
    player.ready = true; // Player.svelte: first frame presented
    await vi.waitFor(() => expect(info).toHaveBeenCalledOnce());
    answer(sample);
    await started;
    expect(ui.appInfo).toEqual(sample);
  });

  it("asks for app info at once when no file was given", async () => {
    vi.spyOn(backend, "getSnapshot").mockResolvedValue(null);
    vi.spyOn(backend, "getLaunchPath").mockResolvedValue(null);
    const info = vi.spyOn(backend, "getAppInfo");
    ui.appInfo = null;
    await startUp();
    expect(info).toHaveBeenCalledOnce();
    expect(ui.appInfo).not.toBeNull();
  });
});

describe("backend notice localisation (PRD 8.5)", () => {
  it("also translates notices returned when opening a project", async () => {
    const previous = await previousProject();
    const toast = vi.spyOn(ui, "toast");
    applyOpenResult({ ...previous, notices: [{ code: "ffmpeg_too_old", level: "warning", message: "backend English" }] });
    expect(toast).toHaveBeenCalledWith({ code: "ffmpeg_too_old", level: "warning", message: t("notice.ffmpegTooOld") });
  });

  it("translates known codes and retains unknown backend messages", () => {
    const toast = vi.spyOn(ui, "toast");
    dispatchEvent({ type: "toast", notice: { code: "ffmpeg_too_old", level: "warning", message: "backend English" } });
    expect(toast).toHaveBeenLastCalledWith({ code: "ffmpeg_too_old", level: "warning", message: t("notice.ffmpegTooOld") });
    expect(t("notice.ffmpegTooOld")).not.toBe("notice.ffmpegTooOld");
    dispatchEvent({ type: "toast", notice: { code: "future_notice", level: "info", message: "backend fallback" } });
    expect(toast).toHaveBeenLastCalledWith({ code: "future_notice", level: "info", message: "backend fallback" });
  });
});
