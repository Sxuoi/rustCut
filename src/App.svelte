<script lang="ts">
  // Root layout (PRD §6.1) and startup (§3.1, §6.4). OWNER: fe-shell. Other
  // agents only change their own components; this file composes them.
  import { onMount } from "svelte";
  import type { ViewState } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { listen } from "@tauri-apps/api/event";
  import { backend, BackendError } from "$lib/ipc";
  import { installGlobalKeyboard } from "$lib/keyboard";
  import { startEventLoop } from "$lib/state/events";
  import { player } from "$lib/state/player.svelte";
  import { project } from "$lib/state/project.svelte";
  import { loadAppContext, openPath, restoreSession } from "$lib/state/session";
  import { ui } from "$lib/state/ui.svelte";
  import { roundMs } from "$lib/util/time";
  import { installFileDrop } from "$lib/components/shell/dragdrop";
  import { isVerticalCrop, stem } from "$lib/components/shell/format";

  import HeaderBar from "$lib/components/shell/HeaderBar.svelte";
  import EmptyState from "$lib/components/shell/EmptyState.svelte";
  import Banners from "$lib/components/shell/Banners.svelte";
  import Toasts from "$lib/components/shell/Toasts.svelte";
  import CheatSheet from "$lib/components/shell/CheatSheet.svelte";
  import SettingsDialog from "$lib/components/shell/SettingsDialog.svelte";
  import LocateMediaDialog from "$lib/components/shell/LocateMediaDialog.svelte";
  import Player from "$lib/components/player/Player.svelte";
  import Transport from "$lib/components/player/Transport.svelte";
  import CropControls from "$lib/components/player/CropControls.svelte";
  import OverviewTimeline from "$lib/components/timeline/OverviewTimeline.svelte";
  import ZoomTimeline from "$lib/components/timeline/ZoomTimeline.svelte";
  import ClipQueue from "$lib/components/queue/ClipQueue.svelte";
  import ExportPanel from "$lib/components/export/ExportPanel.svelte";
  import ExportDialog from "$lib/components/export/ExportDialog.svelte";
  import TagEditor from "$lib/components/tags/TagEditor.svelte";
  import TimestampsDialog from "$lib/components/timestamps/TimestampsDialog.svelte";

  onMount(() => {
    const removeKeys = installGlobalKeyboard();
    const removeDrop = installFileDrop();
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        await startEventLoop();
      } catch (e) {
        ui.error(t("app.eventsFailed", { reason: BackendError.from(e).message }), "events");
      }
      // Second launches hand their file to this window ("open-request").
      if (backend.kind === "tauri") {
        try {
          const un = await listen<string>("open-request", (e) => {
            if (typeof e.payload === "string" && e.payload) void openPath(e.payload);
          });
          if (disposed) un();
          else unlisten = un;
        } catch (e) {
          ui.error(t("app.eventsFailed", { reason: BackendError.from(e).message }), "events");
        }
      }
      await Promise.all([loadAppContext(), restoreSession()]);
      // A file given on the command line opens through the normal open flow
      // (a WebView reload keeps the already-open project instead).
      try {
        const launch = await backend.getLaunchPath();
        if (launch && !project.loaded) await openPath(launch);
      } catch (e) {
        ui.error(BackendError.from(e).message, "launch");
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
      removeKeys();
      removeDrop();
    };
  });

  const ready = $derived(ui.phase === "ready" && project.loaded);
  const vertical = $derived(isVerticalCrop(project.settings));

  // PRJ-03: persist playhead, zoom and selection (throttled in the store).
  // Only once the first frame is shown, so the restored playhead is not
  // overwritten by the 0 of a loading player.
  $effect(() => {
    if (!ready || !player.ready) return;
    const view: ViewState = {
      playhead_sec: roundMs(player.currentTime),
      zoom_window_sec: ui.zoomWindowSec,
      selected_clip_id: project.primaryId,
    };
    project.setViewState(view);
  });

  // QUE-03: the loop follows its clip (retime) and ends when it is deleted.
  $effect(() => {
    const id = player.loopClipId;
    if (!id) return;
    const clip = project.clips.find((c) => c.id === id);
    if (!clip) player.setLoop(null, null);
    else if (player.loopRange?.start !== clip.start_sec || player.loopRange?.end !== clip.end_sec) {
      player.updateLoopRange({ start: clip.start_sec, end: clip.end_sec });
    }
  });

  // §8.5: in-app reduced-motion override (null = follow the OS).
  $effect(() => {
    const rm = ui.settings?.reduced_motion ?? null;
    const root = document.documentElement;
    if (rm === null) delete root.dataset.reducedMotion;
    else root.dataset.reducedMotion = String(rm);
  });

  $effect(() => {
    document.title = project.source ? t("app.titleWithSource", { name: stem(project.source.path) }) : t("app.title");
  });
</script>

<div class="flex h-full flex-col bg-bg text-text">
  <HeaderBar />
  {#if !ready}
    <main class="min-h-0 flex-1">
      <EmptyState />
    </main>
  {:else}
    <h1 class="sr-only">{t("app.title")}</h1>
    <Banners />
    <main class="relative grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_420px] grid-rows-[minmax(0,1fr)_auto] gap-px bg-line">
      <section class="flex min-h-0 flex-col bg-bg" aria-label={t("app.region.player")}>
        <div class="relative min-h-0 flex-1"><Player /></div>
        {#if vertical}<CropControls />{/if}
        <Transport />
      </section>
      <aside class="row-span-1 flex min-h-0 flex-col bg-panel" aria-label={t("app.region.queue")}>
        <ClipQueue />
      </aside>
      <section class="flex flex-col bg-panel" aria-label={t("app.region.timelines")}>
        <div class="h-24"><OverviewTimeline /></div>
        <div class="h-20 border-t border-line"><ZoomTimeline /></div>
      </section>
      <section class="bg-panel" aria-label={t("app.region.export")}>
        <ExportPanel />
      </section>
      {#if ui.dragOver}
        <div class="pointer-events-none absolute inset-0 z-30 flex items-center justify-center border-4 border-dashed border-accent bg-bg/70">
          <p class="text-lg font-semibold">{t("empty.dropNow")}</p>
        </div>
      {/if}
    </main>
  {/if}
  <Toasts />
  <ExportDialog />
  <CheatSheet />
  <SettingsDialog />
  <LocateMediaDialog />
  <TagEditor />
  <TimestampsDialog />
</div>
