import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ChangeEvent, MouseEvent, PointerEvent as ReactPointerEvent } from "react";
import {
  isTauri,
  loadProject,
  onExportEvents,
  openVideoDialog,
  pickOutputFolder,
  onPreviewProgress,
  preparePreview,
  previewFilename,
  detectEncoders,
  canvasInfo,
  probeMedia,
  saveProject,
  startExport,
  waveformPeaks,
} from "./api";
import type {
  Clip,
  ExportFinishedEvent,
  ExportProgressEvent,
  MediaInfo,
  ProjectState,
} from "./types";
import type { SourceSpec } from "./api";

// ffprobe supplies the real frame rate in the Tauri build; frame stepping
// falls back to 30 fps until then.
const FPS_FALLBACK = 30;
const COLORS = ["#3B82F6", "#EF4444", "#22C55E", "#F59E0B", "#A855F7", "#06B6D4"];
const SPEEDS = [0.5, 1, 1.25, 1.5, 2, 3];

// Preview proxy resolution. `fast` (720p) keeps playback smooth when WebView2
// composites in software (RDP sessions, weak iGPUs); `high` keeps 1080p.
type PreviewQuality = "fast" | "high";
const PREVIEW_QUALITY_KEY = "rustcut.previewQuality";

// Guards the dev-only ?devvideo deep link against React StrictMode's
// double-mounted effect.
let devVideoRequested = false;

type Preset = "lossless_copy" | "smart_reencode" | "blurred_background";

// A 9:16 window on a 16:9 box spans (9/16)^2 of that box's width.
const OVERLAY_W_PCT = (9 / 16) ** 2 * 100;

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
  const waveformRef = useRef<HTMLCanvasElement | null>(null);
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const [videoUrl, setVideoUrl] = useState<string | null>(null);
  const [sourcePath, setSourcePath] = useState("");
  const [sourceName, setSourceName] = useState("");
  const [media, setMedia] = useState<MediaInfo | null>(null);
  const [outputDir, setOutputDir] = useState("");
  const [preset, setPreset] = useState<Preset>("lossless_copy");
  const [progress, setProgress] = useState<Record<string, ExportProgressEvent>>({});
  const [peaks, setPeaks] = useState<number[]>([]);
  const [selectedClipId, setSelectedClipId] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
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
  const [previewQuality, setPreviewQuality] = useState<PreviewQuality>(() =>
    localStorage.getItem(PREVIEW_QUALITY_KEY) === "high" ? "high" : "fast",
  );
  const qualityRef = useRef<PreviewQuality>(previewQuality);
  const [namingPattern, setNamingPattern] = useState("{index}_{name}");
  const [namePreview, setNamePreview] = useState("");
  const [lastSaved, setLastSaved] = useState("");
  const [query, setQuery] = useState("");
  const [sortBy, setSortBy] = useState<"index" | "start" | "duration" | "name">("index");
  const [encoders, setEncoders] = useState<
    { id: string; label: string; available: boolean; hardware: boolean }[]
  >([]);
  const [encoderId, setEncoderId] = useState("auto");
  const [workers, setWorkers] = useState(1);
  const [canvas, setCanvas] = useState<{ canvas_w: number; canvas_h: number; crop_w: number } | null>(
    null,
  );
  const [stage, setStage] = useState({ w: 0, h: 0 });
  const videoPaneRef = useRef<HTMLDivElement | null>(null);
  const timelineRef = useRef<HTMLDivElement | null>(null);
  const trimRef = useRef<{ id: string; edge: "in" | "out"; startSec: number; endSec: number } | null>(
    null,
  );

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

  const loadPath = useCallback(async (path: string, directUrl: string) => {
    setVideoUrl(null);
    setSourcePath(path);
    setSourceName(path.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, "") ?? "");
    setClips([]);
    setProgress({});
    setMedia(null);
    setPeaks([]);
    setSelectedClipId(null);
    setStatus("Preparing preview…");

    // HEVC, 10-bit and Matroska sources cannot be decoded by the embedded
    // player; the backend builds a cached H.264 proxy for those. The source
    // file is never modified, exports still read the original.
    let playableUrl = directUrl;
    let previewNote = "";
    const unsubscribe = await onPreviewProgress((fraction) => {
      setStatus(`Preparing preview… ${Math.round(fraction * 100)}%`);
    });
    try {
      const preview = await preparePreview(path, qualityRef.current);
      playableUrl = preview.preview_url;
      if (preview.transcoded) {
        previewNote = ` ${preview.source_codec.toUpperCase()} source: previewing a cached ${preview.quality} H.264 copy${
          preview.encoder ? ` (${preview.encoder})` : ""
        }${preview.cached ? ", already built" : ", built now"}.`;
      }
    } catch (err) {
      previewNote = ` Preview could not be prepared: ${String(err)}.`;
    } finally {
      unsubscribe();
    }
    setVideoUrl(playableUrl);

    let mediaLine = "";
    try {
      const info = await probeMedia(path);
      setMedia(info);
      setDuration(info.duration_sec);
      mediaLine = `Loaded ${info.width}x${info.height}, ${info.fps.toFixed(3)} fps, ${info.codec_name}.`;
    } catch (err) {
      mediaLine = `Video opened, but ffprobe failed: ${String(err)}`;
    }
    mediaLine = `${mediaLine}${previewNote}`;
    // Waveform for the timeline (PRD 4.1); audio-less files just skip it.
    if (isTauri) {
      void waveformPeaks(path, 1200)
        .then(setPeaks)
        .catch(() => setPeaks([]));
    }
    // Crash recovery (PRD 4.5): restore saved markers when the .rustcut
    // file for this video exists; a missing file is the normal case.
    try {
      const saved = await loadProject(projectPathFor(path));
      setClips(saved.clips);
      setPreset(
        saved.export_settings.mode === "lossless_copy"
          ? "lossless_copy"
          : saved.export_settings.crop_style === "blurred_background"
            ? "blurred_background"
            : "smart_reencode",
      );
      setOutputDir(saved.export_settings.output_dir || "");
      setNamingPattern(saved.export_settings.naming_pattern || "{index}_{name}");
      setWorkers(saved.export_settings.parallel_jobs || 1);
      setEncoderId(saved.export_settings.encoder || "auto");
      setDuration((d) => d || saved.source_video.duration_sec || 0);
      setStatus(`${mediaLine} Restored ${saved.clips.length} saved clip(s) from the project file.`);
    } catch {
      setStatus(mediaLine);
    }
  }, []);

  const openVideo = useCallback(async () => {
    try {
      const picked = await openVideoDialog();
      if (!picked) {
        return;
      }
      await loadPath(picked.path, picked.url);
    } catch (err) {
      setStatus(`Open failed: ${String(err)}`);
    }
  }, [loadPath]);

  const onQualityChange = useCallback(
    (value: PreviewQuality) => {
      qualityRef.current = value;
      setPreviewQuality(value);
      localStorage.setItem(PREVIEW_QUALITY_KEY, value);
      // Rebuild/reload the current file at the new quality immediately.
      if (sourcePath) {
        void loadPath(sourcePath, "");
      }
    },
    [sourcePath, loadPath],
  );

  // Dev-only deep link: /?devvideo=<absolute path> loads a file without the
  // native dialog, so the player can be exercised in automated runs. The
  // module-level flag keeps React StrictMode's double mount from loading the
  // same file twice.
  useEffect(() => {
    if (!import.meta.env.DEV || devVideoRequested) {
      return;
    }
    const devPath = new URLSearchParams(window.location.search).get("devvideo");
    if (devPath) {
      devVideoRequested = true;
      void loadPath(devPath, "");
    }
  }, [loadPath]);

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
    setSelectedClipId(clip.id);
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
    setSelectedClipId((prev) => (prev === id ? null : prev));
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
        naming_pattern: namingPattern,
        mode: preset === "lossless_copy" ? "lossless_copy" : "smart_reencode",
        aspect_ratio_override: null,
        crop_style: preset === "blurred_background" ? "blurred_background" : "center_crop",
        crop_x_offset: 0,
        parallel_jobs: workers,
        encoder: encoderId,
      },
      clips,
    };
  }, [sourcePath, sourceName, duration, media, fps, outputDir, preset, namingPattern, workers, encoderId, clips]);

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

  // PRD 4.5: every marker edit is written to the project file automatically.
  // Debounced so typing a clip name does not write on each keystroke; the
  // serialized state is compared first so no-op renders never touch the disk.
  const lastSavedJson = useRef("");
  useEffect(() => {
    if (!isTauri || !sourcePath) {
      return;
    }
    const state = buildProjectState();
    const json = JSON.stringify(state);
    if (json === lastSavedJson.current) {
      return;
    }
    const timer = setTimeout(() => {
      void saveProject(projectPathFor(sourcePath), state)
        .then(() => {
          lastSavedJson.current = json;
          const now = new Date();
          setLastSaved(
            `${String(now.getHours()).padStart(2, "0")}:${String(now.getMinutes()).padStart(2, "0")}`,
          );
        })
        .catch(() => {
          /* autosave is best-effort; the status line stays for real errors */
        });
    }, 700);
    return () => clearTimeout(timer);
  }, [sourcePath, buildProjectState]);

  // Filename preview uses the same Rust naming code as the export planner.
  useEffect(() => {
    if (!isTauri) {
      return;
    }
    const clip = clips.find((c) => c.selected_for_export) ?? clips[0];
    void previewFilename({
      pattern: namingPattern || "{index}_{name}",
      index: clip?.index ?? 1,
      clipName: clip?.name || "Clip_01",
      sourceName: sourceName || "source",
      startSec: clip?.start_sec ?? 0,
      endSec: clip?.end_sec ?? 10,
    })
      .then(setNamePreview)
      .catch(() => setNamePreview(""));
  }, [namingPattern, clips, sourceName]);

  // Probe which encoders actually work once, so the panel can offer hardware
  // acceleration only where it exists (PRD 4.4).
  useEffect(() => {
    if (!isTauri) {
      return;
    }
    void detectEncoders()
      .then(setEncoders)
      .catch(() => setEncoders([]));
  }, []);

  // Working-canvas dimensions come from the same Rust function the export
  // filter uses, so the preview cannot disagree with the output (PRD 4.4).
  useEffect(() => {
    if (!isTauri || !media || !media.width || !media.height) {
      setCanvas(null);
      return;
    }
    void canvasInfo(media.width, media.height)
      .then(setCanvas)
      .catch(() => setCanvas(null));
  }, [media]);

  // Size the canvas stage to fit the player pane while keeping its aspect.
  useEffect(() => {
    const el = videoPaneRef.current;
    if (!el || !canvas) {
      setStage({ w: 0, h: 0 });
      return;
    }
    const compute = () => {
      const { width, height } = el.getBoundingClientRect();
      const aspect = canvas.canvas_w / canvas.canvas_h;
      const w = Math.min(width, height * aspect);
      setStage({ w: Math.max(0, Math.floor(w)), h: Math.max(0, Math.floor(w / aspect)) });
    };
    compute();
    const observer = new ResizeObserver(compute);
    observer.observe(el);
    return () => observer.disconnect();
  }, [canvas, videoUrl]);

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
    const source: SourceSpec = {
      width: media?.width ?? 1920,
      height: media?.height ?? 1080,
      bitrate_bps: media?.bitrate_bps ?? 8_000_000,
    };
    try {
      setProgress({});
      setExportSummary("");
      setExporting(true);
      const jobs = await startExport(state, source, encoderId, workers);
      setStatus(`Exporting ${jobs} clip(s)...`);
    } catch (err) {
      setExporting(false);
      setStatus(`Export failed to start: ${String(err)}`);
    }
  }, [sourcePath, outputDir, clips, buildProjectState, media, encoderId, workers]);

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

  const onOverlayDrag = useCallback(
    (e: MouseEvent<HTMLDivElement>) => {
      if (!dragging || !selectedClipId || !wrapperRef.current) {
        return;
      }
      const rect = wrapperRef.current.getBoundingClientRect();
      const useCanvas = canvas !== null && preset !== "lossless_copy";
      const cropFraction =
        useCanvas && canvas ? canvas.crop_w / canvas.canvas_w : OVERLAY_W_PCT / 100;
      const usable = rect.width * (1 - cropFraction);
      if (usable <= 0) {
        return;
      }
      const x = Math.min(
        Math.max(0, e.clientX - rect.left - (rect.width * cropFraction) / 2),
        usable,
      );
      setCropOffset(selectedClipId, (x / usable) * 2 - 1);
    },
    [dragging, selectedClipId, setCropOffset, canvas, preset],
  );

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

  // Draw the audio waveform on the timeline canvas (PRD 4.1).
  useEffect(() => {
    const canvas = waveformRef.current;
    if (!canvas || peaks.length === 0) {
      return;
    }
    const ctx = canvas.getContext("2d");
    if (!ctx) {
      return;
    }
    const { width, height } = canvas;
    ctx.clearRect(0, 0, width, height);
    ctx.fillStyle = "#3B82F6";
    peaks.forEach((p, i) => {
      const h = Math.max(1, p * height);
      ctx.fillRect(i, (height - h) / 2, 1, h);
    });
  }, [peaks]);

  const updateClipRange = useCallback(
    (id: string, startSec: number, endSec: number) => {
      const total = duration || 0;
      if (total <= 0) {
        return;
      }
      const minLen = 0.2;
      let start = Math.max(0, Math.min(startSec, total - minLen));
      let end = Math.min(total, Math.max(endSec, start + minLen));
      if (end - start < minLen) {
        start = Math.max(0, end - minLen);
      }
      setClips((prev) =>
        prev.map((c) => (c.id === id ? { ...c, start_sec: start, end_sec: end } : c)),
      );
    },
    [duration],
  );

  const beginTrim = useCallback((e: ReactPointerEvent, clip: Clip, edge: "in" | "out") => {
    e.preventDefault();
    e.stopPropagation();
    try {
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    } catch {
      // Synthetic pointers (tests) have no capture target; dragging still works.
    }
    trimRef.current = { id: clip.id, edge, startSec: clip.start_sec, endSec: clip.end_sec };
  }, []);

  const moveTrim = useCallback(
    (e: ReactPointerEvent) => {
      const drag = trimRef.current;
      const rect = timelineRef.current?.getBoundingClientRect();
      if (!drag || !rect || rect.width === 0 || !duration) {
        return;
      }
      const fraction = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
      const sec = fraction * duration;
      if (drag.edge === "in") {
        updateClipRange(drag.id, sec, drag.endSec);
      } else {
        updateClipRange(drag.id, drag.startSec, sec);
      }
    },
    [duration, updateClipRange],
  );

  const endTrim = useCallback(() => {
    trimRef.current = null;
  }, []);

  const setClipColor = useCallback((id: string, color: string) => {
    setClips((prev) => prev.map((c) => (c.id === id ? { ...c, color } : c)));
  }, []);

  const visibleClips = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = q
      ? clips.filter(
          (c) => (c.name || "").toLowerCase().includes(q) || String(c.index) === q,
        )
      : [...clips];
    switch (sortBy) {
      case "start":
        list.sort((a, b) => a.start_sec - b.start_sec);
        break;
      case "duration":
        list.sort((a, b) => b.end_sec - b.start_sec - (a.end_sec - a.start_sec));
        break;
      case "name":
        list.sort((a, b) => (a.name || "").localeCompare(b.name || ""));
        break;
      default:
        list.sort((a, b) => a.index - b.index);
    }
    return list;
  }, [clips, query, sortBy]);

  const selectedCount = clips.filter((c) => c.selected_for_export).length;
  const selectedClip = clips.find((c) => c.id === selectedClipId) ?? null;
  const overlayVisible = preset === "smart_reencode" && selectedClip !== null;
  const cropOffset = selectedClip?.crop_x_offset ?? 0;
  // WYSIWYG geometry: the stage is the 16:9 working canvas and the overlay is
  // the exact 9:16 window the export filter will cut (PRD 4.4).
  const usingCanvas = canvas !== null && preset !== "lossless_copy" && stage.w > 0;
  const cropWidthPct =
    usingCanvas && canvas ? (canvas.crop_w / canvas.canvas_w) * 100 : OVERLAY_W_PCT;
  const cropLeftPct =
    usingCanvas && canvas
      ? ((((canvas.canvas_w - canvas.crop_w) * (1 + cropOffset)) / 2) / canvas.canvas_w) * 100
      : ((cropOffset + 1) / 2) * (100 - OVERLAY_W_PCT);
  const progressList = Object.values(progress);
  const doneCount = progressList.filter((p) => p.phase !== "running").length;
  const overallProgress =
    selectedCount > 0 && progressList.length > 0
      ? Math.min(1, progressList.reduce((sum, p) => sum + p.fraction, 0) / selectedCount)
      : 0;

  return (
    <div className="flex h-screen min-h-0 flex-col overflow-hidden bg-slate-950 text-slate-100">
      <header className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-2 border-b border-slate-800 px-4 py-2">
        <span className="shrink-0 text-lg font-semibold tracking-tight">RustCut</span>
        <span className="min-w-0 flex-1 truncate text-sm text-slate-400">
          {sourceName ? `Project: ${sourceName}.rustcut` : "No project loaded"}
        </span>
        {lastSaved && (
          <span className="shrink-0 text-xs text-slate-500" title="Project file written automatically">
            auto-saved {lastSaved}
          </span>
        )}
        <div className="flex flex-wrap items-center justify-end gap-2 text-sm">
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
          <label htmlFor="preview-quality" className="text-slate-400">
            Preview
          </label>
          <select
            id="preview-quality"
            value={previewQuality}
            onChange={(e) => onQualityChange(e.target.value as PreviewQuality)}
            className="rounded border border-slate-700 bg-slate-900 px-2 py-1"
            title="Proxy resolution for the player; exports always use the original"
          >
            <option value="fast">720p (smooth)</option>
            <option value="high">1080p (sharp)</option>
          </select>
          <label htmlFor="mode" className="text-slate-400">
            Preset
          </label>
          <select
            id="mode"
            value={preset}
            onChange={(e) => setPreset(e.target.value as Preset)}
            className="rounded border border-slate-700 bg-slate-900 px-2 py-1"
          >
            <option value="lossless_copy">Lossless (fast copy)</option>
            <option value="smart_reencode">9:16 Center Crop</option>
            <option value="blurred_background">9:16 Blurred Background</option>
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
        <section className="flex min-h-0 min-w-0 flex-[2] flex-col">
          <div
            ref={videoPaneRef}
            className="flex min-h-0 flex-1 items-center justify-center overflow-hidden bg-black"
          >
            {videoUrl ? (
              <div
                ref={wrapperRef}
                className={
                  usingCanvas
                    ? "relative bg-black ring-1 ring-slate-800"
                    : "relative inline-flex max-h-full max-w-full"
                }
                style={usingCanvas ? { width: stage.w, height: stage.h } : undefined}
                onPointerMove={onOverlayDrag}
                onPointerUp={() => setDragging(false)}
                onPointerLeave={() => setDragging(false)}
              >
                <video
                  ref={videoRef}
                  src={videoUrl}
                  className={
                    usingCanvas
                      ? "h-full w-full object-contain"
                      : "max-h-full max-w-full object-contain"
                  }
                  onTimeUpdate={onTimeUpdate}
                  onLoadedMetadata={onLoadedMetadata}
                  onPlay={() => setPlaying(true)}
                  onPause={() => setPlaying(false)}
                  onClick={togglePlay}
                  onError={() => {
                    const code = videoRef.current?.error?.code;
                    const labels: Record<number, string> = {
                      1: "loading was aborted",
                      2: "network error while reading the file",
                      3: "the decoder could not read the video stream",
                      4: "this format is not supported by the embedded player",
                    };
                    setStatus(
                      `Preview failed: ${labels[code ?? 0] ?? "unknown media error"}. The source itself may still export correctly.`,
                    );
                  }}
                />
                {overlayVisible && (
                  <div
                    className="absolute top-0 h-full cursor-ew-resize border-2 border-white/80 bg-white/10"
                    style={{
                      left: `${cropLeftPct}%`,
                      width: `${cropWidthPct}%`,
                    }}
                    onPointerDown={(e) => {
                      e.preventDefault();
                      setDragging(true);
                    }}
                    title="Drag to set the 9:16 crop position for the selected clip"
                  />
                )}
              </div>
            ) : (
              <div className="text-center text-slate-500">
                {sourcePath ? (
                  <p className="mb-3">Preparing preview…</p>
                ) : (
                  <>
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
                  </>
                )}
              </div>
            )}
          </div>

          <div className="flex shrink-0 flex-wrap items-center gap-2 border-t border-slate-800 px-4 py-2 text-sm">
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

        <aside className="flex w-80 shrink-0 flex-col border-l border-slate-800 max-[1100px]:w-72">
          <div className="flex items-center justify-between border-b border-slate-800 px-3 py-2">
            <span className="font-semibold">Clip Queue ({clips.length})</span>
            <span className="text-xs text-slate-400">{selectedCount} selected</span>
          </div>
          <div className="flex gap-2 border-b border-slate-800 px-3 py-2">
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search clips…"
              className="min-w-0 flex-1 rounded bg-slate-900 px-2 py-1 text-xs ring-slate-700 focus:outline-none focus:ring-1"
            />
            <select
              value={sortBy}
              onChange={(e) => setSortBy(e.target.value as typeof sortBy)}
              title="Sort the queue"
              className="rounded border border-slate-700 bg-slate-900 px-1 py-1 text-xs"
            >
              <option value="index">Queue order</option>
              <option value="start">Start time</option>
              <option value="duration">Duration</option>
              <option value="name">Name</option>
            </select>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto">
            {clips.length === 0 ? (
              <p className="p-3 text-sm text-slate-500">
                No clips yet. Press <b>I</b> and <b>O</b> to mark a range, then <b>Enter</b>.
              </p>
            ) : (
              visibleClips.map((clip) => (
                <div
                  key={clip.id}
                  onClick={() => setSelectedClipId(clip.id)}
                  className={`cursor-pointer border-b border-slate-800 p-2 text-sm ${
                    loopId === clip.id ? "bg-slate-800" : ""
                  } ${selectedClipId === clip.id ? "ring-1 ring-blue-500" : ""}`}
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
                      type="color"
                      value={clip.color}
                      onChange={(e) => setClipColor(clip.id, e.target.value)}
                      title="Clip color tag"
                      className="h-4 w-4 shrink-0 cursor-pointer rounded border border-slate-700 bg-transparent p-0"
                    />
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
            <div className="mb-2 flex gap-2">
              <label className="min-w-0 flex-1 text-xs text-slate-400">
                Encoder
                <select
                  value={encoderId}
                  onChange={(e) => setEncoderId(e.target.value)}
                  className="mt-1 w-full rounded border border-slate-700 bg-slate-900 px-1 py-1 text-xs"
                  title="Hardware encoders are only listed when they actually work on this machine"
                >
                  <option value="auto">Auto (best available)</option>
                  {encoders.map((enc) => (
                    <option key={enc.id} value={enc.id} disabled={!enc.available}>
                      {enc.label}
                      {enc.available ? "" : " - unavailable"}
                    </option>
                  ))}
                </select>
              </label>
              <label className="w-20 shrink-0 text-xs text-slate-400">
                Jobs
                <select
                  value={workers}
                  onChange={(e) => setWorkers(Number(e.target.value))}
                  className="mt-1 w-full rounded border border-slate-700 bg-slate-900 px-1 py-1 text-xs"
                  title="1 = sequential, 2-4 = parallel exports"
                >
                  {[1, 2, 3, 4].map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </select>
              </label>
            </div>
            <label htmlFor="naming" className="mb-1 block text-xs text-slate-400">
              File name template
            </label>
            <input
              id="naming"
              value={namingPattern}
              onChange={(e) => setNamingPattern(e.target.value)}
              spellCheck={false}
              placeholder="{index}_{name}"
              title="Variables: {index} {name} {clip_name} {source_name} {start_time} {end_time} {duration}"
              className="w-full rounded bg-slate-900 px-2 py-1 font-mono text-xs ring-slate-700 focus:outline-none focus:ring-1"
            />
            <div className="mb-2 mt-1 truncate font-mono text-[11px] text-slate-500" title={namePreview}>
              {namePreview ? `→ ${namePreview}` : "→ 01_Clip_01.mp4"}
            </div>
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

      <footer className="shrink-0 border-t border-slate-800 px-4 py-2">
        <div className="mb-2 text-xs text-slate-400">
          Timeline - full duration {fmtTime(duration)}
          {markIn !== null ? ` | In ${fmtTime(markIn)}` : ""}
          {markOut !== null ? ` | Out ${fmtTime(markOut)}` : ""}
        </div>
        {peaks.length > 0 && (
          <canvas
            ref={waveformRef}
            width={peaks.length}
            height={64}
            className="mb-1 h-10 w-full rounded bg-slate-900"
            title="Audio waveform"
          />
        )}
        <div
          ref={timelineRef}
          onClick={onTimelineClick}
          onPointerMove={moveTrim}
          onPointerUp={endTrim}
          onPointerLeave={endTrim}
          className="relative h-12 cursor-pointer rounded bg-slate-900"
          title="Click to seek; drag a clip edge to trim it"
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
            >
              <div
                className="absolute left-0 top-0 h-full w-1.5 cursor-ew-resize bg-white/80 hover:w-2"
                onPointerDown={(e) => beginTrim(e, clip, "in")}
                onClick={(e) => e.stopPropagation()}
                title="Drag to trim the clip start"
              />
              <div
                className="absolute right-0 top-0 h-full w-1.5 cursor-ew-resize bg-white/80 hover:w-2"
                onPointerDown={(e) => beginTrim(e, clip, "out")}
                onClick={(e) => e.stopPropagation()}
                title="Drag to trim the clip end"
              />
            </div>
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
        <div className="mt-2 truncate text-xs text-slate-500" title={status}>
          {status}
        </div>
      </footer>
    </div>
  );
}
