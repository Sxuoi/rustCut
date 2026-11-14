import { describe, expect, it } from "vitest";
import type { Tag } from "$lib/bindings";
import { compareClips, matchesQuery, normalizeText, SORT_KEYS, tokenize, visibleClips } from "./search";
import { makeClip } from "./testClips";

const tags: Tag[] = [{ id: "funny", label: "Funny", color: "#ef4444" }];
const ctx = { tags };

const opening = makeClip({ id: "a", index: 1, name: "Opening Moment", start_sec: 130.5, end_sec: 180 });
const jump = makeClip({
  id: "b",
  index: 2,
  name: "Jump Scare Reaction",
  start_sec: 920.12,
  end_sec: 955.8,
  tag_id: "funny",
});
const patch = makeClip({ id: "c", index: 3, name: "Épico Patch 2.0", start_sec: 2520, end_sec: 2610 });
const unnamed = makeClip({ id: "d", index: 12, name: "", start_sec: 10, end_sec: 15 });
const all = [opening, jump, patch, unnamed];

describe("QUE-06 search", () => {
  it("normalises case and accents and splits tokens", () => {
    expect(normalizeText("Épico")).toBe("epico");
    expect(tokenize("  jump   scare ")).toEqual(["jump", "scare"]);
    expect(tokenize("   ")).toEqual([]);
  });

  it("matches names case- and accent-insensitively, every token (AND)", () => {
    expect(matchesQuery(jump, "jump scare", ctx)).toBe(true);
    expect(matchesQuery(jump, "jump opening", ctx)).toBe(false);
    expect(matchesQuery(patch, "epico", ctx)).toBe(true);
    expect(matchesQuery(patch, "ÉPICO", ctx)).toBe(true);
  });

  it("matches clip numbers with #NN or plain digits", () => {
    expect(matchesQuery(jump, "#2", ctx)).toBe(true);
    expect(matchesQuery(jump, "#02", ctx)).toBe(true);
    expect(matchesQuery(jump, "#3", ctx)).toBe(false);
    expect(matchesQuery(unnamed, "12", ctx)).toBe(true);
    expect(matchesQuery(patch, "#", ctx)).toBe(true);
  });

  it("matches times: timecode text or a time inside the range", () => {
    expect(matchesQuery(jump, "15:20", ctx)).toBe(true);
    expect(matchesQuery(jump, "00:15:55.800", ctx)).toBe(true);
    expect(matchesQuery(jump, "15:30", ctx)).toBe(true);
    expect(matchesQuery(jump, "16:30", ctx)).toBe(false);
    expect(matchesQuery(opening, "2:30", ctx)).toBe(true);
  });

  it("matches tag labels", () => {
    expect(matchesQuery(jump, "funny", ctx)).toBe(true);
    expect(matchesQuery(opening, "funny", ctx)).toBe(false);
  });

  it("an empty query matches everything", () => {
    expect(visibleClips(all, { query: " ", sortKey: "index", sortDir: "asc", tags })).toHaveLength(4);
  });
});

describe("QUE-06 sort (view only)", () => {
  const ids = (key: (typeof SORT_KEYS)[number], dir: "asc" | "desc" = "asc") =>
    visibleClips(all, { query: "", sortKey: key, sortDir: dir, tags }).map((c) => c.id);

  it("sorts by number, time, duration and name", () => {
    expect(ids("index")).toEqual(["a", "b", "c", "d"]);
    expect(ids("index", "desc")).toEqual(["d", "c", "b", "a"]);
    expect(ids("time")).toEqual(["d", "a", "b", "c"]);
    expect(ids("duration")).toEqual(["d", "b", "a", "c"]);
    expect(ids("name")).toEqual(["c", "b", "a", "d"]);
  });

  it("keeps unnamed clips last in both directions", () => {
    expect(ids("name", "desc")).toEqual(["a", "b", "c", "d"]);
  });

  it("breaks ties by clip number", () => {
    const x = makeClip({ id: "x", index: 7, start_sec: 5, end_sec: 6 });
    const y = makeClip({ id: "y", index: 4, start_sec: 5, end_sec: 6 });
    expect([x, y].sort(compareClips("time", "asc")).map((c) => c.id)).toEqual(["y", "x"]);
    expect([x, y].sort(compareClips("time", "desc")).map((c) => c.id)).toEqual(["y", "x"]);
  });

  it("never renumbers or mutates the clips", () => {
    const before = JSON.stringify(all);
    const sorted = visibleClips(all, { query: "", sortKey: "name", sortDir: "desc", tags });
    expect(JSON.stringify(all)).toBe(before);
    expect(sorted.map((c) => c.index).sort((p, q) => p - q)).toEqual([1, 2, 3, 12]);
    expect(sorted).not.toBe(all);
  });
});
