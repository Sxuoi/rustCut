// Keyboard entry point: the shared action runner wired to the real stores,
// and the global listener installer (PRD §5). OWNER: fe-shell.
//
// Components trigger §5 actions through `actions.run(...)` (buttons, menus)
// so mouse and keyboard share one implementation. Mark actions must be given
// the time captured synchronously: `actions.run("markIn", player.now())`.

import { player } from "$lib/state/player.svelte";
import { project } from "$lib/state/project.svelte";
import { openWithDialog } from "$lib/state/session";
import { ui } from "$lib/state/ui.svelte";
import { createActions } from "./actions";
import { installKeyboard } from "./controller";

export { createActions } from "./actions";
export type { Actions } from "./actions";
export * from "./shortcuts";

/** The app-wide action runner. */
export const actions = createActions({ player, project, ui, open: openWithDialog });

/** Install the global key handler and WebView guards; returns the uninstaller. */
export function installGlobalKeyboard(): () => void {
  return installKeyboard({
    platform: () => ui.platform,
    dialogOpen: () => ui.anyDialogOpen,
    production: !import.meta.env.DEV,
    captureNow: () => player.now(),
    run: (action, now, repeat) => actions.run(action, now, repeat),
    closeDialogs: () => ui.closeAllDialogs(),
    focusPlayer: () => ui.focusPlayer(),
    decoderBusy: () => player.seeking,
  });
}
