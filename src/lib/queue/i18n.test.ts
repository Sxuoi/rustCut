// §8.5: every UI string of the queue/export/tag/timestamp components is
// externalised. This test scans their sources for t("…") / tp("…") keys and
// checks the dynamic key families, so a missing message fails CI instead of
// showing a raw key in the UI.
import { describe, expect, it } from "vitest";
import common from "$lib/i18n/en/common";
import exportMessages from "$lib/i18n/en/export";
import queue from "$lib/i18n/en/queue";
import timeline from "$lib/i18n/en/timeline";
import { SORT_KEYS } from "./search";
import { INCLUDE_MODES } from "./rows";
import { CONFLICT_POLICIES, PRESETS, TEMPLATE_VARIABLES } from "./exportOptions";
import { TIMESTAMP_FORMATS } from "./timestamps";

const mine: Record<string, string> = { ...queue, ...exportMessages };

const RAW = import.meta.glob(
  [
    "/src/lib/components/{queue,export,tags,timestamps}/*.{svelte,ts}",
    "/src/lib/queue/*.{svelte,ts}",
    "/src/lib/state/exporter.svelte.ts",
    "!/src/**/*.test.ts",
  ],
  { query: "?raw", import: "default", eager: true },
) as Record<string, string>;

function sources(): { file: string; text: string }[] {
  return Object.entries(RAW).filter(([f]) => !f.endsWith(".test.ts")).map(([file, text]) => ({ file, text }));
}

describe("§8.5 externalised strings", () => {
  it("every literal t()/tp() key exists", () => {
    expect(sources().length).toBeGreaterThan(10);
    const missing: string[] = [];
    for (const { file, text } of sources()) {
      for (const m of text.matchAll(/\b(tp?)\(\s*"([^"]+)"/g)) {
        const [, fn, key] = m;
        const needed = fn === "tp" ? [`${key}.one`, `${key}.other`] : [key];
        for (const k of needed) if (!(k in mine)) missing.push(`${file}: ${k}`);
      }
    }
    expect(missing).toEqual([]);
  });

  it("every dynamic key family exists", () => {
    const keys = [
      ...INCLUDE_MODES.map((m) => `queue.include.${m}`),
      ...SORT_KEYS.map((k) => `queue.sort.${k}`),
      ...["pending", "exporting", "done", "failed", "outdated"].map((s) => `queue.status.${s}`),
      ...["disk_full", "output_folder_missing"].map((r) => `export.pause.${r}`),
      ...["ok", "warning", "error"].map((l) => `export.preflight.level.${l}`),
      "export.jobs.hint.lossless",
      "export.jobs.hint.reencode",
      ...TEMPLATE_VARIABLES.map((v) => `export.var.${v}`),
      ...["write", "overwrite", "skip", "replace_previous"].map((a) => `export.action.${a}`),
      ...PRESETS.flatMap((p) => [`export.preset.${p}`, `export.preset.${p}.hint`]),
      ...CONFLICT_POLICIES.map((c) => `export.conflict.${c}`),
      ...["empty_label", "duplicate_label", "bad_color"].map((p) => `tags.editor.problem.${p}`),
      ...["export", "import"].map((tab) => `timestamps.tab.${tab}`),
      ...TIMESTAMP_FORMATS.flatMap((f) => [`timestamps.format.${f}`, `timestamps.format.${f}.hint`]),
      ...["B", "KB", "MB", "GB", "TB"].map((u) => `queue.unit.${u}`),
    ];
    expect(keys.filter((k) => !(k in mine))).toEqual([]);
  });

  it("does not collide with other areas' keys", () => {
    const others = new Set([...Object.keys(common), ...Object.keys(timeline)]);
    expect(Object.keys(mine).filter((k) => others.has(k))).toEqual([]);
    expect(Object.keys(mine).every((k) => /^(queue|tags|export|timestamps)\./.test(k))).toBe(true);
  });
});
