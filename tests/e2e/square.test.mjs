// FRM-09 (and FRM-05): the blur presets' output-only preview equals the export. For Square on Blur
// 9:16, Square Blur 1:1 and Vertical Blur 9:16 on the 16:9 luma ramp, the preview's frame, window
// and blurred background are measured in the DOM and on a CDP screenshot, then the clip is exported
// through the app and its first frame is compared band by band with the screenshot.
import { after, before, describe, test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { basename, dirname, join } from "node:path";
import { writeFileSync } from "node:fs";
import { App, FFPROBE, cleanup, existsSync, makeWorkspace, readdirSync } from "./harness.mjs";
import { playhead, rows, waitFor, waitReady } from "./ui.mjs";

const RAMP = "R16x9_ramp_1920x1080.mp4";
const CASES = [
  { framing: "vertical_square", preset: "vsquare", toggle: "9:16 only", offset: 0.3 },
  { framing: "square_blur", preset: "square", toggle: "1:1 only", offset: 0 },
  { framing: "vertical_blur", preset: "vblur", toggle: "9:16 only", offset: 0 },
];

function ffmpegPath() {
  const sidecar = join(dirname(FFPROBE), basename(FFPROBE).replace(/^ffprobe/, "ffmpeg"));
  const configured = [process.env.RUSTCUT_FFMPEG_DIR, process.env.FFMPEG_DIR].filter(Boolean)
    .map((directory) => join(directory, "ffmpeg.exe")).find(existsSync);
  return configured ?? (existsSync(sidecar) ? sidecar : "ffmpeg");
}

/** Gray (RGB average) of an image file or a video's first frame, area-scaled to `w`×`h`. */
function grayAt(file, w, h) {
  const rgb = execFileSync(ffmpegPath(), ["-hide_banner", "-v", "error", "-i", file, "-frames:v", "1",
    "-vf", `scale=${w}:${h}:flags=area`, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], { maxBuffer: 64 << 20 });
  assert.equal(rgb.length, w * h * 3, `${file}: raw frame size`);
  const gray = new Float64Array(w * h);
  for (let i = 0; i < w * h; i++) gray[i] = (rgb[3 * i] + rgb[3 * i + 1] + rgb[3 * i + 2]) / 3;
  return gray;
}

/** Mean of column `x` over rows [y0, y1). */
function column(gray, w, x, y0, y1) {
  let sum = 0;
  for (let y = y0; y < y1; y++) sum += gray[y * w + x];
  return sum / (y1 - y0);
}

/** DOM boxes of the output-only view: the blurred frame and the window's clip box. */
async function boxes(app) {
  return app.page.evaluate(() => {
    const rect = (el) => {
      const r = el.getBoundingClientRect();
      return { x: r.x, y: r.y, w: r.width, h: r.height };
    };
    const out = document.querySelector('[data-testid="blur-backdrop"]');
    const canvas = out?.querySelector("canvas");
    const fg = document.querySelector('[data-testid="player-clip"]');
    const overlay = document.querySelector('[data-testid="crop-overlay"] [role="slider"]');
    return out && canvas && fg ? { out: rect(out), fg: rect(fg), overlay: overlay ? rect(overlay) : null } : null;
  });
}

describe("FRM-09 blur presets: the preview equals the export", { concurrency: false, timeout: 600000 }, () => {
  const ws = makeWorkspace([RAMP]);
  const app = new App(ws);
  const outDir = join(ws.media, RAMP.replace(/\.mp4$/, "_clips"));
  let clip;
  before(async () => {
    await app.start([ws.files[RAMP]]);
    await waitReady(app);
    await app.invoke("add_clip", { req: { start_sec: 0, end_sec: 1, name: "Ramp", tag_id: null } });
    await waitFor(async () => (await rows(app)).length === 1, { what: "ramp clip" });
    await app.page.keyboard.press("ArrowUp");
    await waitFor(async () => (await playhead(app)) === 0, { what: "selected clip In" });
    [clip] = (await app.invoke("get_snapshot")).project.clips;
  });
  after(async () => { await app.stop(); cleanup(ws); });

  for (const c of CASES) {
    test(`${c.preset}: frame, window and blurred background match the exported file`, async () => {
      const current = (await app.invoke("get_snapshot")).project.export_settings;
      const settings = { ...current, cut_mode: "accurate", framing: c.framing, output_resolution: "1080x1920",
        encoder: "libx264", naming_template: "{index}_{preset}", on_conflict: "overwrite" };
      await app.invoke("set_export_settings", { settings });
      await app.invoke("edit_clips", { req: { ids: [clip.id], edit: { kind: "set_crop", crop_x_offset: c.offset } } });
      const layout = await app.invoke("frame_layout", { framing: c.framing, display_width: 1920, display_height: 1080,
        offset: c.offset, output_resolution: "1080x1920" });
      assert.ok(layout?.blur, `${c.preset} has a blurred background`);

      // Geometry: the output frame keeps the export's aspect and the window sits at the layout's place.
      await app.page.getByRole("checkbox", { name: c.toggle, exact: true }).setChecked(true);
      const want = (b) => {
        const k = b.out.w / layout.out_w;
        return { x: b.out.x + layout.fg.x * k, y: b.out.y + layout.fg.y * k, w: layout.fg.w * k, h: layout.fg.h * k, k };
      };
      const b = await waitFor(async () => {
        const got = await boxes(app);
        if (!got) return null;
        const w = want(got);
        const close = ["x", "y", "w", "h"].every((key) => Math.abs(got.fg[key] - w[key]) <= 1);
        return close && Math.abs(got.out.h - layout.out_h * w.k) <= 1 ? got : null;
      }, { timeout: 15000, what: `${c.preset}: output frame and window boxes within 1 px of the layout` });
      if (b.overlay) {
        for (const key of ["x", "y", "w", "h"]) assert.ok(Math.abs(b.overlay[key] - b.fg[key]) <= 1, `overlay ${key} on the window`);
      }
      // Let the canvas paint the current frame (a seek redraws it too).
      await app.page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
      const shot = join(ws.root, `${c.preset}-preview.png`);
      writeFileSync(shot, await app.page.screenshot({ clip: { x: b.out.x, y: b.out.y, width: b.out.w, height: b.out.h } }));

      // Export the same clip through the app.
      await app.invoke("start_export", { clip_ids: [clip.id], settings });
      await waitFor(async () => (await app.invoke("get_snapshot")).project.clips[0].export_status === "completed",
        { timeout: 120000, every: 250, what: `${c.preset} export` });
      const file = readdirSync(outDir).find((f) => f.endsWith(`_${c.preset}.mp4`));
      assert.ok(file, `exported ${c.preset} file in ${readdirSync(outDir)}`);

      // Compare preview and export on a common quarter-size grid.
      const [w, h] = [layout.out_w / 4, layout.out_h / 4];
      const preview = grayAt(shot, w, h);
      const exported = grayAt(join(outDir, file), w, h);
      const fg = { x: layout.fg.x / 4, y: layout.fg.y / 4, w: layout.fg.w / 4, h: layout.fg.h / 4 };
      const checks = [
        // The sharp window, away from the overlay's border and label.
        { name: "window", x0: fg.x + fg.w * 0.06, x1: fg.x + fg.w * 0.94, y0: fg.y + fg.h * 0.3, y1: fg.y + fg.h * 0.7, tol: 8 },
        // The blurred bands above and below it, nearly to the frame's edges: FFmpeg's boxblur
        // mirrors the picture at its edges, and so does the preview's canvas padding.
        { name: "background above", x0: w * 0.02, x1: w * 0.98, y0: fg.y * 0.15, y1: fg.y * 0.85, tol: 10 },
        { name: "background below", x0: w * 0.02, x1: w * 0.98, y0: fg.y + fg.h + (h - fg.y - fg.h) * 0.15, y1: h - (h - fg.y - fg.h) * 0.15, tol: 10 },
      ];
      for (const ch of checks) {
        const [y0, y1] = [Math.round(ch.y0), Math.round(ch.y1)];
        assert.ok(y1 - y0 >= 4, `${ch.name}: band of ${y1 - y0} rows`);
        let worst = 0;
        let sum = 0;
        let n = 0;
        for (let x = Math.round(ch.x0); x < Math.round(ch.x1); x += 3) {
          const p = column(preview, w, x, y0, y1);
          const e = column(exported, w, x, y0, y1);
          worst = Math.max(worst, Math.abs(p - e));
          sum += e;
          n += 1;
          assert.ok(Math.abs(p - e) <= ch.tol, `${c.preset} ${ch.name} column ${x}: preview ${p.toFixed(1)}, export ${e.toFixed(1)}`);
        }
        // The ramp is mid-gray on average: a black (unpainted) band would fail here, not above.
        assert.ok(sum / n > 40, `${c.preset} ${ch.name}: band mean ${(sum / n).toFixed(1)} is not empty`);
        console.log(`${c.preset} ${ch.name}: largest preview/export difference ${worst.toFixed(1)} gray levels`);
      }
      await app.page.getByRole("checkbox", { name: c.toggle, exact: true }).setChecked(false);
    });
  }
});
