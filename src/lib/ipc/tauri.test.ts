import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...a: unknown[]) => invoke(...a),
  Channel: class {},
  convertFileSrc: (p: string) => p,
}));

import { TauriBackend } from "./tauri";

beforeEach(() => invoke.mockReset());

describe("TauriBackend command contracts", () => {
  it("edit_clips_batch sends { batch } as one call (one undo step)", async () => {
    invoke.mockResolvedValue({ revision: 2 });
    const edits = [
      { ids: ["a"], edit: { kind: "set_included" as const, included: true } },
      { ids: ["b"], edit: { kind: "set_included" as const, included: false } },
    ];
    await new TauriBackend().editClipsBatch(edits);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("edit_clips_batch", { batch: edits });
  });

  it("get_platform_presets takes no arguments", async () => {
    invoke.mockResolvedValue([]);
    expect(await new TauriBackend().getPlatformPresets()).toEqual([]);
    expect(invoke).toHaveBeenCalledWith("get_platform_presets", undefined);
  });
});
