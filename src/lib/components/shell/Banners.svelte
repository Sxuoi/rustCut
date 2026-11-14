<script lang="ts">
  // §6.4 banners: Source missing (Locate…, queue read-only until relinked)
  // and Read-only project (open elsewhere, newer schema). OWNER: fe-shell.
  import type { ReadOnlyReason } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { project } from "$lib/state/project.svelte";
  import { relinkSource } from "$lib/state/session";
  import { ui } from "$lib/state/ui.svelte";
  import { basename } from "./format";

  const status = $derived(project.snapshot?.source_status ?? null);
  const missing = $derived(status?.kind === "missing" || project.readOnly?.kind === "source_missing");
  const suggested = $derived(status?.kind === "missing" ? status.suggested_path : null);
  const otherReason = $derived(project.readOnly && project.readOnly.kind !== "source_missing" ? project.readOnly : null);

  function reasonText(r: ReadOnlyReason): string {
    switch (r.kind) {
      case "unwritable":
        return t("banner.unwritable", { path: r.path });
      case "open_elsewhere":
        return r.holder ? t("banner.openElsewhereBy", { holder: r.holder }) : t("banner.openElsewhere");
      case "newer_schema":
        return t("banner.newerSchema", { found: r.found, supported: r.supported });
      case "source_missing":
        return t("banner.sourceMissing", { path: project.source?.path ?? "" });
    }
  }
</script>

{#if missing && project.source}
  <div role="alert" class="flex flex-wrap items-center gap-3 border-b border-warning/50 bg-warning/10 px-4 py-2 text-sm">
    <span class="font-semibold text-warning">{t("banner.sourceMissingTitle")}</span>
    <span class="min-w-0 flex-1 truncate" title={project.source.path}>
      {t("banner.sourceMissing", { path: project.source.path })}
    </span>
    {#if suggested}
      <button
        type="button"
        class="rounded border border-line bg-panel-2 px-3 py-1 hover:bg-panel-3"
        onclick={() => void relinkSource(suggested, false).then((r) => {
          if (r?.kind === "mismatch") ui.locateMediaOpen = true;
        })}
      >
        {t("banner.useSuggested", { name: basename(suggested) })}
      </button>
    {/if}
    <button type="button" class="rounded bg-accent-strong px-3 py-1 font-medium text-white hover:bg-accent-hover" onclick={() => (ui.locateMediaOpen = true)}>
      {t("banner.locate")}
    </button>
  </div>
{/if}
{#if otherReason}
  <div role="status" class="flex items-center gap-3 border-b border-accent/40 bg-accent/10 px-4 py-2 text-sm">
    <span class="font-semibold text-accent">{t("banner.readOnlyTitle")}</span>
    <span class="min-w-0 flex-1">{reasonText(otherReason)}</span>
  </div>
{/if}
