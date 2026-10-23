import { useCallback, useEffect, useRef, useState } from "react";
import type { ChangeEvent, MouseEvent } from "react";
import {
  isTauri,
  loadProject,
  onExportEvents,
  openVideoDialog,
  pickOutputFolder,
  probeMedia,
  saveProject,
  startExport,
} from "./api";
import type {
  Clip,
  ExportFinishedEvent,
  ExportMode,
  ExportProgressEvent,
  MediaInfo,
  ProjectState,
} from "./types";

// ffprobe supplies the real frame rate in the Tauri build; frame stepping
// falls back to 30 fps until then.
const FPS_FALLBACK = 30;
const COLORS = ["#3B82F6", "#EF4444", "#22C55E", "#F59E0B", "#A855F7", "#06B6D4"];
const SPEEDS = [0.5, 1, 1.25, 1.5, 2, 3];

function fmtTime(secs: number): string {
  const total = Math.max(0, Math.floor(secs));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

function fmtDur(secs: number): string {
  const total = Math.max(0, Math.round(secs));
  return total >= 60 ? `${Math.floor(total / 60)}m${total % 60}s` : `${total}s`;
}

// `video.mp4` -> `video.rustcut`, kept next to the source (PRD 4.5).
function projectPathFor(videoPath: string): string {
  return `${videoPath.replace(/\.[^.]+$/, "")}.rustcut`;
}

export default function App() {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const [videoUrl, setVideoUrl] = useState<string | null>(null);
  const [sourcePath, setSourcePath] = useState("");
  const [sourceName, setSourceName] = useState("");
  const [media, setMedia] = useState<MediaInfo | null>(null);
  const [outputDir, setOutputDir] = useState("");
  const [exportMode, setExportMode] = useState<ExportMode>("lossless_copy");
  const [progress, setProgress] = useState<Record<string, ExportProgressEvent>>({});
  const [exporting, setExporting] = useState(false);
  const [exportSummary, setExportSummary] = useState("");
  const [duration, setDuration] = useState(0);
  const [current, setCurrent] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [speed, setSpeed] = useState(1);
  const [markIn, setMarkIn] = useState<number | null>(null);
  const [markOut, setMarkOut] = useState<number | null>(null);
  const [clips, setClips] = useState<Clip[]>([]);
  const [loopId, setLoopId] = useState<string | null>(null);
  const [status, setStatus] = useState("Open a video to start marking clips.");

  const fps = media && media.fps > 0 ? media.fps : FPS_FALLBACK;

  const onFile = useCallback((e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) {
      return;
    }
    setVideoUrl(URL.createObjectURL(file));
    setSourceName(file.name.replace(/\.[^.]+$/, ""));
    setStatus(`Loaded ${file.name} (browser preview: no ffprobe/export)`);
  }, []);

  const openVideo = useCallback(async () => {
    try {
      const picked = await openVideoDialog();
      if (!picked) {
        return;
      }
      setVideoUrl(picked.url);
      setSourcePath(picked.path);
      setSourceName(picked.path.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, "") ?? "");
      setClips([]);
      setProgress({});
      setMedia(null);
      let mediaLine = "";
      try {
        const info = await probeMedia(picked.path);
        setMedia(info);
        setDuration(info.duration_sec);
        mediaLine = `Loaded ${info.width}x${info.height}, ${info.fps.toFixed(3)} fps, ${info.codec_name}.`;
      } catch (err) {
        mediaLine = `Video opened, but ffprobe failed: ${String(err)}`;
      }
      // Crash recovery (PRD 4.5): restore saved markers when the .rustcut
      // file for this video exists; a missing file is the normal case.
      try {
        const saved = await loadProject(projectPathFor(picked.path));
        setClips(saved.clips);
        setExportMode(saved.export_settings.mode);
        setOutputDir(saved.export_settings.output_dir || "");
        setDuration((d) => d || saved.source_video.duration_sec || 0);
        setStatus(
          `${mediaLine} Restored ${saved.clips.length} saved clip(s) from the project file.`,
        );
      } catch {
        setStatus(mediaLine);
      }
    } catch (err) {
      setStatus(`Open failed: ${String(err)}`);
    }
  }, []);

  const chooseOutput = useCallback(async () => {
    const dir = await pickOutputFolder();
    if (dir) {
      setOutputDir(dir);
      setStatus(`Output folder: ${dir}`);
    }
  }, []);

  const seekTo = useCallback(
    (t: number) => {
      const v = videoRef.current;
      if (!v) {
        return;
      }
      v.currentTime = Math.min(Math.max(0, t), duration || v.duration || 0);
      setCurrent(v.currentTime);
    },
    [duration],
  );

  const seekBy = useCallback(
    (delta: number) => {
      const v = videoRef.current;
      if (!v) {
        return;
      }
      seekTo(v.currentTime + delta);
    },
    [seekTo],
  );

  const togglePlay = useCallback(() => {
    const v = videoRef.current;
    if (!v) {
      return;
    }
    if (v.paused) {
      void v.play().catch(() => setStatus("Playback was blocked; click the video first."));
    } else {
      v.pause();
    }
  }, []);

  const onSpeedChange = useCallback((e: ChangeEvent<HTMLSelectElement>) => {
    const value = Number(e.target.value);
    setSpeed(value);
    const v = videoRef.current;
    if (v) {
      v.playbackRate = value;
    }
  }, []);

  const onLoadedMetadata = useCallback(() => {
    const v = videoRef.current;
    if (v) {
      setDuration(v.duration || 0);
    }
  }, []);

  const onTimeUpdate = useCallback(() => {
    const v = videoRef.current;
    if (!v) {
      return;
    }
    let t = v.currentTime;
    if (loopId !== null) {
      const clip = clips.find((c) => c.id === loopId);
      if (clip && (t < clip.start_sec - 0.05 || t >= clip.end_sec)) {
        v.currentTime = clip.start_sec;
        t = clip.start_sec;
      }
    }
    setCurrent(t);
  }, [loopId, clips]);

  // Mark Out before Mark In swaps the pair (PRD edge-case handling).
  const addClip = useCallback(() => {
    if (markIn === null || markOut === null) {
      setStatus("Set both Mark In (I) and Mark Out (O) first.");
      return;
    }
    const start = Math.min(markIn, markOut);
    const end = Math.max(markIn, markOut);
    if (end - start < 0.05) {
      setStatus("Clip is too short - move the playhead further before pressing O.");
      return;
    }
    const index = clips.length + 1;
    const clip: Clip = {
      id: crypto.randomUUID(),
      index,
      name: "",
      start_sec: start,
      end_sec: end,
      color: COLORS[(index - 1) % COLORS.length],
      crop_x_offset: 0,
      selected_for_export: true,
      export_status: "pending",
    };
    setClips((prev) => [...prev, clip]);
    setMarkIn(null);
    setMarkOut(null);
    setStatus(`Added clip #${String(index).padStart(2, "0")} (${fmtDur(end - start)}).`);
  }, [markIn, markOut, clips.length]);

  const toggleClip = useCallback((id: string) => {
    setClips((prev) =>
      prev.map((c) =>
        c.id === id ? { ...c, selected_for_export: !c.selected_for_export } : c,
      ),
    );
  }, []);

  const renameClip = useCallback((id: string, name: string) => {
    setClips((prev) => prev.map((c) => (c.id === id ? { ...c, name } : c)));
  }, []);

  const setCropOffset = useCallback((id: string, value: number) => {
    setClips((prev) =>
      prev.map((c) => (c.id === id ? { ...c, crop_x_offset: value } : c)),
    );
  }, []);

  const removeClip = useCallback((id: string) => {
    setLoopId((prev) => (prev === id ? null : prev));
    setClips((prev) => prev.filter((c) => c.id !== id).map((c, i) => ({ ...c, index: i + 1 })));
  }, []);

  const jumpToClip = useCallback(
    (clip: Clip) => {
      seekTo(clip.start_sec);
    },
    [seekTo],
  );

  const toggleLoop = useCallback((id: string) => {
    setLoopId((prev) => (prev === id ? null : id));
  }, []);

  const focusLastRename = useCallback(() => {
    const last = clips[clips.length - 1];
    if (!last) {
      return;
    }
    const el = document.querySelector<HTMLInputElement>(`[data-clip-id="${last.id}"]`);
    el?.focus();
    el?.select();
  }, [clips]);

  const buildProjectState = useCallback((): ProjectState => {
    return {
      project_version: "1.0",
      source_video: {
        path: sourcePath || (sourceName ? `${sourceName}.mp4` : ""),
        duration_sec: duration || media?.duration_sec || 0,
        resolution: media ? `${media.width}x${media.height}` : "1920x1080",
        fps,
      },
      export_settings: {
        output_dir: outputDir,
        naming_pattern: "{index}_{name}",
        mode: exportMode,
        aspect_ratio_override: null,
        crop_style: "center_crop",
        crop_x_offset: 0,
        parallel_jobs: 1,
      },
      clips,
    };
  }, [sourcePath, sourceName, duration, media, fps, outputDir, exportMode, clips]);

  const save = useCallback(() => {
    // The project file lives next to the source video (PRD 4.5), so the
    // next open restores it automatically.
    const target = sourcePath
      ? projectPathFor(sourcePath)
      : `${sourceName || "project"}.rustcut`;
    void saveProject(target, buildProjectState())
      .then(() => setStatus(`Project saved to ${target}.`))
      .catch((err: unknown) => setStatus(`Save failed: ${String(err)}`));
  }, [sourcePath, sourceName, buildProjectState]);

  const runExport = useCallback(async () => {
    if (!isTauri) {
      setStatus("Batch export runs in the desktop app.");
      return;
    }
    if (!sourcePath) {
      setStatus("Open a video first.");
      return;
    }
    if (!outputDir) {
      setStatus("Choose an output folder first.");
      return;
    }
    const selected = clips.filter((c) => c.selected_for_export);
    if (selected.length === 0) {
      setStatus("No clips are checked for export.");
      return;
    }
    const state = buildProjectState();
    state.export_settings.output_dir = outputDir;
    const source = {
      width: media?.width ?? 1920,
      height: media?.height ?? 1080,
      bitrate_bps: media?.bitrate_bps ?? 8_000_000,
    };
    try {
      setProgress({});
      setExportSummary("");
      setExporting(true);
      const jobs = await startExport(state, source);
      setStatus(`Exporting ${jobs} clip(s)...`);
    } catch (err) {
      setExporting(false);
      setStatus(`Export failed to start: ${String(err)}`);
    }
  }, [sourcePath, outputDir, clips, buildProjectState, media]);

  // Live export progress from the Rust layer.
  useEffect(() => {
    if (!isTauri) {
      return;
    }
    let unsubscribe: (() => void) | undefined;
    let cancelled = false;
    void onExportEvents(
      (e: ExportProgressEvent) => {
        setProgress((prev) => ({ ...prev, [e.clip_id]: e }));
      },
      (e: ExportFinishedEvent) => {
        setExporting(false);
        const summary = `Export finished: ${e.completed.length} completed, ${e.failed.length} failed, ${e.skipped.length} skipped.`;
        setExportSummary(summary);
        setStatus(e.failed.length > 0 ? `${summary} First error: ${e.failed[0][1]}` : summary);
      },
    ).then((un) => {
      if (cancelled) {
        un();
      } else {
        unsubscribe = un;
      }
    });
    return () => {
      cancelled = true;
      unsubscribe?.();
    };
  }, []);

  const onTimelineClick = useCallback(
    (e: MouseEvent<HTMLDivElement>) => {
      const rect = e.currentTarget.getBoundingClientRect();
      const ratio = rect.width > 0 ? (e.clientX - rect.left) / rect.width : 0;
      seekTo(ratio * duration);
    },
    [duration, seekTo],
  );

  // Keyboard-first workflow (PRD 5). Input fields keep their own keys.
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA")) {
        return;
      }
      const key = e.key;
      switch (key) {
        case " ":
        case "k":
        case "K":
          e.preventDefault();
          togglePlay();
          break;
        case "j":
        case "J":
          seekBy(-5);
          break;
        case "l":
        case "L":
          seekBy(5);
          break;
        case "ArrowLeft":
          e.preventDefault();
          seekBy(-1);
          break;
        case "ArrowRight":
          e.preventDefault();
          seekBy(1);
          break;
        case ",":
          seekBy(-1 / fps);
          break;
        case ".":
          seekBy(1 / fps);
          break;
        case "i":
        case "I":
        case "[":
          setMarkIn(videoRef.current?.currentTime ?? 0);
          setStatus("Mark In set.");
          break;
        case "o":
        case "O":
        case "]":
          setMarkOut(videoRef.current?.currentTime ?? 0);
          setStatus("Mark Out set.");
          break;
        case "Enter":
        case "a":
        case "A":
          addClip();
          break;
        case "n":
        case "N":
          focusLastRename();
          break;
        case "s":
        case "S":
          if (e.ctrlKey || e.metaKey) {
            e.preventDefault();
            save();
          }
          break;
        case "e":
        case "E":
          if (e.ctrlKey || e.metaKey) {
            e.preventDefault();
            void runExport();
          }
          break;
        default:
          break;
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [togglePlay, seekBy, addClip, focusLastRename, save, runExport, fps]);

  const selectedCount = clips.filter((c) => c.selected_for_export).length;
  const progressList = Object.values(progress);
  const doneCount = progressList.filter((p) => p.phase !== "running").length;
  const overallProgress =
    selectedCount > 0 && progressList.length > 0
      ? Math.min(1, progressList.reduce((sum, p) => sum + p.fraction, 0) / selectedCount)
      : 0;

  return (
    <div className="flex h-screen flex-col bg-slate-950 text-slate-100">
      <header className="flex items-center gap-4 border-b border-slate-800 px-4 py-2">
        <span className="text-lg font-semibold tracking-tight">RustCut</span>
        <span className="text-sm text-slate-400">
          {sourceName ? `Project: ${sourceName}.rustcut` : "No project loaded"}
        </span>
        <div className="ml-auto flex items-center gap-2 text-sm">
          {isTauri && (
            <>
              <button className="btn" onClick={openVideo}>
                Open Video
              </button>
              <button
                className="btn"
                onClick={chooseOutput}
                title="Export destination folder"
              >
                {outputDir ? `Out: ${outputDir.split(/[\\/]/).pop()}` : "Output folder"}
              </button>
            </>
          )}
          <label htmlFor="mode" className="text-slate-400">
            Preset
          </label>
          <select
            id="mode"
            value={exportMode}
            onChange={(e) => setExportMode(e.target.value as ExportMode)}
            className="rounded border border-slate-700 bg-slate-900 px-2 py-1"
          >
            <option value="lossless_copy">Lossless (fast copy)</option>
            <option value="smart_reencode">Frame-accurate 9:16</option>
          </select>
          <label htmlFor="speed" className="text-slate-400">
            Speed
          </label>
          <select
            id="speed"
            value={speed}
            onChange={onSpeedChange}
            className="rounded border border-slate-700 bg-slate-900 px-2 py-1"
          >
            {SPEEDS.map((s) => (
              <option key={s} value={s}>
                {s}x
              </option>
            ))}
          </select>
        </div>
      </header>

      <main className="flex min-h-0 flex-1">
        <section className="flex min-w-0 flex-[2] flex-col">
          <div className="flex flex-1 items-center justify-center bg-black">
            {videoUrl ? (
              <video
                ref={videoRef}
                src={videoUrl}
                className="max-h-full max-w-full"
                onTimeUpdate={onTimeUpdate}
                onLoadedMetadata={onLoadedMetadata}
                onPlay={() => setPlaying(true)}
                onPause={() => setPlaying(false)}
                onClick={togglePlay}
              />
            ) : (
              <div className="text-center text-slate-500">
                <p className="mb-3">Open a video file to start clipping</p>
                {isTauri ? (
                  <button className="btn btn-primary" onClick={openVideo}>
                    Open Video
                  </button>
                ) : (
                  <input
                    type="file"
                    accept="video/*"
                    onChange={onFile}
                    className="block text-sm text-slate-400 file:mr-3 file:rounded file:border-0 file:bg-blue-600 file:px-3 file:py-1.5 file:text-white"
                  />
                )}
              </div>
            )}
          </div>

          <div className="flex flex-wrap items-center gap-2 border-t border-slate-800 px-4 py-2 text-sm">
            <button className="btn" onClick={() => seekTo(0)}>
              |&lt;
            </button>
            <button className="btn" onClick={() => seekBy(-5)}>
              -5s
            </button>
            <button className="btn btn-primary" onClick={togglePlay}>
              {playing ? "PAUSE" : "PLAY"}
            </button>
            <button className="btn" onClick={() => seekBy(5)}>
              +5s
            </button>
            <button className="btn" onClick={() => seekTo(duration)}>
              &gt;|
            </button>
            <span className="ml-3 font-mono text-slate-300">
              {fmtTime(current)} / {fmtTime(duration)}
            </span>
            {media && (
              <span className="text-xs text-slate-500">
                {media.width}x{media.height} / {media.codec_name} / {media.fps.toFixed(3)} fps
              </span>
            )}
            <div className="ml-auto flex items-center gap-2">
              <button className="btn" onClick={() => setMarkIn(videoRef.current?.currentTime ?? 0)}>
                Set In (I)
              </button>
              <button className="btn" onClick={() => setMarkOut(videoRef.current?.currentTime ?? 0)}>
                Set Out (O)
              </button>
              <button className="btn btn-primary" onClick={addClip}>
                + Add to Queue (Enter)
              </button>
            </div>
          </div>
        </section>

        <aside className="flex w-80 flex-col border-l border-slate-800">
          <div className="flex items-center justify-between border-b border-slate-800 px-3 py-2">
            <span className="font-semibold">Clip Queue ({clips.length})</span>
            <span className="text-xs text-slate-400">{selectedCount} selected</span>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto">
            {clips.length === 0 ? (
              <p className="p-3 text-sm text-slate-500">
                No clips yet. Press <b>I</b> and <b>O</b> to mark a range, then <b>Enter</b>.
              </p>
            ) : (
              clips.map((clip) => (
                <div
                  key={clip.id}
                  className={`border-b border-slate-800 p-2 text-sm ${
                    loopId === clip.id ? "bg-slate-800" : ""
                  }`}
                >
                  <div className="flex items-center gap-2">
                    <input
                      type="checkbox"
                      checked={clip.selected_for_export}
                      onChange={() => toggleClip(clip.id)}
                    />
                    <span className="font-mono text-xs" style={{ color: clip.color }}>
                      #{String(clip.index).padStart(2, "0")}
                    </span>
                    <input
                      data-clip-id={clip.id}
                      value={clip.name}
                      placeholder={`Clip_${String(clip.index).padStart(2, "0")}`}
                      onChange={(e) => renameClip(clip.id, e.target.value)}
                      className="min-w-0 flex-1 rounded bg-slate-900 px-2 py-1 ring-slate-700 focus:outline-none focus:ring-1"
                    />
                  </div>
                  <div className="mt-1 flex items-center gap-2 text-xs text-slate-400">
                    <button
                      className="font-mono hover:text-slate-200"
                      onClick={() => jumpToClip(clip)}
                      title="Jump to clip"
                    >
                      {fmtTime(clip.start_sec)} {"->"} {fmtTime(clip.end_sec)}
                    </button>
                    <span>{fmtDur(clip.end_sec - clip.start_sec)}</span>
                    <div className="ml-auto flex gap-1">
                      <button
                        className="rounded border border-slate-700 px-1.5 py-0.5 hover:bg-slate-800"
                        onClick={() => toggleLoop(clip.id)}
                        title="Loop preview"
                      >
                        {loopId === clip.id ? "Looping" : "Loop"}
                      </button>
                      <button
                        className="rounded border border-slate-700 px-1.5 py-0.5 hover:bg-slate-800"
                        onClick={() => removeClip(clip.id)}
                        title="Remove marker (source video untouched)"
                      >
                        Del
                      </button>
                    </div>
                  </div>
                  <div className="mt-1 flex items-center gap-2 text-xs text-slate-400">
                    <span title="9:16 crop window position (re-encode mode)">Crop X</span>
                    <input
                      type="range"
                      min={-1}
                      max={1}
                      step={0.05}
                      value={clip.crop_x_offset}
                      onChange={(e) => setCropOffset(clip.id, Number(e.target.value))}
                      className="w-20 accent-blue-500"
                      title="-1 = left edge, 0 = center, +1 = right edge"
                    />
                    <span className="w-8 text-right font-mono">
                      {clip.crop_x_offset.toFixed(2)}
                    </span>
                  </div>
                </div>
              ))
            )}
          </div>
          <div className="border-t border-slate-800 p-3">
            <button
              onClick={() => void runExport()}
              disabled={!isTauri || exporting || selectedCount === 0}
              className="w-full rounded bg-blue-600 py-2 font-semibold text-white disabled:opacity-50"
              title={isTauri ? "Export all checked clips" : "Batch export runs in the desktop app"}
            >
              {exporting ? "Exporting..." : `Export All (${selectedCount})`}
            </button>
            {exporting && (
              <div className="mt-2">
                <div className="h-2 rounded bg-slate-800">
                  <div
                    className="h-full rounded bg-blue-500"
                    style={{ width: `${Math.round(overallProgress * 100)}%` }}
                  />
                </div>
                <div className="mt-1 text-xs text-slate-400">
                  {Math.round(overallProgress * 100)}% ({doneCount}/{selectedCount})
                </div>
              </div>
            )}
            {exportSummary && <div className="mt-2 text-xs text-slate-400">{exportSummary}</div>}
          </div>
        </aside>
      </main>

      <footer className="border-t border-slate-800 px-4 py-2">
        <div className="mb-2 text-xs text-slate-400">
          Timeline - full duration {fmtTime(duration)}
          {markIn !== null ? ` | In ${fmtTime(markIn)}` : ""}
          {markOut !== null ? ` | Out ${fmtTime(markOut)}` : ""}
        </div>
        <div
          onClick={onTimelineClick}
          className="relative h-12 cursor-pointer rounded bg-slate-900"
          title="Click to seek"
        >
          {clips.map((clip) => (
            <div
              key={clip.id}
              className="absolute top-0 h-full rounded-sm opacity-80"
              style={{
                left: `${(clip.start_sec / (duration || 1)) * 100}%`,
                width: `${Math.max(0.5, ((clip.end_sec - clip.start_sec) / (duration || 1)) * 100)}%`,
                backgroundColor: clip.color,
              }}
              title={`#${clip.index} ${clip.name || "(unnamed)"}`}
            />
          ))}
          {markIn !== null && (
            <div
              className="absolute top-0 h-full w-0.5 bg-emerald-400"
              style={{ left: `${(markIn / (duration || 1)) * 100}%` }}
            />
          )}
          {markOut !== null && (
            <div
              className="absolute top-0 h-full w-0.5 bg-rose-400"
              style={{ left: `${(markOut / (duration || 1)) * 100}%` }}
            />
          )}
          <div
            className="absolute top-0 h-full w-0.5 bg-white"
            style={{ left: `${(current / (duration || 1)) * 100}%` }}
          />
        </div>
        <div className="mt-2 text-xs text-slate-500">{status}</div>
      </footer>
    </div>
  );
}
