import type { Backend } from "./backend";
import { MockBackend } from "./mock";
import { TauriBackend } from "./tauri";

export * from "./backend";

/** True inside the Tauri WebView. */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Process-wide backend: Tauri in the app, the in-memory mock in a browser. */
export const backend: Backend = isTauri() ? new TauriBackend() : new MockBackend();
