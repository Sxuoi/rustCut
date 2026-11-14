// UI helpers (keyboard-first, mirrors PRD section 5) shared by the e2e suites.
import { readFileSync } from "node:fs";
import { sleep } from "./harness.mjs";

/** "HH:MM:SS.mmm" to seconds (null when it does not parse). */
export const parseTimecode = (s) => {
  const m = /(\d+):(\d\d):(\d\d)\.(\d{3})/.exec(s);
  return m ? +m[1] * 3600 + +m[2] * 60 + +m[3] + +m[4] / 1000 : null;
};

/** Visible text of the whole window on one line. */
export async function body(app) {
  return app.page.evaluate(() => document.body.innerText.replace(/\n+/g, " | "));
}

/** Playhead in seconds as displayed by the transport. */
export async function playhead(app) {
  const m = /Position \| (\d+:\d\d:\d\d\.\d{3}) \//.exec(await body(app));
  return m ? parseTimecode(m[1]) : null;
}

/** Pending In/Out in seconds (null when unset). */
export async function pending(app) {
  const t = await body(app);
  const i = /Pending In: \| ([-\d:.]+)/.exec(t);
  const o = /Pending Out: \| ([-\d:.]+)/.exec(t);
  return { inSec: i ? parseTimecode(i[1]) : null, outSec: o ? parseTimecode(o[1]) : null };
}

/** Poll `fn` until it returns a truthy value or `timeout` ms pass. */
export async function waitFor(fn, { timeout = 10000, every = 50, what = "condition" } = {}) {
  const end = Date.now() + timeout;
  let last;
  while (Date.now() < end) {
    try {
      last = await fn();
      if (last) return last;
    } catch (e) {
      last = e;
    }
    await sleep(every);
  }
  throw new Error(`Timed out after ${timeout} ms waiting for ${what}${last instanceof Error ? `: ${last.message}` : ""}`);
}

export const press = (app, key) => app.page.keyboard.press(key);

/** Wait until the editor is up with a playable video. */
export async function waitReady(app) {
  await waitFor(
    () =>
      app.page.evaluate(() => {
        const v = document.querySelector("video");
        return !!v && v.readyState >= 2 && /CLIP QUEUE/i.test(document.body.innerText);
      }),
    { timeout: 30000, what: "the editor and a playable video" },
  );
}

/** Seek with real keys until the playhead equals `sec` (J/L = 5 s, arrows = 1 s). */
export async function seekTo(app, sec) {
  for (let guard = 0; guard < 40; guard++) {
    const now = await playhead(app);
    if (Math.abs(sec - now) < 0.02) return;
    const d = Math.round(sec - now);
    const key = d >= 5 ? "l" : d <= -5 ? "j" : d > 0 ? "ArrowRight" : "ArrowLeft";
    const target = now + (key === "l" ? 5 : key === "j" ? -5 : d > 0 ? 1 : -1);
    await press(app, key);
    // A key pressed while the previous seek is still in flight can be dropped: re-evaluate and press again.
    await waitFor(async () => Math.abs((await playhead(app)) - target) < 0.02, { timeout: 1500, what: `playhead ${target}` }).catch(() => {});
  }
  throw new Error(`could not seek to ${sec}`);
}

/** Clip rows as rendered: [{id, label, name, range, status, text}]. */
export async function rows(app) {
  return app.page.evaluate(() =>
    [...document.querySelectorAll("[data-clip-id]")].map((el) => {
      const lines = el.innerText.split("\n").map((s) => s.trim()).filter(Boolean);
      return {
        id: el.getAttribute("data-clip-id"),
        label: lines[0],
        name: el.querySelector("[data-clip-name]")?.textContent?.trim() ?? "",
        range: lines.find((l) => /\d:\d\d:\d\d\.\d{3} > /.test(l)) ?? "",
        status: lines.find((l) => /^(PENDING|EXPORTING|DONE|FAILED|OUTDATED)/i.test(l)) ?? "",
        text: lines.join(" | "),
      };
    }),
  );
}

/** Visible toast messages. */
export async function toasts(app) {
  return app.page.evaluate(() =>
    [...document.querySelectorAll('section[aria-label="Notifications"] [role="status"], section[aria-label="Notifications"] [role="alert"]')].map((e) =>
      e.innerText.replace(/\s+/g, " ").trim(),
    ),
  );
}

/** The sidecar project (PRJ-01): parsed JSON, or null while missing or mid-write. */
export function readProject(file) {
  try {
    return JSON.parse(readFileSync(file, "utf8"));
  } catch {
    return null;
  }
}
