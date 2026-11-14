// End-to-end: AC-12 (PRD section 11). Every golden-set fixture either plays or shows a specific,
// actionable message within 5 s, and nothing hangs. Each file is opened the way a user double-clicks
// one while RustCut is running: a second launch hands the path to this window ("open-request"), which
// runs the normal open flow (probe, playback tier, first-frame check, fallbacks).
import { test, describe, after } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { App, APP_EXE, FIXTURES, makeWorkspace, cleanup, sleep, join } from "./harness.mjs";

const UNICODE_DIR = "M18 ruang kerja ñ 測試";
const UNICODE_FILE = "clip with spaces é.mp4";

// Golden set M01-M18 plus the negative audio-only file. The refused files sit between playable ones
// so recovery after a refusal is covered too.
const MATRIX = [
  "M01_baseline_1080p60.mp4",
  "M02_obs_mkv_4audio.mkv",
  "M03_long_gop_10s.mp4",
  "M04_vfr.mp4",
  "M05_hevc_hlg_rotated.mp4",
  "NEG_audio_only.m4a",
  "M06_ramp_1440x1080.mp4",
  "M07_ramp_2560x1080.mp4",
  "M08_ramp_1080x1920.mp4",
  "M09_ramp_1080x2340.mp4",
  "M10_ramp_720x576_sar.mp4",
  "M11_flv.flv",
  "M12_webm_vp9_opus.webm",
  "M13_start_time_nonzero.mp4",
  "M15_truncated.mp4",
  "M14_no_audio.mp4",
  "M16_av1_144fps.mp4",
  "M17_pcm_audio.mov",
  `${UNICODE_DIR}/${UNICODE_FILE}`,
];
// Refused with a specific message instead of opening: no video stream; index missing (unfinished recording).
const REFUSED = ["NEG_audio_only.m4a", "M15_truncated.mp4"];

const nameOf = (f) => f.split("/").pop();

/** Source path of the open project (null when none). */
async function openSource(app) {
  try {
    const snap = await app.invoke("get_snapshot");
    return snap?.project?.source?.path ?? null;
  } catch {
    return null;
  }
}

/** Error toasts on screen, normalised exactly as `outcome` reads them. */
async function errorToasts(app) {
  return app.page.evaluate(() =>
    [...document.querySelectorAll('section[aria-label="Notifications"] [role="alert"]')].map((e) => e.textContent.replace(/\s+/g, " ").trim()),
  );
}

/** What the window shows for `file`: plays, a message, preparing, or nothing yet. */
async function outcome(app, file, toastsBefore) {
  const src = await openSource(app);
  const current = !!src && src.replace(/\\/g, "/").endsWith(`/${nameOf(file)}`);
  const ui = await app.page.evaluate((before) => {
    const v = document.querySelector("video");
    const inNotifications = (e) => !!e.closest('section[aria-label="Notifications"]');
    return {
      ready: !!v && v.readyState >= 2,
      size: v ? `${v.videoWidth}x${v.videoHeight}` : "",
      preparing: /Preparing preview/i.test(document.body.innerText),
      // The player's own message ("Preview unavailable ..."), not a toast.
      alert: [...document.querySelectorAll('[role="alert"]')].filter((e) => !inNotifications(e)).map((e) => e.textContent.replace(/\s+/g, " ").trim()).join(" / "),
      // Error toasts only: info toasts ("Created a new project for ...") accompany a successful open.
      notes: [...document.querySelectorAll('section[aria-label="Notifications"] [role="alert"]')]
        .map((e) => e.textContent.replace(/\s+/g, " ").trim())
        .filter((t) => t && !before.includes(t)),
    };
  }, toastsBefore);
  if (current && ui.ready && !ui.preparing) return { kind: "plays", detail: ui.size };
  if (current && ui.alert) return { kind: "message", detail: ui.alert };
  if (!current && ui.notes.length > 0) return { kind: "message", detail: ui.notes.join(" / ") };
  if (current && ui.preparing) return { kind: "preparing", detail: "Preparing preview" };
  return { kind: "pending", detail: current ? "opened, no frame yet" : "not opened yet" };
}

/** Second launch with the same isolated environment: it must hand the file over and exit. */
function handOff(ws, path) {
  return new Promise((resolve) => {
    const env = { ...process.env, RUSTCUT_DATA_DIR: ws.appdata, WEBVIEW2_USER_DATA_FOLDER: join(ws.local, "webview") };
    const child = spawn(APP_EXE, [path], { env, stdio: "ignore" });
    const timer = setTimeout(() => {
      try { child.kill(); } catch { /* ignore */ }
      resolve(false);
    }, 15000);
    child.once("exit", () => {
      clearTimeout(timer);
      resolve(true);
    });
  });
}

describe("AC-12 golden-set matrix", { concurrency: false }, () => {
  const ws = makeWorkspace(MATRIX.filter((f) => !f.includes("/")));
  mkdirSync(join(ws.media, UNICODE_DIR), { recursive: true });
  copyFileSync(join(FIXTURES, UNICODE_DIR, UNICODE_FILE), join(ws.media, UNICODE_DIR, UNICODE_FILE));
  const app = new App(ws);
  const results = [];
  after(async () => {
    await app.stop();
    console.log("AC-12 matrix:\n" + results.map((r) => `  ${r.file}: ${r.first.kind} after ${r.firstMs} ms -> ${r.final.kind} (${r.final.detail}) after ${r.finalMs} ms`).join("\n"));
    cleanup(ws);
  });

  test("every fixture plays or explains itself within 5 s, and nothing hangs", async () => {
    await app.start([]);
    for (const file of MATRIX) {
      const before = await errorToasts(app).catch(() => []);
      const t0 = Date.now();
      assert.ok(await handOff(ws, join(ws.media, ...file.split("/"))), `${file}: the second launch did not hand over and exit`);
      // Within 5 s: a decoded frame, a specific message, or the "Preparing preview" progress state.
      let first = { kind: "pending" };
      while (Date.now() - t0 < 5000) {
        first = await outcome(app, file, before);
        if (first.kind !== "pending") break;
        await sleep(100);
      }
      const firstMs = Date.now() - t0;
      assert.notEqual(first.kind, "pending", `${file}: nothing within 5 s (${first.detail})`);
      // Nothing hangs: a preparing state ends in a frame or a message.
      let final = first;
      while (final.kind === "preparing" && Date.now() - t0 < 90000) {
        await sleep(250);
        final = await outcome(app, file, before);
      }
      results.push({ file, first, firstMs, final, finalMs: Date.now() - t0 });
      assert.ok(final.kind === "plays" || final.kind === "message", `${file}: still ${final.kind} after 90 s`);
      if (final.kind === "message") {
        assert.ok(final.detail.length >= 20, `${file}: the message is not specific: "${final.detail}"`);
      }
      await sleep(300); // let the open settle before the next hand-off
    }
    // Everything plays except the two files that must be refused (with the message checked above).
    const notPlaying = results.filter((r) => r.final.kind !== "plays").map((r) => r.file);
    assert.deepEqual(notPlaying, REFUSED, `files that did not play: ${notPlaying.join(", ")}`);
  });
});
