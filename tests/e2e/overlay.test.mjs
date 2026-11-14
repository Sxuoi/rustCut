// AC-18: measure actual DOM geometry against the crop command used by export.
import { after, before, describe, test } from "node:test";
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { basename, dirname, join } from "node:path";
import { App, FFPROBE, cleanup, existsSync, handOffToWindow, makeWorkspace } from "./harness.mjs";
import { playhead, rows, waitFor, waitReady } from "./ui.mjs";

const RAMPS = [
  ["M06_ramp_1440x1080.mp4", 1440, 1080],
  ["M07_ramp_2560x1080.mp4", 2560, 1080],
  ["M08_ramp_1080x1920.mp4", 1080, 1920],
  ["M09_ramp_1080x2340.mp4", 1080, 2340],
  ["M10_ramp_720x576_sar.mp4", 768, 576],
  ["R16x9_ramp_1920x1080.mp4", 1920, 1080],
  // The remaining §7.7 display sizes derive from the same ramp; generated only in our workspace.
  ["R4k_ramp_3840x2160.mp4", 3840, 2160, true],
  ["R720p_ramp_1280x720.mp4", 1280, 720, true],
  ["Rsquare_ramp_1080x1080.mp4", 1080, 1080, true],
];
const OFFSETS = [-1, 0, 0.3, 1];

/** Object-contain leaves letterboxing INSIDE the video element in full-frame mode. */
async function geometry(app, width, height) {
  return app.page.evaluate(([dw, dh]) => {
    const video = document.querySelector("video");
    const box = video.getBoundingClientRect();
    // Use display dimensions, including SAR, rather than videoWidth/coded dimensions.
    const scale = Math.min(box.width / dw, box.height / dh);
    const content = { x: box.x + (box.width - dw * scale) / 2, y: box.y + (box.height - dh * scale) / 2,
      w: dw * scale, h: dh * scale };
    const overlay = document.querySelector('[data-testid="crop-overlay"] [role="slider"]').getBoundingClientRect();
    return { content, screen: { x: overlay.x, y: overlay.y, w: overlay.width, h: overlay.height },
      source: { x: (overlay.x - content.x) / scale, y: (overlay.y - content.y) / scale,
        w: overlay.width / scale, h: overlay.height / scale } };
  }, [width, height]);
}

describe("AC-18 crop overlay equals export window", { concurrency: false, timeout: 600000 }, () => {
  const ws = makeWorkspace(RAMPS.filter(([, , , generated]) => !generated).map(([file]) => file));
  const app = new App(ws);
  before(async () => {
    const sidecar = join(dirname(FFPROBE), basename(FFPROBE).replace(/^ffprobe/, "ffmpeg"));
    const configured = [process.env.RUSTCUT_FFMPEG_DIR, process.env.FFMPEG_DIR].filter(Boolean)
      .map((directory) => join(directory, "ffmpeg.exe")).find(existsSync);
    const ffmpeg = configured ?? (existsSync(sidecar) ? sidecar : "ffmpeg");
    for (const [file, width, height, generated] of RAMPS) {
      if (!generated) continue;
      ws.files[file] = join(ws.media, file);
      await promisify(execFile)(ffmpeg, ["-hide_banner", "-loglevel", "error", "-nostdin", "-y",
        "-stream_loop", "-1", "-i", ws.files["R16x9_ramp_1920x1080.mp4"], "-t", "2",
        "-vf", `fps=2,scale=${width}:${height},setsar=1`, "-an", "-c:v", "libx264",
        "-preset", "ultrafast", "-threads", "2", "-pix_fmt", "yuv420p", ws.files[file]], { windowsHide: true });
    }
    await app.start();
  });
  after(async () => { await app.stop(); cleanup(ws); });

  for (const [file, width, height] of RAMPS) {
    test(`${file}: four offsets in full-frame and 9:16-only modes`, async () => {
      await handOffToWindow(app, ws.files[file]);
      await app.page.waitForFunction((path) => {
        const video = document.querySelector("video");
        return video && decodeURIComponent(video.currentSrc).replaceAll("\\", "/").endsWith(path.replaceAll("\\", "/")) && video.readyState >= 2;
      }, ws.files[file], { timeout: 30000 });
      await waitReady(app);
      const snapshot = await app.invoke("get_snapshot");
      assert.equal(snapshot.project.source.video.display_width, width, "SAR-normalised width");
      assert.equal(snapshot.project.source.video.display_height, height);
      await app.invoke("set_export_settings", { settings: {
        ...snapshot.project.export_settings, cut_mode: "accurate", framing: "vertical_crop",
        output_resolution: "1080x1920",
      } });
      await app.invoke("add_clip", { req: { start_sec: 0, end_sec: 1, name: null, tag_id: null } });
      await waitFor(async () => (await rows(app)).length === 1, { what: "ramp clip" });
      await app.page.keyboard.press("ArrowUp");
      await waitFor(async () => (await playhead(app)) === 0, { what: "selected clip In" });
      const [clip] = (await app.invoke("get_snapshot")).project.clips;
      const toggle = app.page.getByRole("checkbox", { name: "9:16 only", exact: true });
      let fixed = null;
      let firstContent = null;
      for (const only of [false, true]) {
        await toggle.setChecked(only);
        for (const offset of OFFSETS) {
          await app.invoke("edit_clips", { req: { ids: [clip.id], edit: { kind: "set_crop", crop_x_offset: offset } } });
          const expected = await app.invoke("crop_window", { display_width: width, display_height: height, offset });
          await waitFor(async () => {
            const value = await app.page.locator('[data-testid="crop-overlay"] [role="slider"]').getAttribute("aria-valuenow");
            return Number(value) === offset && value !== null;
          }, { timeout: 15000, what: `committed offset ${offset}` });
          // Wait for both the state patch and the authoritative remote window to render.
          const actual = await waitFor(async () => {
            const result = await geometry(app, width, height);
            return Object.keys(expected).every((k) => Math.abs(result.source[k] - expected[k]) <= 1) && result;
          }, { timeout: 15000, what: `${file}, only=${only}, offset=${offset}: geometry within one display pixel of ${JSON.stringify(expected)}` });
          for (const component of ["x", "y", "w", "h"]) {
            assert.ok(Math.abs(actual.source[component] - expected[component]) <= 1,
              `${component}: DOM ${actual.source[component]}, export ${expected[component]}`);
          }
          if (only) {
            fixed ??= actual.screen;
            for (const k of ["x", "y", "w", "h"]) assert.ok(Math.abs(actual.screen[k] - fixed[k]) < 0.1, `9:16 window ${k} stays fixed`);
            firstContent ??= actual.content;
            if (offset === 1 && expected.w < width) assert.ok(actual.content.x < firstContent.x - 1, "video moves left under the fixed window");
          }
        }
      }
    });
  }
});
