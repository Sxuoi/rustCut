// PRD 8.1 / AC-14. Opt-in desktop measurements; never imported by the test runner.
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, renameSync, writeFileSync,
  openSync, readSync, writeSync, closeSync, ftruncateSync, createReadStream, createWriteStream, linkSync } from "node:fs";
import { pipeline } from "node:stream/promises";
import { basename, delimiter, dirname, join, resolve, sep } from "node:path";

const args = new Set(process.argv.slice(2));
const allowed = new Set(["--quick", "--release", "--help"]);
for (const arg of args) if (!allowed.has(arg)) throw new Error(`Unknown option: ${arg}`);
if (args.has("--help")) {
  console.log("Usage: npm.cmd run perf -- [--quick] [--release]\nWindows only. Close RustCut first. See docs/PERFORMANCE.md. No app is launched by --help.");
} else {
  await main();
}

async function main() {
  if (process.platform !== "win32") throw new Error("The CDP harness and Win32 process counters require Windows.");
  const { App, APP_EXE, FFPROBE, ROOT, cleanup, handOffToWindow, makeWorkspace, sleep, startMeasured } = await import("../tests/e2e/harness.mjs");
  const { waitFor, waitReady, readProject } = await import("../tests/e2e/ui.mjs");
  const exec = promisify(execFile);
  const ps = async (source) => (await exec("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", source],
    { windowsHide: true, maxBuffer: 8 * 1024 * 1024 })).stdout.trim();
  // Single-instance hand-off would measure somebody else's window. Never terminate it.
  if (await ps("@(Get-CimInstance Win32_Process -Filter \"Name='rustcut.exe'\").Count" ) !== "0") {
    throw new Error("Close the existing RustCut instance before measuring (it has not been touched).");
  }
  const quick = args.has("--quick");
  const executable = args.has("--release") ? join(process.env.CARGO_TARGET_DIR ?? join(ROOT, "target"), "release", "rustcut.exe") : APP_EXE;
  if (!existsSync(executable)) throw new Error(`Build an embedded frontend first: ${executable}`);
  const directory = join(ROOT, ".fixtures", "perf");
  mkdirSync(directory, { recursive: true });
  const candidates = [process.env.RUSTCUT_FFMPEG_DIR, process.env.FFMPEG_DIR, dirname(FFPROBE),
    ...(process.env.PATH ?? "").split(delimiter)].filter(Boolean);
  function tool(name) {
    for (const dir of candidates) {
      const names = existsSync(dir) && statSync(dir).isDirectory() ? readdirSync(dir).filter((f) => f === `${name}.exe` || (f.startsWith(`${name}-`) && f.endsWith(".exe"))) : [];
      if (names.length) return join(dir, names[0]);
    }
    throw new Error(`${name}.exe not found; set RUSTCUT_FFMPEG_DIR or PATH.`);
  }
  const ffmpeg = tool("ffmpeg");
  const ffprobe = tool("ffprobe");
  const probe = async (file) => JSON.parse((await exec(ffprobe, ["-v", "error", "-show_format", "-show_streams", "-of", "json", file])).stdout);
  async function generate(file, options) {
    if (existsSync(file)) return;
    console.error(`Generating ${file} (excluded from timings)`);
    const partial = `${file}.part.mp4`;
    await new Promise((resolve, reject) => {
      const child = spawn(ffmpeg, ["-hide_banner", "-loglevel", "error", "-nostdin", "-y", ...options, partial],
        { windowsHide: true, stdio: ["ignore", "ignore", "pipe"] });
      let tail = "";
      child.stderr.on("data", (data) => { tail = (tail + data).slice(-6000); });
      child.once("error", reject);
      child.once("exit", (code) => code === 0 ? resolve() : reject(new Error(`ffmpeg exit ${code}: ${tail}`)));
    });
    renameSync(partial, file);
  }

  const prefix = quick ? "quick-v1" : "full-v1";
  let seed = join(directory, `${prefix}-seed.mp4`);
  let source = join(directory, `${prefix}-source.mp4`);
  let waveform = join(directory, `${prefix}-waveform.mp4`);
  let padded = join(directory, `${prefix}-tail-index.mp4`);
  const sourceSeconds = quick ? 60 : 10800;
  const clipSeconds = quick ? 6 : 60;
  const seedSeconds = quick ? 6 : 60;
  const waveformSeconds = quick ? 120 : 10800;
  const dimensions = quick ? "640x360" : "1920x1080";
  const fps = quick ? 30 : 60;
  await generate(seed, ["-f", "lavfi", "-i", `testsrc2=size=${dimensions}:rate=${fps}`,
    "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000", "-t", String(seedSeconds),
    "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-g", String(2 * fps),
    "-b:v", quick ? "1M" : "8M", "-minrate", quick ? "1M" : "8M", "-maxrate", quick ? "1M" : "8M",
    "-bufsize", "16M", "-x264-params", "nal-hrd=cbr:force-cfr=1", "-c:a", "aac", "-b:a", "128k"]);
  // Stream-copy a repeated seed: full mode has a real three-hour sample index without three hours of encoding.
  await generate(source, ["-stream_loop", String(sourceSeconds / seedSeconds - 1), "-i", seed,
    "-t", String(sourceSeconds), "-map", "0", "-c", "copy"]);
  await generate(waveform, ["-f", "lavfi", "-i", "color=c=gray:size=160x90:rate=1",
    "-f", "lavfi", "-i", "sine=frequency=330:sample_rate=48000", "-t", String(waveformSeconds),
    "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac", "-b:a", "64k"]);
  await padTailIndex(source, padded, quick ? 128 * 1024 ** 2 : 20_000_000_000, exec);
  const metadata = await probe(source);
  const stream = metadata.streams.find((s) => s.codec_type === "video");
  if (stream.width !== Number(dimensions.split("x")[0]) || Math.abs(Number(metadata.format.duration) - sourceSeconds) > 1) {
    throw new Error(`Cached fixture has unexpected dimensions/duration: ${source}`);
  }
  const rows = [
    ["cold_start_ms", "Cold start to interactive", "≤1500 ms", 1500],
    ["open_ms", "3 h / 20 GB MP4 to first frame", "≤2000 ms", 2000],
    ["open_read_bytes", "AC-14 reads before first frame (rustcut.exe + FFmpeg)", "<50 MB", 50_000_000],
    ["open_tree_read_bytes", "AC-14 process-tree reads at first frame (incl. WebView2)", "INFO", null],
    ["seek_p50_ms", "Seek p50", "INFO", null],
    ["seek_p95_ms", "Seek p95", "≤300 ms", 300],
    ["step_forward_ms", "Frame step forward p95", "≤100 ms", 100],
    ["step_backward_ms", "Frame step backward p95", "≤500 ms", 500],
    ["mark_capture", "Paused mark equals presented frame", "Exact frame; no IPC before capture", null],
    ["timeline_fps", "Timeline with 500 clips and waveform", "≥60 fps", 60, "min"],
    ["core_private_bytes", "Rust core private bytes", "≤100 MB", 100_000_000],
    ["tree_private_bytes", "Playback process-tree private bytes", "≤400 MB", 400_000_000],
    ["large_tree_private_bytes", "AC-14 large-file playback private bytes", "≤400 MB; flat vs small file", 400_000_000],
    ["large_core_private_bytes", "AC-14 large-file Rust core private bytes", "≤100 MB", 100_000_000],
    ["lossless_ms", "Lossless 10 × 60 s incl. process start", "<10000 ms", 10000],
    ["vertical_software_x", "Vertical software 1080×1920 60 fps", "≥1× realtime", 1, "min"],
    ["vertical_hardware_x", "Vertical hardware 1080×1920 60 fps", "≥4× realtime", 4, "min"],
    ["waveform_first_ms", "Waveform first nonempty chunk", "≤5000 ms", 5000],
    ["waveform_total_ms", "Waveform full 3 h track", "≤60000 ms", 60000],
    ["autosave_ms", "Auto-save to disk", "≤1000 ms", 1000],
  ].map(([id, metric, target, limit, direction = "max"]) => ({ id, metric, target, limit, direction, status: "INFO", value: null, detail: "Not measured" }));
  function record(id, value, detail = "", eligible = !quick) {
    const row = rows.find((r) => r.id === id);
    const passed = row.limit === null ? null : row.direction === "min" ? value >= row.limit :
      ["open_read_bytes", "lossless_ms"].includes(id) ? value < row.limit : value <= row.limit;
    Object.assign(row, { value, status: eligible && passed !== null ? (passed ? "PASS" : "FAIL") : "INFO", detail });
  }
  async function stage(ids, action) {
    try { await action(); }
    catch (error) {
      console.error(error);
      for (const id of ids) Object.assign(rows.find((r) => r.id === id), { status: "FAIL", detail: String(error) });
    }
  }
  const ws = makeWorkspace([]);
  // Hard-link on the fixture volume: no 20 GB copy and no reusable .rustcut sidecars.
  // makeWorkspace uses %TEMP%, which may be on a different drive; use a workspace-local
  // media folder while keeping the app/WebView data in the harness's isolated profile.
  ws.media = join(directory, `run-${process.pid}-${Date.now()}`);
  mkdirSync(ws.media);
  [seed, source, waveform, padded] = [seed, source, waveform, padded].map((file) => {
    const path = join(ws.media, basename(file));
    linkSync(file, path);
    return path;
  });
  const app = new App(ws);
  const report = { generatedAt: new Date().toISOString(), mode: quick ? "quick" : "full", executable,
    build: args.has("--release") ? "release" : "debug", ffmpeg, platform: process.platform,
    source: { dimensions, fps, seconds: sourceSeconds, bitrate: metadata.format.bit_rate, bytes: statSync(source).size },
    largeFile: { bytes: statSync(padded).size, sparsePadding: true }, rows };
  const key = (binding) => app.page.keyboard.press(binding);
  const snapshot = () => app.invoke("get_snapshot");
  const percentile = (values, quantile) => [...values].sort((a, b) => a - b)[Math.max(0, Math.ceil(values.length * quantile) - 1)];

  /** Snapshot the measured Rust PID and its currently discoverable descendants. */
  async function counters() {
    const raw = await ps(`$all = @(Get-CimInstance Win32_Process); $ids = @([uint32]${app.pid}); do { $next = @($all | Where-Object { $_.ParentProcessId -in $ids -and $_.ProcessId -notin $ids } | ForEach-Object { $_.ProcessId }); $ids += $next } while ($next.Count -gt 0); ConvertTo-Json -Compress -InputObject @($all | Where-Object { $_.ProcessId -in $ids } | Select-Object ProcessId,ParentProcessId,Name,ReadTransferCount,PrivatePageCount)`);
    const list = JSON.parse(raw || "[]");
    if (!list.some((p) => p.ProcessId === app.pid)) throw new Error("Measured RustCut PID disappeared");
    return list.map((p) => ({ pid: p.ProcessId, parent: p.ParentProcessId, name: p.Name,
      reads: Number(p.ReadTransferCount), private: Number(p.PrivatePageCount) }));
  }
  /**
   * AC-14 sampler: the measured tree's ReadTransferCount about every 30 ms (Toolhelp snapshot and
   * GetProcessIoCounters in a helper PowerShell process; no WMI load during the open). Stops by
   * `stop()`, when the measured app exits, or after two minutes. Sample times are epoch ms, like
   * `performance.timeOrigin + performance.now()` in the page.
   */
  async function startReadSampler(root) {
    const output = join(ws.media, "read-samples.jsonl");
    const stopFile = join(ws.media, "read-samples.stop");
    const quoted = (path) => `'${path.replaceAll("'", "''")}'`;
    const script = `$ErrorActionPreference = 'Stop'\nAdd-Type -TypeDefinition @'\n${readSamplerSource()}\n'@\n` +
      `[PerfReadSampler]::Run(${root}, ${quoted(output)}, ${quoted(stopFile)})`;
    const child = spawn("powershell.exe", ["-NoProfile", "-NonInteractive", "-EncodedCommand",
      Buffer.from(script, "utf16le").toString("base64")], { windowsHide: true, stdio: ["ignore", "ignore", "pipe"] });
    let stderr = "";
    child.stderr.on("data", (data) => { stderr = (stderr + data).slice(-4000); });
    const exited = new Promise((resolve) => child.once("exit", resolve));
    await waitFor(() => {
      if (child.exitCode !== null) throw new Error(`Read sampler exited ${child.exitCode}: ${stderr}`);
      return existsSync(output) && readFileSync(output, "utf8").includes("\n");
    }, { timeout: 30000, every: 50, what: "the read sampler's first sample" });
    return {
      async stop() {
        writeFileSync(stopFile, "");
        await Promise.race([exited, sleep(5000)]);
        if (child.exitCode === null) child.kill(); // only the sampler started here
        return readFileSync(output, "utf8").split(/\r?\n/).flatMap((line) => {
          try { return line ? [JSON.parse(line)] : []; } catch { return []; } // a line cut by kill()
        });
      },
    };
  }
  /**
   * Reads per process from just after `baseline` up to `sample`: each process's last value in that
   * span minus its value at `baseline`. Processes created after `frameAt` read nothing before the
   * first frame and are left out. Only rustcut.exe and its FFmpeg children open the media file;
   * WebView2 processes receive its bytes from rustcut.exe over IPC, so they count only in `tree`.
   */
  function readsUpTo(samples, baseline, sample, frameAt) {
    const before = new Map(baseline.p.map((p) => [p.pid, p.reads]));
    const last = new Map();
    for (const s of samples) if (s.t0 > baseline.t0 && s.t0 <= sample.t0) for (const p of s.p) last.set(p.pid, p);
    const byProcess = [...last.values()].filter((p) => p.created <= frameAt)
      .map((p) => ({ pid: p.pid, name: p.name, bytes: Math.max(0, p.reads - (before.get(p.pid) ?? 0)) }));
    const media = byProcess.filter((p) => /^(rustcut|ffmpeg|ffprobe)\.exe$/i.test(p.name));
    return { app: media.reduce((n, p) => n + p.bytes, 0), tree: byProcess.reduce((n, p) => n + p.bytes, 0), byProcess };
  }
  async function observeEvents() {
    await app.page.evaluate(() => {
      window.__perfWave = { first: null, done: null };
      // Tap existing callbacks without replacing the app's single event subscription.
      // Tauri 2.11 exposes this Map; preserve and call every original callback.
      const callbacks = window.__TAURI_INTERNALS__.callbacks;
      if (!(callbacks instanceof Map)) throw new Error("Tauri callback instrumentation unavailable");
      for (const [id, callback] of callbacks) callbacks.set(id, (raw) => {
        const event = raw?.message;
        if (event?.type === "waveform") {
          const now = performance.now();
          if (event.chunk.peaks.length && window.__perfWave.first === null) window.__perfWave.first = now;
          if (event.chunk.done) window.__perfWave.done = now;
        }
        return callback(raw);
      });
    });
  }
  async function openMeasured(file) {
    // Arm before hand-off; reject callbacks from the previous source's last frame.
    await app.page.evaluate((file) => {
      document.querySelector("video")?.pause();
      window.__perfOpen = { start: performance.now(), frame: null };
      const attached = new WeakSet();
      const presented = () => {
        const video = document.querySelector("video");
        if (video && decodeURIComponent(video.currentSrc).replaceAll("\\", "/").endsWith(file.replaceAll("\\", "/"))) {
          window.__perfOpen.frame ??= performance.now();
        }
      };
      const listeners = [];
      const arm = () => {
        const video = document.querySelector("video");
        if (!video || attached.has(video)) return;
        attached.add(video);
        const loaded = () => video.requestVideoFrameCallback(presented);
        video.addEventListener("loadeddata", loaded);
        listeners.push(() => video.removeEventListener("loadeddata", loaded));
        video.requestVideoFrameCallback(presented);
      };
      window.__perfOpenObserver?.disconnect();
      window.__perfOpenObserver = new MutationObserver(arm);
      window.__perfOpenObserver.observe(document.body, { childList: true, subtree: true });
      window.__perfOpenCleanup = () => listeners.forEach((remove) => remove());
      arm();
    }, file);
    await handOffToWindow(app, file, executable);
    await waitFor(() => app.page.evaluate(() => window.__perfOpen.frame !== null),
      { timeout: 120000, every: 10, what: "presented first frame" });
    await waitReady(app);
    return app.page.evaluate(() => {
      window.__perfOpenObserver.disconnect();
      window.__perfOpenCleanup();
      return window.__perfOpen.frame - window.__perfOpen.start;
    });
  }
  /** One actual keydown/seek to a newly PRESENTED frame; excludes CDP round trips. */
  async function navigation({ binding, target }) {
    await app.page.evaluate(({ binding, target }) => {
      window.__perfNavigation = new Promise((resolve, reject) => {
        const video = document.querySelector("video");
        const timer = setTimeout(() => { removeEventListener("keydown", down, true); reject(new Error("No presented navigation frame within 30 s")); }, 30000);
        function down() {
          removeEventListener("keydown", down, true);
          const start = performance.now();
          video.requestVideoFrameCallback((_, meta) => {
            clearTimeout(timer);
            resolve({ ms: performance.now() - start, time: meta.mediaTime });
          });
        }
        if (binding) addEventListener("keydown", down, true);
        else { down(); video.currentTime = target; }
      });
      // Keep the rejection handled until the CDP caller reads it.
      window.__perfNavigation.catch(() => {});
    }, { binding, target });
    if (binding) await key(binding);
    return app.page.evaluate(() => window.__perfNavigation);
  }
  async function exportBatch(ids, settings) {
    const start = performance.now();
    await app.invoke("start_export", { clip_ids: ids, settings });
    const completed = await waitFor(async () => {
      const clips = (await snapshot()).project.clips.filter((c) => ids.includes(c.id));
      const failed = clips.find((c) => c.export_status === "failed");
      if (failed) return { error: failed.last_export?.error ?? "An export failed (see app logs)" };
      return clips.length === ids.length && clips.every((c) => c.export_status === "completed");
    }, { timeout: 1200000, every: 100, what: "export completion including FFmpeg process start" });
    if (completed.error) throw new Error(completed.error);
    return performance.now() - start;
  }
  try {
    const { spawnedAt } = await startMeasured(app, { executable, args: [seed] });
    await waitReady(app);
    record("cold_start_ms", performance.now() - spawnedAt, "Spawn to playable editor, includes CDP attach/poll overhead; fresh app/WebView profile; OS cache not flushed.");
    await observeEvents();

    await stage(["open_ms", "open_read_bytes", "open_tree_read_bytes", "large_tree_private_bytes", "large_core_private_bytes"], async () => {
      await app.invoke("close_project");
      const before = await counters();
      const sampler = await startReadSampler(app.pid);
      let ms, samples;
      try {
        ms = await openMeasured(padded);
        await sleep(200); // samples after the first frame bound its reads from above
      } finally {
        samples = await sampler.stop();
      }
      const after = await counters();
      const { handoffAt, frameAt } = await app.page.evaluate(() => ({
        handoffAt: performance.timeOrigin + window.__perfOpen.start,
        frameAt: performance.timeOrigin + window.__perfOpen.frame,
      }));
      const baseline = samples.filter((s) => s.t1 <= handoffAt).at(-1);
      const lower = samples.filter((s) => s.t1 <= frameAt).at(-1);
      const upper = samples.find((s) => s.t0 >= frameAt);
      if (!baseline || !lower || !upper) throw new Error(`Read samples do not bracket the hand-off and the first frame (${samples.length} samples)`);
      const atFrame = readsUpTo(samples, baseline, upper, frameAt);
      const beforeFrame = readsUpTo(samples, baseline, lower, frameAt);
      const gaps = samples.slice(1).map((s, i) => s.t0 - samples[i].t0);
      report.readCounters = { before, after, endpoint: "CIM snapshots before the hand-off and after waitReady; context only" };
      report.firstFrameReads = { handoffAt, frameAt, sampleCount: samples.length, medianIntervalMs: percentile(gaps, 0.5),
        baseline, lower, upper, upperByProcess: atFrame.byProcess, lowerByProcess: beforeFrame.byProcess };
      const mb = (bytes) => `${(bytes / 1e6).toFixed(1)} MB`;
      record("open_ms", ms);
      record("open_read_bytes", atFrame.app, `Upper bound at the first sample ${(upper.t0 - frameAt).toFixed(0)} ms after the first frame; ${mb(beforeFrame.app)} at the last sample ${(frameAt - lower.t1).toFixed(0)} ms before it. ReadTransferCount of rustcut.exe and its FFmpeg children (all reads, not only the media file); processes started after the first frame are excluded; children that exited earlier report their final count.`);
      record("open_tree_read_bytes", atFrame.tree, "Same sample including the WebView2 processes, which receive the file's bytes from rustcut.exe over IPC (a double count).");
      await key("k");
      const memory = [];
      for (let i = 0; i < 5; i++) { await sleep(1000); memory.push(await counters()); }
      record("large_tree_private_bytes", Math.max(...memory.map((sample) => sample.reduce((n, p) => n + p.private, 0))));
      record("large_core_private_bytes", Math.max(...memory.map((sample) => sample.find((p) => p.pid === app.pid).private)));
      await app.page.evaluate(() => document.querySelector("video").pause());
    });

    await stage(["seek_p50_ms", "seek_p95_ms", "step_forward_ms", "step_backward_ms", "core_private_bytes", "tree_private_bytes", "mark_capture"], async () => {
      await app.invoke("close_project");
      await openMeasured(seed);
      // Deterministic LCG, 30 non-sequential seeks over the complete source.
      let random = 0x52555354;
      const seeks = [];
      for (let i = 0; i < 30; i++) {
        random = (Math.imul(1664525, random) + 1013904223) >>> 0;
        const target = (0.1 + (random / 2 ** 32) * 0.8) * seedSeconds;
        seeks.push((await navigation({ target })).ms);
      }
      record("seek_p50_ms", percentile(seeks, 0.5), "30 random seeks, frame-presentation endpoint");
      record("seek_p95_ms", percentile(seeks, 0.95));
      const forward = [], backward = [];
      for (let i = 0; i < 10; i++) {
        forward.push((await navigation({ binding: "Period" })).ms);
        backward.push((await navigation({ binding: "Comma" })).ms);
      }
      report.navigation = { seeks, forward, backward };
      record("step_forward_ms", percentile(forward, 0.95), "10 real Period presses");
      record("step_backward_ms", percentile(backward, 0.95), "10 real Comma presses");

      await app.page.evaluate(() => {
        window.__perfMark = null;
        addEventListener("keydown", (event) => {
          if (event.key.toLowerCase() === "i") window.__perfMark = document.querySelector('[role="timer"]').textContent;
        }, { capture: true, once: true });
      });
      await key("i");
      const marks = await app.page.evaluate(() => ({ captured: window.__perfMark,
        text: document.body.innerText }));
      const time = /\d+:\d\d:\d\d\.\d{3}/.exec(marks.captured)?.[0];
      const pending = /Pending In:\s*(\d+:\d\d:\d\d\.\d{3})/.exec(marks.text)?.[1];
      record("mark_capture", { presented: time, pending, equal: !!time && time === pending },
        "Paused keyboard capture equality; no-IPC property is covered by frontend unit tests, not inferred from elapsed time.", false);
      if (!time || time !== pending) Object.assign(rows.find((r) => r.id === "mark_capture"), { status: "FAIL" });
      await key("Escape");
      await app.page.evaluate(async () => {
        const video = document.querySelector("video");
        video.loop = true;
        video.currentTime = 0;
        await video.play();
      });
      const memory = [];
      for (let i = 0; i < 5; i++) { await sleep(500); memory.push(await counters()); }
      await app.page.evaluate(() => {
        const video = document.querySelector("video");
        video.pause();
        video.loop = false;
      });
      report.playbackMemory = memory;
      const core = Math.max(...memory.map((sample) => sample.find((p) => p.pid === app.pid).private));
      const tree = Math.max(...memory.map((sample) => sample.reduce((n, p) => n + p.private, 0)));
      record("core_private_bytes", core, "Peak of five PrivatePageCount samples during small-file playback");
      record("tree_private_bytes", tree, "Peak includes all descendants (WebView renderers, GPU, utilities)");
      const large = rows.find((r) => r.id === "large_tree_private_bytes");
      if (large.value !== null) large.detail = `Large/small tree delta: ${large.value - tree} bytes (flatness is informational; PRD sets no delta tolerance).`;
    });

    await stage(["waveform_first_ms", "waveform_total_ms", "timeline_fps", "autosave_ms"], async () => {
      await app.invoke("close_project");
      await app.page.evaluate(() => { window.__perfWave = { first: null, done: null }; window.__perfWaveStart = performance.now(); });
      await openMeasured(waveform); // normal open automatically starts the first audio track
      await waitFor(() => app.page.evaluate(() => window.__perfWave.done !== null && window.__perfWave.first !== null),
        { timeout: 300000, what: "complete waveform" });
      const wave = await app.page.evaluate(() => ({ first: window.__perfWave.first - window.__perfWaveStart,
        total: window.__perfWave.done - window.__perfWaveStart }));
      record("waveform_first_ms", wave.first, "Open request to first nonempty chunk on a fresh cache");
      record("waveform_total_ms", wave.total, `${waveformSeconds} s audio, including open/probe`);
      // One backend transaction avoids measuring 500 auto-saves as timeline work.
      await app.invoke("import_timestamps", { items: Array.from({ length: 500 }, (_, i) => ({
        line: i + 1, start_sec: i * (waveformSeconds - 1) / 500,
        end_sec: i * (waveformSeconds - 1) / 500 + 0.5, name: `Perf ${i + 1}`, error: null,
      })) });
      if ((await snapshot()).project.clips.length !== 500) throw new Error("Timeline requires exactly 500 clips");
      const timeline = await app.page.evaluate(() => new Promise((resolve, reject) => {
        const canvas = document.querySelector('[aria-describedby="zoom-summary"]');
        if (!canvas) { reject(new Error("Zoom timeline canvas missing")); return; }
        const summary = document.getElementById("zoom-summary");
        const before = summary.textContent;
        const intervals = [];
        let previous = null, changes = 0;
        const start = performance.now();
        const tick = (now) => {
          if (previous !== null) intervals.push(now - previous);
          previous = now;
          if (summary.textContent !== before) changes++;
          const box = canvas.getBoundingClientRect();
          canvas.dispatchEvent(new WheelEvent("wheel", { deltaY: Math.floor((now - start) / 1000) % 2 ? 20 : -20,
            clientX: box.x + box.width / 2, clientY: box.y + box.height / 2, bubbles: true, cancelable: true }));
          if (now - start < 5000) requestAnimationFrame(tick);
          else resolve({ intervals, changes });
        };
        requestAnimationFrame(tick);
      }));
      if (!timeline.changes) throw new Error("Timeline zoom never changed during measurement");
      report.timeline = { intervals: timeline.intervals, p95IntervalMs: percentile(timeline.intervals, 0.95), changes: timeline.changes };
      record("timeline_fps", 1000 / (timeline.intervals.reduce((a, b) => a + b, 0) / timeline.intervals.length),
        `5 s of animated wheel zoom; rAF scheduling proxy, p95 interval ${report.timeline.p95IntervalMs.toFixed(2)} ms; monitor refresh affects result.`);
      const snap = await snapshot();
      const start = performance.now();
      await app.invoke("edit_clips", { req: { ids: [snap.project.clips[0].id], edit: { kind: "rename", name: "Auto-save measurement" } } });
      const file = waveform.replace(/\.mp4$/, ".rustcut");
      await waitFor(() => readProject(file)?.clips[0]?.name === "Auto-save measurement",
        { timeout: 15000, every: 10, what: "auto-save written to disk" });
      record("autosave_ms", performance.now() - start);
    });

    await stage(["lossless_ms", "vertical_software_x", "vertical_hardware_x"], async () => {
      await app.invoke("close_project");
      await openMeasured(source);
      // Encoder probing belongs inside the first export's measured start, as in a cold user workflow.
      const ids = [];
      for (let i = 0; i < 10; i++) ids.push((await app.invoke("add_clip", { req: {
        start_sec: i * clipSeconds, end_sec: (i + 1) * clipSeconds, name: `Perf ${i + 1}`, tag_id: null,
      } })).clip.id);
      const settings = { ...(await snapshot()).project.export_settings, output_dir: ws.out,
        cut_mode: "lossless", framing: "original", parallel_jobs: 1, on_conflict: "rename" };
      record("lossless_ms", await exportBatch(ids, settings), `10 × ${clipSeconds} s, from start_export through persisted completed statuses`);
      const one = [ids[0]];
      const vertical = { ...settings, cut_mode: "accurate", framing: "vertical_crop", output_resolution: "1080x1920", fps_cap: 60, encoder: "libx264" };
      // Settings change marks old exports outdated before polling completion again.
      await app.invoke("set_export_settings", { settings: vertical });
      record("vertical_software_x", clipSeconds / ((await exportBatch(one, vertical)) / 1000), "libx264, includes process startup");
      const hardware = (await app.invoke("detect_hw_encoders", { force: false })).find((e) => e.available);
      if (!hardware) rows.find((r) => r.id === "vertical_hardware_x").detail = "No available hardware encoder; not measured";
      else {
        const hwSettings = { ...vertical, encoder: hardware.name };
        await app.invoke("set_export_settings", { settings: hwSettings });
        record("vertical_hardware_x", clipSeconds / ((await exportBatch(one, hwSettings)) / 1000), hardware.name);
      }
    });
  } catch (error) {
    report.error = String(error);
    process.exitCode = 1;
  } finally {
    await app.stop();
    // Remove only the isolated paths created by this run, after closing our app tree.
    if (!resolve(ws.media).startsWith(resolve(directory) + sep)) throw new Error("Refusing cleanup outside performance fixture directory");
    cleanup({ root: ws.media });
    cleanup(ws);
    const markdown = ["| Metric | Target | Value | Result |", "| :--- | :--- | :--- | :--- |",
      ...rows.map((row) => `| ${row.metric} | ${row.target} | ${row.value === null ? "—" : typeof row.value === "number" ? row.value.toFixed(2) : JSON.stringify(row.value)} | ${row.status} |`)].join("\n");
    const stem = join(directory, `${prefix}-results`);
    writeFileSync(`${stem}.json`, JSON.stringify(report, null, 2) + "\n");
    writeFileSync(`${stem}.md`, markdown + "\n");
    console.log(JSON.stringify(report, null, 2));
    console.log("\n" + markdown);
    if (rows.some((row) => row.status === "FAIL")) process.exitCode = 1;
  }
}

/**
 * C# for the AC-14 read sampler (Windows PowerShell 5.1 compiles it with Add-Type, so C# 5): one
 * JSON line per sample, `{"t0","t1","p":[{pid,parent,name,reads,created}]}`, times in epoch ms.
 */
function readSamplerSource() {
  return String.raw`using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class PerfReadSampler {
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  struct ProcessEntry {
    public uint Size, Usage, ProcessId;
    public IntPtr DefaultHeapId;
    public uint ModuleId, Threads, ParentProcessId;
    public int PriorityClassBase;
    public uint Flags;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string ExeFile;
  }

  [StructLayout(LayoutKind.Sequential)]
  struct IoCounters { public ulong ReadOperations, WriteOperations, OtherOperations, ReadBytes, WriteBytes, OtherBytes; }

  [DllImport("kernel32.dll")] static extern IntPtr CreateToolhelp32Snapshot(uint flags, uint processId);
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern bool Process32FirstW(IntPtr snapshot, ref ProcessEntry entry);
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern bool Process32NextW(IntPtr snapshot, ref ProcessEntry entry);
  [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint access, bool inherit, uint processId);
  [DllImport("kernel32.dll")] static extern bool GetProcessIoCounters(IntPtr process, out IoCounters counters);
  [DllImport("kernel32.dll")] static extern bool GetProcessTimes(IntPtr process, out long created, out long exited, out long kernel, out long user);
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
  [DllImport("kernel32.dll")] static extern void GetSystemTimePreciseAsFileTime(out long fileTime);

  const long UnixEpochFileTime = 116444736000000000L;

  // Every process of the tree seen so far. The handles stay open until the end: an exited
  // child's final counters stay readable and its PID cannot be reused meanwhile.
  static readonly Dictionary<uint, IntPtr> Handles = new Dictionary<uint, IntPtr>();
  static readonly Dictionary<uint, uint> Parents = new Dictionary<uint, uint>();
  static readonly Dictionary<uint, string> Names = new Dictionary<uint, string>();
  static readonly Dictionary<uint, long> Created = new Dictionary<uint, long>();
  static readonly HashSet<uint> Unrelated = new HashSet<uint>();

  static double Now() {
    long fileTime;
    GetSystemTimePreciseAsFileTime(out fileTime);
    return (fileTime - UnixEpochFileTime) / 10000.0;
  }

  public static void Run(uint root, string output, string stop) {
    double end = Now() + 120000;
    try {
      using (var writer = new StreamWriter(output, false, new UTF8Encoding(false))) {
        while (!File.Exists(stop) && Now() < end) {
          string line = Sample(root);
          if (line == null) break; // the measured app has exited
          writer.WriteLine(line);
          writer.Flush();
          Thread.Sleep(10);
        }
      }
    } finally {
      foreach (IntPtr handle in Handles.Values) if (handle != IntPtr.Zero) CloseHandle(handle);
    }
  }

  static bool Track(uint id, uint parent, string name) {
    IntPtr handle = OpenProcess(0x1000, false, id); // PROCESS_QUERY_LIMITED_INFORMATION
    long created = 0, exited = 0, kernel = 0, user = 0;
    if (handle != IntPtr.Zero) GetProcessTimes(handle, out created, out exited, out kernel, out user);
    long parentCreated;
    if (Created.TryGetValue(parent, out parentCreated) && created != 0 && created < parentCreated) {
      // Older than the tracked process with its parent's PID: that PID was reused, not a child.
      if (handle != IntPtr.Zero) CloseHandle(handle);
      Unrelated.Add(id);
      return false;
    }
    Handles[id] = handle;
    Parents[id] = parent;
    Names[id] = name;
    Created[id] = created;
    return true;
  }

  static string Sample(uint root) {
    double t0 = Now();
    var alive = new Dictionary<uint, ProcessEntry>();
    IntPtr snapshot = CreateToolhelp32Snapshot(2, 0); // TH32CS_SNAPPROCESS
    var entry = new ProcessEntry();
    entry.Size = (uint)Marshal.SizeOf(typeof(ProcessEntry));
    for (bool more = Process32FirstW(snapshot, ref entry); more; more = Process32NextW(snapshot, ref entry)) {
      alive[entry.ProcessId] = entry;
    }
    CloseHandle(snapshot);
    if (!alive.ContainsKey(root)) return null;
    if (!Handles.ContainsKey(root)) Track(root, alive[root].ParentProcessId, alive[root].ExeFile);
    for (bool added = true; added;) {
      added = false;
      foreach (var pair in alive) {
        if (pair.Key != 0 && !Handles.ContainsKey(pair.Key) && !Unrelated.Contains(pair.Key) &&
            Handles.ContainsKey(pair.Value.ParentProcessId)) {
          added |= Track(pair.Key, pair.Value.ParentProcessId, pair.Value.ExeFile);
        }
      }
    }
    var items = new List<string>();
    foreach (var tracked in Handles) {
      uint id = tracked.Key;
      var io = new IoCounters();
      if (tracked.Value == IntPtr.Zero || !GetProcessIoCounters(tracked.Value, out io)) continue;
      long created = Created[id];
      // Concatenation, not String.Format: "{4:F3}}}" would read "}}" as part of the format.
      items.Add("{\"pid\":" + id.ToString(CultureInfo.InvariantCulture) +
        ",\"parent\":" + Parents[id].ToString(CultureInfo.InvariantCulture) +
        ",\"name\":\"" + Names[id].Replace("\\", "\\\\").Replace("\"", "\\\"") +
        "\",\"reads\":" + io.ReadBytes.ToString(CultureInfo.InvariantCulture) +
        ",\"created\":" + Ms((created - UnixEpochFileTime) / 10000.0) + "}");
    }
    return "{\"t0\":" + Ms(t0) + ",\"t1\":" + Ms(Now()) + ",\"p\":[" + String.Join(",", items) + "]}";
  }

  static string Ms(double value) {
    return value.ToString("F3", CultureInfo.InvariantCulture);
  }
}`;
}

/** Insert a sparse top-level free atom BEFORE the trailing moov; media offsets stay valid. */
async function padTailIndex(source, destination, bytes, exec) {
  if (existsSync(destination) && statSync(destination).size === bytes) return;
  const input = openSync(source, "r");
  let moov;
  const size = statSync(source).size;
  try {
    for (let position = 0; position < size;) {
      const header = Buffer.alloc(16);
      if (readSync(input, header, 0, 16, position) < 8) throw new Error("Truncated MP4 atom");
      const length32 = header.readUInt32BE(0);
      const length = length32 === 1 ? Number(header.readBigUInt64BE(8)) : length32 === 0 ? size - position : length32;
      if (length < 8 || position + length > size) throw new Error("Invalid MP4 atom size");
      if (header.toString("ascii", 4, 8) === "moov") moov = { position, length };
      position += length;
    }
    if (!moov || moov.position + moov.length !== size) throw new Error("Expected a trailing moov; generate without faststart");
    const padding = bytes - size;
    if (padding < 16) throw new Error("Padding target smaller than source");
    const tail = Buffer.alloc(moov.length);
    readSync(input, tail, 0, tail.length, moov.position);
    const partial = `${destination}.part`;
    closeSync(openSync(partial, "w"));
    await exec("fsutil.exe", ["sparse", "setflag", partial], { windowsHide: true });
    await pipeline(createReadStream(source, { start: 0, end: moov.position - 1 }), createWriteStream(partial, { flags: "r+" }));
    const output = openSync(partial, "r+");
    try {
      ftruncateSync(output, bytes);
      const free = Buffer.alloc(16);
      free.writeUInt32BE(1); free.write("free", 4, "ascii"); free.writeBigUInt64BE(BigInt(padding), 8);
      writeSync(output, free, 0, free.length, moov.position);
      writeSync(output, tail, 0, tail.length, bytes - tail.length);
    } finally { closeSync(output); }
    renameSync(partial, destination);
  } finally { closeSync(input); }
}
