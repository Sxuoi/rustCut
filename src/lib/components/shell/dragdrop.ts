// Window-wide file drop (PRD §3.1 step 1 "Drag and drop"). OWNER: fe-shell.
//
// In the app: Tauri's native webview drag-drop events (real file paths).
// In a browser: HTML5 drag-drop with object URLs for the mock backend.
// Dropping also works while a project is open (it opens the new file).

import { t } from "$lib/i18n";
import { isTauri } from "$lib/ipc";
import { openBrowserFile, openDropped } from "$lib/state/session";
import { ui } from "$lib/state/ui.svelte";

function hasFiles(e: DragEvent): boolean {
  return !!e.dataTransfer && Array.from(e.dataTransfer.types).includes("Files");
}

function installHtmlDrop(win: Window): () => void {
  let depth = 0;
  const onEnter = (e: DragEvent): void => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    depth++;
    ui.dragOver = true;
  };
  const onOver = (e: DragEvent): void => {
    // Always prevent: a file dropped anywhere must never navigate the page.
    e.preventDefault();
    if (e.dataTransfer && hasFiles(e)) e.dataTransfer.dropEffect = "copy";
  };
  const onLeave = (e: DragEvent): void => {
    if (!hasFiles(e)) return;
    depth = Math.max(0, depth - 1);
    if (depth === 0) ui.dragOver = false;
  };
  const onDrop = (e: DragEvent): void => {
    e.preventDefault();
    depth = 0;
    ui.dragOver = false;
    const files = Array.from(e.dataTransfer?.files ?? []);
    if (files.length === 0) return;
    if (files.length > 1) ui.info(t("session.oneFileOnly"), "multiple_files");
    void openBrowserFile(files[0]);
  };
  win.addEventListener("dragenter", onEnter);
  win.addEventListener("dragover", onOver);
  win.addEventListener("dragleave", onLeave);
  win.addEventListener("drop", onDrop);
  return () => {
    win.removeEventListener("dragenter", onEnter);
    win.removeEventListener("dragover", onOver);
    win.removeEventListener("dragleave", onLeave);
    win.removeEventListener("drop", onDrop);
  };
}

async function installTauriDrop(): Promise<() => void> {
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  return getCurrentWebview().onDragDropEvent((event) => {
    const p = event.payload;
    if (p.type === "enter" || p.type === "over") ui.dragOver = true;
    else if (p.type === "leave") ui.dragOver = false;
    else {
      ui.dragOver = false;
      openDropped(p.paths);
    }
  });
}

/** Install window-wide drop handling; returns the uninstaller. */
export function installFileDrop(win: Window = window): () => void {
  if (!isTauri()) return installHtmlDrop(win);
  let unlisten: (() => void) | null = null;
  let disposed = false;
  installTauriDrop()
    .then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    })
    .catch(() => {
      // Without the event the File > Open path still works.
    });
  return () => {
    disposed = true;
    unlisten?.();
  };
}
