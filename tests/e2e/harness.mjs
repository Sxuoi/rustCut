// RustCut end-to-end harness: launches the REAL debug app (embedded frontend) with an isolated
// APPDATA/LOCALAPPDATA and drives its WebView2 over CDP with playwright-core.
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, copyFileSync, mkdirSync, existsSync, readdirSync, rmSync, readFileSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const HERE = dirname(fileURLToPath(import.meta.url));
export const ROOT = resolve(HERE, "../..");
export const APP_EXE = join(process.env.CARGO_TARGET_DIR ?? join(ROOT, "target"), "debug", "rustcut.exe");
export const FIXTURES = join(ROOT, ".fixtures");
export const FFPROBE = join(ROOT, "src-tauri", "binaries", readdirSync(join(ROOT, "src-tauri", "binaries")).find((f) => f.startsWith("ffprobe") && f.endsWith(".exe")) ?? "ffprobe.exe");
const CDP_PORT = 9333;

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Isolated workspace: temp root with app-data, media and output folders. */
export function makeWorkspace(fixtures = ["M03_long_gop_10s.mp4"]) {
  const root = mkdtempSync(join(tmpdir(), "rustcut-e2e-"));
  const media = join(root, "media");
  const out = join(root, "out");
  mkdirSync(media);
  mkdirSync(out);
  const files = {};
  for (const f of fixtures) {
    copyFileSync(join(FIXTURES, f), join(media, f));
    files[f] = join(media, f);
  }
  return { root, media, out, files, appdata: join(root, "appdata"), local: join(root, "local") };
}

export function cleanup(ws) {
  // WebView2 helper processes can hold the profile folder for a few seconds after the app exits:
  // retry for up to ~10 s instead of leaving the folder in %TEMP%.
  try { rmSync(ws.root, { recursive: true, force: true, maxRetries: 20, retryDelay: 500 }); } catch { /* best effort */ }
}

/** PIDs of every process whose name matches and whose command line mentions `needle`. */
export function findProcs(name, needle) {
  const ps = `Get-CimInstance Win32_Process -Filter "Name='${name}'" | Where-Object { $_.CommandLine -like '*${needle.replace(/'/g, "''")}*' } | ForEach-Object { $_.ProcessId }`;
  const out = execFileSync("powershell", ["-NoProfile", "-Command", ps], { encoding: "utf8" });
  return out.split(/\s+/).filter(Boolean).map(Number);
}

export function ffprobeJson(file) {
  return JSON.parse(execFileSync(FFPROBE, ["-v", "error", "-print_format", "json", "-show_streams", "-show_format", file], { encoding: "utf8" }));
}

export class App {
  constructor(ws) { this.ws = ws; this.child = null; this.browser = null; this.page = null; }

  /** Start the app; `args` become command-line arguments (launch path). */
  async start(args = []) {
    if (!existsSync(APP_EXE)) throw new Error(`Build the app first (npm run e2e:build): ${APP_EXE} is missing`);
    const env = {
      ...process.env,
      RUSTCUT_DATA_DIR: this.ws.appdata,
      WEBVIEW2_USER_DATA_FOLDER: join(this.ws.local, "webview"),
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}`,
    };
    mkdirSync(this.ws.appdata, { recursive: true });
    mkdirSync(this.ws.local, { recursive: true });
    this.child = spawn(APP_EXE, args, { env, stdio: "ignore", windowsHide: false });
    this.exited = new Promise((r) => this.child.once("exit", r));
    const deadline = Date.now() + 30000;
    let lastErr;
    while (Date.now() < deadline) {
      try {
        this.browser = await chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`);
        const ctx = this.browser.contexts()[0];
        this.page = ctx.pages()[0] ?? (await ctx.waitForEvent("page", { timeout: 5000 }));
        await this.page.waitForLoadState("domcontentloaded");
        await this.page.waitForSelector("body", { timeout: 10000 });
        await sleep(600);
        return this;
      } catch (e) { lastErr = e; await sleep(400); }
    }
    throw new Error(`App did not expose CDP in 30 s: ${lastErr}`);
  }

  get pid() { return this.child?.pid; }

  /** Invoke a Tauri command from the page. */
  invoke(cmd, args = {}) {
    return this.page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
  }

  /** Graceful stop: close the window via CDP-less path, falling back to a tree kill. */
  async stop({ graceful = true } = {}) {
    try { await this.browser?.close(); } catch { /* ignore */ }
    this.browser = null;
    if (this.child && this.child.exitCode === null) {
      if (graceful) {
        try { execFileSync("taskkill", ["/PID", String(this.pid)], { stdio: "ignore" }); } catch { /* ignore */ }
        await Promise.race([this.exited, sleep(8000)]);
      }
      if (this.child.exitCode === null) this.kill();
      await Promise.race([this.exited, sleep(5000)]);
    }
  }

  /** Hard kill the app tree (simulates a crash / force kill, PRD 8.2). */
  kill({ tree = true } = {}) {
    const args = tree ? ["/F", "/T", "/PID", String(this.pid)] : ["/F", "/PID", String(this.pid)];
    try { execFileSync("taskkill", args, { stdio: "ignore" }); } catch { /* already gone */ }
  }
}

/** Open a video through the real IPC path and wait for the editor. */
export async function openVideo(app, path) {
  const res = await app.invoke("open_path", { path });
  await app.page.waitForSelector('[aria-label="Clip queue"], [role="list"]', { timeout: 15000 }).catch(() => {});
  return res;
}

export function projectFileFor(media, name) {
  // The sidecar project lives next to the media (PRJ-01); returns the first .rustcut in the folder.
  const f = readdirSync(media).find((x) => x.endsWith(".rustcut"));
  return f ? join(media, f) : null;
}
export { existsSync, readdirSync, readFileSync, statSync, join };

/**
 * Performance-only launch: omit the ordinary harness's settling delay and let
 * the caller timestamp readiness. Existing App.start semantics stay unchanged.
 */
export async function startMeasured(app, { executable = APP_EXE, args = [], onPage = async () => {} } = {}) {
  if (!existsSync(executable)) throw new Error(`Build the embedded app first: ${executable}`);
  mkdirSync(app.ws.appdata, { recursive: true });
  mkdirSync(app.ws.local, { recursive: true });
  const spawnedAt = performance.now();
  app.child = spawn(executable, args, { env: {
    ...process.env, RUSTCUT_DATA_DIR: app.ws.appdata,
    WEBVIEW2_USER_DATA_FOLDER: join(app.ws.local, "webview"),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}`,
  }, stdio: "ignore", windowsHide: true });
  let spawnError;
  app.child.once("error", (error) => { spawnError = error; });
  app.exited = new Promise((resolve) => app.child.once("exit", resolve));
  const deadline = performance.now() + 60000;
  let lastError;
  while (performance.now() < deadline) {
    if (spawnError) throw spawnError;
    if (app.child.exitCode !== null) throw new Error(`App exited before CDP: ${app.child.exitCode}`);
    try {
      app.browser = await chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`, { timeout: 1000 });
      const context = app.browser.contexts()[0];
      app.page = context.pages()[0] ?? await context.waitForEvent("page", { timeout: 1000 });
      break;
    } catch (error) {
      lastError = error;
      try { await app.browser?.close(); } catch { /* connection failed */ }
      app.browser = null;
      await sleep(25);
    }
  }
  if (!app.page) throw new Error(`No CDP page within 60 s: ${lastError}`);
  await app.page.waitForLoadState("domcontentloaded");
  await onPage(app.page);
  return { spawnedAt };
}

/** Open through the same single-instance hand-off as AC-12, including UI session setup. */
export async function handOffToWindow(app, path, executable = APP_EXE) {
  await new Promise((resolve, reject) => {
    const child = spawn(executable, [path], { env: {
      ...process.env, RUSTCUT_DATA_DIR: app.ws.appdata,
      WEBVIEW2_USER_DATA_FOLDER: join(app.ws.local, "webview"),
    }, stdio: "ignore", windowsHide: true });
    const timer = setTimeout(() => {
      child.kill(); // only the second-launch process created by this helper
      reject(new Error("Single-instance hand-off did not exit within 30 s"));
    }, 30000);
    child.once("error", (error) => { clearTimeout(timer); reject(error); });
    child.once("exit", (code) => {
      clearTimeout(timer);
      if (code === 0) resolve();
      else reject(new Error(`Single-instance hand-off exited ${code}`));
    });
  });
}
