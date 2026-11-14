// Shared framing UI state (FRM-03, FRM-04). OWNER: fe-timeline.
//
// CONTRACT with Player.svelte (fe-shell):
//   * `framing.nineSixteenOnly` (boolean): the "9:16 only" toggle of
//     CropControls. When true (and the Vertical Crop preset is active) the
//     player should show only the 9:16 window region of the video instead of
//     the full frame with the dimmed outside. It is a view preference, not
//     persisted and not undoable. The window rectangle in source pixels is
//     `backend.cropWindow(displayWidth, displayHeight, framing.offsetFor(clip))`.
//   * `framing.offsetFor(clip)`: the crop offset to preview for a clip. It
//     equals the committed `clip.crop_x_offset` except while the user drags the
//     overlay or the slider, when it follows the uncommitted value so the
//     overlay and the slider stay in sync before the edit is committed on
//     pointer-up / change.
//   * `framing.offsetIgnored`: true when the source is 9:16 or taller, so the
//     crop position has no effect (FRM-02/FRM-03); set by CropOverlay.

import type { Clip } from "$lib/bindings";

export class FramingUi {
  /** "9:16 only" toggle (FRM-04). */
  nineSixteenOnly = $state(false);
  /** Uncommitted offset while dragging the overlay or the slider. */
  preview = $state<{ id: string; offset: number } | null>(null);
  /** The crop position is ignored for this source (offset has no effect). */
  offsetIgnored = $state(false);

  /** Offset to display for `clip` (live preview if one is in progress). */
  offsetFor(clip: Pick<Clip, "id" | "crop_x_offset"> | null): number {
    if (!clip) return 0;
    const p = this.preview;
    return p && p.id === clip.id ? p.offset : clip.crop_x_offset;
  }

  setPreview(id: string, offset: number): void {
    this.preview = { id, offset };
  }

  clearPreview(): void {
    this.preview = null;
  }
}

export const framing = new FramingUi();
