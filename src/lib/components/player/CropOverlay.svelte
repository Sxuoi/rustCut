<script lang="ts">
  // FRM-03/FRM-04, PRD 6.3: the 9:16 window over the video with the outside
  // dimmed. Drag horizontally (or use the arrow keys) to change the PRIMARY
  // clip's crop_x_offset; the edit is committed on pointer-up.
  //
  // Props: rect = video content box in CSS px (relative to this overlay's
  // parent), displayWidth/displayHeight = the source's display frame (§7.7).
  //
  // The authoritative window comes from backend.cropWindow (the same Rust
  // function the export uses). util/crop.ts is only used for instant feedback
  // while an edit is in flight.
  import type { CropWindow } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { backend } from "$lib/ipc";
  import { framing } from "$lib/state/framing.svelte";
  import { project } from "$lib/state/project.svelte";
  import { cropWindow, dragWindowX, offsetForX, offsetHasEffect } from "$lib/util/crop";

  type Props = {
    rect: { left: number; top: number; width: number; height: number };
    displayWidth: number;
    displayHeight: number;
  };
  let { rect, displayWidth, displayHeight }: Props = $props();

  const clip = $derived(project.primary);
  const committed = $derived(clip ? clip.crop_x_offset : 0);
  const offset = $derived(framing.offsetFor(clip));
  const effective = $derived(offsetHasEffect(displayWidth, displayHeight));
  const scale = $derived(displayWidth > 0 ? rect.width / displayWidth : 1);
  const live = $derived(framing.preview !== null && clip !== null && framing.preview.id === clip.id);

  // Authoritative window for the committed offset (FRM-04), via IPC.
  let remote = $state<{ key: string; win: CropWindow } | null>(null);
  const key = $derived(`${displayWidth}x${displayHeight}@${committed}`);
  let token = 0;
  $effect(() => {
    const k = key;
    const mine = ++token;
    backend
      .cropWindow(displayWidth, displayHeight, committed)
      .then((win) => {
        if (mine === token) remote = { key: k, win };
      })
      .catch(() => {});
  });

  const win = $derived<CropWindow>(
    live ? cropWindow(displayWidth, displayHeight, offset) : remote && remote.key === key ? remote.win : cropWindow(displayWidth, displayHeight, committed),
  );

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
    const x = dragWindowX(drag.startX, e.clientX - drag.startClientX, scale, framing.nineSixteenOnly);
    drag.last = offsetForX(displayWidth, displayHeight, x);
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
    `left:${win.x * scale}px;top:${win.y * scale}px;width:${win.w * scale}px;height:${win.h * scale}px;`,
  );
</script>

<div class="pointer-events-none absolute inset-0" data-testid="crop-overlay">
  <div class="absolute overflow-hidden" style="left:{rect.left}px;top:{rect.top}px;width:{rect.width}px;height:{rect.height}px;">
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="absolute box-border border-2 border-accent {canEdit ? 'pointer-events-auto cursor-ew-resize' : 'pointer-events-none'} {clip ? '' : 'border-dashed'}"
      style="{style}box-shadow:0 0 0 100vmax rgba(0,0,0,0.62);touch-action:none;"
      role="slider"
      tabindex={canEdit ? 0 : -1}
      aria-label={t("crop.window.label")}
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
      <span class="absolute top-1 left-1 rounded bg-black/70 px-1 text-[10px] leading-4 font-semibold text-text">9:16</span>
    </div>
  </div>

  {#if !clip}
    <div
      class="absolute left-1/2 -translate-x-1/2 rounded bg-black/75 px-2 py-0.5 text-xs text-text"
      style="top:{rect.top + rect.height - 28}px"
      role="status"
    >
      {t("crop.hint.noClip")}
    </div>
  {:else if !effective}
    <div
      class="absolute left-1/2 -translate-x-1/2 rounded bg-black/75 px-2 py-0.5 text-xs text-text"
      style="top:{rect.top + rect.height - 28}px"
      role="status"
    >
      {t("crop.hint.ignored")}
    </div>
  {/if}
</div>
