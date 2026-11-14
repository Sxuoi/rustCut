// EXP-05/06/07: PRD vectors verbatim, plus properties.

import { describe, expect, it } from "vitest";
import {
  MAX_NAME_BYTES,
  TemplateError,
  renderName,
  resolveOutputs,
  sanitizeComponent,
  validateTemplate,
  type NameContext,
  type OutputRequest,
} from "./naming";

const ctx = (over: Partial<NameContext> = {}): NameContext => ({
  index: 2,
  indexWidth: 2,
  name: "Jump Scare Reaction",
  sourceName: "livestream_2026_10_21",
  startSec: 920.12,
  endSec: 955.8,
  date: new Date(2026, 10, 13),
  tagLabel: "Funny",
  preset: "vcrop",
  ...over,
});

describe("EXP-06 sanitisation vectors", () => {
  const cases: [string, string][] = [
    ["Jump Scare Reaction", "Jump_Scare_Reaction"],
    ["What?! Epic/Win: 100%", "What_!_Epic_Win_100%"],
    ["  ...Trailing dots...  ", "Trailing_dots"],
    ["con", "con_"],
    ["aux.final", "aux_.final"],
    ["Rekaman: Bahas Patch 2.0 🔥", "Rekaman_Bahas_Patch_2.0_🔥"],
    ["???", ""],
  ];
  it.each(cases)("%j -> %j", (input, out) => {
    expect(sanitizeComponent(input, true)).toBe(out);
  });

  it("an empty result becomes Clip_{index} when rendered (clip 7)", () => {
    expect(renderName("{name}", ctx({ index: 7, name: "???" }), true)).toBe("Clip_07");
    expect(renderName("{index}_{name}", ctx({ index: 7, name: "" }), true)).toBe("07_Clip_07");
  });

  it("normalises to NFC", () => {
    expect(sanitizeComponent("e\u0301", true)).toBe("\u00e9");
  });

  it("replaces control characters and handles every reserved name", () => {
    expect(sanitizeComponent("a\u0001b\u007fc", true)).toBe("a_b_c");
    for (const n of ["NUL", "prn", "COM1", "lpt9", "Com5.txt"]) {
      expect(sanitizeComponent(n, true)).toMatch(/^[A-Za-z0-9]+_(\.txt)?$/);
    }
    expect(sanitizeComponent("CON1", true)).toBe("CON1");
    expect(sanitizeComponent("console", true)).toBe("console");
  });

  it("keeps spaces when the setting is off (but still trims ends)", () => {
    expect(sanitizeComponent("  Jump Scare  ", false)).toBe("Jump Scare");
  });

  it("truncates to 120 UTF-8 bytes on a character boundary", () => {
    const out = sanitizeComponent("é".repeat(100), true);
    expect(new TextEncoder().encode(out).length).toBeLessThanOrEqual(MAX_NAME_BYTES);
    expect(out).toBe("é".repeat(60));
    const emoji = sanitizeComponent("🔥".repeat(50), true);
    expect(new TextEncoder().encode(emoji).length).toBeLessThanOrEqual(MAX_NAME_BYTES);
    expect(emoji).toBe("🔥".repeat(30));
  });

  it("is idempotent and never produces forbidden characters (property)", () => {
    const pool = ["a", " ", ".", "_", "?", "/", "\\", ":", "é", "🔥", "CON", "\t", "*", "<", ">", "|", '"', "x"];
    let seed = 7;
    const rnd = () => (seed = (seed * 1664525 + 1013904223) >>> 0) / 2 ** 32;
    for (let n = 0; n < 500; n++) {
      const s = Array.from({ length: Math.floor(rnd() * 40) }, () => pool[Math.floor(rnd() * pool.length)]).join("");
      const once = sanitizeComponent(s, true);
      expect(once).not.toMatch(/[<>:"/\\|?*\u0000-\u001f]/);
      expect(once).not.toMatch(/\s/);
      expect(new TextEncoder().encode(once).length).toBeLessThanOrEqual(MAX_NAME_BYTES);
      expect(sanitizeComponent(once, true)).toBe(once);
    }
  });
});

describe("EXP-05 templates", () => {
  it("renders the PRD examples", () => {
    expect(renderName("{index}_{name}", ctx(), true)).toBe("02_Jump_Scare_Reaction");
    expect(renderName("{source_name}_{start}_{name}", ctx({ startSec: 920 }), true)).toBe(
      "livestream_2026_10_21_00-15-20_Jump_Scare_Reaction",
    );
  });

  it("supports every variable", () => {
    // "|" is a forbidden character, so the separators become "_" (EXP-06).
    expect(renderName("{start}|{end}|{duration}|{date}|{tag}|{preset}", ctx(), true)).toBe(
      "00-15-20_00-15-55_36_2026-11-13_Funny_vcrop",
    );
  });

  it("pads {index} to the project width and uses an empty {tag} when untagged", () => {
    expect(renderName("{index}", ctx({ index: 7, indexWidth: 3 }), true)).toBe("007");
    expect(renderName("a{tag}b", ctx({ tagLabel: null }), true)).toBe("ab");
  });

  it("rejects unknown variables, stray and nested braces, and empty templates", () => {
    expect(() => renderName("{nope}", ctx(), true)).toThrow(TemplateError);
    expect(() => renderName("{nope}", ctx(), true)).toThrow(/\{nope\}/);
    expect(() => validateTemplate("{name")).toThrow(/brace/);
    expect(() => validateTemplate("name}")).toThrow(/brace/);
    expect(() => validateTemplate("{a{name}}")).toThrow(/brace/);
    expect(() => validateTemplate("   ")).toThrow(/empty/);
  });

  it("warns when the template ends with an extension", () => {
    expect(validateTemplate("{name}.mp4").warnings[0]).toContain(".mp4");
    expect(validateTemplate("{name}").warnings).toEqual([]);
  });
});

describe("EXP-07 collisions", () => {
  const req = (i: number, base: string, prev: string | null = null): OutputRequest => ({
    clipId: `c${i}`,
    index: i,
    baseName: base,
    previousOutput: prev,
  });
  const none = () => false;

  it("suffixes in-batch duplicates _2, _3 in clip-number order (case-insensitive)", () => {
    const out = resolveOutputs([req(3, "Bit"), req(1, "bit"), req(2, "BIT")], "mp4", "D:/o", "rename", none);
    expect(out.map((o) => o.fileName)).toEqual(["Bit_3.mp4", "bit.mp4", "BIT_2.mp4"]);
  });

  it("keeps the 120-byte limit when adding a suffix", () => {
    const base = "x".repeat(120);
    const out = resolveOutputs([req(1, base), req(2, base)], "mp4", "D:/o", "rename", none);
    expect(out[1]?.fileName).toBe(`${"x".repeat(118)}_2.mp4`);
  });

  it("applies the policy to existing files, except the clip's own previous output", () => {
    const exists = (p: string) => p === "D:/o/a.mp4";
    expect(resolveOutputs([req(1, "a")], "mp4", "D:/o", "rename", exists)[0]).toMatchObject({ fileName: "a_2.mp4", action: "write" });
    expect(resolveOutputs([req(1, "a")], "mp4", "D:/o", "overwrite", exists)[0]?.action).toBe("overwrite");
    expect(resolveOutputs([req(1, "a")], "mp4", "D:/o", "skip", exists)[0]?.action).toBe("skip");
    expect(resolveOutputs([req(1, "a", "D:/o/a.mp4")], "mp4", "D:/o", "rename", exists)[0]).toMatchObject({
      fileName: "a.mp4",
      action: "replace_previous",
    });
  });
});
