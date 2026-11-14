// AC-13: missing P0 transport, loop and persistence coverage, using real keys only.
import { after, before, describe, test } from "node:test";
import assert from "node:assert/strict";
import { App, cleanup, join, makeWorkspace, sleep } from "./harness.mjs";
import { body, playhead, readProject, rows, waitFor, waitReady } from "./ui.mjs";

describe("AC-13 keyboard transport", { concurrency: false, timeout: 240000 }, () => {
  const ws = makeWorkspace(["M03_long_gop_10s.mp4"]);
  const app = new App(ws);
  const key = (value) => app.page.keyboard.press(value);
  const paused = () => app.page.evaluate(() => document.querySelector("video").paused);
  const projectFile = join(ws.media, "M03_long_gop_10s.rustcut");

  before(async () => {
    await app.start([ws.files["M03_long_gop_10s.mp4"]]);
    await waitReady(app);
    // Diagnostics: who unloads or reloads the player's video element (printed when a test fails).
    await app.page.evaluate(() => {
      const log = (window.__mediaCalls = []);
      const t0 = performance.now();
      const wrap = (proto, name, filter = () => true) => {
        const original = proto[name];
        proto[name] = function (...args) {
          if (this instanceof HTMLVideoElement && filter(args)) {
            const text = document.querySelector('[aria-label="Video player"], [role="region"]')?.innerText?.replace(/\s+/g, " ").slice(0, 80) ?? "";
            log.push(`${Math.round(performance.now() - t0)}ms ${name}(${args.join(",")}) rs=${this.readyState} ui="${text}" :: ${(new Error().stack ?? "").split("\n").slice(2, 7).map((s) => s.trim().replace(/\(http:\/\/tauri\.localhost\/assets\//, "(")).join(" < ")}`);
          }
          return original.apply(this, args);
        };
      };
      wrap(HTMLMediaElement.prototype, "load");
      wrap(Element.prototype, "removeAttribute", ([a]) => a === "src");
    });
  });
  after(async () => { await app.stop(); cleanup(ws); });

  async function at(target, tolerance = 0.002) {
    await waitFor(async () => Math.abs((await playhead(app)) - target) <= tolerance &&
      await app.page.evaluate(() => !document.querySelector("video").seeking),
    { timeout: 15000, what: `presented playhead at ${target}` });
  }

  // Reset using keys, without the retry-and-swallow behaviour of the legacy seek helper.
  async function zero() {
    if (!(await paused())) await key("k");
    for (let guard = 0; guard < 20; guard++) {
      const time = await playhead(app);
      if (time === 0) return;
      await key("j");
      await at(Math.max(0, time - 5), 0.04);
    }
    assert.fail("J never reached the beginning");
  }

  test("Space and K each start playback and pause on a stable frame", async () => {
    for (const binding of ["Space", "k"]) {
      await zero();
      await key(binding);
      await waitFor(async () => !(await paused()) && (await playhead(app)) > 0.2,
        { timeout: 15000, what: `${binding} starts advancing` });
      await key(binding);
      await waitFor(paused, { timeout: 10000, what: `${binding} pauses` });
      await sleep(250); // allow the last presented-frame callback to settle
      const time = await playhead(app);
      await sleep(700);
      assert.equal(await playhead(app), time, `${binding}: paused time stays fixed`);
    }
  });

  test("J/L seek five seconds and arrows seek one second in both directions", async () => {
    await zero();
    for (const [binding, target] of [["l", 5], ["l", 10], ["j", 5], ["ArrowRight", 6], ["ArrowLeft", 5]]) {
      await key(binding);
      await at(target);
    }
  });

  test("Period and Comma present exact adjacent 30 fps frames and pause", async () => {
    await zero();
    await key("ArrowRight");
    await at(1);
    // Record presentation timestamps, not currentTime (the decoder seeks 1 ms into a frame).
    await app.page.evaluate(() => {
      window.__keyboardFrames = [];
      const video = document.querySelector("video");
      const frame = (_, meta) => {
        window.__keyboardFrames.push(meta.mediaTime);
        video.requestVideoFrameCallback(frame);
      };
      video.requestVideoFrameCallback(frame);
    });
    for (const [binding, frame] of [["Period", 31], ["Period", 32], ["Comma", 31], ["Comma", 30]]) {
      const count = await app.page.evaluate(() => window.__keyboardFrames.length);
      await key(binding);
      await waitFor(() => app.page.evaluate(([n, expected]) => {
        const times = window.__keyboardFrames;
        return times.length > n && Math.abs(times.at(-1) - expected) < 0.0001;
      }, [count, frame / 30]), { timeout: 15000, what: `frame ${frame} after ${binding}` });
      assert.equal(await paused(), true);
      await at(frame / 30);
    }
    await key("k");
    await waitFor(async () => !(await paused()), { what: "playing before frame step" });
    await key("Period");
    await waitFor(paused, { what: "frame step pauses playback" });
  });

  test("Shift punctuation traverses the speed ladder shown in the transport", async () => {
    await zero();
    for (const [binding, rate] of [
      ["Shift+Comma", 0.5], ["Shift+Period", 1], ["Shift+Period", 1.25],
      ["Shift+Period", 1.5], ["Shift+Period", 2], ["Shift+Period", 3],
      ["Shift+Comma", 2], ["Shift+Comma", 1.5], ["Shift+Comma", 1.25], ["Shift+Comma", 1],
    ]) {
      await key(binding);
      await waitFor(() => app.page.evaluate((expected) => {
        const speed = document.querySelector('button[aria-label^="Playback speed:"]');
        return document.querySelector("video").playbackRate === expected &&
          speed?.textContent.includes(`${expected}×`);
      }, rate), { timeout: 10000, what: `transport speed ${rate}×` });
    }
    // Regression: a speed change re-ran the player's attach effect, whose teardown unloaded the video.
    const loaded = await app.page.evaluate(() => {
      const v = document.querySelector("video");
      return { readyState: v.readyState, networkState: v.networkState, src: v.getAttribute("src") !== null };
    });
    assert.ok(loaded.src && loaded.readyState >= 2 && loaded.networkState !== 0, `the video stays loaded after speed changes: ${JSON.stringify(loaded)}`);
  });

  test("P loops the selected clip repeatedly; Delete/undo and Ctrl+S persist it", async () => {
    await zero();
    await key("ArrowRight"); await at(1); await key("i");
    await key("ArrowRight"); await at(2); await key("o"); await key("Enter");
    await waitFor(async () => (await rows(app)).length === 1, { what: "keyboard-created clip" });
    await key("ArrowUp"); await at(1);
    const snapshot = await rows(app);
    await key("p"); // P enables looping AND starts playback.
    // The player's own "Loop" badge (not the "Loop as exported" control) over a loaded, playing video.
    await waitFor(() => app.page.evaluate(() => {
      const v = document.querySelector("video");
      const badge = [...document.querySelectorAll('[role="region"] div')].some((d) => d.textContent.trim() === "Loop");
      return badge && !v.paused && v.readyState >= 2;
    }), { what: "loop playback" });
    // Sample until two wraps were seen (a 1 s clip wraps about once per second); a busy machine
    // can stall decoding for a while, so allow up to 15 s before calling it a failure.
    const samples = await app.page.evaluate(() => new Promise((resolve) => {
      const video = document.querySelector("video");
      const values = [];
      let wraps = 0;
      const start = performance.now();
      const tick = () => {
        const t = video.currentTime;
        if (values.length > 0 && t < values[values.length - 1] - 0.5) wraps++;
        values.push(t);
        if (wraps < 2 && performance.now() - start < 15000) requestAnimationFrame(tick);
        else resolve(values);
      };
      requestAnimationFrame(tick);
    }));
    const wraps = samples.filter((t, i) => i > 0 && t < samples[i - 1] - 0.5).length;
    if (wraps < 2) {
      const state = await app.page.evaluate(() => {
        const v = document.querySelector("video");
        return { paused: v.paused, seeking: v.seeking, readyState: v.readyState, networkState: v.networkState, rate: v.playbackRate,
          currentTime: v.currentTime, error: v.error?.code ?? null, src: v.currentSrc.slice(-40) };
      });
      console.log(`loop diagnostics: ${JSON.stringify(state)} first ${samples.slice(0, 5)} last ${samples.slice(-5)}`);
      console.log(`media calls since start:\n  ${(await app.page.evaluate(() => window.__mediaCalls ?? [])).slice(-20).join("\n  ")}`);
      console.log(`player text: ${await app.page.evaluate(() => document.querySelector('[role="region"]')?.innerText?.replace(/\s+/g, " ").slice(0, 200))}`);
    }
    assert.ok(samples.length > 20, "sampled live playback");
    assert.ok(samples.every((t) => t >= 1 - 1 / 30 && t <= 2 + 1 / 30), "all samples stay in the selected clip (one-frame tolerance)");
    assert.ok(wraps >= 2, `at least two loop wraps (saw ${wraps} in ${samples.length} samples)`);
    await key("k"); await key("p");
    await key("Delete");
    await waitFor(async () => (await rows(app)).length === 0, { what: "Delete removes selection" });
    await key("Control+z");
    await waitFor(async () => (await rows(app)).length === 1, { what: "Ctrl+Z restores selection" });
    assert.deepEqual((await rows(app)).map(({ id, range }) => [id, range]), snapshot.map(({ id, range }) => [id, range]));
    await key("Control+s");
    await waitFor(async () => readProject(projectFile)?.clips.length === 1 && /Project saved\./.test(await body(app)),
      { timeout: 15000, what: "Ctrl+S acknowledgement and .rustcut file" });
  });

  test("two quick L presses accumulate to +10 seconds across an in-flight seek", async () => {
    let exercisedInFlight = false;
    for (let attempt = 0; attempt < 3 && !exercisedInFlight; attempt++) {
      await zero();
      // "In flight" = the first seek's frame is not painted yet when the second key arrives: that is
      // when the player still shows the old frame and must build on the pending target (PLY-02).
      // (The element's own `seeking` flag can already be false then: a buffered seek resolves at the
      // next stable state, before the second key's task runs.)
      await app.page.evaluate(() => {
        const video = document.querySelector("video");
        const burst = (window.__keyboardBurst = { keys: 0, frames: 0, framesAtFirst: 0, framesBetween: null });
        const onFrame = () => {
          burst.frames++;
          if (burst.framesBetween === null) video.requestVideoFrameCallback(onFrame);
        };
        video.requestVideoFrameCallback(onFrame);
        const capture = (event) => {
          if (event.key.toLowerCase() !== "l") return;
          burst.keys++;
          if (burst.keys === 1) burst.framesAtFirst = burst.frames;
          if (burst.keys === 2) {
            burst.framesBetween = burst.frames - burst.framesAtFirst;
            removeEventListener("keydown", capture, true);
          }
        };
        addEventListener("keydown", capture, true);
      });
      // Two real, separate presses sent back to back (down/up/down/up issued before any of them is
      // awaited), so the second keydown reaches the page while the first seek is still running.
      // Neither keydown is an auto-repeat: the first is released before the second goes down.
      const kb = app.page.keyboard;
      await Promise.all([kb.down("l"), kb.up("l"), kb.down("l"), kb.up("l")]);
      await at(10);
      exercisedInFlight = await app.page.evaluate(() => window.__keyboardBurst.framesBetween === 0);
    }
    assert.ok(exercisedInFlight, "the second real L keydown arrived before the first seek's frame was painted");
    await sleep(500);
    await at(10); // a late callback must not roll the accumulated target back
  });

  test("a frame step pressed right after a seek builds on the seek target (PLY-02)", async () => {
    await zero();
    // Do not wait for the seek to settle: Period must step from the pending target, not the old frame.
    await Promise.all([app.page.keyboard.press("ArrowRight"), app.page.keyboard.press("Period")]);
    await waitFor(async () => (await playhead(app)) >= 1 + 1 / 30 - 0.002 && (await paused()), { timeout: 15000, what: "1 s plus one frame, paused" });
    await at(31 / 30, 0.004);
    assert.equal(await paused(), true);
  });

  test("ArrowUp and ArrowDown select the previous and next clip and go to its In (QUE-03)", async () => {
    // The loop test left clip #1 at [1, 2]; add #2 at [3, 4] with keys only.
    await zero();
    for (const t of [1, 2, 3]) { await key("ArrowRight"); await at(t); }
    await key("i");
    await key("ArrowRight"); await at(4);
    await key("o"); await key("Enter");
    await waitFor(async () => (await rows(app)).length === 2, { what: "second keyboard-created clip" });
    const selected = () => app.page.evaluate(() =>
      [...document.querySelectorAll("[data-clip-id]")].findIndex((r) => r.getAttribute("aria-selected") === "true" || r.closest('[aria-selected="true"]') !== null));
    // Whatever is selected now (nothing, #1 or #2), two Ups end on #1: Up never moves past the first clip.
    await key("ArrowUp");
    await sleep(200);
    await key("ArrowUp");
    await at(1);
    await key("ArrowDown");
    await at(3);
    await key("ArrowDown"); // already the last clip: nothing moves
    await sleep(300);
    await at(3);
    await key("ArrowUp");
    await at(1);
    const index = await selected();
    if (index !== -1) assert.equal(index, 0, "clip #1 is the selected row");
  });
});
