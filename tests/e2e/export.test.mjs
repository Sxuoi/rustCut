// End-to-end: export, failure, cancel and crash-safety against the real desktop app.
// Covers EXP-02/05/08/09/10, AC-09, AC-10, section 8.2 (kill test) and FRM vertical output.
import { test, describe, after } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, existsSync, mkdirSync, writeFileSync, rmSync, statSync } from "node:fs";
import { App, makeWorkspace, cleanup, sleep, join, findProcs, ffprobeJson, FIXTURES } from "./harness.mjs";
import { waitFor, waitReady, press, seekTo, rows, readProject, body } from "./ui.mjs";

const track = [];
function session(fixture) {
  const ws = makeWorkspace([fixture]);
  track.push(ws);
  const name = fixture.replace(/\.[^.]+$/, "");
  return { ws, name, video: ws.files[fixture], project: join(ws.media, `${name}.rustcut`), outDir: join(ws.media, `${name}_clips`), app: new App(ws) };
}
after(() => {
  for (const ws of track) cleanup(ws);
});

/** Add clips with the keyboard only; each range is [in, out] in whole seconds. */
async function addClips(app, ranges) {
  let n = (await rows(app)).length;
  for (const [a, b] of ranges) {
    await seekTo(app, a);
    await press(app, "i");
    await seekTo(app, b);
    await press(app, "o");
    await press(app, "Enter");
    n += 1;
    await waitFor(async () => (await rows(app)).length === n, { what: `clip ${n}` });
  }
}

const statuses = async (app) => (await rows(app)).map((r) => r.status.toUpperCase());
const files = (dir, re) => (existsSync(dir) ? readdirSync(dir).filter((f) => re.test(f)) : []);
const partFiles = (dir) => files(dir, /\.part$/);

async function chooseCtrlE(app) {
  await press(app, "Control+e");
  await app.page.getByRole("button", { name: /^Start/ }).waitFor({ timeout: 10000 });
  // No wait for the pre-flight: Enter pressed while it runs queues the start (PRD 3.1 cold flow).
  // Enter starts the export only once focus is inside the dialog (otherwise it would add a clip).
  await waitFor(() => app.page.evaluate(() => !!document.activeElement?.closest("[role=dialog]")), { what: "focus inside the export dialog" });
}

async function choosePreset(app, label) {
  await app.page.getByRole("button", { name: /^Preset:/ }).click();
  await app.page.getByRole("menuitemradio", { name: label }).or(app.page.getByRole("menuitem", { name: label })).first().click();
}

describe("export (lossless, vertical, failure)", { concurrency: false }, () => {
  const s = session("M03_long_gop_10s.mp4");
  after(async () => s.app.stop());

  test("lossless export of 3 clips: Ctrl+E, Enter -> files on disk and DONE rows (EXP-02, EXP-10)", async () => {
    await s.app.start([s.video]);
    await waitReady(s.app);
    await addClips(s.app, [[2, 5], [8, 11], [14, 17]]);
    await chooseCtrlE(s.app);
    // Cold start (fresh data folder, no hw_encoders.json): a Lossless export must not wait for the
    // old 6-10 s hardware-encoder probe. Time Enter until the first row leaves PENDING.
    const t0 = Date.now();
    await press(s.app, "Enter");
    await waitFor(async () => (await statuses(s.app)).some((x) => !x.startsWith("PENDING")), { timeout: 20000, every: 25, what: "the first row to start" });
    const startMs = Date.now() - t0;
    assert.ok(startMs < 3000, `cold Lossless export took ${startMs} ms to start`);
    await waitFor(async () => (await statuses(s.app)).every((x) => x.startsWith("DONE")), { timeout: 40000, what: "3 DONE rows" });
    const out = files(s.outDir, /\.mp4$/);
    assert.equal(out.length, 3, `outputs: ${out}`);
    assert.deepEqual(partFiles(s.outDir), []);
    for (const f of out) {
      const j = ffprobeJson(join(s.outDir, f));
      assert.ok(j.streams.some((x) => x.codec_type === "video"), `${f} has video`);
      // Lossless starts on the keyframe at or before In (this fixture has up to 8 s of lead-in).
      const dur = Number(j.format.duration);
      assert.ok(dur >= 2.9 && dur <= 11.5, `${f} duration ${dur}`);
    }
    const disk = readProject(s.project).clips.map((c) => c.export_status);
    assert.deepEqual(disk, ["completed", "completed", "completed"], "statuses are persisted");
  });

  test("vertical crop export is 1080x1920 (FRM-02, AC-17)", async () => {
    // This project already exported these clips, so the vertical files replace the lossless ones.
    await choosePreset(s.app, "Vertical Crop 9:16");
    await chooseCtrlE(s.app);
    await press(s.app, "Enter");
    const dims = () => files(s.outDir, /.mp4$/).map((f) => {
      try {
        const v = ffprobeJson(join(s.outDir, f)).streams.find((x) => x.codec_type === "video");
        return [v.width, v.height];
      } catch {
        return null;
      }
    });
    await waitFor(() => partFiles(s.outDir).length === 0 && dims().length === 3 && dims().every((d) => d && d[0] === 1080 && d[1] === 1920), {
      timeout: 120000,
      every: 1000,
      what: "3 outputs of 1080x1920",
    });
    await waitFor(async () => (await statuses(s.app)).every((x) => x.startsWith("DONE")), { what: "DONE rows after the vertical export" });
  });
});

describe("failing clip (AC-09)", { concurrency: false }, () => {
  const s = session("M03_long_gop_10s.mp4");
  after(async () => s.app.stop());

  test("one clip is built to fail: the others finish, the failed row says why and offers Retry", async () => {
    await s.app.start([s.video]);
    await waitReady(s.app);
    await addClips(s.app, [[2, 5], [8, 11], [14, 17]]);
    // Build the failure: a non-empty folder sits where clip #02's file must go, so its final rename
    // cannot succeed (the other two clips are unaffected). "Overwrite" lets the pre-flight accept it.
    mkdirSync(join(s.outDir, "02_Clip_02.mp4"), { recursive: true });
    writeFileSync(join(s.outDir, "02_Clip_02.mp4", "blocker.txt"), "x");
    await chooseCtrlE(s.app);
    await s.app.page.getByRole("radio", { name: "Overwrite" }).check();
    await waitFor(() => s.app.page.getByRole("button", { name: /^Start/ }).isEnabled(), { what: "Start enabled for Overwrite" });
    await s.app.page.getByRole("button", { name: /^Start/ }).click();
    await waitFor(async () => (await statuses(s.app)).every((x) => !/PENDING|EXPORTING/.test(x)), { timeout: 60000, what: "the batch to finish" });
    const st = await statuses(s.app);
    assert.ok(st[0].startsWith("DONE") && st[2].startsWith("DONE"), `statuses ${st}`);
    assert.equal(st[1], "FAILED");
    assert.deepEqual(partFiles(s.outDir), []);
    assert.ok(existsSync(join(s.outDir, "02_Clip_02.mp4", "blocker.txt")), "the blocking folder is untouched");
    const row = s.app.page.locator("[data-clip-id]").nth(1);
    const retry = row.getByTestId("retry-clip");
    await retry.waitFor({ timeout: 5000 });
    // The row itself shows a readable reason next to Retry (not a bare error code).
    const reason = ((await row.getByTestId("failure-reason").textContent()) ?? "").trim();
    assert.ok(reason.length > 20 && /[a-z]{4,} [a-z]{3,}/i.test(reason), `reason: ${reason}`);
    assert.ok(await retry.isEnabled(), "Retry is enabled once the batch is finished");
    // Fix the cause and Retry: only the failed clip runs again and now completes.
    rmSync(join(s.outDir, "02_Clip_02.mp4"), { recursive: true, force: true });
    await retry.click();
    await waitFor(async () => (await statuses(s.app)).every((x) => x.startsWith("DONE")), { timeout: 60000, what: "all rows DONE after Retry" });
    assert.ok(statSync(join(s.outDir, "02_Clip_02.mp4")).isFile());
  });
});

describe("bulk include (QUE-04)", { concurrency: false }, () => {
  const s = session("M03_long_gop_10s.mp4");
  after(async () => s.app.stop());

  test("Include > Not exported is one undo step", async () => {
    await s.app.start([s.video]);
    await waitReady(s.app);
    await addClips(s.app, [[2, 4], [6, 8], [10, 12]]);
    const box = (i) => s.app.page.locator("[data-clip-id] input[type=checkbox]").nth(i);
    const included = () => s.app.page.evaluate(() => [...document.querySelectorAll("[data-clip-id] input[type=checkbox]")].map((c) => c.checked));
    // Export clip 1 only.
    await box(1).uncheck();
    await box(2).uncheck();
    await chooseCtrlE(s.app);
    await press(s.app, "Enter");
    await waitFor(async () => (await statuses(s.app))[0].startsWith("DONE"), { timeout: 30000, what: "clip 1 DONE" });
    await box(0).check();
    await box(1).check();
    await box(2).check();
    const before = await included();
    assert.deepEqual(before, [true, true, true]);
    // The menu button shows "Include"; its accessible name is the longer aria-label.
    await s.app.page.getByRole("button", { name: "Choose which clips to include in the export" }).click();
    await s.app.page.getByRole("menuitem", { name: /^Not exported/ }).click();
    await waitFor(async () => JSON.stringify(await included()) === "[false,true,true]", { what: "Not exported applied" });
    await s.app.page.locator("[data-clip-id]").nth(1).click();
    await press(s.app, "Control+z");
    await waitFor(async () => JSON.stringify(await included()) === JSON.stringify(before), { what: "one undo restores all three checkboxes" });
  });
});

describe("cancel and crash safety (AC-10, section 8.2)", { concurrency: false }, () => {
  async function startSlowExport(s) {
    await s.app.start([s.video]);
    await waitReady(s.app);
    await addClips(s.app, [[1, 11]]);
    await choosePreset(s.app, "Vertical Blur 9:16"); // software re-encode: seconds, not milliseconds
    await chooseCtrlE(s.app);
    await press(s.app, "Enter");
    await waitFor(() => partFiles(s.outDir).length > 0, { timeout: 30000, every: 20, what: "a .part file (FFmpeg running)" });
    await waitFor(() => findProcs("ffmpeg.exe", s.ws.root).length > 0, { timeout: 10000, every: 20, what: "an FFmpeg process" });
  }

  test("AC-10 cancelling mid-clip leaves no .part and no FFmpeg process", async () => {
    const s = session("M01_baseline_1080p60.mp4");
    try {
      await startSlowExport(s);
      await s.app.page.getByRole("button", { name: /Cancel the export of clip #01/ }).click();
      await waitFor(async () => !(await statuses(s.app)).some((x) => x.startsWith("EXPORTING")), { timeout: 15000, what: "the row to leave EXPORTING" });
      await waitFor(() => findProcs("ffmpeg.exe", s.ws.root).length === 0, { timeout: 8000, what: "no FFmpeg process" });
      assert.deepEqual(partFiles(s.outDir), [], "no .part left");
      assert.deepEqual(files(s.outDir, /\.mp4$/), [], "no partial output left");
      assert.ok(!(await statuses(s.app)).some((x) => x.startsWith("DONE")));
    } finally {
      await s.app.stop();
    }
  });

  test("killing the app mid-export leaves no orphan FFmpeg; relaunch resets statuses and removes the stale .part", async () => {
    const s = session("M01_baseline_1080p60.mp4");
    try {
      await startSlowExport(s);
      s.app.kill({ tree: false }); // only the app: FFmpeg must die through the Job Object (PRD 7.8)
      await s.app.exited;
      await waitFor(() => findProcs("ffmpeg.exe", s.ws.root).length === 0, { timeout: 10000, what: "no orphaned FFmpeg after the app was killed" });
      await sleep(500);
      assert.equal(readProject(s.project)?.clips.length, 1, "the project is intact after the kill");
      s.app = new App(s.ws);
      await s.app.start([s.video]);
      await waitReady(s.app);
      await waitFor(async () => (await statuses(s.app)).every((x) => x.startsWith("PENDING")), { timeout: 15000, what: "statuses reset to pending" });
      await waitFor(() => partFiles(s.outDir).length === 0, { timeout: 10000, what: "the stale .part to be removed" });
      assert.equal(findProcs("ffmpeg.exe", s.ws.root).length, 0);
    } finally {
      await s.app.stop();
    }
  });
});
