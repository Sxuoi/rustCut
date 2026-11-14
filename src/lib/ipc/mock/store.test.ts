// Executable documentation of the ProjectStore contract the mock mirrors
// (MRK-01/03/04/06, QUE-02/05/07). The Rust store has the same rules.

import { describe, expect, it } from "vitest";
import type { Clip, ExportRecord, Project } from "$lib/bindings";
import { BackendError } from "../backend";
import { UNDO_LIMIT } from "./model";
import { sampleSource, newProject } from "./sources";
import { MockProjectStore, clipFields, contentHash, duplicateOf, indexWidth } from "./store";

function makeStore(minClip = 0.5): MockProjectStore {
  return new MockProjectStore(newProject(sampleSource(), true), { minClipSec: () => minClip });
}

function thrown(fn: () => unknown): BackendError {
  try {
    fn();
  } catch (e) {
    if (e instanceof BackendError) return e;
    throw e;
  }
  throw new Error("expected an error");
}

/** The user-visible state undo/redo must restore exactly. */
function userState(p: Project) {
  return JSON.stringify({
    clips: p.clips.map((c) => ({ id: c.id, index: c.index, ...clipFields(c) })),
    tags: p.tags,
  });
}

const add = (s: MockProjectStore, a: number, b: number, name = "") => s.addClip({ start_sec: a, end_sec: b, name, tag_id: null });

describe("MRK-04 crop undo coalescing", () => {
  function timedStore() {
    let now = 0;
    const s = new MockProjectStore(newProject(sampleSource(), true), { minClipSec: () => 0.5, now: () => now });
    return { s, advance: (ms: number) => { now += ms; } };
  }

  it("keeps the first before and latest after with a sliding 1.5 s window", () => {
    const { s, advance } = timedStore();
    const a = add(s, 1, 5).clip;
    const depth = s.undoDepth();
    const revision = s.revision;
    for (const offset of [0.1, 0.2, 0.3]) {
      s.editClips([a.id], { kind: "set_crop", crop_x_offset: offset });
      advance(1500);
    }
    expect(s.undoDepth()).toBe(depth + 1);
    expect(s.revision).toBe(revision + 3);
    s.undo();
    expect(s.clip(a.id)!.crop_x_offset).toBe(0);
    s.redo();
    expect(s.clip(a.id)!.crop_x_offset).toBe(0.3);
  });

  it("compares requested sets including unchanged clips and ignores order and duplicates", () => {
    const { s } = timedStore();
    const a = add(s, 1, 5).clip;
    const b = add(s, 6, 10).clip;
    s.editClips([a.id], { kind: "set_crop", crop_x_offset: 0.1 });
    s.editClips([a.id, b.id], { kind: "set_crop", crop_x_offset: 0.1 });
    const depth = s.undoDepth();
    s.editClips([b.id, a.id, a.id], { kind: "set_crop", crop_x_offset: 0.2 });
    expect(s.undoDepth()).toBe(depth);
    s.undo();
    expect(s.clip(a.id)!.crop_x_offset).toBe(0.1);
    expect(s.clip(b.id)!.crop_x_offset).toBe(0);
    s.redo();
    expect(s.clip(a.id)!.crop_x_offset).toBe(0.2);
    expect(s.clip(b.id)!.crop_x_offset).toBe(0.2);
  });

  it.each(["timeout", "rename", "undo", "batch"])("starts a new entry after %s", (reason) => {
    const { s, advance } = timedStore();
    const a = add(s, 1, 5).clip;
    s.editClips([a.id], { kind: "set_crop", crop_x_offset: 0.1 });
    if (reason === "timeout") advance(1501);
    if (reason === "rename") s.editClips([a.id], { kind: "rename", name: "renamed" });
    if (reason === "undo") { s.undo(); s.redo(); }
    if (reason === "batch") s.editClipsBatch([{ ids: [a.id], edit: { kind: "set_crop", crop_x_offset: 0.15 } }]);
    const depth = s.undoDepth();
    s.editClips([a.id], { kind: "set_crop", crop_x_offset: 0.2 });
    expect(s.undoDepth()).toBe(depth + 1);
    expect(s.history().can_redo).toBe(false);
  });

  it("does not break a chain for no-ops or view and system updates", () => {
    const { s } = timedStore();
    const a = add(s, 1, 5).clip;
    s.editClips([a.id], { kind: "set_crop", crop_x_offset: 0.1 });
    const depth = s.undoDepth();
    s.editClips([a.id], { kind: "rename", name: "" });
    s.setViewState({ playhead_sec: 3, zoom_window_sec: 20, selected_clip_id: a.id });
    s.markExporting([a.id]);
    s.editClips([a.id], { kind: "set_crop", crop_x_offset: 0.2 });
    expect(s.undoDepth()).toBe(depth);
  });
});

describe("atomic batch edits", () => {
  it("validates sequential edits against their predecessors and omits net no-ops", () => {
    const s = makeStore();
    const a = add(s, 1, 5).clip;
    const before = userState(s.project);
    const revision = s.revision;
    const depth = s.undoDepth();
    s.editClipsBatch([
      { ids: [a.id], edit: { kind: "set_included", included: false } },
      { ids: [a.id], edit: { kind: "set_included", included: true } },
    ]);
    expect(s.revision).toBe(revision);
    expect(s.undoDepth()).toBe(depth);
    expect(userState(s.project)).toBe(before);
    s.editClipsBatch([
      { ids: [a.id], edit: { kind: "retime", start_sec: 2, end_sec: null } },
      { ids: [a.id], edit: { kind: "retime", start_sec: null, end_sec: 3 } },
    ]);
    expect([s.clip(a.id)!.start_sec, s.clip(a.id)!.end_sec]).toEqual([2, 3]);
    expect(s.undoDepth()).toBe(depth + 1);
    s.undo();
    expect(userState(s.project)).toBe(before);
  });
});

describe("add_clip validation (MRK-01)", () => {
  it("clamps to [0, duration] and rounds to milliseconds", () => {
    const s = makeStore();
    const r = s.addClip({ start_sec: -5, end_sec: 1000, name: null, tag_id: null });
    expect([r.clip.start_sec, r.clip.end_sec]).toEqual([0, 180]);
    const r2 = add(s, 1.23456, 2.98765);
    expect([r2.clip.start_sec, r2.clip.end_sec]).toEqual([1.235, 2.988]);
  });

  it("rejects clips shorter than the minimum with clip_too_short stating the minimum", () => {
    const s = makeStore();
    const e = thrown(() => add(s, 10, 10.4));
    expect(e.code).toBe("clip_too_short");
    expect(e.message).toContain("0.5 s");
    expect(s.project.clips).toHaveLength(0);
    expect(s.revision).toBe(1);
  });

  it("uses the min-duration setting (0.1-5 s)", () => {
    expect(() => add(makeStore(0.1), 10, 10.15)).not.toThrow();
    const e = thrown(() => add(makeStore(2), 10, 11.9));
    expect(e.code).toBe("clip_too_short");
    expect(e.message).toContain("2 s");
  });

  it("accepts exactly the minimum", () => {
    expect(add(makeStore(), 10, 10.5).clip.end_sec).toBe(10.5);
  });

  it("requires Out > In (invalid_range), and In before the source end", () => {
    const s = makeStore();
    expect(thrown(() => add(s, 20, 10)).code).toBe("invalid_range");
    expect(thrown(() => add(s, 10, 10)).code).toBe("invalid_range");
    const past = thrown(() => add(s, 181, 200));
    expect(past.code).toBe("invalid_range");
    expect(past.message).toContain("end of the source");
    expect(thrown(() => add(s, Number.NaN, 5)).code).toBe("invalid_range");
  });

  it("rejects an unknown tag and keeps a known one", () => {
    const s = makeStore();
    expect(thrown(() => s.addClip({ start_sec: 1, end_sec: 5, name: null, tag_id: "nope" })).code).toBe("invalid_input");
    expect(s.addClip({ start_sec: 1, end_sec: 5, name: " Joke ", tag_id: "funny" }).clip).toMatchObject({
      tag_id: "funny",
      name: "Joke",
    });
  });

  it("flags identical ranges as duplicates (MRK-03) but allows overlap", () => {
    const s = makeStore();
    const a = add(s, 10, 20);
    expect(a.duplicate_of).toBeNull();
    expect(add(s, 15, 25).duplicate_of).toBeNull();
    const dup = add(s, 10, 20);
    expect(dup.duplicate_of).toBe(a.clip.index);
    expect(duplicateOf(s.project, 10, 20, a.clip.id)).toBe(dup.clip.index);
  });

  it("defaults: included, pending, centred crop, UUID id", () => {
    const c = add(makeStore(), 1, 5).clip;
    expect(c).toMatchObject({ selected_for_export: true, export_status: "pending", crop_x_offset: 0, last_export: null });
    expect(c.id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-/);
  });
});

describe("revisions and patches", () => {
  it("bumps the revision by exactly one per change and returns the matching patch", () => {
    const s = makeStore();
    const r0 = s.revision;
    const a = add(s, 1, 5);
    expect(a.patch.revision).toBe(r0 + 1);
    expect(a.patch.upserted.map((c) => c.id)).toEqual([a.clip.id]);
    const p2 = s.editClips([a.clip.id], { kind: "rename", name: "x" });
    expect(p2.revision).toBe(r0 + 2);
    const p3 = s.deleteClips([a.clip.id]);
    expect(p3.revision).toBe(r0 + 3);
    expect(p3.removed).toEqual([a.clip.id]);
    expect(s.undo()?.revision).toBe(r0 + 4);
    expect(s.redo()?.revision).toBe(r0 + 5);
    expect(s.setTags([{ id: "a", label: "A", color: "#112233" }]).revision).toBe(r0 + 6);
    expect(s.renumberClips().revision).toBe(r0 + 6); // nothing to renumber: no-op
  });

  it("a no-op returns the current revision with no upserts and no undo step", () => {
    const s = makeStore();
    const a = add(s, 1, 5, "x");
    const depth = s.undoDepth();
    const rev = s.revision;
    const patch = s.editClips([a.clip.id], { kind: "rename", name: "x" });
    expect(patch).toMatchObject({ revision: rev, upserted: [], removed: [], tags: null, export_settings: null });
    expect(s.undoDepth()).toBe(depth);
    expect(s.deleteClips([]).revision).toBe(rev);
    expect(s.setExportSettings(s.project.export_settings).revision).toBe(rev);
  });

  it("patch carries next_index, history and the changed tags/settings", () => {
    const s = makeStore();
    add(s, 1, 5);
    const p = s.setTags([{ id: "x", label: "X", color: "#abcdef" }]);
    expect(p.tags).toEqual([{ id: "x", label: "X", color: "#ABCDEF" }]);
    expect(p.next_index).toBe(2);
    expect(p.history).toMatchObject({ can_undo: true, undo_label: "Undo Edit tags" });
    const q = s.setExportSettings({ ...s.project.export_settings, parallel_jobs: 9, naming_template: "  " });
    expect(q.export_settings).toMatchObject({ parallel_jobs: 4, naming_template: "{index}_{name}" });
  });

  it("view state changes neither the revision nor the undo history", () => {
    const s = makeStore();
    const a = add(s, 1, 5);
    const rev = s.revision;
    s.setViewState({ playhead_sec: 999, zoom_window_sec: 1, selected_clip_id: a.clip.id });
    expect(s.project.view_state).toEqual({ playhead_sec: 180, zoom_window_sec: 2, selected_clip_id: a.clip.id });
    s.setViewState({ playhead_sec: 5, zoom_window_sec: 30, selected_clip_id: "ghost" });
    expect(s.project.view_state.selected_clip_id).toBeNull();
    expect(s.revision).toBe(rev);
  });
});

describe("stable clip numbers (QUE-02)", () => {
  it("never reuses a number after delete", () => {
    const s = makeStore();
    const [a, b, c] = [add(s, 1, 5), add(s, 6, 9), add(s, 10, 14)];
    expect([a, b, c].map((x) => x.clip.index)).toEqual([1, 2, 3]);
    s.deleteClips([c.clip.id]);
    expect(add(s, 20, 25).clip.index).toBe(4);
  });

  it("never reuses a number after undo of an add, and redo restores the same number", () => {
    const s = makeStore();
    const a = add(s, 1, 5);
    s.undo();
    expect(s.project.clips).toHaveLength(0);
    expect(s.project.next_index).toBe(2);
    const b = add(s, 6, 9);
    expect(b.clip.index).toBe(2);
    expect(a.clip.index).toBe(1);
    s.undo();
    expect(s.undo()).toBeNull(); // a new add cleared the redo stack; nothing left
    expect(s.project.clips).toHaveLength(0);
    s.redo();
    expect(s.project.clips[0]?.index).toBe(2); // redo restores the same number
    expect(s.project.next_index).toBe(3);
  });

  it("restores a deleted clip with its number and position", () => {
    const s = makeStore();
    const ids = [add(s, 1, 5), add(s, 6, 9), add(s, 10, 14)].map((x) => x.clip.id);
    s.deleteClips([ids[0] as string, ids[2] as string]);
    s.undo();
    expect(s.project.clips.map((c) => [c.id, c.index])).toEqual([
      [ids[0], 1],
      [ids[1], 2],
      [ids[2], 3],
    ]);
  });

  it("renumber chronologically keeps next_index and is undoable", () => {
    const s = makeStore();
    add(s, 50, 55);
    add(s, 10, 15);
    add(s, 30, 35);
    s.renumberClips();
    expect(s.project.clips.map((c) => [c.start_sec, c.index])).toEqual([
      [50, 3],
      [10, 1],
      [30, 2],
    ]);
    expect(s.project.next_index).toBe(4);
    expect(add(s, 1, 3).clip.index).toBe(4);
    s.undo(); // the add
    s.undo(); // the renumber
    expect(s.project.clips.map((c) => c.index)).toEqual([1, 2, 3]);
  });

  it("index_width pads to max(2, digits of the highest number)", () => {
    const s = makeStore();
    expect(indexWidth(s.project)).toBe(2);
    s.project.next_index = 100;
    add(s, 1, 5);
    expect(indexWidth(s.project)).toBe(3);
  });
});

describe("edit_clips (MRK-04/05/06, QUE-07)", () => {
  it("rename trims", () => {
    const s = makeStore();
    const a = add(s, 1, 5);
    s.editClips([a.clip.id], { kind: "rename", name: "  Boss fight  " });
    expect(s.clip(a.clip.id)?.name).toBe("Boss fight");
  });

  it("retime validates like add and keeps the other side", () => {
    const s = makeStore();
    const a = add(s, 10, 20);
    s.editClips([a.clip.id], { kind: "retime", start_sec: 12, end_sec: null });
    expect(s.clip(a.clip.id)).toMatchObject({ start_sec: 12, end_sec: 20 });
    expect(thrown(() => s.editClips([a.clip.id], { kind: "retime", start_sec: 19.9, end_sec: null })).code).toBe("clip_too_short");
    expect(thrown(() => s.editClips([a.clip.id], { kind: "retime", start_sec: null, end_sec: 11 })).code).toBe("invalid_range");
    s.editClips([a.clip.id], { kind: "retime", start_sec: null, end_sec: 9999 });
    expect(s.clip(a.clip.id)?.end_sec).toBe(180);
    expect(s.clip(a.clip.id)?.start_sec).toBe(12);
  });

  it("set_tag requires an existing tag; null clears", () => {
    const s = makeStore();
    const a = add(s, 1, 5);
    expect(thrown(() => s.editClips([a.clip.id], { kind: "set_tag", tag_id: "zzz" })).code).toBe("invalid_input");
    s.editClips([a.clip.id], { kind: "set_tag", tag_id: "edu" });
    expect(s.clip(a.clip.id)?.tag_id).toBe("edu");
    s.editClips([a.clip.id], { kind: "set_tag", tag_id: null });
    expect(s.clip(a.clip.id)?.tag_id).toBeNull();
  });

  it("set_crop clamps to [-1, 1] at 1e-4 precision and rejects NaN", () => {
    const s = makeStore();
    const a = add(s, 1, 5);
    s.editClips([a.clip.id], { kind: "set_crop", crop_x_offset: 7 });
    expect(s.clip(a.clip.id)?.crop_x_offset).toBe(1);
    s.editClips([a.clip.id], { kind: "set_crop", crop_x_offset: -0.123456 });
    expect(s.clip(a.clip.id)?.crop_x_offset).toBe(-0.1235);
    expect(thrown(() => s.editClips([a.clip.id], { kind: "set_crop", crop_x_offset: Number.NaN })).code).toBe("invalid_input");
  });

  it("a bulk edit is ONE undo step; a failing clip changes none of them", () => {
    const s = makeStore();
    const a = add(s, 1, 5);
    const b = add(s, 6, 9);
    const depth = s.undoDepth();
    const p = s.editClips([a.clip.id, b.clip.id], { kind: "set_included", included: false });
    expect(p.upserted).toHaveLength(2);
    expect(p.label).toBeNull();
    expect(s.undoDepth()).toBe(depth + 1);
    expect(s.history().undo_label).toBe("Undo Toggle include (2 clips)");
    s.undo();
    expect(s.project.clips.every((c) => c.selected_for_export)).toBe(true);
    const before = userState(s.project);
    expect(thrown(() => s.editClips([a.clip.id, "ghost"], { kind: "rename", name: "q" })).code).toBe("not_found");
    expect(userState(s.project)).toBe(before);
  });

  it("only clips that actually change are upserted", () => {
    const s = makeStore();
    const a = add(s, 1, 5);
    const b = add(s, 6, 9);
    s.editClips([a.clip.id], { kind: "set_included", included: false });
    const p = s.editClips([a.clip.id, b.clip.id], { kind: "set_included", included: false });
    expect(p.upserted.map((c) => c.id)).toEqual([b.clip.id]);
  });

  it("delete is undoable and clears the selection", () => {
    const s = makeStore();
    const a = add(s, 1, 5);
    s.setViewState({ playhead_sec: 0, zoom_window_sec: 20, selected_clip_id: a.clip.id });
    s.deleteClips([a.clip.id]);
    expect(s.project.view_state.selected_clip_id).toBeNull();
    expect(thrown(() => s.deleteClips([a.clip.id])).code).toBe("not_found");
  });
});

describe("tags (MRK-06)", () => {
  it("validates labels, colours and unique ids", () => {
    const s = makeStore();
    expect(thrown(() => s.setTags([{ id: "a", label: " ", color: "#112233" }])).message).toContain("label");
    expect(thrown(() => s.setTags([{ id: "a", label: "A", color: "red" }])).message).toContain("#RRGGBB");
    expect(
      thrown(() =>
        s.setTags([
          { id: "a", label: "A", color: "#112233" },
          { id: "a", label: "B", color: "#112233" },
        ]),
      ).message,
    ).toContain("twice");
  });

  it("removing a tag clears it from clips in the same undo step; undo restores both", () => {
    const s = makeStore();
    const a = s.addClip({ start_sec: 1, end_sec: 5, name: null, tag_id: "funny" });
    const b = s.addClip({ start_sec: 6, end_sec: 9, name: null, tag_id: "edu" });
    const before = userState(s.project);
    const p = s.setTags(s.project.tags.filter((t) => t.id !== "funny"));
    expect(p.upserted.map((c) => c.id)).toEqual([a.clip.id]);
    expect(s.clip(a.clip.id)?.tag_id).toBeNull();
    expect(s.clip(b.clip.id)?.tag_id).toBe("edu");
    s.undo();
    expect(userState(s.project)).toBe(before);
    s.redo();
    expect(s.clip(a.clip.id)?.tag_id).toBeNull();
  });
});

describe("undo / redo (MRK-04)", () => {
  it("labels are 'Undo Add clip' etc. and flip on redo", () => {
    const s = makeStore();
    expect(s.undo()).toBeNull();
    expect(s.redo()).toBeNull();
    add(s, 1, 5);
    expect(s.history()).toMatchObject({ can_undo: true, can_redo: false, undo_label: "Undo Add clip", redo_label: null });
    const u = s.undo();
    expect(u?.label).toBe("Undo Add clip");
    expect(u?.removed).toHaveLength(1);
    expect(s.history()).toMatchObject({ can_undo: false, can_redo: true, redo_label: "Redo Add clip" });
    expect(s.redo()?.label).toBe("Redo Add clip");
  });

  it("a new change clears the redo stack", () => {
    const s = makeStore();
    add(s, 1, 5);
    s.undo();
    add(s, 2, 6);
    expect(s.history().can_redo).toBe(false);
  });

  it("keeps at least 200 steps", () => {
    expect(UNDO_LIMIT).toBeGreaterThanOrEqual(200);
    const s = makeStore();
    const a = add(s, 1, 5);
    for (let i = 0; i < 250; i++) s.editClips([a.clip.id], { kind: "rename", name: `n${i}` });
    let n = 0;
    while (s.undo()) n++;
    expect(n).toBeGreaterThanOrEqual(200);
  });

  it("export status is recomputed, never rolled back (EXP-01, QUE-05)", () => {
    const s = makeStore();
    const a = add(s, 10, 20, "Bit");
    s.markExporting([a.clip.id]);
    const clip = s.clip(a.clip.id) as Clip;
    const rec: ExportRecord = {
      clip_id: clip.id,
      outcome: { outcome: "done", output_path: "C:/x/01_Bit.mp4", size_bytes: 10 },
      exported_at: "2026-01-01T00:00:00.000Z",
      preset: "lossless",
      settings_hash: "abc",
      content_hash: contentHash(clip, "lossless"),
    };
    s.applyExportRecord(rec);
    expect(s.clip(clip.id)?.export_status).toBe("completed");
    s.editClips([clip.id], { kind: "rename", name: "Renamed" });
    expect(s.clip(clip.id)?.export_status).toBe("outdated");
    s.editClips([clip.id], { kind: "set_included", included: false });
    expect(s.clip(clip.id)?.export_status).toBe("outdated");
    s.undo();
    s.undo();
    expect(s.clip(clip.id)?.export_status).toBe("completed");
    s.redo();
    expect(s.clip(clip.id)?.export_status).toBe("outdated");
  });

  it("crop only matters for Vertical Crop's content hash", () => {
    const s = makeStore();
    const a = add(s, 10, 20, "Bit");
    const clip = s.clip(a.clip.id) as Clip;
    const h = (p: "lossless" | "vcrop") => contentHash(clip, p);
    const [l0, v0] = [h("lossless"), h("vcrop")];
    clip.crop_x_offset = 0.5;
    expect(h("lossless")).toBe(l0);
    expect(h("vcrop")).not.toBe(v0);
    expect(l0).toMatch(/^[0-9a-f]{8}$/);
  });

  it("restores exactly over random operation sequences (undo all, then redo all)", () => {
    for (let seed = 1; seed <= 25; seed++) {
      const rnd = mulberry32(seed);
      const pick = <T>(xs: T[]): T | undefined => xs[Math.floor(rnd() * xs.length)];
      const s = makeStore();
      const states = [userState(s.project)];
      for (let step = 0; step < 60; step++) {
        const clips = s.project.clips;
        const id = pick(clips)?.id;
        const ids = clips.filter(() => rnd() < 0.4).map((c) => c.id);
        const t = rnd() * 170;
        const before = s.revision;
        try {
          switch (Math.floor(rnd() * 11)) {
            case 0:
            case 1:
              s.addClip({ start_sec: t, end_sec: t + 0.4 + rnd() * 30, name: `c${step}`, tag_id: rnd() < 0.3 ? "funny" : null });
              break;
            case 2:
              if (ids.length) s.deleteClips(ids);
              break;
            case 3:
              if (id) s.editClips(ids.length ? ids : [id], { kind: "rename", name: `r${step}` });
              break;
            case 4:
              if (id) s.editClips([id], { kind: "retime", start_sec: t, end_sec: t + 1 + rnd() * 20 });
              break;
            case 5:
              if (id) s.editClips(ids.length ? ids : [id], { kind: "set_tag", tag_id: pick(["funny", "edu", null]) ?? null });
              break;
            case 6:
              if (id) s.editClips(ids.length ? ids : [id], { kind: "set_crop", crop_x_offset: rnd() * 2 - 1 });
              break;
            case 7:
              if (id) s.editClips(ids.length ? ids : [id], { kind: "set_included", included: rnd() < 0.5 });
              break;
            case 8:
              s.setTags(rnd() < 0.5 ? s.project.tags.slice(1) : [...s.project.tags, { id: `t${step}`, label: `T${step}`, color: "#00FF00" }]);
              break;
            case 9:
              s.renumberClips();
              break;
            default:
              if (rnd() < 0.5) {
                s.undo();
                s.redo();
              }
          }
        } catch (e) {
          if (!(e instanceof BackendError)) throw e; // validation errors are fine, they change nothing
        }
        if (s.revision !== before) {
          // A coalesced crop replaces the latest state's "after" without adding depth.
          states[s.undoDepth()] = userState(s.project);
        }
      }
      const final = userState(s.project);
      const depth = s.undoDepth();
      // Undo everything: each undo must land on the previous recorded state.
      for (let i = depth - 1; i >= 0; i--) {
        expect(s.undo()).not.toBeNull();
        expect(userState(s.project)).toBe(states[i]);
      }
      expect(s.undo()).toBeNull();
      for (let i = 1; i <= depth; i++) {
        expect(s.redo()).not.toBeNull();
        expect(userState(s.project)).toBe(states[i]);
      }
      expect(userState(s.project)).toBe(final);
      // Numbers stay unique and below next_index.
      const idx = s.project.clips.map((c) => c.index);
      expect(new Set(idx).size).toBe(idx.length);
      expect(Math.max(0, ...idx)).toBeLessThan(s.project.next_index);
    }
  });
});

describe("import_clips (EXP-17)", () => {
  it("adds valid items as one undo step and returns the rest as skipped", () => {
    const s = makeStore();
    const r = s.importClips([
      { line: 1, start_sec: 10, end_sec: 20, name: "A", error: null },
      { line: 2, start_sec: 30, end_sec: 30.1, name: "B", error: null },
      { line: 3, start_sec: 0, end_sec: 0, name: "C", error: "no time" },
      { line: 4, start_sec: 40, end_sec: 50, name: "D", error: null },
    ]);
    expect(r.added).toHaveLength(2);
    expect(r.skipped.map((i) => i.line)).toEqual([2, 3]);
    expect(r.skipped[0]?.error).toContain("at least");
    expect(r.patch.history.undo_label).toBe("Undo Import 2 clips");
    s.undo();
    expect(s.project.clips).toHaveLength(0);
  });
});

describe("read-only projects", () => {
  it("reject changes with read_only and keep undo inert", () => {
    const s = new MockProjectStore(newProject(sampleSource(), true), { minClipSec: () => 0.5 }, { kind: "source_missing" });
    const e = thrown(() => add(s, 1, 5));
    expect(e.code).toBe("read_only");
    expect(e.message).toContain("Locate");
    expect(s.undo()).toBeNull();
  });
});

function mulberry32(seed: number): () => number {
  let a = seed;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
