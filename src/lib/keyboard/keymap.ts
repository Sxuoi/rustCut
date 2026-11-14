// Pure key -> decision mapping (PRD §5 rules 1-3, AC-13). OWNER: fe-shell.
//
// `resolveKey` is the whole policy of the global key handler, testable
// without a DOM: the controller (controller.ts) only feeds it a KeyboardEvent
// plus a description of the focused element, then performs the decision.
//
// Rule 1: shortcuts are suspended while a text field has focus (Esc leaves a
//         field that did not handle Esc itself; the field's own Enter/Esc
//         handlers run first and call preventDefault).
// Rule 2: matching is in shortcuts.ts (punctuation by code, letters by key).
// Rule 3: WebView browser accelerators are blocked everywhere, text fields and
//         dialogs included, so they can never fire.
// Native controls keep their own keys: Space/Enter activate a focused button,
// arrow keys move inside sliders, listboxes, menus and radio groups.

import { letterOf, matchShortcut, primaryDown, type ActionId, type KeyInput, type Platform, type ShortcutDef } from "./shortcuts";

/** What the focused element does with keys (computed by `describeTarget`). */
export interface TargetInfo {
  /** Text input, textarea, select, contenteditable, textbox/combobox role. */
  textEntry: boolean;
  /** Space/Enter activate it natively (button, link, checkbox, menuitem…). */
  activates: boolean;
  /** Arrow keys navigate inside it (slider, listbox, menu, radio group…). */
  arrowNav: boolean;
  /** Inside an open dialog (`<dialog>`, role=dialog/alertdialog). */
  inDialog: boolean;
}

/** Focus on the document body / player: every shortcut applies. */
export const NO_TARGET: Readonly<TargetInfo> = Object.freeze({
  textEntry: false,
  activates: false,
  arrowNav: false,
  inDialog: false,
});

/** The DOM surface `describeTarget` reads (fakeable in tests). */
export interface ElementLike {
  tagName: string;
  getAttribute(name: string): string | null;
  isContentEditable?: boolean;
  parentElement: ElementLike | null;
}

const NON_TEXT_INPUTS = new Set(["button", "checkbox", "radio", "range", "submit", "reset", "color", "file", "image", "hidden"]);
const ACTIVATING_INPUTS = new Set(["button", "checkbox", "radio", "submit", "reset", "color", "file", "image"]);
const TEXT_ROLES = new Set(["textbox", "searchbox", "combobox", "spinbutton"]);
const ACTIVATING_ROLES = new Set([
  "button",
  "link",
  "menuitem",
  "menuitemradio",
  "menuitemcheckbox",
  "option",
  "tab",
  "checkbox",
  "radio",
  "switch",
  "treeitem",
]);
const ARROW_ROLES = new Set([
  "slider",
  "listbox",
  "menu",
  "menubar",
  "tablist",
  "radiogroup",
  "tree",
  "treegrid",
  "grid",
  "scrollbar",
  "spinbutton",
]);
const DIALOG_ROLES = new Set(["dialog", "alertdialog"]);
const ARROW_KEYS = new Set(["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"]);

function isElementLike(x: unknown): x is ElementLike {
  return (
    typeof x === "object" &&
    x !== null &&
    typeof (x as ElementLike).tagName === "string" &&
    typeof (x as ElementLike).getAttribute === "function"
  );
}

/** Classify the focused element (the KeyboardEvent target). */
export function describeTarget(target: unknown): TargetInfo {
  if (!isElementLike(target)) return NO_TARGET;
  const tag = target.tagName.toLowerCase();
  const role = target.getAttribute("role");
  const type = (target.getAttribute("type") ?? "text").toLowerCase();
  const textEntry =
    tag === "textarea" ||
    tag === "select" ||
    (tag === "input" && !NON_TEXT_INPUTS.has(type)) ||
    target.isContentEditable === true ||
    (role !== null && TEXT_ROLES.has(role));
  const activates =
    tag === "button" ||
    tag === "summary" ||
    (tag === "a" && target.getAttribute("href") !== null) ||
    (tag === "input" && ACTIVATING_INPUTS.has(type)) ||
    (role !== null && ACTIVATING_ROLES.has(role));
  let arrowNav = tag === "select" || (tag === "input" && (type === "range" || type === "radio"));
  let inDialog = false;
  let el: ElementLike | null = target;
  for (let depth = 0; el && depth < 64; depth++, el = el.parentElement) {
    const r = el.getAttribute("role");
    if (r !== null && ARROW_ROLES.has(r)) arrowNav = true;
    if (el.tagName.toLowerCase() === "dialog" || (r !== null && DIALOG_ROLES.has(r))) {
      inDialog = true;
      break;
    }
  }
  return { textEntry, activates, arrowNav, inDialog };
}

/** Keys that are browser accelerators on their own (any modifiers). */
const BLOCKED_KEYS = new Set([
  "F3", // find next
  "F5", // reload (also Ctrl+F5, Shift+F5)
  "F7", // caret browsing
  "BrowserBack",
  "BrowserForward",
  "BrowserRefresh",
  "BrowserSearch",
  "BrowserFavorites",
  "BrowserHome",
  "BrowserStop",
]);
/**
 * Ctrl/Cmd + letter accelerators: reload (R), print (P), find (F, G, H), view
 * source (U), save page (S), open file (O), search (E), downloads (J),
 * bookmark (D), new window/tab (N, T). None of them edits text, so blocking
 * them inside text fields is safe; Ctrl+A/C/V/X/Z/Y are never touched.
 */
const BLOCKED_PRIMARY_LETTERS = new Set(["r", "p", "f", "g", "u", "s", "o", "e", "j", "h", "d", "n", "t"]);
/** Ctrl/Cmd + plus/minus/0 page zoom. */
const ZOOM_CODES = new Set(["Equal", "Minus", "Digit0", "NumpadAdd", "NumpadSubtract", "Numpad0"]);
const ZOOM_KEYS = new Set(["+", "-", "=", "_", "0"]);

/** AltGr reports Ctrl+Alt on Windows; it types characters, never shortcuts. */
function isAltGraph(e: KeyInput, platform: Platform): boolean {
  return platform !== "macos" && e.ctrlKey && e.altKey;
}

/**
 * §5 rule 3: is this key press a WebView browser accelerator (reload, print,
 * find, history navigation, page zoom, …) that must be prevented?
 * `inTextField` keeps macOS Cmd+←/→ (line start/end) working while typing.
 */
export function isBrowserAccelerator(e: KeyInput, platform: Platform, inTextField: boolean): boolean {
  if (BLOCKED_KEYS.has(e.key)) return true;
  if (isAltGraph(e, platform)) return false;
  if (primaryDown(e, platform)) {
    const letter = letterOf(e);
    if (letter !== null && BLOCKED_PRIMARY_LETTERS.has(letter)) return true;
    if (ZOOM_CODES.has(e.code) || ZOOM_KEYS.has(e.key)) return true;
  }
  const horizontal = e.key === "ArrowLeft" || e.key === "ArrowRight";
  if (platform === "macos") {
    if (e.metaKey && (e.code === "BracketLeft" || e.code === "BracketRight")) return true; // Cmd+[ / Cmd+] history
    if (e.metaKey && horizontal && !inTextField) return true; // Cmd+←/→ history
    return false;
  }
  return e.altKey && horizontal; // Alt+←/→ history (Windows, Linux)
}

/** A key event as the policy sees it. */
export interface KeyEventLike extends KeyInput {
  /** IME composition in progress: never a shortcut. */
  isComposing?: boolean;
}

/** Everything besides the key that the policy depends on. */
export interface KeyContext {
  platform: Platform;
  target: TargetInfo;
  /** A modal dialog is open (`ui.anyDialogOpen`): global shortcuts pause. */
  dialogOpen: boolean;
  /** A component already handled the key (it called preventDefault). */
  handledByComponent: boolean;
}

export type KeyDecision =
  /** Let the event through untouched. */
  | { kind: "none" }
  /** preventDefault only (browser accelerator, or a swallowed auto-repeat). */
  | { kind: "block" }
  /** Esc in a text field that did not handle it: blur, focus the player. */
  | { kind: "leaveField" }
  /** Esc while a dialog is open but focus is outside it: close dialogs. */
  | { kind: "closeDialogs" }
  /** preventDefault and run the action. */
  | { kind: "action"; action: ActionId; def: ShortcutDef; repeat: boolean };

const NONE: KeyDecision = { kind: "none" };
const BLOCK: KeyDecision = { kind: "block" };

function isPlainEscape(e: KeyInput): boolean {
  return e.key === "Escape" && !e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey;
}

/**
 * The global key policy (§5, AC-13): which shortcut a key press triggers in
 * the current focus context, or what else to do with it.
 */
export function resolveKey(e: KeyEventLike, ctx: KeyContext): KeyDecision {
  if (e.isComposing === true || e.key === "Process") return NONE;
  const pass = isBrowserAccelerator(e, ctx.platform, ctx.target.textEntry) ? BLOCK : NONE;
  if (ctx.handledByComponent) return pass;
  if (ctx.dialogOpen) {
    return isPlainEscape(e) && !ctx.target.inDialog ? { kind: "closeDialogs" } : pass;
  }
  if (ctx.target.textEntry) {
    return isPlainEscape(e) ? { kind: "leaveField" } : pass;
  }
  const def = matchShortcut(e, ctx.platform);
  if (!def) return pass;
  const activationKey = e.key === " " || e.key === "Spacebar" || e.key === "Enter";
  if (activationKey && ctx.target.activates) return pass;
  if (ARROW_KEYS.has(e.key) && ctx.target.arrowNav) return pass;
  if (e.repeat === true && !def.repeat) return BLOCK;
  return { kind: "action", action: def.action, def, repeat: e.repeat === true };
}

/**
 * Actions whose auto-repeat waits for the decoder (PLY-02 "holding a key
 * repeats at the decoder's pace"): a repeat is dropped while a seek is still
 * in flight instead of queueing seeks faster than frames can be shown.
 */
export const DECODER_PACED: ReadonlySet<ActionId> = new Set<ActionId>([
  "seekBack5",
  "seekForward5",
  "seekBack1",
  "seekForward1",
  "frameBack",
  "frameForward",
]);
