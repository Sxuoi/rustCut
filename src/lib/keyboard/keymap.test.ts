// §5 rules 1-3 and AC-13: the pure key -> decision policy.
import { describe, expect, it } from "vitest";
import {
  DECODER_PACED,
  describeTarget,
  isBrowserAccelerator,
  NO_TARGET,
  resolveKey,
  type ElementLike,
  type KeyContext,
  type KeyEventLike,
  type TargetInfo,
} from "./keymap";
import type { Platform } from "./shortcuts";

function press(key: string, opts: Partial<KeyEventLike> = {}): KeyEventLike {
  const code = opts.code ?? (/^[a-z]$/i.test(key) ? `Key${key.toUpperCase()}` : key === " " ? "Space" : key);
  return { key, code, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, repeat: false, ...opts };
}

function ctx(over: Partial<KeyContext> = {}): KeyContext {
  return { platform: "windows", target: NO_TARGET, dialogOpen: false, handledByComponent: false, ...over };
}

const TEXT: TargetInfo = { ...NO_TARGET, textEntry: true };
const BUTTON: TargetInfo = { ...NO_TARGET, activates: true };
const LISTBOX_OPTION: TargetInfo = { ...NO_TARGET, activates: true, arrowNav: true };

function actionOf(e: KeyEventLike, c: KeyContext = ctx()): string | null {
  const d = resolveKey(e, c);
  return d.kind === "action" ? d.action : null;
}

describe("rule 1 / AC-13: no shortcut fires while typing in a text field", () => {
  const typing = ["i", "o", "a", "j", "k", "l", "n", "p", " ", "Enter", "Backspace", "Delete", "ArrowLeft", "ArrowUp", ",", "."];
  it.each(typing)("%j in a text field does nothing", (k) => {
    expect(resolveKey(press(k), ctx({ target: TEXT }))).toEqual({ kind: "none" });
  });

  it("modified shortcuts are suspended too (Ctrl+Z is the field's own undo)", () => {
    expect(resolveKey(press("z", { ctrlKey: true }), ctx({ target: TEXT }))).toEqual({ kind: "none" });
    expect(resolveKey(press("i", { altKey: true }), ctx({ target: TEXT }))).toEqual({ kind: "none" });
  });

  it("Esc leaves a field that did not handle it", () => {
    expect(resolveKey(press("Escape"), ctx({ target: TEXT }))).toEqual({ kind: "leaveField" });
  });

  it("a field that handled Enter/Esc itself (rename editor) wins", () => {
    expect(resolveKey(press("Escape"), ctx({ target: TEXT, handledByComponent: true }))).toEqual({ kind: "none" });
    expect(resolveKey(press("Enter"), ctx({ target: TEXT, handledByComponent: true }))).toEqual({ kind: "none" });
  });

  it("browser accelerators stay blocked inside text fields", () => {
    expect(resolveKey(press("F5"), ctx({ target: TEXT }))).toEqual({ kind: "block" });
    expect(resolveKey(press("r", { ctrlKey: true }), ctx({ target: TEXT }))).toEqual({ kind: "block" });
  });

  it("IME composition is never a shortcut", () => {
    expect(resolveKey(press("i", { isComposing: true }), ctx())).toEqual({ kind: "none" });
    expect(resolveKey(press("Process", { code: "KeyI" }), ctx())).toEqual({ kind: "none" });
  });
});

describe("dialogs pause global shortcuts", () => {
  it("no action while a dialog is open", () => {
    expect(resolveKey(press("i"), ctx({ dialogOpen: true, target: { ...NO_TARGET, inDialog: true } }))).toEqual({ kind: "none" });
    expect(resolveKey(press("Enter"), ctx({ dialogOpen: true, target: { ...NO_TARGET, inDialog: true } }))).toEqual({ kind: "none" });
  });

  it("Esc with focus outside the open dialog closes it", () => {
    expect(resolveKey(press("Escape"), ctx({ dialogOpen: true }))).toEqual({ kind: "closeDialogs" });
    expect(resolveKey(press("Escape"), ctx({ dialogOpen: true, target: { ...NO_TARGET, inDialog: true } }))).toEqual({
      kind: "none",
    });
  });
});

describe("native controls keep their keys", () => {
  it("Space/Enter activate a focused button instead of play/add", () => {
    expect(resolveKey(press(" "), ctx({ target: BUTTON }))).toEqual({ kind: "none" });
    expect(resolveKey(press("Enter"), ctx({ target: BUTTON }))).toEqual({ kind: "none" });
  });

  it("other shortcuts still work with a button focused", () => {
    expect(actionOf(press("i"), ctx({ target: BUTTON }))).toBe("markIn");
    expect(actionOf(press("ArrowRight"), ctx({ target: BUTTON }))).toBe("seekForward1");
  });

  it("arrow keys navigate listboxes, menus and sliders", () => {
    expect(resolveKey(press("ArrowDown"), ctx({ target: LISTBOX_OPTION }))).toEqual({ kind: "none" });
    expect(resolveKey(press("ArrowLeft"), ctx({ target: { ...NO_TARGET, arrowNav: true } }))).toEqual({ kind: "none" });
    expect(actionOf(press("Delete"), ctx({ target: LISTBOX_OPTION }))).toBe("deleteSelection");
  });

  it("Alt+← in a slider still blocks history navigation", () => {
    expect(resolveKey(press("ArrowLeft", { altKey: true }), ctx({ target: { ...NO_TARGET, arrowNav: true } }))).toEqual({
      kind: "block",
    });
  });
});

describe("auto-repeat", () => {
  it("toggles ignore repeats but swallow them", () => {
    expect(resolveKey(press(" ", { repeat: true }), ctx())).toEqual({ kind: "block" });
    expect(resolveKey(press("i", { repeat: true }), ctx())).toEqual({ kind: "block" });
    expect(resolveKey(press("Enter", { repeat: true }), ctx())).toEqual({ kind: "block" });
  });

  it("seeks and steps repeat (paced by the decoder in the controller)", () => {
    const d = resolveKey(press("l", { repeat: true }), ctx());
    expect(d).toMatchObject({ kind: "action", action: "seekForward5", repeat: true });
    expect(DECODER_PACED.has("seekForward5")).toBe(true);
    expect(DECODER_PACED.has("frameBack")).toBe(true);
    expect(DECODER_PACED.has("undo")).toBe(false);
  });
});

describe("rule 3: WebView accelerators are blocked", () => {
  const win: [string, KeyEventLike][] = [
    ["F5", press("F5")],
    ["Ctrl+F5", press("F5", { ctrlKey: true })],
    ["Ctrl+R", press("r", { ctrlKey: true })],
    ["Ctrl+Shift+R", press("R", { ctrlKey: true, shiftKey: true })],
    ["Ctrl+P", press("p", { ctrlKey: true })],
    ["Ctrl+F", press("f", { ctrlKey: true })],
    ["Ctrl+G", press("g", { ctrlKey: true })],
    ["Ctrl+U", press("u", { ctrlKey: true })],
    ["Alt+←", press("ArrowLeft", { altKey: true })],
    ["Alt+→", press("ArrowRight", { altKey: true })],
    ["BrowserBack", press("BrowserBack", { code: "BrowserBack" })],
    ["BrowserForward", press("BrowserForward", { code: "BrowserForward" })],
    ["F3", press("F3")],
    ["F7", press("F7")],
    ["Ctrl+plus", press("+", { ctrlKey: true, shiftKey: true, code: "Equal" })],
    ["Ctrl+=", press("=", { ctrlKey: true, code: "Equal" })],
    ["Ctrl+minus", press("-", { ctrlKey: true, code: "Minus" })],
    ["Ctrl+0", press("0", { ctrlKey: true, code: "Digit0" })],
    ["Ctrl+Numpad+", press("+", { ctrlKey: true, code: "NumpadAdd" })],
  ];
  it.each(win)("%s (Windows)", (_label, e) => {
    expect(isBrowserAccelerator(e, "windows", false)).toBe(true);
  });

  it("macOS: Cmd+R, Cmd+[ / Cmd+], Cmd+←/→ outside text fields", () => {
    const mac: Platform = "macos";
    expect(isBrowserAccelerator(press("r", { metaKey: true }), mac, false)).toBe(true);
    expect(isBrowserAccelerator(press("[", { metaKey: true, code: "BracketLeft" }), mac, false)).toBe(true);
    expect(isBrowserAccelerator(press("ArrowLeft", { metaKey: true }), mac, false)).toBe(true);
    // Cmd+← moves to the line start while typing: allowed there.
    expect(isBrowserAccelerator(press("ArrowLeft", { metaKey: true }), mac, true)).toBe(false);
    // Option+← moves by word on macOS: not an accelerator.
    expect(isBrowserAccelerator(press("ArrowLeft", { altKey: true }), mac, true)).toBe(false);
  });

  it("text editing keys are never blocked", () => {
    for (const k of ["a", "c", "v", "x", "z", "y"]) {
      expect(isBrowserAccelerator(press(k, { ctrlKey: true }), "windows", true), k).toBe(false);
    }
    expect(isBrowserAccelerator(press("ArrowLeft", { ctrlKey: true }), "windows", true)).toBe(false);
  });

  it("AltGr characters are typed, not blocked (Polish ś = AltGr+S)", () => {
    expect(isBrowserAccelerator(press("ś", { code: "KeyS", ctrlKey: true, altKey: true }), "windows", true)).toBe(false);
  });

  it("app shortcuts that are also accelerators still run", () => {
    expect(resolveKey(press("s", { ctrlKey: true }), ctx())).toMatchObject({ kind: "action", action: "saveNow" });
    expect(resolveKey(press("e", { ctrlKey: true }), ctx())).toMatchObject({ kind: "action", action: "exportDialog" });
    expect(resolveKey(press("o", { ctrlKey: true }), ctx())).toMatchObject({ kind: "action", action: "open" });
    expect(resolveKey(press("ArrowLeft", { altKey: true }), ctx())).toMatchObject({ kind: "action", action: "cropLeft" });
  });

  it("accelerators are blocked even when a dialog is open", () => {
    expect(resolveKey(press("p", { ctrlKey: true }), ctx({ dialogOpen: true }))).toEqual({ kind: "block" });
    expect(resolveKey(press("F5"), ctx({ handledByComponent: true }))).toEqual({ kind: "block" });
  });

  it("unrelated keys pass through", () => {
    expect(resolveKey(press("Tab", { code: "Tab" }), ctx())).toEqual({ kind: "none" });
    expect(resolveKey(press("F12"), ctx())).toEqual({ kind: "none" });
    expect(resolveKey(press("q"), ctx())).toEqual({ kind: "none" });
  });
});

describe("describeTarget", () => {
  function el(tagName: string, attrs: Record<string, string> = {}, parent: ElementLike | null = null, editable = false): ElementLike {
    return { tagName, getAttribute: (n) => attrs[n] ?? null, isContentEditable: editable, parentElement: parent };
  }

  it("text inputs, textareas, selects and contenteditable are text fields", () => {
    expect(describeTarget(el("INPUT")).textEntry).toBe(true);
    expect(describeTarget(el("INPUT", { type: "search" })).textEntry).toBe(true);
    expect(describeTarget(el("INPUT", { type: "number" })).textEntry).toBe(true);
    expect(describeTarget(el("TEXTAREA")).textEntry).toBe(true);
    expect(describeTarget(el("SELECT")).textEntry).toBe(true);
    expect(describeTarget(el("DIV", {}, null, true)).textEntry).toBe(true);
    expect(describeTarget(el("DIV", { role: "textbox" })).textEntry).toBe(true);
  });

  it("checkboxes, ranges and buttons are not text fields", () => {
    expect(describeTarget(el("INPUT", { type: "checkbox" }))).toMatchObject({ textEntry: false, activates: true });
    expect(describeTarget(el("INPUT", { type: "range" }))).toMatchObject({ textEntry: false, arrowNav: true });
    expect(describeTarget(el("BUTTON"))).toMatchObject({ textEntry: false, activates: true, arrowNav: false });
    expect(describeTarget(el("A", { href: "#" })).activates).toBe(true);
    expect(describeTarget(el("A")).activates).toBe(false);
  });

  it("finds arrow-key containers and dialogs among the ancestors", () => {
    const dialog = el("DIALOG");
    const menu = el("DIV", { role: "menu" }, dialog);
    const item = el("BUTTON", { role: "menuitem" }, menu);
    expect(describeTarget(item)).toEqual({ textEntry: false, activates: true, arrowNav: true, inDialog: true });
    const listbox = el("UL", { role: "listbox" });
    expect(describeTarget(el("LI", { role: "option" }, listbox))).toMatchObject({ activates: true, arrowNav: true });
    expect(describeTarget(el("DIV", {}, el("DIV", { role: "dialog" }))).inDialog).toBe(true);
  });

  it("the body / non-elements are neutral", () => {
    expect(describeTarget(el("BODY"))).toEqual(NO_TARGET);
    expect(describeTarget(null)).toEqual(NO_TARGET);
    expect(describeTarget(window_like())).toEqual(NO_TARGET);
  });

  function window_like(): unknown {
    return { addEventListener: () => {} };
  }
});
