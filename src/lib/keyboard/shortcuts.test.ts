// §5 shortcut table: every P0/P1 row matches, labels, single source of truth.
import { describe, expect, it } from "vitest";
import common from "$lib/i18n/en/common";
import {
  ariaKeyShortcuts,
  detectPlatform,
  formatBinding,
  letterOf,
  matchShortcut,
  SHORTCUT_GROUPS,
  shortcutFor,
  shortcutLabel,
  shortcutLabels,
  SHORTCUTS,
  toPlatform,
  type ActionId,
  type KeyInput,
  type Platform,
} from "./shortcuts";

/** Build a key press. `code` defaults to the US-layout code of `key`. */
function press(key: string, opts: Partial<KeyInput> = {}): KeyInput {
  const usCode: Record<string, string> = {
    " ": "Space",
    ",": "Comma",
    ".": "Period",
    "<": "Comma",
    ">": "Period",
    "[": "BracketLeft",
    "]": "BracketRight",
    "?": "Slash",
    "/": "Slash",
  };
  const code = opts.code ?? (/^[a-z]$/i.test(key) ? `Key${key.toUpperCase()}` : (usCode[key] ?? key));
  return { key, code, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, repeat: false, ...opts };
}

function actionOf(e: KeyInput, platform: Platform = "windows"): ActionId | null {
  return matchShortcut(e, platform)?.action ?? null;
}

describe("§5 P0 rows (Windows)", () => {
  const cases: [string, KeyInput, ActionId][] = [
    ["Space", press(" "), "playPause"],
    ["K", press("k"), "playPause"],
    ["J", press("j"), "seekBack5"],
    ["L", press("l"), "seekForward5"],
    ["←", press("ArrowLeft"), "seekBack1"],
    ["→", press("ArrowRight"), "seekForward1"],
    [",", press(","), "frameBack"],
    [".", press("."), "frameForward"],
    ["Shift+,", press("<", { shiftKey: true }), "speedDown"],
    ["Shift+.", press(">", { shiftKey: true }), "speedUp"],
    ["I", press("i"), "markIn"],
    ["[", press("["), "markIn"],
    ["O", press("o"), "markOut"],
    ["]", press("]"), "markOut"],
    ["Enter", press("Enter"), "addClip"],
    ["A", press("a"), "addClip"],
    ["Esc", press("Escape"), "escape"],
    ["N", press("n"), "rename"],
    ["↑", press("ArrowUp"), "selectPrevious"],
    ["↓", press("ArrowDown"), "selectNext"],
    ["P", press("p"), "loopToggle"],
    ["Delete", press("Delete"), "deleteSelection"],
    ["Backspace", press("Backspace"), "deleteSelection"],
    ["Ctrl+Z", press("z", { ctrlKey: true }), "undo"],
    ["Ctrl+Shift+Z", press("Z", { ctrlKey: true, shiftKey: true }), "redo"],
    ["Ctrl+Y", press("y", { ctrlKey: true }), "redo"],
    ["Ctrl+E", press("e", { ctrlKey: true }), "exportDialog"],
    ["Ctrl+S", press("s", { ctrlKey: true }), "saveNow"],
    ["Ctrl+O", press("o", { ctrlKey: true }), "open"],
  ];
  it.each(cases)("%s", (_label, e, action) => {
    expect(actionOf(e)).toBe(action);
  });
});

describe("§5 P1 rows", () => {
  const cases: [string, KeyInput, ActionId][] = [
    ["Shift+I", press("I", { shiftKey: true }), "goToIn"],
    ["Shift+O", press("O", { shiftKey: true }), "goToOut"],
    ["Alt+I", press("i", { altKey: true }), "retimeIn"],
    ["Alt+O", press("o", { altKey: true }), "retimeOut"],
    ["Alt+←", press("ArrowLeft", { altKey: true }), "cropLeft"],
    ["Alt+→", press("ArrowRight", { altKey: true }), "cropRight"],
    ["Alt+Shift+←", press("ArrowLeft", { altKey: true, shiftKey: true }), "cropLeftLarge"],
    ["Alt+Shift+→", press("ArrowRight", { altKey: true, shiftKey: true }), "cropRightLarge"],
    ["?", press("?", { shiftKey: true }), "cheatSheet"],
  ];
  it.each(cases)("%s", (_label, e, action) => {
    expect(actionOf(e)).toBe(action);
  });
});

describe("rule 2: letters by typed character, punctuation by physical key", () => {
  it("letters are case-insensitive (Caps Lock)", () => {
    expect(actionOf(press("I"))).toBe("markIn"); // Caps Lock: "I" without Shift
    expect(actionOf(press("J"))).toBe("seekBack5");
  });

  it("letters follow the typed character on AZERTY (A is on KeyQ)", () => {
    expect(actionOf(press("a", { code: "KeyQ" }))).toBe("addClip");
    expect(actionOf(press("q", { code: "KeyA" }))).toBeNull();
  });

  it("non-Latin layouts fall back to the physical letter key", () => {
    expect(actionOf(press("ш", { code: "KeyI" }))).toBe("markIn"); // Russian
    expect(letterOf({ key: "ı", code: "KeyI" })).toBe("i"); // Turkish dotless i
  });

  it("macOS Option combinations (dead keys) use the physical key", () => {
    expect(actionOf(press("ˆ", { code: "KeyI", altKey: true }), "macos")).toBe("retimeIn");
    expect(actionOf(press("Dead", { code: "KeyI", altKey: true }), "macos")).toBe("retimeIn");
  });

  it("punctuation uses KeyboardEvent.code, whatever it types", () => {
    // German layout: the key right of L types "ö" (code Semicolon); the
    // Comma/Period codes still step frames; BracketLeft types "ü".
    expect(actionOf(press(";", { code: "Comma" }))).toBe("frameBack");
    expect(actionOf(press(":", { code: "Period" }))).toBe("frameForward");
    expect(actionOf(press("ü", { code: "BracketLeft" }))).toBe("markIn");
    expect(actionOf(press("+", { code: "BracketRight" }))).toBe("markOut");
    // "?" typed on any layout opens the cheat sheet.
    expect(actionOf(press("?", { code: "Minus", shiftKey: true }))).toBe("cheatSheet");
  });

  it("modifiers must match exactly", () => {
    expect(actionOf(press("i", { ctrlKey: true }))).toBeNull();
    expect(actionOf(press("e"))).toBeNull(); // E alone is not export
    expect(actionOf(press(",", { ctrlKey: true }))).toBeNull();
    expect(actionOf(press("ArrowLeft", { shiftKey: true }))).toBeNull();
    expect(actionOf(press(" ", { shiftKey: true }))).toBeNull();
    expect(actionOf(press("z", { ctrlKey: true, metaKey: true }))).toBeNull(); // Win key held
  });

  it("AltGr (Ctrl+Alt on Windows) never triggers a shortcut", () => {
    expect(actionOf(press("[", { code: "Digit8", ctrlKey: true, altKey: true }))).toBeNull();
    expect(actionOf(press("ś", { code: "KeyS", ctrlKey: true, altKey: true }))).toBeNull();
  });
});

describe("Ctrl = Cmd on macOS", () => {
  it("uses Meta as the primary modifier", () => {
    expect(actionOf(press("z", { metaKey: true }), "macos")).toBe("undo");
    expect(actionOf(press("z", { metaKey: true, shiftKey: true }), "macos")).toBe("redo");
    expect(actionOf(press("e", { metaKey: true }), "macos")).toBe("exportDialog");
    expect(actionOf(press("z", { ctrlKey: true }), "macos")).toBeNull();
  });

  it("Ctrl+Y redo exists on Windows/Linux only", () => {
    expect(actionOf(press("y", { ctrlKey: true }), "linux")).toBe("redo");
    expect(actionOf(press("y", { metaKey: true }), "macos")).toBeNull();
    expect(shortcutLabels("redo", "macos")).toEqual(["⌘⇧Z"]);
    expect(shortcutLabels("redo", "windows")).toEqual(["Ctrl+Shift+Z", "Ctrl+Y"]);
  });
});

describe("single source of truth", () => {
  it("every action appears exactly once", () => {
    const actions = SHORTCUTS.map((s) => s.action);
    expect(new Set(actions).size).toBe(actions.length);
    expect(shortcutFor("markIn").capturesTime).toBe(true);
  });

  it("every label and group has an English message", () => {
    for (const s of SHORTCUTS) expect(common[s.labelKey], s.labelKey).toBeTruthy();
    for (const g of SHORTCUT_GROUPS) expect(common[g.labelKey], g.labelKey).toBeTruthy();
    for (const s of SHORTCUTS) expect(SHORTCUT_GROUPS.some((g) => g.group === s.group)).toBe(true);
  });

  it("§5 rule 4: mark keys capture the time synchronously", () => {
    const capturing = SHORTCUTS.filter((s) => s.capturesTime).map((s) => s.action);
    expect(capturing.sort()).toEqual(["addClip", "markIn", "markOut", "retimeIn", "retimeOut"]);
  });

  it("toggles never auto-repeat; seeks and steps do", () => {
    for (const a of ["playPause", "markIn", "markOut", "addClip", "loopToggle", "deleteSelection", "exportDialog"] as const) {
      expect(shortcutFor(a).repeat, a).toBe(false);
    }
    for (const a of ["seekBack5", "seekForward1", "frameBack", "frameForward", "cropLeft"] as const) {
      expect(shortcutFor(a).repeat, a).toBe(true);
    }
  });
});

describe("labels", () => {
  it("formats bindings per platform", () => {
    expect(shortcutLabels("playPause", "windows")).toEqual(["Space", "K"]);
    expect(shortcutLabel("exportDialog", "windows")).toBe("Ctrl+E");
    expect(shortcutLabel("exportDialog", "macos")).toBe("⌘E");
    expect(shortcutLabels("markIn", "windows")).toEqual(["I", "["]);
    expect(shortcutLabel("speedDown", "windows")).toBe("Shift+,");
    expect(shortcutLabel("cropLeftLarge", "windows")).toBe("Alt+Shift+←");
    expect(shortcutLabels("cheatSheet", "windows")).toEqual(["?"]);
    expect(formatBinding({ letter: "i", alt: true }, "macos")).toBe("⌥I");
  });

  it("produces aria-keyshortcuts values", () => {
    expect(ariaKeyShortcuts("playPause", "windows")).toBe("Space K");
    expect(ariaKeyShortcuts("undo", "windows")).toBe("Control+Z");
    expect(ariaKeyShortcuts("undo", "macos")).toBe("Meta+Z");
    expect(ariaKeyShortcuts("frameBack", "windows")).toBe(",");
  });
});

describe("platform detection", () => {
  it("reads navigator and AppInfo", () => {
    expect(detectPlatform({ platform: "MacIntel" })).toBe("macos");
    expect(detectPlatform({ platform: "Win32" })).toBe("windows");
    expect(detectPlatform({ userAgent: "X11; Linux x86_64" })).toBe("linux");
    expect(detectPlatform(undefined)).toBe("linux");
    expect(toPlatform("macos", "windows")).toBe("macos");
    expect(toPlatform("beos", "windows")).toBe("windows");
  });
});
