// Construction options of the MockBackend (all optional; tests inject them).

import type { ExportTiming } from "./exporter";
import type { KeyValueStore } from "./util";

export interface MockOptions {
  /** Where projects, settings and recents persist (default: `localStorage`, else memory). */
  storage?: KeyValueStore;
  /** Delay between a change and the simulated save (default 300 ms). */
  saveDelayMs?: number;
  /** Export simulation pacing. */
  exportTiming?: Partial<ExportTiming>;
  /** Total time the synthetic waveform takes to arrive (default 2000 ms). */
  waveformMs?: number;
  /** Clock used for timestamps and the `{date}` variable. */
  now?: () => Date;
  /** `save_text_file` sink (default: a browser download). */
  download?: (fileName: string, text: string) => void;
  /** Fake free space of the output drive in bytes (`null` = unknown). */
  freeBytes?: number | null;
}

/** Default `save_text_file`: trigger a browser download (no-op outside a browser). */
export function browserDownload(fileName: string, text: string): void {
  if (typeof document === "undefined") return;
  const url = URL.createObjectURL(new Blob([text], { type: "text/plain;charset=utf-8" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = fileName;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
