// Global keyboard and WebView guard listeners (PRD §5 rules 1-4). OWNER: fe-shell.
//
// The policy is the pure `resolveKey` (keymap.ts); this file only wires DOM
// events to it and performs the decision.

import { DECODER_PACED, describeTarget, isBrowserAccelerator, resolveKey } from "./keymap";
import type { ActionId, Platform } from "./shortcuts";

export interface ControllerDeps {
  platform(): Platform;
  /** A modal dialog is open (global shortcuts pause). */
  dialogOpen(): boolean;
  /** Production build: the WebView context menu is disabled outside text fields. */
  production: boolean;
  /** `player.now()`: called synchronously before anything else (§5 rule 4). */
  captureNow(): number;
  run(action: ActionId, now: number, repeat: boolean): void;
  closeDialogs(): void;
  focusPlayer(): void;
  /** A seek is in flight: auto-repeat waits for the decoder (PLY-02). */
  decoderBusy(): boolean;
}

/** Install the listeners on `win`; returns the uninstaller. */
export function installKeyboard(deps: ControllerDeps, win: Window = window): () => void {
  // Events whose default the guard (not a component) prevented.
  const guarded = new WeakSet<Event>();

  // Capture phase: block browser accelerators even if a component stops
  // propagation (§5 rule 3).
  const onKeyDownCapture = (e: KeyboardEvent): void => {
    if (isBrowserAccelerator(e, deps.platform(), describeTarget(e.target).textEntry)) {
      e.preventDefault();
      guarded.add(e);
    }
  };

  // Bubble phase: components (rename editor, menus, dialogs) handled their
  // own keys first and called preventDefault.
  const onKeyDown = (e: KeyboardEvent): void => {
    const decision = resolveKey(e, {
      platform: deps.platform(),
      target: describeTarget(e.target),
      dialogOpen: deps.dialogOpen(),
      handledByComponent: e.defaultPrevented && !guarded.has(e),
    });
    switch (decision.kind) {
      case "none":
        return;
      case "block":
        e.preventDefault();
        return;
      case "leaveField":
        e.preventDefault();
        if (e.target instanceof HTMLElement) e.target.blur();
        deps.focusPlayer();
        return;
      case "closeDialogs":
        e.preventDefault();
        deps.closeDialogs();
        return;
      case "action": {
        // §5 rule 4: the playhead is read first, synchronously.
        const now = decision.def.capturesTime ? deps.captureNow() : Number.NaN;
        e.preventDefault();
        if (decision.repeat && DECODER_PACED.has(decision.action) && deps.decoderBusy()) return;
        deps.run(decision.action, now, decision.repeat);
        return;
      }
    }
  };

  // Ctrl+wheel / trackpad pinch page zoom (§5 rule 3). Components still get
  // the event (the zoom timeline uses the wheel).
  const onWheel = (e: WheelEvent): void => {
    if (e.ctrlKey || (deps.platform() === "macos" && e.metaKey)) e.preventDefault();
  };
  const onGesture = (e: Event): void => e.preventDefault(); // WKWebView pinch zoom

  const onContextMenu = (e: MouseEvent): void => {
    if (deps.production && !describeTarget(e.target).textEntry) e.preventDefault();
  };

  // Mouse back/forward buttons navigate the WebView history.
  const onMouseButton = (e: MouseEvent): void => {
    if (e.button === 3 || e.button === 4) e.preventDefault();
  };

  const nonPassive: AddEventListenerOptions = { passive: false };
  win.addEventListener("keydown", onKeyDownCapture, true);
  win.addEventListener("keydown", onKeyDown);
  win.addEventListener("wheel", onWheel, nonPassive);
  win.addEventListener("gesturestart", onGesture, nonPassive);
  win.addEventListener("contextmenu", onContextMenu);
  win.addEventListener("mousedown", onMouseButton);
  win.addEventListener("mouseup", onMouseButton);
  win.addEventListener("auxclick", onMouseButton);

  return () => {
    win.removeEventListener("keydown", onKeyDownCapture, true);
    win.removeEventListener("keydown", onKeyDown);
    win.removeEventListener("wheel", onWheel);
    win.removeEventListener("gesturestart", onGesture);
    win.removeEventListener("contextmenu", onContextMenu);
    win.removeEventListener("mousedown", onMouseButton);
    win.removeEventListener("mouseup", onMouseButton);
    win.removeEventListener("auxclick", onMouseButton);
  };
}
