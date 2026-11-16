<script lang="ts">
  // Video player (PRD §7.3 playback tiers, PLY-01..04, §6.4 states, FRM-04
  // crop overlay, FRM-09 output-only view with the blurred background).
  // OWNER: fe-shell.
  //
  // Tier handling: canPlayType(mime_type) with the exact codec string, then a
  // decoded first frame within 3 s; on failure (or any later player error)
  // request `next_tier`. Remux/proxy tiers show "Preparing preview" progress.
  // No tier left -> a specific message (codec, why, what to do), never black.
  import { untrack } from "svelte";
  import type { FrameLayout, Framing, PlaybackInfo, PlaybackTier } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { backend, BackendError } from "$lib/ipc";
  import { media } from "$lib/state/media.svelte";
  import {
    computeTimeOffset,
    confirmFirstFrame,
    containRect,
    formatEta,
    ignorePlaybackInfo,
    mediaErrorReason,
    nextPlaybackTier,
    tierLabel,
    unsupportedMessage,
  } from "$lib/state/playback";
  import { player } from "$lib/state/player.svelte";
  import { project } from "$lib/state/project.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import { framing } from "$lib/state/framing.svelte";
  import { presetOf } from "$lib/queue/exportOptions";
  import { outputView } from "$lib/util/blurPreview";
  import { frameLayout, windowShape } from "$lib/util/crop";
  import BlurBackdrop from "./BlurBackdrop.svelte";
  import CropOverlay from "./CropOverlay.svelte";

  type Status = "idle" | "missing" | "preparing" | "loading" | "switching" | "ready" | "unsupported";

  let wrap: HTMLDivElement | undefined = $state();
  let video: HTMLVideoElement | undefined = $state();
  let box = $state({ w: 0, h: 0 });
  let status = $state<Status>("idle");
  let message = $state<string | null>(null);

  /** Increments per load attempt; stale async results are dropped. */
  let attempt = 0;
  /** `${tier}|${media_path}` currently loaded into the element. */
  let loadedKey: string | null = null;
  /** Tiers that failed for the current project (each is tried once). */
  let tried = new Set<PlaybackTier>();
  /** Tiers whose preparation was requested for the current project (each once). */
  let requested = new Set<PlaybackTier>();
  let latestTier: PlaybackTier | null = null;
  let triedGeneration = -1;

  const displayW = $derived(project.source?.video.display_width ?? 0);
  const displayH = $derived(project.source?.video.display_height ?? 0);
  const fullRect = $derived(containRect(box.w, box.h, displayW, displayH));
  /** The framing the export uses (Lossless always exports the original framing). */
  const framingKind = $derived<Framing>(
    project.settings && presetOf(project.settings) !== "lossless" ? project.settings.framing : "original",
  );
  const resolution = $derived(project.settings?.output_resolution ?? "1080x1920");
  /** 9:16 or 1:1 crop window positioned per clip; `null` for the other framings. */
  const shape = $derived(windowShape(framingKind));
  const committedOffset = $derived(shape && project.primary ? project.primary.crop_x_offset : 0);
  /** An overlay or slider edit is in flight for the primary clip. */
  const editing = $derived(
    shape !== null && framing.preview !== null && project.primary !== null && framing.preview.id === project.primary.id,
  );

  // FRM-04: the authoritative layout comes from Rust (the function the export
  // uses) for the committed offset; the TS port only bridges an edit in flight.
  let remote = $state.raw<{ key: string; layout: FrameLayout | null } | null>(null);
  let layoutToken = 0;
  const layoutKey = $derived(`${framingKind}|${displayW}x${displayH}|${resolution}|${committedOffset}`);
  $effect(() => {
    const key = layoutKey;
    const f = framingKind;
    const dw = displayW;
    const dh = displayH;
    const res = resolution;
    const off = committedOffset;
    if (f === "original" || dw <= 0 || dh <= 0) return;
    const mine = ++layoutToken;
    backend
      .frameLayout(f, dw, dh, off, res)
      .then((layout) => {
        if (mine === layoutToken) remote = { key, layout };
      })
      .catch(() => {});
  });
  const layout = $derived.by<FrameLayout | null>(() => {
    if (framingKind === "original" || displayW <= 0 || displayH <= 0) return null;
    if (!editing && remote && remote.key === layoutKey) return remote.layout;
    const off = editing ? framing.offsetFor(project.primary) : committedOffset;
    return frameLayout(framingKind, displayW, displayH, off, resolution);
  });
  /**
   * FRM-04 "9:16 only" / "1:1 only": just what the export contains. Vertical
   * Crop shows the window region; the blur presets show the whole output
   * frame, the window at its place over the blurred background.
   */
  const view = $derived(layout && framing.outputOnly ? outputView(layout, box.w, box.h) : null);
  /** Clipping wrapper (CSS px in the player box) where the window is shown. */
  const clipBox = $derived(view ? view.fg : null);
  /** Whole-video rect relative to the player box (overlay coordinate space). */
  const rect = $derived(
    view && layout
      ? {
          left: view.fg.left - layout.window.x * view.vx,
          top: view.fg.top - layout.window.y * view.vy,
          width: displayW * view.vx,
          height: displayH * view.vy,
        }
      : fullRect,
  );
  /** The video element's box inside its parent (clip wrapper or the player). */
  const videoStyle = $derived(
    view && layout
      ? `left:${-layout.window.x * view.vx}px;top:${-layout.window.y * view.vy}px;width:${displayW * view.vx}px;height:${displayH * view.vy}px;`
      : "left:0;top:0;width:100%;height:100%;",
  );
  let pb = $state.raw<PlaybackInfo | null>(null);
  const pct = $derived(pb?.progress === null || pb?.progress === undefined ? null : Math.round(Math.min(1, Math.max(0, pb.progress)) * 100));

  function unload(el: HTMLVideoElement | undefined = video): void {
    if (!el || loadedKey === null) return;
    loadedKey = null;
    el.pause();
    el.removeAttribute("src");
    el.load();
  }

  /** Try the next tier, or show the specific "cannot preview" message. */
  async function fallback(id: number, info: PlaybackInfo, reason: string): Promise<void> {
    tried.add(info.tier);
    player.ready = false;
    const next = nextPlaybackTier(info, tried, requested);
    if (next) {
      status = "switching";
      message = reason;
      unload();
      if (player.restoreTime === null && player.currentTime > 0) player.restoreTime = player.currentTime;
      requested.add(next);
      latestTier = next;
      try {
        const nextInfo = await backend.requestPlayback(next);
        if (id === attempt) player.playback = nextInfo;
      } catch (e) {
        if (id === attempt) {
          status = "unsupported";
          message = unsupportedMessage(project.source?.video.codec ?? "", BackendError.from(e).message);
          player.error = message;
        }
      }
      return;
    }
    unload();
    status = "unsupported";
    message = unsupportedMessage(project.source?.video.codec ?? "", reason);
    player.error = message;
  }

  async function handlePlayback(el: HTMLVideoElement, info: PlaybackInfo | null): Promise<void> {
    if (player.generation !== triedGeneration) {
      triedGeneration = player.generation;
      tried = new Set();
      requested = new Set();
      latestTier = null;
    }
    // Old cancellation/progress/completion events must not cancel a newer first-frame check.
    if (info && ignorePlaybackInfo(info, latestTier)) return;
    const id = ++attempt;
    pb = info;
    if (info) latestTier = info.tier;
    if (!info) {
      latestTier = null;
      unload(el);
      player.ready = false;
      status = project.snapshot?.source_status.kind === "missing" ? "missing" : "idle";
      message = null;
      return;
    }
    if (info.unsupported_reason) {
      await fallback(id, info, info.unsupported_reason);
      return;
    }
    if (info.preparing || !info.media_path) {
      unload(el);
      player.ready = false;
      status = "preparing";
      message = null;
      // §7.3: a source the WebView cannot open (FLV, ...) starts at Remux, and the open result only
      // announces it. Ask for the preparation once; progress and the result arrive as playback
      // events, which re-enter here without requesting again.
      if (info.preparing && !info.media_path && !requested.has(info.tier)) {
        requested.add(info.tier);
        const generation = player.generation;
        backend.requestPlayback(info.tier).catch((e: unknown) => {
          if (generation !== player.generation || latestTier !== info.tier || tried.has(info.tier)) return;
          void fallback(++attempt, info, BackendError.from(e).message);
        });
      }
      return;
    }
    const key = `${info.tier}|${info.media_path}`;
    if (key === loadedKey && status === "ready" && player.ready) return; // same media, still fine
    if (info.mime_type && el.canPlayType(info.mime_type) === "") {
      await fallback(id, info, t("player.reason.canPlayType", { mime: info.mime_type }));
      return;
    }
    status = "loading";
    message = null;
    player.ready = false;
    loadedKey = key;
    el.src = backend.mediaUrl(info.media_path);
    el.load();
    const r = await confirmFirstFrame(el);
    if (id !== attempt) return;
    if (!r.ok) {
      await fallback(id, info, r.reason);
      return;
    }
    // §7.3 time origin: compare the first presented frame with the source.
    const expected = media.keyframes.length > 0 ? media.keyframes[0] : 0;
    player.timeOffset = computeTimeOffset(r.mediaTime, expected, player.frameDuration);
    player.error = null;
    status = "ready";
    player.ready = true;
    const restore = player.restoreTime;
    player.restoreTime = null;
    if (restore !== null) player.seek(restore);
    else player.syncFromElement();
  }

  // Attach first so the playback effect below loads into an attached element.
  $effect(() => {
    const el = video;
    if (!el) return;
    // Only the element is a dependency. attach() reads player state (the playback speed); tracked,
    // a speed change re-ran this effect and its teardown unloaded the video (blank player).
    untrack(() => player.attach(el));
    return () => {
      player.detach();
      unload(el);
    };
  });

  $effect(() => {
    const el = video;
    const info = player.playback;
    if (!el) return;
    untrack(() => void handlePlayback(el, info));
  });

  $effect(() => {
    ui.registerPlayerFocus(wrap ?? null);
    return () => ui.registerPlayerFocus(null);
  });

  $effect(() => {
    const el = wrap;
    if (!el) return;
    // Observe the player box, not the video: in 9:16-only mode the video is
    // larger than the visible region.
    const ro = new ResizeObserver((entries) => {
      const r = entries[0]?.contentRect;
      if (r) box = { w: r.width, h: r.height };
    });
    ro.observe(el);
    return () => ro.disconnect();
  });

  function sync(): void {
    player.syncFromElement();
  }

  function onRateChange(): void {
    if (video) player.rate = video.playbackRate;
  }

  function onDurationChange(): void {
    if (video && !(player.duration > 0) && Number.isFinite(video.duration)) player.duration = player.toSource(video.duration);
  }

  /** §7.3 "any player error falls back to the next tier automatically". */
  function onMediaError(): void {
    const info = pb;
    if (status !== "ready" || !video || !info) return;
    player.restoreTime = player.currentTime;
    loadedKey = null;
    void fallback(++attempt, info, mediaErrorReason(video.error?.code));
  }
</script>

<div
  bind:this={wrap}
  tabindex="-1"
  role="region"
  aria-label={t("player.region")}
  class="relative h-full w-full overflow-hidden bg-black outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset"
>
  {#if view && layout?.blur && video && status === "ready"}
    <BlurBackdrop {video} {layout} box={view.out} displayWidth={displayW} displayHeight={displayH} />
  {/if}
  <div
    class="absolute overflow-hidden"
    data-testid="player-clip"
    style={clipBox ? `left:${clipBox.left}px;top:${clipBox.top}px;width:${clipBox.width}px;height:${clipBox.height}px;` : "inset:0;"}
  >
    <!-- svelte-ignore a11y_media_has_caption, a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <video
      bind:this={video}
      class="absolute max-w-none {view ? 'object-fill' : 'object-contain'}"
      style={videoStyle}
      preload="auto"
      playsinline
      disablepictureinpicture
      aria-label={t("player.video")}
      ontimeupdate={sync}
      onseeked={sync}
      onplay={sync}
      onpause={sync}
      onended={sync}
      onratechange={onRateChange}
      ondurationchange={onDurationChange}
      onerror={onMediaError}
      onclick={() => {
        if (status === "ready") player.toggle();
      }}
    ></video>
  </div>

  {#if shape && layout && status === "ready" && rect && displayW > 0 && displayH > 0}
    <div class="absolute inset-0">
      <CropOverlay {rect} displayWidth={displayW} displayHeight={displayH} win={layout.window} {shape} outputMode={view !== null} />
    </div>
  {/if}

  {#if player.loopClipId && status === "ready"}
    <div class="pointer-events-none absolute top-2 left-2 rounded bg-black/70 px-2 py-0.5 text-xs text-text">{t("player.looping")}</div>
  {/if}

  {#if status !== "ready"}
    <div class="absolute inset-0 flex items-center justify-center bg-bg/90 p-6">
      {#if status === "preparing" && pb}
        <div role="status" class="w-full max-w-md text-center">
          <p class="font-medium">{t("player.preparing", { tier: tierLabel(pb.tier) })}</p>
          <div
            class="mt-3 h-2 w-full overflow-hidden rounded bg-panel-3"
            role="progressbar"
            aria-label={t("player.preparingAria")}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={pct ?? undefined}
          >
            <div class="h-full bg-accent transition-[width]" style:width="{pct ?? 0}%"></div>
          </div>
          <p class="tabular mt-2 text-sm text-muted">
            {#if pct !== null}{pct}% ·
            {/if}{formatEta(pb.eta_sec)}
          </p>
          <p class="mt-3 text-xs text-muted">{t("player.preparingNote")}</p>
        </div>
      {:else if status === "loading" || status === "switching"}
        <div role="status" class="flex max-w-md flex-col items-center gap-3 text-center text-sm text-muted">
          <div class="size-8 animate-spin rounded-full border-4 border-panel-3 border-t-accent" aria-hidden="true"></div>
          <p>{status === "switching" ? t("player.switching", { reason: message ?? "" }) : t("player.loading")}</p>
        </div>
      {:else if status === "unsupported"}
        <div role="alert" class="max-w-lg text-center">
          <p class="font-semibold text-warning">{t("player.unsupportedTitle")}</p>
          <p class="mt-2 text-sm">{message}</p>
        </div>
      {:else if status === "missing"}
        <div class="max-w-lg text-center">
          <p class="font-semibold text-warning">{t("player.missingTitle")}</p>
          <p class="mt-2 text-sm text-muted">{t("player.missing")}</p>
          <button
            type="button"
            class="mt-3 rounded bg-accent-strong px-3 py-1.5 text-sm font-medium text-white hover:bg-accent-hover"
            onclick={() => (ui.locateMediaOpen = true)}
          >
            {t("banner.locate")}
          </button>
        </div>
      {:else}
        <p class="text-sm text-muted">{t("player.idle")}</p>
      {/if}
    </div>
  {/if}
</div>
