# Keyboard (PRD §5) — conventions for every component

Owner: fe-shell. Files:

| File | Role |
| :--- | :--- |
| `shortcuts.ts` | **The** shortcut table (`SHORTCUTS`): the key handler, the cheat sheet, menus and tooltips all read it. Matching (`matchShortcut`) and labels (`shortcutLabel`, `ariaKeyShortcuts`). |
| `keymap.ts` | Pure policy `resolveKey(event, context)`: text fields, dialogs, native controls, browser accelerators (rule 3). |
| `actions.ts` | What each action does (`createActions`), shared by keys, Transport buttons and menus. |
| `controller.ts` | Window listeners (keydown, Ctrl+wheel, context menu, mouse back/forward). |
| `index.ts` | `actions` wired to the real stores, `installGlobalKeyboard()` (called once by `App.svelte`). |

## Focus convention (rule 1)

The global handler listens on `window` in the **bubble** phase, so a focused
component always sees a key first.

1. **Text fields suspend every shortcut.** Any `<input>` (text-like types),
   `<textarea>`, `<select>`, `contenteditable` element or element with role
   `textbox` / `searchbox` / `combobox` / `spinbutton` is a text field. Nothing
   in §5 fires while one has focus (AC-13). Browser accelerators (F5, Ctrl+R,
   Ctrl+P, …) are still blocked there.
2. **A component that handles a key calls `event.preventDefault()`.** The
   global handler ignores keys whose default was prevented by a component.
3. **Inline rename editor (fe-queue, `ui.renameRequest`):**
   * on `Enter`: commit the rename (`project.editClips([id], { kind: "rename", name })`),
     call `event.preventDefault()`, then `ui.focusPlayer()` so the next `I`/`O`/`Space`
     works immediately;
   * on `Esc`: cancel (restore the old name), call `event.preventDefault()`, then
     `ui.focusPlayer()`;
   * on blur: commit (or cancel when unchanged) without moving focus.
4. **Any other text field** that does not handle `Esc` itself is left by `Esc`:
   the global handler blurs it and calls `ui.focusPlayer()` (§5 "Esc: leave a
   text field").
5. **Dialogs:** while `ui.anyDialogOpen` is true no global shortcut fires. Use a
   native `<dialog>` opened with `showModal()` (Esc closes it and fires `close`;
   set your `ui.*Open` flag to `false` there). If focus is somehow outside the
   open dialog, `Esc` closes all dialogs (`ui.closeAllDialogs()`).
6. **Native controls keep their keys:** `Space`/`Enter` activate a focused
   button, link, checkbox, menu item, option or tab (no play/pause, no add);
   arrow keys move inside sliders, listboxes, menus, radio groups, grids and
   tab lists. Custom widgets with those ARIA roles get the same treatment.
   Toolbar buttons that should not keep focus after a mouse click (so `Space`
   keeps controlling playback) prevent focus on `mousedown`, as Transport does.

## Using actions from components

```ts
import { actions } from "$lib/keyboard";
import { player } from "$lib/state/player.svelte";

// Mark actions: capture the time synchronously, first thing in the handler.
onclick={() => actions.run("markIn", player.now())}
onclick={() => actions.run("selectNext")}
```

Show the shortcut with `shortcutLabel(action, ui.platform)` and expose it to
assistive technology with `aria-keyshortcuts={ariaKeyShortcuts(action, ui.platform)}`.

## Store hooks for other components

* `ui.queueOrder`: the queue's visible order (sort + search) as clip ids; `↑`/`↓`
  follow it (null = by clip number).
* `ui.zoomWindowSec` / `ui.setZoomWindow(sec)`: zoom timeline window (2 s – 10 min),
  restored from and saved to the project's view state.
* `ui.focusPlayer()`: give focus back to the player.
