import { describe, expect, it } from "vitest";
import {
  finalizeTags,
  isHexColor,
  nextTagColor,
  slugify,
  tagLetter,
  TAG_PALETTE,
  tagUsage,
  uniqueTagId,
  validateTags,
} from "./tags";

describe("MRK-06 tag ids", () => {
  it("slugifies labels", () => {
    expect(slugify("Funny Moments!")).toBe("funny-moments");
    expect(slugify("  Épico  ")).toBe("epico");
    expect(slugify("Rekaman 2.0")).toBe("rekaman-2-0");
    expect(slugify("???")).toBe("tag");
    expect(slugify("日本")).toBe("日本");
  });

  it("makes ids unique", () => {
    expect(uniqueTagId("Funny", [])).toBe("funny");
    expect(uniqueTagId("Funny", ["funny"])).toBe("funny-2");
    expect(uniqueTagId("funny", ["funny", "funny-2"])).toBe("funny-3");
  });

  it("keeps existing ids and gives new tags unique slugs", () => {
    const tags = finalizeTags([
      { id: "funny", label: " Hilarious ", color: "#EF4444" },
      { id: null, label: "Funny", color: "#22c55e" },
      { id: null, label: "Funny", color: "#3b82f6" },
    ]);
    expect(tags).toEqual([
      { id: "funny", label: "Hilarious", color: "#ef4444" },
      { id: "funny-2", label: "Funny", color: "#22c55e" },
      { id: "funny-3", label: "Funny", color: "#3b82f6" },
    ]);
  });
});

describe("MRK-06 colours and validation", () => {
  it("picks the first unused palette colour, cycling when all are used", () => {
    expect(nextTagColor([])).toBe(TAG_PALETTE[0]);
    expect(nextTagColor([{ color: TAG_PALETTE[0].toUpperCase() }])).toBe(TAG_PALETTE[1]);
    const all = TAG_PALETTE.map((color) => ({ color }));
    expect(nextTagColor(all)).toBe(TAG_PALETTE[all.length % TAG_PALETTE.length]);
  });

  it("accepts only #RRGGBB", () => {
    expect(isHexColor("#22c55e")).toBe(true);
    expect(isHexColor("#22C55E")).toBe(true);
    expect(isHexColor("#fff")).toBe(false);
    expect(isHexColor("red")).toBe(false);
  });

  it("requires non-empty unique labels and valid colours", () => {
    expect(
      validateTags([
        { label: "Funny", color: "#ef4444" },
        { label: " ", color: "#ef4444" },
        { label: "funny", color: "#ef4444" },
        { label: "Epic", color: "blue" },
      ]),
    ).toEqual([
      { index: 1, problem: "empty_label" },
      { index: 2, problem: "duplicate_label" },
      { index: 3, problem: "bad_color" },
    ]);
    expect(validateTags([{ label: "A", color: "#000000" }])).toEqual([]);
  });
});

describe("§8.5 tag chips", () => {
  it("abbreviates labels to one letter (never colour alone)", () => {
    expect(tagLetter("funny")).toBe("F");
    expect(tagLetter("  épico")).toBe("É");
    expect(tagLetter("🔥 hot")).toBe("🔥");
    expect(tagLetter("")).toBe("?");
  });

  it("counts clips per tag", () => {
    const usage = tagUsage([{ tag_id: "a" }, { tag_id: null }, { tag_id: "a" }, { tag_id: "b" }]);
    expect([...usage]).toEqual([
      ["a", 2],
      ["b", 1],
    ]);
  });
});
