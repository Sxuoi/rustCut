// End-to-end: keyboard-first editing, persistence and relink against the real desktop app.
// Covers AC-01, AC-02, AC-03, AC-05, AC-06, AC-13 (PRD section 10) and PRJ-01/03/04, MRK-01/04.
import { test, describe, after, before } from "node:test";
import assert from "node:assert/strict";
import { copyFileSync, renameSync } from "node:fs";
import { App, makeWorkspace, cleanup, sleep, join, FIXTURES } from "./harness.mjs";
import { waitFor, waitReady, press, seekTo, rows, toasts, pending, playhead, readProject, body } from "./ui.mjs";

const NAME = "M03_long_gop_10s";
const ws = makeWorkspace([`${NAME}.mp4`]);
const video = ws.files[`${NAME}.mp4`];
const projectFile = join(ws.media, `${NAME}.rustcut`);
let app = new App(ws);
const clipsOnDisk = () => readProject(projectFile)?.clips ?? [];

describe("editing session", { concurrency: false }, () => {
  before(async () => {
    await app.start([video]);
    await waitReady(app);
  });
  after(async () => {
    await app.stop();
    cleanup(ws);
  });

  test("AC-01 I, O, Enter adds a clip: row within 100 ms, file on disk within 1 s", async () => {
    await seekTo(app, 2);
    await press(app, "i");
    await seekTo(app, 7);
    await press(app, "o");
    assert.deepEqual(await pending(app), { inSec: 2, outSec: 7 });
    // Measure inside the page: keydown (capture) -> first clip row inserted.
    await app.page.evaluate(() => {
      window.__lat = null;
      const before = document.querySelectorAll("[data-clip-id]").length;
      addEventListener("keydown", (e) => { if (e.key === "Enter") window.__t0 = e.timeStamp; }, { capture: true, once: true });
      new MutationObserver((_, obs) => {
        if (document.querySelectorAll("[data-clip-id]").length > before) {
          window.__lat = performance.now() - window.__t0;
          obs.disconnect();
        }
      }).observe(document.body, { childList: true, subtree: true });
    });
    const t0 = Date.now();
    await press(app, "Enter");
    await waitFor(async () => (await app.page.evaluate(() => window.__lat)) !== null, { what: "the queue row" });
    const lat = await app.page.evaluate(() => window.__lat);
    assert.ok(lat < 100, `queue row appeared after ${lat.toFixed(1)} ms (limit 100)`);
    await waitFor(() => clipsOnDisk().length === 1, { timeout: 1000, every: 20, what: "the clip in the .rustcut file within 1 s" });
    assert.ok(Date.now() - t0 <= 1000, `on disk after ${Date.now() - t0} ms`);
    const [c] = clipsOnDisk();
    assert.equal(c.start_sec, 2);
    assert.equal(c.end_sec, 7);
    assert.deepEqual(await pending(app), { inSec: null, outSec: null }, "marks are cleared after Enter");
  });

  test("AC-02 Out first, then an earlier In, then Enter; In then Enter uses the playhead", async () => {
    await seekTo(app, 12);
    await press(app, "o");
    await seekTo(app, 9);
    await press(app, "i");
    await press(app, "Enter");
    await waitFor(async () => (await rows(app)).length === 2, { what: "clip #2" });
    await seekTo(app, 15);
    await press(app, "i");
    await seekTo(app, 20);
    await press(app, "Enter");
    await waitFor(async () => (await rows(app)).length === 3, { what: "clip #3" });
    await waitFor(() => clipsOnDisk().length === 3, { timeout: 2000, what: "3 clips on disk" });
    assert.deepEqual(clipsOnDisk().map((c) => [c.start_sec, c.end_sec]), [[2, 7], [9, 12], [15, 20]]);
  });

  test("AC-03 a conflicting mark clears the other and shows a toast; no clip has Out <= In", async () => {
    await seekTo(app, 25);
    await press(app, "i");
    await seekTo(app, 22);
    await press(app, "o");
    await waitFor(async () => (await toasts(app)).length > 0, { what: "a conflict toast" });
    const p = await pending(app);
    assert.ok(p.inSec === null || p.outSec === null, `the conflicting mark was not cleared: ${JSON.stringify(p)}`);
    await press(app, "Enter"); // must not create an invalid clip
    await sleep(400);
    for (const c of clipsOnDisk()) assert.ok(c.end_sec > c.start_sec, `invalid clip ${c.start_sec}-${c.end_sec}`);
    for (const r of await rows(app)) {
      const [a, b] = r.range.split(" > ");
      assert.ok(a < b, `row ${r.range}`);
    }
    await press(app, "Escape");
    assert.deepEqual(await pending(app), { inSec: null, outSec: null });
  });

  test("AC-13 N renames; shortcuts are suspended while typing; Enter commits and returns focus", async () => {
    // Settle: drop any clip the previous test's Enter may have produced from a valid leftover mark.
    const count = (await rows(app)).length;
    const before = await playhead(app);
    await press(app, "n");
    await waitFor(() => app.page.evaluate(() => document.activeElement?.matches("input[type=text]")), { what: "the rename field" });
    await app.page.keyboard.type("Kick off io", { delay: 15 });
    assert.deepEqual(await pending(app), { inSec: null, outSec: null }, "typed i/o must not set marks");
    assert.equal(await playhead(app), before, "typed text must not move the playhead");
    assert.equal(await app.page.evaluate(() => document.querySelector("video").paused), true, "typed space/k must not start playback");
    await press(app, "Enter");
    await waitFor(async () => (await rows(app)).some((r) => r.name === "Kick off io"), { what: "renamed row" });
    await waitFor(() => clipsOnDisk().some((c) => c.name === "Kick off io"), { timeout: 2000, what: "name on disk" });
    assert.equal((await rows(app)).length, count);
    await press(app, "i"); // focus is back on the player: shortcuts work again
    assert.notEqual((await pending(app)).inSec, null);
    await press(app, "Escape");
  });

  test("undo / redo restore the exact prior state (rename, delete)", async () => {
    const names = async () => (await rows(app)).map((r) => r.name);
    const n0 = (await rows(app)).length;
    await press(app, "Control+z");
    await waitFor(async () => !(await names()).includes("Kick off io"), { what: "undo of the rename" });
    await press(app, "Control+Shift+z");
    await waitFor(async () => (await names()).includes("Kick off io"), { what: "redo of the rename" });
    const snapshot = await rows(app);
    await press(app, "Delete");
    await waitFor(async () => (await rows(app)).length === n0 - 1, { what: "delete" });
    await press(app, "Control+z");
    await waitFor(async () => (await rows(app)).length === n0, { what: "undo of the delete" });
    assert.deepEqual((await rows(app)).map((r) => [r.name, r.range]), snapshot.map((r) => [r.name, r.range]));
    // Name clip #1 too, keyboard only: Up selects the previous clip and seeks to its In.
    for (let i = 0; i < n0; i++) await press(app, "ArrowUp");
    await waitFor(async () => (await playhead(app)) === 2, { what: "selection seek to the first clip" });
    await press(app, "n");
    await app.page.keyboard.type("Intro");
    await press(app, "Enter");
    await waitFor(() => clipsOnDisk().some((c) => c.name === "Intro"), { timeout: 2000, what: "Intro on disk" });
  });

  test("AC-05 restart: clips, names, playhead and view state are restored", async () => {
    await seekTo(app, 18);
    // The detail timeline zooms with +/- while it has keyboard focus (Tab-reachable).
    await app.page.locator('[aria-describedby="zoom-summary"]').focus();
    await press(app, "+");
    await press(app, "+");
    await sleep(1500); // view state is debounced
    const brief = async () => (await rows(app)).map(({ label, name, range }) => ({ label, name, range }));
    const expectRows = await brief();
    const pos = await playhead(app);
    const zoomText = async () => /Detail window of ([d:.]+)/.exec(await body(app))?.[1];
    const zoomBefore = await zoomText();
    const view = readProject(projectFile).view_state;
    assert.ok(view.zoom_window_sec < 20, `zoom changed from the 20 s default: ${view.zoom_window_sec}`);
    assert.equal(view.playhead_sec, pos, "playhead saved");
    await app.stop();
    app = new App(ws);
    await app.start([video]);
    await waitReady(app);
    await waitFor(async () => (await rows(app)).length === expectRows.length, { what: "clips after restart" });
    assert.deepEqual(await brief(), expectRows);
    assert.equal(await playhead(app), pos, "playhead");
    assert.equal(await zoomText(), zoomBefore, "the detail timeline zoom shown in the UI");
    assert.deepEqual(readProject(projectFile).view_state, view, "view state (zoom, selection) unchanged on disk");
    assert.ok(expectRows.some((r) => r.name === "Intro") && expectRows.some((r) => r.name === "Kick off io"));
  });

  test("AC-06 moved source: Locate offers the match and restores every marker; a different file warns", async () => {
    await app.stop();
    const renamed = join(ws.media, "renamed_copy.mp4");
    renameSync(video, renamed);
    const impostor = join(ws.media, "impostor.mp4");
    copyFileSync(join(FIXTURES, "M06_ramp_1440x1080.mp4"), impostor); // 2 s long: clips fall outside it
    app = new App(ws);
    await app.start([projectFile]);
    await waitFor(async () => /Source video missing/.test(await body(app)), { timeout: 20000, what: "the missing-source banner" });
    assert.equal((await rows(app)).length, 3, "clips stay visible while the source is missing");

    // A different file: the backend refuses without accept_mismatch and says why (PRJ-04).
    const res = await app.invoke("relink_source", { path: impostor, accept_mismatch: false });
    assert.equal(res.kind, "mismatch", JSON.stringify(res));
    assert.ok(res.clips_out_of_range > 0, "the mismatch reports clips beyond the new duration");

    // The matching file: the dialog suggests it (fingerprint match) and one click relinks.
    await app.page.getByRole("button", { name: "Locate…" }).first().click();
    const suggestion = app.page.getByRole("button", { name: renamed });
    await suggestion.waitFor({ timeout: 10000 });
    await suggestion.click();
    await waitReady(app);
    await waitFor(async () => !/Source video missing/.test(await body(app)), { what: "the banner to go away" });
    assert.deepEqual((await rows(app)).map((r) => r.range), ["00:00:02.000 > 00:00:07.000", "00:00:09.000 > 00:00:12.000", "00:00:15.000 > 00:00:20.000"]);
    await waitFor(() => readProject(projectFile)?.source?.path === renamed, { timeout: 3000, what: "the new source path on disk" });

    // And it survives a restart.
    await app.stop();
    app = new App(ws);
    await app.start([projectFile]);
    await waitReady(app);
    assert.equal((await rows(app)).length, 3);
  });
});
