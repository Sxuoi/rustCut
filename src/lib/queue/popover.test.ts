import { describe, expect, it } from "vitest";
import { focusHomeUnlessModal, isTextEntry, placePopover } from "./popover";

describe("focusHomeUnlessModal (async queue actions)", () => {
  function fakeDoc(activeInModal: boolean) {
    let focused = 0;
    const home = { focus: () => void focused++ };
    const doc = {
      activeElement: { closest: (sel: string) => (activeInModal && sel.includes("aria-modal") ? {} : null) },
      querySelector: () => home,
    } as unknown as Document;
    return { doc, focused: () => focused };
  }

  it("returns focus to the player after the action", () => {
    const f = fakeDoc(false);
    focusHomeUnlessModal(f.doc);
    expect(f.focused()).toBe(1);
  });

  it("leaves a dialog that opened meanwhile (Ctrl+E during the edit) its focus", () => {
    const f = fakeDoc(true);
    focusHomeUnlessModal(f.doc);
    expect(f.focused()).toBe(0);
  });
});
import { importableItems, sourceStem, timestampFileName, TIMESTAMP_FORMATS } from "./timestamps";

const viewport = { width: 1000, height: 800 };
const size = { width: 200, height: 150 };

describe("placePopover", () => {
  it("opens below the anchor when it fits", () => {
    expect(placePopover({ left: 100, top: 100, right: 160, bottom: 120 }, size, viewport)).toEqual({
      left: 100,
      top: 124,
      side: "below",
    });
  });

  it("flips above near the bottom edge", () => {
    expect(placePopover({ left: 100, top: 700, right: 160, bottom: 720 }, size, viewport)).toEqual({
      left: 100,
      top: 546,
      side: "above",
    });
  });

  it("clamps horizontally and when neither side fits", () => {
    const p = placePopover({ left: 950, top: 390, right: 990, bottom: 410 }, { width: 200, height: 700 }, viewport);
    expect(p.left).toBe(792);
    // bottom edge pinned to the viewport margin: 92 + 700 = 792
    expect(p.top).toBe(92);
    expect(p.side).toBe("below");
  });
});

describe("isTextEntry (global shortcuts are suspended in text fields)", () => {
  const el = (tagName: string, extra: Record<string, unknown> = {}) =>
    ({ tagName, isContentEditable: false, ...extra }) as unknown as EventTarget;

  it("treats text inputs, textareas and contenteditable as text entry", () => {
    expect(isTextEntry(el("INPUT", { type: "text" }))).toBe(true);
    expect(isTextEntry(el("INPUT", { type: "search" }))).toBe(true);
    expect(isTextEntry(el("TEXTAREA"))).toBe(true);
    expect(isTextEntry(el("DIV", { isContentEditable: true }))).toBe(true);
  });

  it("does not treat buttons, checkboxes or radios as text entry", () => {
    expect(isTextEntry(el("INPUT", { type: "checkbox" }))).toBe(false);
    expect(isTextEntry(el("INPUT", { type: "radio" }))).toBe(false);
    expect(isTextEntry(el("BUTTON"))).toBe(false);
    expect(isTextEntry(null)).toBe(false);
  });
});

describe("EXP-16/17 timestamp helpers", () => {
  it("lists the four formats", () => {
    expect(TIMESTAMP_FORMATS).toEqual(["youtube_chapters", "csv", "edl", "fcpxml"]);
  });

  it("derives the source stem from Windows and POSIX paths", () => {
    expect(sourceStem("D:\\Recordings\\livestream_2026_10_21.mkv")).toBe("livestream_2026_10_21");
    expect(sourceStem("/home/a/my.clip.mp4")).toBe("my.clip");
    expect(sourceStem("/home/a/.hidden")).toBe(".hidden");
    expect(sourceStem("")).toBe("timestamps");
  });

  it("builds default save names", () => {
    expect(timestampFileName("live", "youtube_chapters", "txt")).toBe("live_chapters.txt");
    expect(timestampFileName("live", "edl", ".edl")).toBe("live_clips.edl");
    expect(timestampFileName("live", "csv", "")).toBe("live_clips");
  });

  it("imports only lines without errors", () => {
    const items = [
      { line: 1, start_sec: 730, end_sec: 785, name: "Jump scare", error: null },
      { line: 2, start_sec: 0, end_sec: 0, name: "", error: "Out before In" },
    ];
    expect(importableItems(items).map((i) => i.line)).toEqual([1]);
  });
});
