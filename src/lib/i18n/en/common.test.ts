// §8.5: every UI string of the fe-shell files is externalised; a missing key
// would silently show the raw key, so every literal t("…") key must exist.
import { describe, expect, it } from "vitest";
import common from "./common";

// Raw sources of every file in scope (Vite glob; no node typings needed).
const RAW = import.meta.glob(
  [
    "/src/App.svelte",
    "/src/lib/components/player/Player.svelte",
    "/src/lib/components/player/Transport.svelte",
    "/src/lib/components/shell/*.{svelte,ts}",
    "/src/lib/keyboard/*.ts",
    "/src/lib/util/marks.ts",
    "/src/lib/state/playback.ts",
    "/src/lib/state/session.ts",
    "/src/lib/state/project.svelte.ts",
    "!/src/**/*.test.ts",
  ],
  { query: "?raw", import: "default", eager: true },
) as Record<string, string>;

const FILES = Object.keys(RAW).filter((f) => !f.endsWith(".test.ts"));

describe("common messages", () => {
  it("scans a non-trivial set of files", () => {
    expect(FILES.length).toBeGreaterThan(10);
  });

  it.each(FILES)("every t() key used in %s exists", (file) => {
    const text = RAW[file];
    const keys = [...text.matchAll(/\bt\(\s*"([^"]+)"/g)].map((m) => m[1]);
    for (const k of keys) expect(common[k], `${file}: ${k}`).toBeTruthy();
  });

  it("placeholders are well-formed", () => {
    for (const [k, v] of Object.entries(common)) {
      expect(v.split("{").length, k).toBe(v.split("}").length);
    }
  });
});
