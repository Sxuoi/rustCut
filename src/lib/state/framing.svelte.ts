// Shared framing UI state (FRM-03, FRM-04, FRM-09). OWNER: fe-timeline.
//
// CONTRACT with Player.svelte (fe-shell):
//   * `framing.outputOnly` (boolean): the "9:16 only" / "1:1 only" toggle of
//     CropControls. When true (and a framed preset is active) the player shows
//     only what the export contains: the crop window for Vertical Crop, or the
//     whole output frame with its blurred background for the blur presets
//     (Vertical Blur, Square on Blur, Square Blur). It is a view preference,
//     not persisted and not undoable. The geometry is
//     `backend.frameLayout(framing, displayWidth, displayHeight, offset, resolution)`.
//   * `framing.offsetFor(clip)`: the crop offset to preview for a clip. It
//     equals the committed `clip.crop_x_offset` except while the user drags the
//     overlay or the slider, when it follows the uncommitted value so the
//     overlay and the slider stay in sync before the edit is committed on
//     pointer-up / change.
//   * `framing.offsetIgnored`: true when the source already fits the window's
//     width (9:16 or taller for Vertical Crop, 1:1 or taller for Square on
//     Blur), so the crop position has no effect (FRM-03); set by CropOverlay.

import type { Clip } from "$lib/bindings";

export class FramingUi {
  /** "9:16 only" / "1:1 only" toggle (FRM-04). */
  outputOnly = $state(false);
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
