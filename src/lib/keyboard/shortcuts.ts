// Keyboard shortcuts (PRD §5). OWNER: fe-shell.
//
// SHORTCUTS is the single source of truth: the global key handler
// (controller.ts), the cheat sheet, the menus and button tooltips all read it.
//
// Matching rules (§5):
//   2. Punctuation binds to physical keys (KeyboardEvent.code: Comma, Period,
//      BracketLeft/Right, Slash) so it works on non-US layouts; letters follow
//      the typed character (KeyboardEvent.key, case-insensitive). Modifiers must
//      match exactly, so `I`, `Shift+I` and `Alt+I` are different shortcuts.
//      When the typed character is not a Latin letter (non-Latin layouts, or
//      macOS Option combinations that type "ˆ"/"ø"), the physical key is used.
//   On macOS, Ctrl = Cmd: the "primary" modifier is Meta there, Ctrl elsewhere.

/** Desktop platform; decides the primary modifier and platform-only bindings. */
export type Platform = "windows" | "macos" | "linux";

/** Every keyboard-invocable action (§5 table). */
export type ActionId =
  | "playPause"
  | "seekBack5"
  | "seekForward5"
  | "seekBack1"
  | "seekForward1"
  | "frameBack"
  | "frameForward"
  | "speedDown"
  | "speedUp"
  | "markIn"
  | "markOut"
  | "addClip"
  | "escape"
  | "rename"
  | "selectPrevious"
  | "selectNext"
  | "loopToggle"
  | "deleteSelection"
  | "undo"
  | "redo"
  | "exportDialog"
  | "saveNow"
  | "open"
  | "goToIn"
  | "goToOut"
  | "retimeIn"
  | "retimeOut"
  | "cropLeft"
  | "cropRight"
  | "cropLeftLarge"
  | "cropRightLarge"
  | "cheatSheet";

/**
 * One key combination. Exactly one of `letter`, `code` or `named` is set.
 * Modifiers not listed must be up (Shift is ignored only for `char` aliases).
 */
export interface KeyBinding {
  /** Latin letter matched on the typed character, case-insensitive ("i"). */
  letter?: string;
  /** Physical key (KeyboardEvent.code), for punctuation ("Comma"). */
  code?: string;
  /** Named key (KeyboardEvent.key): " ", "Enter", "Escape", "ArrowLeft", … */
  named?: string;
  /** Typed character alias, any layout ("?"); Shift state is not checked. */
  char?: string;
  /** Ctrl on Windows/Linux, Cmd on macOS. */
  primary?: boolean;
  shift?: boolean;
  alt?: boolean;
  /** Only on these platforms (e.g. Ctrl+Y redo: not on macOS). */
  platforms?: readonly Platform[];
}

export type ShortcutGroup = "playback" | "marking" | "queue" | "project" | "framing" | "help";

/** One row of the §5 table. */
export interface ShortcutDef {
  action: ActionId;
  /** Alternatives; the first one is the canonical label. */
  bindings: readonly KeyBinding[];
  group: ShortcutGroup;
  priority: "P0" | "P1";
  /** i18n key of the description shown in the cheat sheet and menus. */
  labelKey: string;
  /** Auto-repeats while held (seeks, steps, nudges); toggles never repeat. */
  repeat: boolean;
  /**
   * Captures the playhead synchronously (§5 rule 4): the controller reads
   * `player.now()` before anything else and passes it to the action.
   */
  capturesTime?: boolean;
}

const NOT_MAC: readonly Platform[] = ["windows", "linux"];

/** PRD §5, P0 and P1 rows. Order = cheat-sheet order within each group. */
export const SHORTCUTS: readonly ShortcutDef[] = [
  // Playback (PLY-02, PLY-03)
  { action: "playPause", bindings: [{ named: " " }, { letter: "k" }], group: "playback", priority: "P0", labelKey: "kb.playPause", repeat: false },
  { action: "seekBack5", bindings: [{ letter: "j" }], group: "playback", priority: "P0", labelKey: "kb.seekBack5", repeat: true },
  { action: "seekForward5", bindings: [{ letter: "l" }], group: "playback", priority: "P0", labelKey: "kb.seekForward5", repeat: true },
  { action: "seekBack1", bindings: [{ named: "ArrowLeft" }], group: "playback", priority: "P0", labelKey: "kb.seekBack1", repeat: true },
  { action: "seekForward1", bindings: [{ named: "ArrowRight" }], group: "playback", priority: "P0", labelKey: "kb.seekForward1", repeat: true },
  { action: "frameBack", bindings: [{ code: "Comma" }], group: "playback", priority: "P0", labelKey: "kb.frameBack", repeat: true },
  { action: "frameForward", bindings: [{ code: "Period" }], group: "playback", priority: "P0", labelKey: "kb.frameForward", repeat: true },
  { action: "speedDown", bindings: [{ code: "Comma", shift: true }], group: "playback", priority: "P0", labelKey: "kb.speedDown", repeat: false },
  { action: "speedUp", bindings: [{ code: "Period", shift: true }], group: "playback", priority: "P0", labelKey: "kb.speedUp", repeat: false },
  // Marking (MRK-01, MRK-05)
  { action: "markIn", bindings: [{ letter: "i" }, { code: "BracketLeft" }], group: "marking", priority: "P0", labelKey: "kb.markIn", repeat: false, capturesTime: true },
  { action: "markOut", bindings: [{ letter: "o" }, { code: "BracketRight" }], group: "marking", priority: "P0", labelKey: "kb.markOut", repeat: false, capturesTime: true },
  { action: "addClip", bindings: [{ named: "Enter" }, { letter: "a" }], group: "marking", priority: "P0", labelKey: "kb.addClip", repeat: false, capturesTime: true },
  { action: "escape", bindings: [{ named: "Escape" }], group: "marking", priority: "P0", labelKey: "kb.escape", repeat: false },
  { action: "goToIn", bindings: [{ letter: "i", shift: true }], group: "marking", priority: "P1", labelKey: "kb.goToIn", repeat: false },
  { action: "goToOut", bindings: [{ letter: "o", shift: true }], group: "marking", priority: "P1", labelKey: "kb.goToOut", repeat: false },
  { action: "retimeIn", bindings: [{ letter: "i", alt: true }], group: "marking", priority: "P1", labelKey: "kb.retimeIn", repeat: false, capturesTime: true },
  { action: "retimeOut", bindings: [{ letter: "o", alt: true }], group: "marking", priority: "P1", labelKey: "kb.retimeOut", repeat: false, capturesTime: true },
  // Clip queue (QUE-03, QUE-04, MRK-04)
  { action: "rename", bindings: [{ letter: "n" }], group: "queue", priority: "P0", labelKey: "kb.rename", repeat: false },
  { action: "selectPrevious", bindings: [{ named: "ArrowUp" }], group: "queue", priority: "P0", labelKey: "kb.selectPrevious", repeat: true },
  { action: "selectNext", bindings: [{ named: "ArrowDown" }], group: "queue", priority: "P0", labelKey: "kb.selectNext", repeat: true },
  { action: "loopToggle", bindings: [{ letter: "p" }], group: "queue", priority: "P0", labelKey: "kb.loopToggle", repeat: false },
  { action: "deleteSelection", bindings: [{ named: "Delete" }, { named: "Backspace" }], group: "queue", priority: "P0", labelKey: "kb.deleteSelection", repeat: false },
  { action: "undo", bindings: [{ letter: "z", primary: true }], group: "queue", priority: "P0", labelKey: "kb.undo", repeat: true },
  {
    action: "redo",
    bindings: [{ letter: "z", primary: true, shift: true }, { letter: "y", primary: true, platforms: NOT_MAC }],
    group: "queue",
    priority: "P0",
    labelKey: "kb.redo",
    repeat: true,
  },
  // Project (EXP-01, PRJ-01)
  { action: "exportDialog", bindings: [{ letter: "e", primary: true }], group: "project", priority: "P0", labelKey: "kb.exportDialog", repeat: false },
  { action: "saveNow", bindings: [{ letter: "s", primary: true }], group: "project", priority: "P0", labelKey: "kb.saveNow", repeat: false },
  { action: "open", bindings: [{ letter: "o", primary: true }], group: "project", priority: "P0", labelKey: "kb.open", repeat: false },
  // Framing (FRM-03)
  { action: "cropLeft", bindings: [{ named: "ArrowLeft", alt: true }], group: "framing", priority: "P1", labelKey: "kb.cropLeft", repeat: true },
  { action: "cropRight", bindings: [{ named: "ArrowRight", alt: true }], group: "framing", priority: "P1", labelKey: "kb.cropRight", repeat: true },
  { action: "cropLeftLarge", bindings: [{ named: "ArrowLeft", alt: true, shift: true }], group: "framing", priority: "P1", labelKey: "kb.cropLeftLarge", repeat: true },
  { action: "cropRightLarge", bindings: [{ named: "ArrowRight", alt: true, shift: true }], group: "framing", priority: "P1", labelKey: "kb.cropRightLarge", repeat: true },
  // Help
  { action: "cheatSheet", bindings: [{ code: "Slash", shift: true }, { char: "?" }], group: "help", priority: "P1", labelKey: "kb.cheatSheet", repeat: false },
];

/** Cheat-sheet group order and titles. */
export const SHORTCUT_GROUPS: readonly { group: ShortcutGroup; labelKey: string }[] = [
  { group: "playback", labelKey: "kb.group.playback" },
  { group: "marking", labelKey: "kb.group.marking" },
  { group: "queue", labelKey: "kb.group.queue" },
  { group: "project", labelKey: "kb.group.project" },
  { group: "framing", labelKey: "kb.group.framing" },
  { group: "help", labelKey: "kb.group.help" },
];

const BY_ACTION = new Map<ActionId, ShortcutDef>(SHORTCUTS.map((s) => [s.action, s]));

/** The table row of an action. */
export function shortcutFor(action: ActionId): ShortcutDef {
  const def = BY_ACTION.get(action);
  if (!def) throw new Error(`unknown shortcut action: ${action}`);
  return def;
}

/** The subset of KeyboardEvent the matcher reads (testable without a DOM). */
export interface KeyInput {
  key: string;
  code: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  repeat?: boolean;
}

/** Detect the platform from `navigator` (AppInfo.platform wins when known). */
export function detectPlatform(nav: { platform?: string; userAgent?: string } | undefined): Platform {
  const s = `${nav?.platform ?? ""} ${nav?.userAgent ?? ""}`;
  if (/Mac|iPhone|iPad/i.test(s)) return "macos";
  if (/Win/i.test(s)) return "windows";
  return "linux";
}

/** Normalize AppInfo.platform ("windows" | "macos" | "linux"). */
export function toPlatform(name: string | null | undefined, fallback: Platform): Platform {
  return name === "windows" || name === "macos" || name === "linux" ? name : fallback;
}

/** Ctrl on Windows/Linux, Cmd on macOS. */
export function primaryDown(e: KeyInput, platform: Platform): boolean {
  return platform === "macos" ? e.metaKey : e.ctrlKey;
}

/** The modifier that is not the primary one (Ctrl on macOS, Meta/Win elsewhere). */
function otherModifierDown(e: KeyInput, platform: Platform): boolean {
  return platform === "macos" ? e.ctrlKey : e.metaKey;
}

/**
 * The Latin letter of a key press: the typed character when it is a Latin
 * letter, else the physical key (KeyA–KeyZ) for non-Latin layouts and macOS
 * Option combinations. `null` for non-letter keys.
 */
export function letterOf(e: Pick<KeyInput, "key" | "code">): string | null {
  if (e.key.length === 1 && /^[a-z]$/i.test(e.key)) return e.key.toLowerCase();
  const typedSomethingElse = e.key.length === 1 || e.key === "Dead" || e.key === "Unidentified";
  const m = /^Key([A-Z])$/.exec(e.code);
  return typedSomethingElse && m ? m[1].toLowerCase() : null;
}

/** Does `e` press binding `b` on `platform`? */
export function bindingMatches(b: KeyBinding, e: KeyInput, platform: Platform): boolean {
  if (b.platforms && !b.platforms.includes(platform)) return false;
  if (otherModifierDown(e, platform)) return false;
  if (primaryDown(e, platform) !== !!b.primary) return false;
  if (e.altKey !== !!b.alt) return false;
  if (b.char !== undefined) return e.key === b.char;
  if (e.shiftKey !== !!b.shift) return false;
  if (b.letter !== undefined) return letterOf(e) === b.letter;
  if (b.code !== undefined) return e.code === b.code;
  if (b.named !== undefined) return e.key === b.named || (b.named === " " && e.key === "Spacebar");
  return false;
}

/** The shortcut a key press triggers, or `null` (§5 rules 2 and 3). */
export function matchShortcut(e: KeyInput, platform: Platform): ShortcutDef | null {
  for (const def of SHORTCUTS) {
    if (def.bindings.some((b) => bindingMatches(b, e, platform))) return def;
  }
  return null;
}

const CODE_LABELS: Record<string, string> = {
  Comma: ",",
  Period: ".",
  BracketLeft: "[",
  BracketRight: "]",
  Slash: "/",
};

const NAMED_LABELS: Record<string, string> = {
  " ": "Space",
  ArrowLeft: "←",
  ArrowRight: "→",
  ArrowUp: "↑",
  ArrowDown: "↓",
  Escape: "Esc",
  Enter: "Enter",
  Delete: "Delete",
  Backspace: "Backspace",
};

function keyLabel(b: KeyBinding): string {
  if (b.char !== undefined) return b.char;
  if (b.letter !== undefined) return b.letter.toUpperCase();
  if (b.code !== undefined) return CODE_LABELS[b.code] ?? b.code;
  return NAMED_LABELS[b.named ?? ""] ?? b.named ?? "";
}

/** Human label of one binding: `Ctrl+Shift+Z`, `⌘⇧Z`, `Shift+,`, `?`. */
export function formatBinding(b: KeyBinding, platform: Platform): string {
  // `Shift+/` reads as `?` on the cheat sheet (the PRD names the key `?`).
  if (b.code === "Slash" && b.shift && !b.primary && !b.alt) return "?";
  const key = keyLabel(b);
  if (platform === "macos") {
    return `${b.primary ? "⌘" : ""}${b.alt ? "⌥" : ""}${b.shift ? "⇧" : ""}${key}`;
  }
  const mods = [b.primary ? "Ctrl" : null, b.alt ? "Alt" : null, b.shift ? "Shift" : null].filter(Boolean);
  return [...mods, key].join("+");
}

/** Bindings of an action that exist on `platform` (cheat sheet, menus). */
export function bindingsOn(def: ShortcutDef, platform: Platform): KeyBinding[] {
  const seen = new Set<string>();
  const out: KeyBinding[] = [];
  for (const b of def.bindings) {
    if (b.platforms && !b.platforms.includes(platform)) continue;
    const label = formatBinding(b, platform);
    if (seen.has(label)) continue; // "?" alias duplicates Shift+/
    seen.add(label);
    out.push(b);
  }
  return out;
}

/** All labels of an action: `["Space", "K"]`. */
export function shortcutLabels(action: ActionId, platform: Platform): string[] {
  return bindingsOn(shortcutFor(action), platform).map((b) => formatBinding(b, platform));
}

/** The canonical label of an action (menus, tooltips): `Ctrl+E`. */
export function shortcutLabel(action: ActionId, platform: Platform): string {
  return shortcutLabels(action, platform)[0] ?? "";
}

/** `aria-keyshortcuts` value (WAI-ARIA syntax): `Space K`, `Control+Z`. */
export function ariaKeyShortcuts(action: ActionId, platform: Platform): string {
  return bindingsOn(shortcutFor(action), platform)
    .map((b) => {
      const mods = [
        b.primary ? (platform === "macos" ? "Meta" : "Control") : null,
        b.alt ? "Alt" : null,
        b.shift ? "Shift" : null,
      ].filter(Boolean);
      let key: string;
      if (b.char !== undefined) key = b.char;
      else if (b.letter !== undefined) key = b.letter.toUpperCase();
      else if (b.code !== undefined) key = CODE_LABELS[b.code] ?? b.code;
      else key = b.named === " " ? "Space" : (b.named ?? "");
      return [...mods, key].join("+");
    })
    .join(" ");
}
