<script lang="ts">
  // MRK-02, MRK-03, MRK-06, PLY-05, PRD 6.1: the overview timeline. Canvas,
  // whole source duration, every clip as a coloured bar in lanes (max 4 + "+N"),
  // hatched pending range, waveform strip, playhead. Redrawn with
  // requestAnimationFrame only when an input changes (§8.1: 60 fps with 500
  // clips and a waveform): static layers are cheap batched rect fills and the
  // waveform columns are cached per (version, width, duration).
  import { onMount } from "svelte";
  import { formatDuration, formatIndex, formatTimecode } from "$lib/util/time";
  import { t } from "$lib/i18n";
  import { media } from "$lib/state/media.svelte";
  import { player } from "$lib/state/player.svelte";
  import { project } from "$lib/state/project.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import { FrameScheduler, makeHatch, readTheme, sizeCanvas, type Theme } from "$lib/timeline/canvas";
  import { NEUTRAL_BAR, contrastText, tagLetter } from "$lib/timeline/colors";
  import { hitTest, laneY, type LaneGeom } from "$lib/timeline/hit";
  import { MAX_VISIBLE_LANES, assignLanes, duplicateFlags } from "$lib/timeline/lanes";
  import { overviewViewport, timeToX, xToTime, type Viewport } from "$lib/timeline/scale";
  import { clipEdges, type SnapKind } from "$lib/timeline/snap";
  import { trimAtPointer, type TrimEdge } from "$lib/timeline/trim";
  import { forEachTick, formatTick, rulerStep } from "$lib/timeline/ticks";
  import { WaveColumns, computeColumns } from "$lib/timeline/waveform";

  const RULER_H = 14;

  let host: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let width = $state(0);
  let height = $state(0);
  let theme: Theme = readTheme();
  let hatchKey = "";
  let hatch: CanvasPattern | null = null;
  const wave = new WaveColumns();
  let waveKey = "";

  const clips = $derived(project.clips);
  const spans = $derived(clips.map((c) => ({ start: c.start_sec, end: c.end_sec })));
  const layout = $derived(assignLanes(spans));
  const dups = $derived(duplicateFlags(spans));
  const tagById = $derived(new Map(project.tags.map((tg) => [tg.id, tg] as const)));
  const selKey = $derived([...project.selection].join(","));
  const duration = $derived(player.duration);
  let trim = $state<{
    id: string; index: number; edge: TrimEdge; original: number; time: number;
    span: { start: number; end: number }; vp: Viewport; pointer: number; snap: SnapKind | null; committing: boolean;
  } | null>(null);

  function range(index: number): { start: number; end: number } {
    const c = clips[index];
    return {
      start: trim?.index === index && trim.edge === "start" ? trim.time : c.start_sec,
      end: trim?.index === index && trim.edge === "end" ? trim.time : c.end_sec,
    };
  }

  function geom(h: number): LaneGeom {
    const waveH = Math.max(14, Math.round((h - RULER_H) * 0.38));
    const top = RULER_H + 2;
    const avail = Math.max(8, h - waveH - 2 - top);
    return { top, laneHeight: Math.min(18, avail / MAX_VISIBLE_LANES), gap: 2 };
  }

  function waveTop(h: number): number {
    return h - Math.max(14, Math.round((h - RULER_H) * 0.38));
  }

  function draw(): void {
    if (!canvas || width <= 0 || height <= 0) return;
    const ctx = sizeCanvas(canvas, width, height);
    if (!ctx) return;
    const W = width;
    const H = height;
    const vp = overviewViewport(duration, W);
    const g = geom(H);
    ctx.clearRect(0, 0, W, H);
    ctx.fillStyle = theme.panel;
    ctx.fillRect(0, 0, W, H);
    if (duration <= 0) return;

    // Ruler.
    ctx.font = "10px system-ui, sans-serif";
    ctx.textBaseline = "top";
    ctx.textAlign = "left";
    ctx.fillStyle = theme.muted;
    ctx.strokeStyle = theme.line;
    ctx.lineWidth = 1;
    const step = rulerStep(duration, W, 76);
    ctx.beginPath();
    forEachTick(0, duration, step, (tk) => {
      const x = Math.round(timeToX(tk, vp)) + 0.5;
      ctx.moveTo(x, RULER_H - 4);
      ctx.lineTo(x, RULER_H);
    });
    ctx.stroke();
    forEachTick(0, duration, step, (tk) => {
      const x = timeToX(tk, vp);
      if (x < W - 30) ctx.fillText(formatTick(tk, step), x + 3, 1);
    });

    drawWaveform(ctx, vp.width, H);
    drawPending(ctx, vp.width, g, H);
    drawClips(ctx, g, vp);

    // Playhead.
    const px = Math.round(timeToX(player.currentTime, vp)) + 0.5;
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

  function drawWaveform(ctx: CanvasRenderingContext2D, W: number, H: number): void {
    const w = media.waveform;
    const top = waveTop(H);
    const mid = (top + H) / 2;
    ctx.strokeStyle = theme.line;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, Math.round(mid) + 0.5);
    ctx.lineTo(W, Math.round(mid) + 0.5);
    ctx.stroke();
    if (!w) return;
    const key = `${media.waveformVersion}|${W}|${duration}`;
    if (key !== waveKey) {
      computeColumns(w, 0, duration, W, wave);
      waveKey = key;
    }
    const sc = (H - top - 2) / 2 / 127;
    ctx.strokeStyle = theme.accent;
    ctx.globalAlpha = 0.7;
    ctx.lineWidth = 1;
    ctx.beginPath();
    const cols = wave.width;
    for (let x = 0; x < cols; x++) {
      if (!wave.has[x]) continue;
      ctx.moveTo(x + 0.5, mid - wave.max[x] * sc);
      ctx.lineTo(x + 0.5, mid - wave.min[x] * sc + 1);
    }
    ctx.stroke();
    ctx.globalAlpha = 1;
  }

  function drawPending(ctx: CanvasRenderingContext2D, W: number, g: LaneGeom, H: number): void {
    const pin = player.pendingIn;
    const pout = player.pendingOut;
    if (pin === null && pout === null) return;
    const vp = overviewViewport(duration, W);
    const top = RULER_H;
    const bottom = waveTop(H);
    if (pin !== null) {
      // An open-ended In is drawn up to the playhead.
      const end = pout ?? (player.currentTime > pin ? player.currentTime : null);
      if (end !== null) {
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
    }
    ctx.lineWidth = 2;
    if (pin !== null) {
      const x = Math.round(timeToX(pin, vp)) + 1;
      ctx.strokeStyle = theme.pendingIn;
      ctx.beginPath();
      ctx.moveTo(x, top);
      ctx.lineTo(x, bottom);
      ctx.moveTo(x, top + 1);
      ctx.lineTo(x + 4, top + 1);
      ctx.stroke();
    }
    if (pout !== null) {
      const x = Math.round(timeToX(pout, vp)) - 1;
      ctx.strokeStyle = theme.pendingOut;
      ctx.beginPath();
      ctx.moveTo(x, top);
      ctx.lineTo(x, bottom);
      ctx.moveTo(x, top + 1);
      ctx.lineTo(x - 4, top + 1);
      ctx.stroke();
    }
    void g;
  }

  function drawClips(ctx: CanvasRenderingContext2D, g: LaneGeom, vp: ReturnType<typeof overviewViewport>): void {
    const barH = g.laneHeight - g.gap;
    const W = vp.width;
    const lanes = layout.lane;
    ctx.textBaseline = "middle";
    ctx.textAlign = "left";
    ctx.font = "9px system-ui, sans-serif";
    // Pass 1: bodies, colour changes only when needed.
    let lastColor = "";
    for (let i = 0; i < clips.length; i++) {
      if (lanes[i] >= MAX_VISIBLE_LANES) continue;
      const c = clips[i];
      const color = tagById.get(c.tag_id ?? "")?.color ?? NEUTRAL_BAR;
      if (color !== lastColor) {
        ctx.fillStyle = color;
        lastColor = color;
      }
      const r = range(i);
      const x0 = timeToX(r.start, vp);
      const w = Math.max(2, timeToX(r.end, vp) - x0);
      if (x0 + w < 0 || x0 > W) continue;
      ctx.fillRect(x0, laneY(lanes[i], g), w, barH);
    }
    // Pass 2: marks (duplicate outline, letter, index, check, selection).
    ctx.lineWidth = 1;
    for (let i = 0; i < clips.length; i++) {
      if (lanes[i] >= MAX_VISIBLE_LANES) continue;
      const c = clips[i];
      const selected = project.selection.has(c.id);
      const done = c.export_status === "completed";
      const tag = tagById.get(c.tag_id ?? "");
      const r = range(i);
      const x0 = timeToX(r.start, vp);
      const w = Math.max(2, timeToX(r.end, vp) - x0);
      const y = laneY(lanes[i], g);
      if (dups[i]) {
        ctx.strokeStyle = theme.text;
        ctx.setLineDash([3, 2]);
        ctx.strokeRect(x0 + 0.5, y + 0.5, Math.max(1, w - 1), barH - 1);
        ctx.setLineDash([]);
      }
      if (selected) {
        ctx.strokeStyle = theme.playhead;
        ctx.lineWidth = 2;
        ctx.strokeRect(x0 + 1, y + 1, Math.max(1, w - 2), Math.max(1, barH - 2));
        ctx.lineWidth = 1;
        if (project.primaryId === c.id && !project.readOnly) {
          ctx.fillStyle = theme.playhead;
          ctx.fillRect(x0, y, 3, barH);
          ctx.fillRect(x0 + w - 3, y, 3, barH);
        }
      }
      if (w >= 12 && barH >= 8) {
        let x = x0 + 2;
        const base = tag?.color ?? NEUTRAL_BAR;
        ctx.fillStyle = contrastText(base);
        const letter = tagLetter(tag?.label);
        if (letter) {
          ctx.fillText(letter, x, y + barH / 2 + 0.5);
          x += 8;
        }
        if (w - (x - x0) >= 22) ctx.fillText(formatIndex(c.index), x, y + barH / 2 + 0.5);
        if (done && w >= 24) {
          ctx.textAlign = "right";
          ctx.fillText("✓", x0 + w - 2, y + barH / 2 + 0.5);
          ctx.textAlign = "left";
        }
      } else if (done && w >= 6) {
        ctx.fillStyle = contrastText(tag?.color ?? NEUTRAL_BAR);
        ctx.fillText("✓", x0 + 1, y + barH / 2 + 0.5);
      }
    }
    // Overflow badges.
    ctx.textBaseline = "middle";
    for (const o of layout.overflow) {
      const label = `+${o.count}`;
      const tw = ctx.measureText(label).width + 6;
      const x = Math.min(W - tw, Math.max(0, timeToX(o.end, vp) - tw));
      const y = laneY(MAX_VISIBLE_LANES - 1, g);
      ctx.fillStyle = theme.bg;
      ctx.fillRect(x, y, tw, barH);
      ctx.strokeStyle = theme.text;
      ctx.strokeRect(x + 0.5, y + 0.5, tw - 1, barH - 1);
      ctx.fillStyle = theme.text;
      ctx.fillText(label, x + 3, y + barH / 2 + 0.5);
    }
  }

  const scheduler = new FrameScheduler(draw);

  // Redraw only when an input changes.
  $effect(() => {
    void [clips, layout, dups, tagById, selKey, duration, width, height, player.currentTime, player.pendingIn, player.pendingOut, media.waveformVersion, media.waveform, trim?.time, project.primaryId, project.readOnly];
    scheduler.request();
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
    return () => {
      ro.disconnect();
      scheduler.cancel();
    };
  });

  // --- interaction ---------------------------------------------------------
  let hover = $state<{ index: number; x: number } | null>(null);
  let scrubbing = false;

  function pos(e: PointerEvent): { x: number; y: number } {
    const r = canvas.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  function hitAt(x: number, y: number) {
    return hitTest(x, y, {
      spans,
      lanes: layout.lane,
      maxLanes: MAX_VISIBLE_LANES,
      geom: geom(height),
      vp: overviewViewport(duration, width),
      playheadX: timeToX(player.currentTime, overviewViewport(duration, width)),
      handles: !project.readOnly,
      handleIndex: clips.findIndex((c) => c.id === project.primaryId),
    });
  }

  function onPointerDown(e: PointerEvent): void {
    if (e.button !== 0 || duration <= 0) return;
    canvas.focus();
    if (trim) return;
    const { x, y } = pos(e);
    const h = hitAt(x, y);
    if (h.kind === "handle-start" || h.kind === "handle-end") {
      const c = clips[h.index];
      const edge = h.kind === "handle-start" ? "start" : "end";
      const original = edge === "start" ? c.start_sec : c.end_sec;
      trim = { id: c.id, index: h.index, edge, original, time: original, span: spans[h.index],
        vp: overviewViewport(duration, width), pointer: e.pointerId, snap: null, committing: false };
      hover = null;
      canvas.setPointerCapture(e.pointerId);
      return;
    }
    if (h.kind === "clip") {
      const c = clips[h.index];
      project.select(c.id, e.ctrlKey || e.metaKey ? "toggle" : e.shiftKey ? "range" : "replace");
      player.seek(c.start_sec);
      return;
    }
    scrubbing = true;
    canvas.setPointerCapture(e.pointerId);
    player.seek(xToTime(Math.min(width, Math.max(0, x)), overviewViewport(duration, width)));
  }

  function onPointerMove(e: PointerEvent): void {
    const { x, y } = pos(e);
    if (trim) {
      if (e.pointerId === trim.pointer) updateTrim(x, e.shiftKey);
      return;
    }
    if (scrubbing) {
      player.seek(xToTime(Math.min(width, Math.max(0, x)), overviewViewport(duration, width)));
      return;
    }
    const h = duration > 0 ? hitAt(x, y) : { kind: "none" as const };
    const idx = h.kind === "clip" ? h.index : -1;
    if (idx < 0) hover = null;
    else if (!hover || hover.index !== idx || Math.abs(hover.x - x) > 1) hover = { index: idx, x };
    canvas.style.cursor = h.kind === "handle-start" || h.kind === "handle-end" ? "ew-resize" : idx >= 0 ? "pointer" : "default";
  }

  function updateTrim(x: number, shift: boolean): void {
    if (!trim || trim.committing) return;
    const result = trimAtPointer(x, trim.vp, trim.edge, trim.span, duration, ui.minClipDuration,
      shift ? { playhead: player.currentTime, keyframes: media.keyframes, edges: clipEdges(spans, trim.index) } : null);
    trim.time = result.time;
    trim.snap = result.kind;
  }

  function onPointerUp(e: PointerEvent): void {
    if (trim && e.pointerId === trim.pointer && !trim.committing) {
      updateTrim(pos(e).x, e.shiftKey);
      const tr = trim;
      if (Math.abs(tr.time - tr.original) >= 0.0005 && !project.readOnly && project.clip(tr.id)) {
        tr.committing = true;
        const edit = tr.edge === "start"
          ? { kind: "retime" as const, start_sec: tr.time, end_sec: null }
          : { kind: "retime" as const, start_sec: null, end_sec: tr.time };
        void project.editClips([tr.id], edit).finally(() => { trim = null; });
      } else trim = null;
    }
    scrubbing = false;
    if (canvas.hasPointerCapture(e.pointerId)) canvas.releasePointerCapture(e.pointerId);
  }

  function cancelTrim(): void {
    scrubbing = false;
    if (!trim || trim.committing) return;
    const pointer = trim.pointer;
    trim = null;
    if (canvas.hasPointerCapture(pointer)) canvas.releasePointerCapture(pointer);
  }

  function onKeyDown(e: KeyboardEvent): void {
    if (e.key !== "Escape" || !trim || trim.committing) return;
    cancelTrim();
    e.preventDefault();
    e.stopPropagation();
  }

  const trimTipLeft = $derived(trim ? Math.min(Math.max(timeToX(trim.time, trim.vp), 110), Math.max(110, width - 110)) : 0);

  const hovered = $derived(hover ? (clips[hover.index] ?? null) : null);
  const tipLeft = $derived(hover ? Math.min(Math.max(hover.x, 110), Math.max(110, width - 110)) : 0);
</script>

<div bind:this={host} class="relative h-full w-full overflow-hidden bg-panel">
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_interactive_element_to_noninteractive_role -->
  <canvas
    bind:this={canvas}
    class="absolute inset-0 block h-full w-full touch-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
    role="application"
    tabindex="0"
    aria-label={t("timeline.overview.label")}
    aria-describedby="overview-summary"
    style="width:{width}px;height:{height}px"
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointercancel={cancelTrim}
    onkeydown={onKeyDown}
    onpointerleave={() => (hover = null)}
  ></canvas>
  <p id="overview-summary" class="sr-only">
    {t("timeline.overview.summary", { count: clips.length, duration: formatDuration(duration) })}
  </p>

  {#if trim}
    <div class="pointer-events-none absolute z-10 w-max -translate-x-1/2 rounded border border-line bg-panel-3 px-2 py-1 font-mono text-xs text-text shadow-lg"
      style="left:{trimTipLeft}px;top:{RULER_H + 2}px" role="tooltip">
      {trim.edge === "start" ? t("timeline.trim.in") : t("timeline.trim.out")}
      {formatTimecode(trim.time)} ({formatDuration(trim.edge === "start" ? trim.span.end - trim.time : trim.time - trim.span.start)})
      {trim.snap ? ` · ${t(`timeline.snap.${trim.snap}`)}` : ""}
    </div>
  {:else if hovered && hover}
    {@const tag = tagById.get(hovered.tag_id ?? "")}
    <div
      class="pointer-events-none absolute z-10 w-max max-w-[min(26rem,90%)] -translate-x-1/2 rounded border border-line bg-panel-3 px-2 py-1 text-xs text-text shadow-lg"
      style="left:{tipLeft}px;top:{RULER_H + 2}px"
      role="tooltip"
    >
      <div class="font-semibold">
        {formatIndex(hovered.index)}
        {hovered.name}
        {#if tag}<span class="ml-1 rounded px-1" style="background:{tag.color};color:{contrastText(tag.color)}">{tag.label}</span>{/if}
      </div>
      <div class="font-mono text-muted">
        {formatTimecode(hovered.start_sec)} &rarr; {formatTimecode(hovered.end_sec)} ({formatDuration(hovered.end_sec - hovered.start_sec)})
      </div>
      {#if dups[hover.index]}<div class="text-warning">{t("timeline.duplicate")}</div>{/if}
      {#if hovered.export_status === "completed"}<div class="text-success">{t("timeline.exported")}</div>{/if}
    </div>
  {/if}
</div>
