<script lang="ts">
  // FRM-05 / FRM-09: the blurred background of the output-only view. The
  // layout's background source is drawn from the player's own <video> into a
  // small canvas and blurred with CSS (util/blurPreview.ts), so the preview
  // matches the export without a second video element. Redrawn on every
  // presented frame (requestVideoFrameCallback), on seeks and loads, and when
  // the layout or the box changes (e.g. dragging the crop while paused).
  import { untrack } from "svelte";
  import type { FrameLayout } from "$lib/bindings";
  import { blurPlan, paintBlurSource } from "$lib/util/blurPreview";

  type Props = {
    video: HTMLVideoElement;
    layout: FrameLayout;
    /** The output frame's box in the player (CSS px). */
    box: { left: number; top: number; width: number; height: number };
    displayWidth: number;
    displayHeight: number;
  };
  let { video, layout, box, displayWidth, displayHeight }: Props = $props();

  let canvas: HTMLCanvasElement | undefined = $state();
  const plan = $derived(blurPlan(layout, box.width, box.height));

  function draw(): void {
    const c = canvas;
    const p = plan;
    const source = layout.blur?.source;
    if (!c || !p || !source || video.readyState < 2) return;
    const ctx = c.getContext("2d", { alpha: false });
    if (!ctx) return;
    paintBlurSource(ctx, c, video, source, p, displayWidth, displayHeight);
  }

  // One draw per presented frame while playing; seeks and loads redraw too.
  $effect(() => {
    const el = video;
    const perFrame = typeof el.requestVideoFrameCallback === "function";
    let handle = 0;
    let stopped = false;
    const onFrame = (): void => {
      if (stopped) return;
      untrack(draw);
      handle = el.requestVideoFrameCallback(onFrame);
    };
    if (perFrame) handle = el.requestVideoFrameCallback(onFrame);
    const redraw = (): void => untrack(draw);
    const events = perFrame ? ["seeked", "loadeddata"] : ["seeked", "loadeddata", "timeupdate", "play"];
    for (const name of events) el.addEventListener(name, redraw);
    return () => {
      stopped = true;
      if (perFrame) el.cancelVideoFrameCallback(handle);
      for (const name of events) el.removeEventListener(name, redraw);
    };
  });

  // Layout, size or canvas changed: redraw now (the video may be paused).
  $effect(() => {
    void layout;
    void plan;
    void canvas;
    void displayWidth;
    void displayHeight;
    draw();
  });
</script>

{#if plan}
  <div
    class="pointer-events-none absolute overflow-hidden"
    style="left:{box.left}px;top:{box.top}px;width:{box.width}px;height:{box.height}px;"
    data-testid="blur-backdrop"
    aria-hidden="true"
  >
    <canvas
      bind:this={canvas}
      width={plan.contentW + 2 * plan.margin}
      height={plan.contentH + 2 * plan.margin}
      class="absolute max-w-none"
      style="left:{plan.left}px;top:{plan.top}px;width:{plan.width}px;height:{plan.height}px;filter:blur({plan.blurPx}px);"
    ></canvas>
  </div>
{/if}
