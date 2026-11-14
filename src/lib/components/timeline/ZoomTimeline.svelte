<script lang="ts">
  // PLY-06, MRK-05, MRK-07, PRD 6.1: the zoom timeline. A detail window
  // (2 s - 10 min, mouse wheel) centred on the playhead with frame ticks,
  // keyframe ticks, pending [I / O], clip bars with trim handles and the
  // lossless effective start. Canvas, redrawn via requestAnimationFrame only
  // when an input changes; only clips inside the window are drawn.
  //
  // Interaction: wheel = zoom keeping the time under the cursor fixed; drag
  // empty space = scrub (the timeline follows the pointer); click = seek;
  // drag a clip edge = trim (Shift snaps to playhead/keyframes/clip edges),
  // committed with one retime edit on pointer-up; Escape cancels.
  import { onMount } from "svelte";
  import { presetOf } from "$lib/components/shell/format";
  import { t } from "$lib/i18n";
  import { media } from "$lib/state/media.svelte";
  import { player } from "$lib/state/player.svelte";
  import { project } from "$lib/state/project.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import { FrameScheduler, makeHatch, readTheme, sizeCanvas, type Theme } from "$lib/timeline/canvas";
  import { NEUTRAL_BAR, contrastText, tagLetter } from "$lib/timeline/colors";
  import { hitTest, laneY, type LaneGeom } from "$lib/timeline/hit";
  import { MAX_VISIBLE_LANES, assignLanes } from "$lib/timeline/lanes";
  import { clampWindow, pxPerSec, timeToX, xToTime, zoomAround, zoomViewport, type Viewport } from "$lib/timeline/scale";
  import { clipEdges, type SnapKind } from "$lib/timeline/snap";
  import { forEachTick, formatTick, frameStride, rulerStep, sortedRange } from "$lib/timeline/ticks";
  import { trimAtPointer, type TrimEdge } from "$lib/timeline/trim";
  import { WaveColumns, computeColumns } from "$lib/timeline/waveform";
  import { formatDuration, formatIndex, formatTimecode } from "$lib/util/time";

  const RULER_H = 16;

  let host: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let width = $state(0);
  let height = $state(0);
  let theme: Theme = readTheme();
  let hatchKey = "";
  let hatch: CanvasPattern | null = null;
  const wave = new WaveColumns();

  /** View centre = playhead + offset (offset is 0 unless the user zoomed/clicked off-centre). */
  let offset = $state(0);

  interface Trim {
    index: number;
    id: string;
    edge: TrimEdge;
    original: number;
    time: number;
    snap: SnapKind | null;
    pointer: number;
    frozen: Viewport;
    committing: boolean;
  }
  let trim = $state<Trim | null>(null);

  const clips = $derived(project.clips);
  const spans = $derived(clips.map((c) => ({ start: c.start_sec, end: c.end_sec })));
  const layout = $derived(assignLanes(spans));
  const tagById = $derived(new Map(project.tags.map((tg) => [tg.id, tg] as const)));
  const selKey = $derived([...project.selection].join(","));
  const windowSec = $derived(clampWindow(ui.zoomWindowSec));
  const lossless = $derived(project.settings ? presetOf(project.settings) === "lossless" : false);

  function viewport(): Viewport {
    return trim ? trim.frozen : zoomViewport(player.currentTime + offset, windowSec, width);
  }

  function geom(): LaneGeom {
    const areaH = Math.round((height - RULER_H - 2) * 0.5);
    const lanes = Math.max(1, layout.laneCount);
    return { top: RULER_H + 2, laneHeight: Math.min(18, areaH / lanes), gap: 2 };
  }

  function waveBounds(): { top: number; bottom: number } {
    const top = RULER_H + 2 + Math.round((height - RULER_H - 2) * 0.5) + 2;
    return { top, bottom: height - 1 };
  }

  /** Clip [start, end] with the in-flight trim applied. */
  function range(i: number): { start: number; end: number } {
    const c = clips[i];
    if (trim && trim.index === i) {
      return trim.edge === "start" ? { start: trim.time, end: c.end_sec } : { start: c.start_sec, end: trim.time };
    }
    return { start: c.start_sec, end: c.end_sec };
  }

  function draw(): void {
    if (!canvas || width <= 0 || height <= 0) return;
    const ctx = sizeCanvas(canvas, width, height);
    if (!ctx) return;
    const W = width;
    const H = height;
    const vp = viewport();
    const pps = pxPerSec(vp);
    ctx.clearRect(0, 0, W, H);
    ctx.fillStyle = theme.panel;
    ctx.fillRect(0, 0, W, H);

    drawWaveform(ctx, vp);
    drawRuler(ctx, vp, pps);
    drawKeyframes(ctx, vp, pps);
    drawPending(ctx, vp);
    drawClips(ctx, vp);
    drawLossless(ctx, vp);

    // Snap guide while trimming.
    if (trim && trim.snap) {
      const x = Math.round(timeToX(trim.time, vp)) + 0.5;
      ctx.strokeStyle = theme.accent;
      ctx.lineWidth = 1;
      ctx.setLineDash([2, 2]);
      ctx.beginPath();
      ctx.moveTo(x, RULER_H);
      ctx.lineTo(x, H);
      ctx.stroke();
      ctx.setLineDash([]);
    }

    // Playhead.
    const px = Math.round(timeToX(player.currentTime, vp)) + 0.5;
    if (px >= -5 && px <= W + 5) {
      ctx.strokeStyle = theme.playhead;
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(px, 0);
      ctx.lineTo(px, H);
      ctx.stroke();
      ctx.fillStyle = theme.playhead;
      ctx.beginPath();
      ctx.moveTo(px - 4, 0);
      ctx.lineTo(px + 4, 0);
      ctx.lineTo(px, 5);
      ctx.closePath();
      ctx.fill();
    }
  }

  function drawWaveform(ctx: CanvasRenderingContext2D, vp: Viewport): void {
    const w = media.waveform;
    const { top, bottom } = waveBounds();
    const mid = (top + bottom) / 2;
    ctx.strokeStyle = theme.line;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, Math.round(mid) + 0.5);
    ctx.lineTo(width, Math.round(mid) + 0.5);
    ctx.stroke();
    if (!w) return;
    computeColumns(w, vp.start, vp.end, width, wave);
    const sc = (bottom - top) / 2 / 127;
    ctx.strokeStyle = theme.accent;
    ctx.globalAlpha = 0.7;
    ctx.beginPath();
    for (let x = 0; x < wave.width; x++) {
      if (!wave.has[x]) continue;
      ctx.moveTo(x + 0.5, mid - wave.max[x] * sc);
      ctx.lineTo(x + 0.5, mid - wave.min[x] * sc + 1);
    }
    ctx.stroke();
    ctx.globalAlpha = 1;
  }

  function drawRuler(ctx: CanvasRenderingContext2D, vp: Viewport, pps: number): void {
    const step = rulerStep(vp.end - vp.start, width, 84);
    ctx.font = "10px system-ui, sans-serif";
    ctx.textBaseline = "top";
    ctx.textAlign = "left";
    ctx.strokeStyle = theme.line;
    ctx.fillStyle = theme.muted;
    ctx.lineWidth = 1;
    ctx.beginPath();
    forEachTick(vp.start, vp.end, step, (tk) => {
      const x = Math.round(timeToX(tk, vp)) + 0.5;
      ctx.moveTo(x, RULER_H - 6);
      ctx.lineTo(x, RULER_H);
    });
    ctx.stroke();
    forEachTick(vp.start, vp.end, step, (tk) => {
      if (tk >= 0 && (player.duration <= 0 || tk <= player.duration + 1e-6)) ctx.fillText(formatTick(tk, step), timeToX(tk, vp) + 3, 1);
    });

    // Frame ticks (only when a frame is wide enough to tell apart).
    const times = media.index?.frame_times ?? null;
    const frameDur = media.index?.frame_duration_sec ?? player.frameDuration;
    if (frameStride(frameDur, pps) === 0) return;
    ctx.strokeStyle = theme.muted;
    ctx.globalAlpha = 0.6;
    ctx.beginPath();
    if (times && times.length > 0) {
      const [lo, hi] = sortedRange(times, vp.start, vp.end);
      for (let i = lo; i < hi; i++) {
        const x = Math.round(timeToX(times[i], vp)) + 0.5;
        ctx.moveTo(x, RULER_H - 3);
        ctx.lineTo(x, RULER_H);
      }
    } else {
      forEachTick(Math.max(0, vp.start), Math.min(vp.end, player.duration > 0 ? player.duration : vp.end), frameDur, (tk) => {
        const x = Math.round(timeToX(tk, vp)) + 0.5;
        ctx.moveTo(x, RULER_H - 3);
        ctx.lineTo(x, RULER_H);
      });
    }
    ctx.stroke();
    ctx.globalAlpha = 1;
  }

  function drawKeyframes(ctx: CanvasRenderingContext2D, vp: Viewport, pps: number): void {
    const kf = media.keyframes;
    if (kf.length === 0) return;
    const [lo, hi] = sortedRange(kf, vp.start, vp.end);
    if (hi <= lo) return;
    const bottom = waveBounds().top;
    ctx.strokeStyle = theme.accent;
    ctx.globalAlpha = 0.45;
    ctx.lineWidth = 1;
    ctx.beginPath();
    for (let i = lo; i < hi; i++) {
      const x = Math.round(timeToX(kf[i], vp)) + 0.5;
      ctx.moveTo(x, RULER_H);
      ctx.lineTo(x, bottom);
    }
    ctx.stroke();
    ctx.globalAlpha = 1;
    // "K" labels when there is room.
    const spacing = hi - lo > 1 ? ((kf[hi - 1] - kf[lo]) / (hi - lo - 1)) * pps : Infinity;
    if (spacing >= 24) {
      ctx.fillStyle = theme.accent;
      ctx.font = "9px system-ui, sans-serif";
      ctx.textBaseline = "alphabetic";
      for (let i = lo; i < hi; i++) ctx.fillText("K", timeToX(kf[i], vp) + 2, bottom - 1);
    }
  }

  function drawPending(ctx: CanvasRenderingContext2D, vp: Viewport): void {
    const pin = player.pendingIn;
    const pout = player.pendingOut;
    if (pin === null && pout === null) return;
    const top = RULER_H;
    const bottom = waveBounds().top;
    const end = pout ?? (pin !== null && player.currentTime > pin ? player.currentTime : null);
    if (pin !== null && end !== null) {
      if (hatchKey !== theme.pendingIn || !hatch) {
        hatch = makeHatch(ctx, theme.pendingIn);
        hatchKey = theme.pendingIn;
      }
      const x0 = timeToX(pin, vp);
      const x1 = Math.max(x0 + 1, timeToX(end, vp));
      if (hatch) {
        ctx.fillStyle = hatch;
        ctx.fillRect(x0, top, x1 - x0, bottom - top);
      }
    }
    ctx.lineWidth = 2;
    ctx.font = "bold 10px system-ui, sans-serif";
    ctx.textBaseline = "top";
    if (pin !== null) {
      const x = Math.round(timeToX(pin, vp)) + 1;
      ctx.strokeStyle = theme.pendingIn;
      ctx.fillStyle = theme.pendingIn;
      ctx.beginPath();
      ctx.moveTo(x, top);
      ctx.lineTo(x, bottom);
      ctx.stroke();
      ctx.textAlign = "left";
      ctx.fillText("[I", x + 3, top + 1);
    }
    if (pout !== null) {
      const x = Math.round(timeToX(pout, vp)) - 1;
      ctx.strokeStyle = theme.pendingOut;
      ctx.fillStyle = theme.pendingOut;
      ctx.beginPath();
      ctx.moveTo(x, top);
      ctx.lineTo(x, bottom);
      ctx.stroke();
      ctx.textAlign = "right";
      ctx.fillText("O]", x - 3, top + 1);
      ctx.textAlign = "left";
    }
  }

  function drawClips(ctx: CanvasRenderingContext2D, vp: Viewport): void {
    const g = geom();
    const barH = g.laneHeight - g.gap;
    const lanes = layout.lane;
    ctx.textBaseline = "middle";
    ctx.textAlign = "left";
    ctx.font = "9px system-ui, sans-serif";
    for (let i = 0; i < clips.length; i++) {
      if (lanes[i] >= MAX_VISIBLE_LANES) continue;
      const r = range(i);
      if (r.end < vp.start || r.start > vp.end) continue;
      const c = clips[i];
      const tag = tagById.get(c.tag_id ?? "");
      const base = tag?.color ?? NEUTRAL_BAR;
      const x0 = timeToX(r.start, vp);
      const w = Math.max(2, timeToX(r.end, vp) - x0);
      const y = laneY(lanes[i], g);
      ctx.fillStyle = base;
      ctx.fillRect(x0, y, w, barH);
      const selected = project.selection.has(c.id);
      if (selected) {
        ctx.strokeStyle = theme.playhead;
        ctx.lineWidth = 1.5;
        ctx.strokeRect(x0 + 0.75, y + 0.75, Math.max(1, w - 1.5), Math.max(1, barH - 1.5));
        // Trim handles (MRK-05).
        ctx.fillStyle = theme.playhead;
        ctx.fillRect(x0, y, 3, barH);
        ctx.fillRect(x0 + w - 3, y, 3, barH);
      }
      if (w >= 16 && barH >= 8) {
        ctx.fillStyle = contrastText(base);
        const letter = tagLetter(tag?.label);
        let x = Math.max(x0, 0) + 5;
        if (letter) {
          ctx.fillText(letter, x, y + barH / 2 + 0.5);
          x += 8;
        }
        ctx.fillText(formatIndex(c.index), x, y + barH / 2 + 0.5);
      }
    }
  }

  /** MRK-07: effective start (keyframe at or before In) and lead-in of the selected clip. */
  function drawLossless(ctx: CanvasRenderingContext2D, vp: Viewport): void {
    if (!lossless || media.keyframes.length === 0) return;
    const primary = project.primary;
    if (!primary) return;
    const i = clips.findIndex((c) => c.id === primary.id);
    if (i < 0 || layout.lane[i] >= MAX_VISIBLE_LANES) return;
    const start = range(i).start;
    const eff = media.keyframeAtOrBefore(start);
    if (eff === null) return;
    const g = geom();
    const y = laneY(layout.lane[i], g) + g.laneHeight - g.gap;
    const xe = timeToX(eff, vp);
    const xs = timeToX(start, vp);
    ctx.strokeStyle = theme.success;
    ctx.fillStyle = theme.success;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(xe, y + 1);
    ctx.lineTo(xs, y + 1);
    ctx.moveTo(xe + 0.5, y - 3);
    ctx.lineTo(xe + 0.5, y + 4);
    ctx.stroke();
    ctx.beginPath();
    ctx.moveTo(xe - 3, y + 6);
    ctx.lineTo(xe + 3, y + 6);
    ctx.lineTo(xe, y + 2);
    ctx.closePath();
    ctx.fill();
    ctx.font = "bold 9px system-ui, sans-serif";
    ctx.textBaseline = "top";
    ctx.textAlign = "left";
    ctx.fillText(t("timeline.leadIn", { lead: Math.max(0, start - eff).toFixed(1) }), Math.max(xe, 0) + 5, y + 3);
  }

  const scheduler = new FrameScheduler(draw);

  $effect(() => {
    void [clips, layout, tagById, selKey, windowSec, offset, width, height, lossless, trim, trim?.time, trim?.snap, player.currentTime, player.duration, player.pendingIn, player.pendingOut, player.frameDuration, media.waveformVersion, media.waveform, media.keyframes, media.index, project.primaryId];
    scheduler.request();
  });

  // The playhead left the window: re-centre.
  $effect(() => {
    if (Math.abs(offset) > windowSec / 2) offset = 0;
  });

  onMount(() => {
    theme = readTheme();
    const ro = new ResizeObserver((entries) => {
      const r = entries[0]?.contentRect;
      if (!r) return;
      width = Math.floor(r.width);
      height = Math.floor(r.height);
    });
    ro.observe(host);
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = canvas.getBoundingClientRect();
      if (rect.width <= 0) return;
      const dy = e.deltaY * (e.deltaMode === 1 ? 16 : 1);
      zoomBy(Math.exp(dy * 0.0015), (e.clientX - rect.left) / rect.width);
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      canvas.removeEventListener("wheel", onWheel);
      ro.disconnect();
      scheduler.cancel();
    };
  });

  function zoomBy(factor: number, frac: number): void {
    const z = zoomAround({ center: player.currentTime + offset, window: windowSec }, frac, factor);
    ui.setZoomWindow(z.window);
    offset = z.center - player.currentTime;
  }

  // --- interaction ---------------------------------------------------------
  type Pan = { startX: number; startPlayhead: number; startCenter: number; vp: Viewport; pointer: number; moved: boolean };
  let pan: Pan | null = null;

  function pos(e: PointerEvent): { x: number; y: number } {
    const r = canvas.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  function hitAt(x: number, y: number) {
    const vp = viewport();
    return hitTest(x, y, {
      spans,
      lanes: layout.lane,
      maxLanes: MAX_VISIBLE_LANES,
      geom: geom(),
      vp,
      playheadX: timeToX(player.currentTime, vp),
      handles: true,
    });
  }

  function onPointerDown(e: PointerEvent): void {
    if (e.button !== 0 || player.duration <= 0) return;
    canvas.focus();
    const { x, y } = pos(e);
    const h = hitAt(x, y);
    const vp = viewport();
    if ((h.kind === "handle-start" || h.kind === "handle-end") && !project.readOnly) {
      const c = clips[h.index];
      if (!project.selection.has(c.id) || project.primaryId !== c.id) project.select(c.id, "replace");
      const edge: TrimEdge = h.kind === "handle-start" ? "start" : "end";
      const original = edge === "start" ? c.start_sec : c.end_sec;
      trim = { index: h.index, id: c.id, edge, original, time: original, snap: null, pointer: e.pointerId, frozen: vp, committing: false };
      canvas.setPointerCapture(e.pointerId);
      return;
    }
    if (h.kind === "clip") project.select(clips[h.index].id, e.ctrlKey || e.metaKey ? "toggle" : e.shiftKey ? "range" : "replace");
    pan = { startX: x, startPlayhead: player.currentTime, startCenter: player.currentTime + offset, vp, pointer: e.pointerId, moved: false };
    canvas.setPointerCapture(e.pointerId);
  }

  function updateTrim(x: number, shift: boolean): void {
    if (!trim || trim.committing) return;
    const c = clips[trim.index];
    if (!c) return;
    const result = trimAtPointer(x, trim.frozen, trim.edge, { start: c.start_sec, end: c.end_sec }, player.duration, ui.minClipDuration,
      shift ? { playhead: player.currentTime, keyframes: media.keyframes, edges: clipEdges(spans, trim.index) } : null);
    trim.time = result.time;
    trim.snap = result.kind;
  }

  function onPointerMove(e: PointerEvent): void {
    const { x, y } = pos(e);
    if (trim) {
      if (e.pointerId === trim.pointer) updateTrim(x, e.shiftKey);
      return;
    }
    if (pan && e.pointerId === pan.pointer) {
      const dx = x - pan.startX;
      if (!pan.moved && Math.abs(dx) > 3) pan.moved = true;
      if (pan.moved) player.seek(pan.startPlayhead - dx / pxPerSec(pan.vp));
      return;
    }
    const h = player.duration > 0 ? hitAt(x, y) : { kind: "none" as const };
    canvas.style.cursor = h.kind === "handle-start" || h.kind === "handle-end" ? "ew-resize" : h.kind === "clip" ? "pointer" : "default";
  }

  function onPointerUp(e: PointerEvent): void {
    if (canvas.hasPointerCapture(e.pointerId)) canvas.releasePointerCapture(e.pointerId);
    if (trim && e.pointerId === trim.pointer) {
      updateTrim(pos(e).x, e.shiftKey);
      commitTrim();
      return;
    }
    if (pan && e.pointerId === pan.pointer) {
      const p = pan;
      pan = null;
      if (!p.moved) {
        // Click: seek there, keep the view still.
        player.seek(xToTime(p.startX, p.vp));
        offset = p.startCenter - player.currentTime;
      }
    }
  }

  function commitTrim(): void {
    const tr = trim;
    if (!tr || tr.committing) return;
    if (Math.abs(tr.time - tr.original) < 0.0005) {
      trim = null;
      return;
    }
    tr.committing = true;
    const edit =
      tr.edge === "start"
        ? ({ kind: "retime", start_sec: tr.time, end_sec: null } as const)
        : ({ kind: "retime", start_sec: null, end_sec: tr.time } as const);
    void project.editClips([tr.id], edit).finally(() => {
      trim = null;
    });
  }

  function onPointerCancel(): void {
    pan = null;
    if (trim && !trim.committing) trim = null;
  }

  function onKeyDown(e: KeyboardEvent): void {
    if (e.key === "Escape" && trim && !trim.committing) {
      trim = null;
      e.stopPropagation();
      return;
    }
    if (e.ctrlKey || e.altKey || e.metaKey) return;
    if (e.key === "+" || e.key === "=") zoomBy(1 / 1.5, 0.5);
    else if (e.key === "-" || e.key === "_") zoomBy(1.5, 0.5);
    else return;
    e.preventDefault();
    e.stopPropagation();
  }

  const tipX = $derived(trim ? Math.min(Math.max(timeToX(trim.time, trim.frozen), 90), Math.max(90, width - 90)) : 0);
  const tipDuration = $derived.by(() => {
    if (!trim) return 0;
    const r = range(trim.index);
    return r.end - r.start;
  });
</script>

<div bind:this={host} class="relative h-full w-full overflow-hidden bg-panel">
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_interactive_element_to_noninteractive_role -->
  <canvas
    bind:this={canvas}
    class="absolute inset-0 block h-full w-full touch-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
    role="application"
    tabindex="0"
    aria-label={t("timeline.zoom.label")}
    aria-describedby="zoom-summary"
    style="width:{width}px;height:{height}px"
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointercancel={onPointerCancel}
    onkeydown={onKeyDown}
  ></canvas>
  <p id="zoom-summary" class="sr-only">
    {t("timeline.zoom.summary", { window: formatDuration(windowSec) })}
  </p>

  {#if trim}
    <div
      class="pointer-events-none absolute z-10 w-max -translate-x-1/2 rounded border border-line bg-panel-3 px-2 py-0.5 font-mono text-xs text-text shadow-lg"
      style="left:{tipX}px;top:{RULER_H + 2}px"
      role="tooltip"
    >
      {trim.edge === "start" ? t("timeline.trim.in") : t("timeline.trim.out")}
      {formatTimecode(trim.time)} ({formatDuration(tipDuration)}){trim.snap ? ` · ${t(`timeline.snap.${trim.snap}`)}` : ""}
    </div>
  {/if}
</div>
