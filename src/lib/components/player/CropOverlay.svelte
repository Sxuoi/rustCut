<script lang="ts">
  // FRM-03/FRM-04/FRM-09, PRD 6.3: the crop window over the video (9:16 for
  // Vertical Crop, 1:1 for Square on Blur) with the outside dimmed. Drag
  // horizontally (or use the arrow keys) to change the PRIMARY clip's
  // crop_x_offset; the edit is committed on pointer-up.
  //
  // Props: rect = the whole video's box in CSS px (relative to this overlay's
  // parent), displayWidth/displayHeight = the source's display frame (§7.7),
  // win = the window to draw (source px). It comes from the Player's layout:
  // the authoritative Rust layout (backend.frameLayout, the function the
  // export uses) for the committed offset, util/crop.ts while an edit is in
  // flight. outputMode = the output-only view: the window stays put on screen
  // while the video slides under it, and nothing is dimmed (the blurred
  // background around a square window is part of the output).
  import type { CropWindow } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { framing } from "$lib/state/framing.svelte";
  import { project } from "$lib/state/project.svelte";
  import { dragWindowX, offsetForX, offsetHasEffect, type WindowShape } from "$lib/util/crop";

  type Props = {
    rect: { left: number; top: number; width: number; height: number };
    displayWidth: number;
    displayHeight: number;
    win: CropWindow;
    shape: WindowShape;
    outputMode: boolean;
  };
  let { rect, displayWidth, displayHeight, win, shape, outputMode }: Props = $props();

  const clip = $derived(project.primary);
  const committed = $derived(clip ? clip.crop_x_offset : 0);
  const offset = $derived(framing.offsetFor(clip));
  const effective = $derived(offsetHasEffect(displayWidth, displayHeight, shape));
  const scale = $derived(displayWidth > 0 ? rect.width / displayWidth : 1);
  const scaleY = $derived(displayHeight > 0 ? rect.height / displayHeight : 1);

  $effect(() => {
    framing.offsetIgnored = !effective;
    return () => {
      framing.offsetIgnored = false;
    };
  });

  const canEdit = $derived(clip !== null && effective && !project.readOnly);
  const round3 = (v: number) => Math.round(v * 1000) / 1000;

  function commit(id: string, value: number): void {
    const v = round3(Math.min(1, Math.max(-1, value)));
    if (Math.abs(v - committed) < 1e-6) {
      framing.clearPreview();
      return;
    }
    framing.setPreview(id, v);
    void project.editClips([id], { kind: "set_crop", crop_x_offset: v }).finally(() => framing.clearPreview());
  }

  let drag: { id: string; startClientX: number; startX: number; pointer: number; last: number } | null = null;

  function onPointerDown(e: PointerEvent): void {
    if (!canEdit || !clip || e.button !== 0) return;
    drag = { id: clip.id, startClientX: e.clientX, startX: win.x, pointer: e.pointerId, last: offset };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    e.preventDefault();
  }

  function onPointerMove(e: PointerEvent): void {
    if (!drag || e.pointerId !== drag.pointer || scale <= 0) return;
    const x = dragWindowX(drag.startX, e.clientX - drag.startClientX, scale, outputMode);
    drag.last = offsetForX(displayWidth, displayHeight, x, shape);
    framing.setPreview(drag.id, drag.last);
  }

  function onPointerUp(e: PointerEvent): void {
    if (!drag || e.pointerId !== drag.pointer) return;
    const d = drag;
    drag = null;
    commit(d.id, d.last);
  }

  function onPointerCancel(): void {
    drag = null;
    framing.clearPreview();
  }

  function onKeyDown(e: KeyboardEvent): void {
    if (!canEdit || !clip || e.altKey || e.ctrlKey || e.metaKey) return;
    const step = e.shiftKey ? 0.25 : 0.05;
    let next: number | null = null;
    if (e.key === "ArrowLeft") next = offset - step;
    else if (e.key === "ArrowRight") next = offset + step;
    if (next === null) return;
    e.preventDefault();
    e.stopPropagation();
    commit(clip.id, next);
  }

  const style = $derived(
    `left:${win.x * scale}px;top:${win.y * scaleY}px;width:${win.w * scale}px;height:${win.h * scaleY}px;` +
      (outputMode ? "" : "box-shadow:0 0 0 100vmax rgba(0,0,0,0.62);"),
  );
</script>

<div class="pointer-events-none absolute inset-0" data-testid="crop-overlay" data-shape={shape}>
  <div class="absolute overflow-hidden" style="left:{rect.left}px;top:{rect.top}px;width:{rect.width}px;height:{rect.height}px;">
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="absolute box-border border-2 border-accent {canEdit ? 'pointer-events-auto cursor-ew-resize' : 'pointer-events-none'} {clip ? '' : 'border-dashed'}"
      style="{style}touch-action:none;"
      role="slider"
      tabindex={canEdit ? 0 : -1}
      aria-label={t("crop.window.label", { shape })}
      aria-valuemin={-1}
      aria-valuemax={1}
      aria-valuenow={Math.round(offset * 100) / 100}
      aria-valuetext={t("crop.window.valuetext", { offset: offset.toFixed(2) })}
      aria-disabled={!canEdit}
      onpointerdown={onPointerDown}
      onpointermove={onPointerMove}
      onpointerup={onPointerUp}
      onpointercancel={onPointerCancel}
      onkeydown={onKeyDown}
    >
      <span class="absolute top-1 left-1 rounded bg-black/70 px-1 text-[10px] leading-4 font-semibold text-text">{shape}</span>
    </div>
  </div>

  {#if !clip}
    <div
      class="absolute left-1/2 -translate-x-1/2 rounded bg-black/75 px-2 py-0.5 text-xs text-text"
      style="top:{rect.top + rect.height - 28}px"
      role="status"
    >
      {t("crop.hint.noClip", { shape })}
    </div>
  {:else if !effective}
    <div
      class="absolute left-1/2 -translate-x-1/2 rounded bg-black/75 px-2 py-0.5 text-xs text-text"
      style="top:{rect.top + rect.height - 28}px"
      role="status"
    >
      {t("crop.hint.ignored", { shape })}
    </div>
  {/if}
</div>
