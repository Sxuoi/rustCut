// QUE-04: bulk include changes are ONE backend call and therefore ONE undo step.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { backend } from "$lib/ipc";
import { player } from "./player.svelte";
import { project } from "./project.svelte";

beforeEach(async () => {
  player.reset();
  project.clear();
  const r = await backend.openPath("/sample/bulk.mp4");
  project.load(r.snapshot);
  for (const [i, name] of ["A", "B", "C"].entries()) {
    await backend.addClip({ start_sec: 10 + i * 30, end_sec: 30 + i * 30, name, tag_id: null });
  }
  project.load((await backend.getSnapshot())!);
});
afterEach(() => vi.restoreAllMocks());

describe("project.editClipsBatch", () => {
  it("sends one call without empty edits, and one undo restores everything", async () => {
    const spy = vi.spyOn(backend, "editClipsBatch");
    const [a, b, c] = project.clips.map((x) => x.id);
    const before = project.clips.map((x) => x.selected_for_export);
    await project.editClipsBatch([
      { ids: [a], edit: { kind: "set_included", included: false } },
      { ids: [], edit: { kind: "set_included", included: true } },
      { ids: [b, c], edit: { kind: "set_included", included: false } },
    ]);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0]).toHaveLength(2);
    expect(project.clips.map((x) => x.selected_for_export)).toEqual([false, false, false]);
    await project.undo();
    expect(project.clips.map((x) => x.selected_for_export)).toEqual(before);
  });

  it("does nothing when every edit is empty", async () => {
    const spy = vi.spyOn(backend, "editClipsBatch");
    await project.editClipsBatch([{ ids: [], edit: { kind: "set_included", included: true } }]);
    expect(spy).not.toHaveBeenCalled();
  });
});
