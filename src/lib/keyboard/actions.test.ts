// §5 actions against fake stores: mark capture, add flow (MRK-01, AC-01..03),
// rename, selection, loop (QUE-03), delete/undo (MRK-04), retime (MRK-05),
// crop nudges (FRM-03).
import { describe, expect, it } from "vitest";
import type { AddClipRequest, AddClipResult, Clip, ClipEdit, ExportSettings, NoticeLevel, ReadOnlyReason } from "$lib/bindings";
import type { PendingMarks } from "$lib/util/marks";
import { adjacentClipId, createActions, mostRecentClip, nudgeOffset, readOnlyMessage, type PlayerPort, type ProjectPort, type UiPort } from "./actions";

it("explains an unwritable project location without losing read-only protection", () => {
  expect(readOnlyMessage({ kind: "unwritable", path: "D:/locked/project.rustcut" })).toContain("not writable");
});

function clip(id: string, index: number, start: number, end: number, crop = 0): Clip {
  return {
    id,
    index,
    name: `Clip ${index}`,
    start_sec: start,
    end_sec: end,
    tag_id: null,
    crop_x_offset: crop,
    selected_for_export: true,
    export_status: "pending",
    last_export: null,
  };
}

class FakePlayer implements PlayerPort {
  ready = true;
  duration = 600;
  loopClipId: string | null = null;
  pendingIn: number | null = null;
  pendingOut: number | null = null;
  time = 0;
  calls: string[] = [];
  get marks(): PendingMarks {
    return { in: this.pendingIn, out: this.pendingOut };
  }
  setMarks(m: PendingMarks): void {
    this.pendingIn = m.in;
    this.pendingOut = m.out;
  }
  clearPending(): void {
    this.pendingIn = null;
    this.pendingOut = null;
  }
  now(): number {
    return this.time;
  }
  toggle(): void {
    this.calls.push("toggle");
  }
  seek(t: number): void {
    this.calls.push(`seek:${t}`);
  }
  seekBy(dt: number): void {
    this.calls.push(`seekBy:${dt}`);
  }
  stepFrame(dir: 1 | -1): void {
    this.calls.push(`step:${dir}`);
  }
  speedStep(dir: 1 | -1): void {
    this.calls.push(`speed:${dir}`);
  }
  setLoop(id: string | null, range: { start: number; end: number } | null): void {
    this.loopClipId = id;
    this.calls.push(range ? `loop:${id}:${range.start}-${range.end}` : "loop:off");
  }
}

class FakeProject implements ProjectPort {
  loaded = true;
  readOnly: ReadOnlyReason | null = null;
  clips: Clip[] = [];
  primaryId: string | null = null;
  selection: string[] = [];
  settings: ExportSettings | null = { cut_mode: "lossless", framing: "original" } as ExportSettings;
  save: { error: string | null } | null = { error: null };
  addResult: "ok" | "fail" = "ok";
  added: AddClipRequest[] = [];
  edits: { ids: string[]; edit: ClipEdit }[] = [];
  deleted: string[][] = [];
  calls: string[] = [];
  /** Resolves the pending addClip (to test what happens meanwhile). */
  release: (() => void) | null = null;
  holdAdd = false;

  get primary(): Clip | null {
    return this.clips.find((c) => c.id === this.primaryId) ?? null;
  }
  select(id: string): void {
    this.primaryId = id;
    this.selection = [id];
  }
  targetIds(): string[] {
    return this.selection.length ? [...this.selection] : this.primaryId ? [this.primaryId] : [];
  }
  clip(id: string): Clip | undefined {
    return this.clips.find((c) => c.id === id);
  }
  async addClip(req: AddClipRequest): Promise<AddClipResult | null> {
    this.added.push(req);
    if (this.holdAdd) await new Promise<void>((r) => (this.release = r));
    if (this.addResult === "fail") return null;
    const c = clip(`c${this.clips.length + 1}`, this.clips.length + 1, req.start_sec, req.end_sec);
    this.clips.push(c);
    this.select(c.id);
    return { clip: c } as AddClipResult;
  }
  async editClips(ids: string[], edit: ClipEdit): Promise<void> {
    this.edits.push({ ids, edit });
  }
  async deleteClips(ids: string[]): Promise<void> {
    this.deleted.push(ids);
    this.clips = this.clips.filter((c) => !ids.includes(c.id));
  }
  async undo(): Promise<void> {
    this.calls.push("undo");
  }
  async redo(): Promise<void> {
    this.calls.push("redo");
  }
  async saveNow(): Promise<void> {
    this.calls.push("save");
  }
}

class FakeUi implements UiPort {
  minClipDuration = 0.5;
  queueOrder: string[] | null = null;
  exportDialogOpen = false;
  cheatSheetOpen = false;
  toasts: { level: NoticeLevel; message: string; code: string | null }[] = [];
  renames: string[] = [];
  toast(n: { level: NoticeLevel; message: string; code?: string | null }): number {
    this.toasts.push({ level: n.level, message: n.message, code: n.code ?? null });
    return this.toasts.length;
  }
  info(message: string, code?: string): number {
    return this.toast({ level: "info", message, code });
  }
  requestRename(id: string): void {
    this.renames.push(id);
  }
}

function setup() {
  const player = new FakePlayer();
  const project = new FakeProject();
  const ui = new FakeUi();
  let opened = 0;
  let clock = 1000;
  const actions = createActions({ player, project, ui, open: () => void opened++, clock: () => clock });
  return {
    player,
    project,
    ui,
    actions,
    opened: () => opened,
    tick: (ms: number) => (clock += ms),
  };
}

const flush = () => new Promise<void>((r) => setTimeout(r, 0));

describe("§5 rule 4: marks use the time captured by the caller", () => {
  it("I / O use `now`, not a later player.now()", () => {
    const { player, actions } = setup();
    player.time = 999; // the player moved on since the key was pressed
    actions.run("markIn", 12.3456);
    actions.run("markOut", 20);
    expect(player.marks).toEqual({ in: 12.346, out: 20 });
  });

  it("a conflicting mark clears the other one with a toast (AC-03)", () => {
    const { player, ui, actions } = setup();
    actions.run("markOut", 10);
    actions.run("markIn", 15);
    expect(player.marks).toEqual({ in: 15, out: null });
    expect(ui.toasts.at(-1)).toMatchObject({ message: "Out point cleared", code: "out_cleared" });
    actions.run("markOut", 15);
    expect(player.marks).toEqual({ in: null, out: 15 });
    expect(ui.toasts.at(-1)).toMatchObject({ message: "In point cleared", code: "in_cleared" });
  });

  it("marking waits for the preview (§6.4 Preparing preview)", () => {
    const { player, ui, actions } = setup();
    player.ready = false;
    actions.run("markIn", 5);
    expect(player.marks).toEqual({ in: null, out: null });
    expect(ui.toasts.at(-1)?.message).toBe("Marking starts once the preview plays.");
  });
});

describe("MRK-01 add (Enter / A)", () => {
  it("AC-01: adds [In, Out], clears the marks; the store selects the clip", async () => {
    const { player, project, actions } = setup();
    player.duration = 3600; // marks clamp to [0, duration]
    actions.run("markIn", 920.12);
    actions.run("markOut", 955.8);
    await actions.addPending(960);
    expect(project.added).toEqual([{ start_sec: 920.12, end_sec: 955.8, name: null, tag_id: null }]);
    expect(player.marks).toEqual({ in: null, out: null });
    expect(project.primaryId).toBe("c1");
  });

  it("AC-02: only In set -> Out = playhead at the key press", async () => {
    const { project, actions } = setup();
    actions.run("markIn", 10);
    await actions.addPending(25.5);
    expect(project.added[0]).toMatchObject({ start_sec: 10, end_sec: 25.5 });
  });

  it("AC-02: Out first, then an earlier In, then Enter", async () => {
    const { project, actions } = setup();
    actions.run("markOut", 50);
    actions.run("markIn", 40);
    actions.run("addClip", 70);
    await flush();
    expect(project.added[0]).toMatchObject({ start_sec: 40, end_sec: 50 });
  });

  it("errors are specific and nothing is sent (no In, too short)", async () => {
    const { project, ui, actions } = setup();
    await actions.addPending(10);
    expect(ui.toasts.at(-1)).toMatchObject({ level: "warning", code: "no_in" });
    actions.run("markIn", 10);
    await actions.addPending(10.2);
    expect(ui.toasts.at(-1)).toMatchObject({ code: "clip_too_short" });
    expect(ui.toasts.at(-1)?.message).toContain("0.5 s");
    expect(project.added).toHaveLength(0);
  });

  it("a fast second Enter cannot add the same range twice", async () => {
    const { project, actions } = setup();
    project.holdAdd = true;
    actions.run("markIn", 1);
    actions.run("markOut", 5);
    const first = actions.addPending(9);
    const second = actions.addPending(9); // marks already cleared
    project.release?.();
    await Promise.all([first, second]);
    expect(project.added).toHaveLength(1);
  });

  it("a failed add gives the marks back", async () => {
    const { player, project, actions } = setup();
    project.addResult = "fail";
    actions.run("markIn", 1);
    actions.run("markOut", 5);
    await actions.addPending(9);
    expect(player.marks).toEqual({ in: 1, out: 5 });
  });

  it("...unless new marks were set while it was in flight", async () => {
    const { player, project, actions } = setup();
    project.addResult = "fail";
    project.holdAdd = true;
    actions.run("markIn", 1);
    actions.run("markOut", 5);
    const p = actions.addPending(9);
    actions.run("markIn", 30);
    project.release?.();
    await p;
    expect(player.marks).toEqual({ in: 30, out: null });
  });

  it("read-only projects cannot add (marks still work)", async () => {
    const { player, project, ui, actions } = setup();
    project.readOnly = { kind: "open_elsewhere", holder: null };
    actions.run("markIn", 1);
    await actions.addPending(9);
    expect(player.marks.in).toBe(1);
    expect(project.added).toHaveLength(0);
    expect(ui.toasts.at(-1)).toMatchObject({ level: "warning", code: "read_only" });
  });

  it("Esc clears the pending marks", () => {
    const { player, actions } = setup();
    actions.run("markIn", 1);
    actions.run("markOut", 3);
    actions.run("escape");
    expect(player.marks).toEqual({ in: null, out: null });
  });
});

describe("queue actions", () => {
  function withClips() {
    const s = setup();
    s.project.clips = [clip("a", 1, 10, 20), clip("b", 2, 30, 40), clip("c", 3, 50, 60)];
    return s;
  }

  it("N renames the selected clip, else the most recent one", () => {
    const { project, ui, actions } = withClips();
    actions.run("rename");
    expect(ui.renames).toEqual(["c"]);
    expect(project.primaryId).toBe("c");
    project.select("a");
    actions.run("rename");
    expect(ui.renames).toEqual(["c", "a"]);
  });

  it("N with no clips explains why", () => {
    const { ui, actions } = setup();
    actions.run("rename");
    expect(ui.renames).toEqual([]);
    expect(ui.toasts.at(-1)?.code).toBe("no_clip");
  });

  it("↑ / ↓ select the previous / next clip and seek to its In", () => {
    const { player, project, actions } = withClips();
    actions.run("selectNext");
    expect(project.primaryId).toBe("a");
    actions.run("selectNext");
    expect(project.primaryId).toBe("b");
    expect(player.calls.at(-1)).toBe("seek:30");
    actions.run("selectPrevious");
    expect(project.primaryId).toBe("a");
    actions.run("selectPrevious"); // stays at the first
    expect(project.primaryId).toBe("a");
  });

  it("↑ / ↓ follow the queue's visible order when set", () => {
    const { project, ui, actions } = withClips();
    ui.queueOrder = ["c", "a"];
    actions.run("selectNext");
    expect(project.primaryId).toBe("c");
    actions.run("selectNext");
    expect(project.primaryId).toBe("a");
  });

  it("P toggles the loop of the selected clip", () => {
    const { player, project, ui, actions } = withClips();
    actions.run("loopToggle");
    expect(ui.toasts.at(-1)?.code).toBe("no_clip");
    project.select("b");
    actions.run("loopToggle");
    expect(player.calls.at(-1)).toBe("loop:b:30-40");
    actions.run("loopToggle");
    expect(player.calls.at(-1)).toBe("loop:off");
  });

  it("Delete removes the selection without confirmation and stops its loop", async () => {
    const { player, project, ui, actions } = withClips();
    project.select("b");
    player.loopClipId = "b";
    actions.run("deleteSelection");
    await flush();
    expect(project.deleted).toEqual([["b"]]);
    expect(player.loopClipId).toBeNull();
    expect(ui.toasts.at(-1)?.code).toBe("deleted");
  });

  it("undo / redo / save / export / open / cheat sheet", async () => {
    const { project, ui, actions, opened } = withClips();
    actions.run("undo");
    actions.run("redo");
    actions.run("saveNow");
    await flush();
    expect(project.calls).toEqual(["undo", "redo", "save"]);
    expect(ui.toasts.at(-1)?.code).toBe("saved");
    actions.run("exportDialog");
    expect(ui.exportDialogOpen).toBe(true);
    actions.run("open");
    expect(opened()).toBe(1);
    actions.run("cheatSheet");
    expect(ui.cheatSheetOpen).toBe(true);
    actions.run("cheatSheet");
    expect(ui.cheatSheetOpen).toBe(false);
  });

  it("undo is blocked while read-only", () => {
    const { project, actions } = withClips();
    project.readOnly = { kind: "source_missing" };
    actions.run("undo");
    expect(project.calls).toEqual([]);
  });
});

describe("transport and navigation", () => {
  it("seeks, steps, speed and play/pause", () => {
    const { player, actions } = setup();
    actions.run("seekBack5");
    actions.run("seekForward5");
    actions.run("seekBack1");
    actions.run("seekForward1");
    actions.run("frameBack");
    actions.run("frameForward");
    actions.run("speedDown");
    actions.run("speedUp");
    actions.run("playPause");
    expect(player.calls).toEqual(["seekBy:-5", "seekBy:5", "seekBy:-1", "seekBy:1", "step:-1", "step:1", "speed:-1", "speed:1", "toggle"]);
  });

  it("Shift+I / Shift+O go to the pending mark, else the selected clip", () => {
    const { player, project, actions } = setup();
    project.clips = [clip("a", 1, 10, 20)];
    project.select("a");
    actions.run("goToIn");
    actions.run("goToOut");
    player.pendingIn = 3;
    actions.run("goToIn");
    expect(player.calls).toEqual(["seek:10", "seek:20", "seek:3"]);
  });
});

describe("MRK-05 Alt+I / Alt+O retime the selected clip", () => {
  it("sends a retime of one edge", () => {
    const { project, actions } = setup();
    project.clips = [clip("a", 1, 10, 20)];
    project.select("a");
    actions.run("retimeIn", 12);
    actions.run("retimeOut", 25);
    expect(project.edits).toEqual([
      { ids: ["a"], edit: { kind: "retime", start_sec: 12, end_sec: null } },
      { ids: ["a"], edit: { kind: "retime", start_sec: null, end_sec: 25 } },
    ]);
  });

  it("rejects invalid retimes with a specific message", () => {
    const { project, ui, actions } = setup();
    project.clips = [clip("a", 1, 10, 20)];
    project.select("a");
    actions.run("retimeIn", 25);
    expect(project.edits).toHaveLength(0);
    expect(ui.toasts.at(-1)?.level).toBe("warning");
  });
});

describe("FRM-03 Alt+←/→ crop nudges", () => {
  it("only with the Vertical Crop preset", () => {
    const { project, ui, actions } = setup();
    project.clips = [clip("a", 1, 10, 20)];
    project.select("a");
    actions.run("cropRight");
    expect(project.edits).toHaveLength(0);
    expect(ui.toasts.at(-1)?.code).toBe("crop_not_vertical");
  });

  it("nudges ±0.05 / ±0.25 and chains fast repeats", () => {
    const { project, actions, tick } = setup();
    project.settings = { cut_mode: "accurate", framing: "vertical_crop" } as ExportSettings;
    project.clips = [clip("a", 1, 10, 20, 0.9)];
    project.select("a");
    actions.run("cropRight", Number.NaN, false);
    actions.run("cropRight", Number.NaN, true); // store still says 0.9
    actions.run("cropRight", Number.NaN, true); // clamped at +1: nothing sent
    tick(5000);
    actions.run("cropLeftLarge");
    const offsets = project.edits.map((e) => (e.edit.kind === "set_crop" ? e.edit.crop_x_offset : null));
    expect(offsets).toEqual([0.95, 1, 0.65]);
  });
});

describe("pure helpers", () => {
  it("adjacentClipId", () => {
    expect(adjacentClipId([], null, 1)).toBeNull();
    expect(adjacentClipId(["a", "b"], null, 1)).toBe("a");
    expect(adjacentClipId(["a", "b"], null, -1)).toBe("b");
    expect(adjacentClipId(["a", "b"], "b", 1)).toBe("b");
    expect(adjacentClipId(["a", "b"], "gone", -1)).toBe("b");
  });

  it("nudgeOffset clamps and rounds", () => {
    expect(nudgeOffset(0.98, 0.05)).toBe(1);
    expect(nudgeOffset(-0.98, -0.25)).toBe(-1);
    expect(nudgeOffset(0.1, 0.05)).toBe(0.15);
    expect(nudgeOffset(Number.NaN, 0.25)).toBe(0.25);
  });

  it("mostRecentClip picks the highest clip number", () => {
    expect(mostRecentClip([])).toBeNull();
    expect(mostRecentClip([clip("x", 4, 0, 1), clip("y", 9, 0, 1), clip("z", 2, 0, 1)])?.id).toBe("y");
  });
});
