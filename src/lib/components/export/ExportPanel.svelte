<script lang="ts">
  // Export panel, bottom right of the main window (§6.1): "N included  est.
  // ~X MB  ~Ys", the EXPORT N CLIPS (Ctrl+E) button, the batch progress row
  // with ETA and Cancel all (EXP-09, shown only while exporting), the pause
  // prompt (EXP-10) and the end-of-batch summary with Retry failed.
  import { t } from "$lib/i18n";
  import { project } from "$lib/state/project.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import { exporter } from "$lib/state/exporter.svelte";
  import { ariaKeyShortcuts, detectPlatform, shortcutLabel } from "$lib/keyboard/shortcuts";
  import { formatBytes, formatPercent, formatSeconds, tp } from "$lib/queue/format";
  import { includedClips } from "$lib/queue/rows";
  import { finishedCount, overallFraction, summaryCounts } from "$lib/queue/progress";
  import { btn } from "$lib/queue/ui";
  import Icon from "$lib/components/queue/Icon.svelte";

  /** Debounce of the estimate pre-flight (EXP-11) after edits. */
  const ESTIMATE_DEBOUNCE_MS = 400;

  const platform = detectPlatform(typeof navigator === "undefined" ? undefined : navigator);
  const exportKey = shortcutLabel("exportDialog", platform);
  const exportAria = ariaKeyShortcuts("exportDialog", platform);

  const includedIds = $derived(includedClips(project.clips).map((c) => c.id));
  const count = $derived(includedIds.length);
  const estimate = $derived(count > 0 ? exporter.estimate : null);
  const progress = $derived(exporter.phase === "running" ? exporter.progress : null);
  const fraction = $derived(overallFraction(progress));
  const counts = $derived(exporter.summary ? summaryCounts(exporter.summary) : null);
  const disabledReason = $derived(
    !project.loaded
      ? t("export.panel.noProject")
      : exporter.running
        ? t("export.panel.busy")
        : count === 0
          ? t("export.panel.nothing")
          : null,
  );

  $effect(() => {
    const ids = includedIds;
    const settings = project.settings;
    if (!settings || exporter.running) return;
    const timer = setTimeout(() => void exporter.refreshEstimate(ids, settings), ESTIMATE_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  });
</script>

<div class="flex flex-col gap-2 p-3">
  <p class="flex flex-wrap items-baseline gap-x-5 gap-y-1 text-xs">
    <span class="font-semibold">{tp("export.panel.included", count)}</span>
    {#if estimate}
      <span class="text-muted">{t("export.panel.estimate", { size: formatBytes(estimate.estimate_bytes, true) })}</span>
      {#if estimate.estimate_seconds !== null}
        <span class="text-muted">{formatSeconds(estimate.estimate_seconds, true)}</span>
      {/if}
    {/if}
  </p>

  <button
    type="button"
    class="{btn.primary} h-9 w-full text-sm tracking-wide uppercase"
    disabled={disabledReason !== null}
    title={disabledReason ?? undefined}
    aria-keyshortcuts={exportAria || undefined}
    onclick={() => (ui.exportDialogOpen = true)}
  >
    {tp("export.panel.button", count)}
    {#if exportKey}<span class="font-normal normal-case opacity-80">({exportKey})</span>{/if}
  </button>

  {#if exporter.phase === "running"}
    <div class="flex items-center gap-2">
      <div
        class="h-2 min-w-0 flex-1 overflow-hidden rounded bg-panel-3"
        role="progressbar"
        aria-label={t("export.panel.progressLabel")}
        aria-valuemin="0"
        aria-valuemax="100"
        aria-valuenow={Math.floor(fraction * 100)}
        aria-valuetext={progress
          ? t("export.panel.progressText", {
              percent: formatPercent(fraction),
              done: finishedCount(progress),
              total: progress.total,
            })
          : t("export.panel.starting")}
      >
        <div class="h-full rounded bg-accent transition-[width] duration-300" style:width="{fraction * 100}%"></div>
      </div>
      <span class="tabular w-10 text-right text-xs">{formatPercent(fraction)}</span>
      {#if progress}
        <span class="tabular text-xs text-muted">
          {t("export.panel.count", { done: finishedCount(progress), total: progress.total })}
        </span>
      {/if}
    </div>
    <div class="flex items-center justify-between gap-2 text-xs text-muted">
      <span>
        {#if !progress}
          {t("export.panel.starting")}
        {:else if exporter.etaSec !== null}
          {t("export.panel.eta", { eta: formatSeconds(exporter.etaSec, true) })}
        {:else}
          {t("export.panel.etaUnknown")}
        {/if}
        {#if progress && progress.failed > 0}
          <span class="ml-2 text-danger">{tp("export.panel.failedSoFar", progress.failed)}</span>
        {/if}
      </span>
      <button type="button" class={btn.danger} onclick={() => void exporter.cancel(null)}>
        {t("export.panel.cancelAll")}
      </button>
    </div>
  {:else if exporter.phase === "paused" && exporter.pause}
    <div class="space-y-2 rounded-md border border-warning/60 bg-warning/10 p-2 text-xs" role="alert">
      <p class="flex items-start gap-1.5 font-semibold text-warning">
        <Icon name="alert" size={14} class="mt-px" />
        {t(`export.pause.${exporter.pause.reason}`)}
      </p>
      {#if exporter.pause.message}<p class="text-muted">{exporter.pause.message}</p>{/if}
      <div class="flex justify-end gap-2">
        <button type="button" class={btn.danger} onclick={() => void exporter.cancel(null)}>
          {t("export.panel.cancelAll")}
        </button>
        <button type="button" class={btn.primary} onclick={() => void exporter.resume()}>
          {t("export.pause.resume")}
        </button>
      </div>
    </div>
  {/if}

  {#if exporter.phase === "finished" && exporter.summary && counts}
    <div class="space-y-2 rounded-md border border-line bg-panel-2 p-2 text-xs" role="status">
      <div class="flex items-start justify-between gap-2">
        <p class="font-semibold">{t("export.summary.title", { elapsed: formatSeconds(exporter.summary.elapsed_sec) })}</p>
        <button
          type="button"
          class={btn.icon}
          aria-label={t("export.summary.dismiss")}
          title={t("export.summary.dismiss")}
          onclick={() => exporter.dismissSummary()}
        >
          <Icon name="x" size={14} />
        </button>
      </div>
      <p class="flex flex-wrap gap-x-3">
        <span class="text-success">{tp("export.summary.done", counts.done)}</span>
        <span class={counts.failed > 0 ? "text-danger" : "text-muted"}>{tp("export.summary.failed", counts.failed)}</span>
        <span class="text-muted">{tp("export.summary.skipped", counts.skipped)}</span>
        <span class="text-muted">{tp("export.summary.cancelled", counts.cancelled)}</span>
      </p>
      <div class="flex flex-wrap justify-end gap-2">
        {#if counts.failed > 0}
          <button type="button" class={btn.secondary} disabled={exporter.starting} onclick={() => void exporter.retryFailed()}>
            <Icon name="retry" size={14} />{tp("export.summary.retry", counts.failed)}
          </button>
        {/if}
        <button type="button" class={btn.secondary} onclick={() => void exporter.openOutputFolder()}>
          <Icon name="folder" size={14} />{t("export.summary.openFolder")}
        </button>
      </div>
    </div>
  {/if}
</div>
