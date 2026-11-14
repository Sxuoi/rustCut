<script lang="ts">
  // EXP-11 pre-flight list of the Export dialog: ok / warning / error items
  // (icon + text, never colour alone); errors are listed first and block Start.
  import type { CheckLevel, PreflightItem } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { project } from "$lib/state/project.svelte";
  import { formatIndex } from "$lib/util/time";
  import { indexWidth, tp } from "$lib/queue/format";
  import Icon, { type IconName } from "$lib/components/queue/Icon.svelte";

  interface Props {
    items: PreflightItem[];
    loading: boolean;
    error: string | null;
  }

  let { items, loading, error }: Props = $props();

  const uid = $props.id();
  const ORDER: Record<CheckLevel, number> = { error: 0, warning: 1, ok: 2 };
  const ICON: Record<CheckLevel, IconName> = { ok: "check", warning: "alert", error: "error" };
  const COLOR: Record<CheckLevel, string> = { ok: "text-success", warning: "text-warning", error: "text-danger" };
  /** Clip numbers listed per item before "+N". */
  const MAX_CLIPS = 6;

  const sorted = $derived([...items].sort((a, b) => ORDER[a.level] - ORDER[b.level]));
  const errors = $derived(items.filter((i) => i.level === "error").length);
  const warnings = $derived(items.filter((i) => i.level === "warning").length);
  const width = $derived(indexWidth(project.clips.map((c) => c.index)));

  function clipList(ids: string[]): string {
    const labels = ids
      .map((id) => project.clip(id))
      .filter((c) => c !== undefined)
      .sort((a, b) => a.index - b.index)
      .map((c) => formatIndex(c.index, width));
    const shown = labels.slice(0, MAX_CLIPS).join(", ");
    return labels.length > MAX_CLIPS ? t("export.preflight.moreClips", { list: shown, n: labels.length - MAX_CLIPS }) : shown;
  }
</script>

<section aria-labelledby="{uid}-title" class="space-y-1.5">
  <div class="flex items-baseline justify-between gap-2">
    <h3 id="{uid}-title" class="text-xs font-semibold tracking-wider text-muted uppercase">{t("export.preflight.title")}</h3>
    <p class="text-xs" aria-live="polite">
      {#if loading}
        <span class="text-muted">{t("export.preflight.checking")}</span>
      {:else if error}
        <span class="text-danger">{t("export.preflight.failed")}</span>
      {:else if errors > 0}
        <span class="text-danger">{tp("export.preflight.blocking", errors)}</span>
      {:else if warnings > 0}
        <span class="text-warning">{tp("export.preflight.warnings", warnings)}</span>
      {:else if items.length > 0}
        <span class="text-success">{t("export.preflight.allOk")}</span>
      {/if}
    </p>
  </div>
  {#if error}
    <p class="flex items-start gap-2 text-danger"><Icon name="error" size={16} class="mt-0.5" />{error}</p>
  {:else}
    <ul class="space-y-1">
      {#each sorted as item, i (i)}
        <li class="flex items-start gap-2">
          <Icon name={ICON[item.level]} size={16} class="mt-0.5 {COLOR[item.level]}" />
          <span class="sr-only">{t(`export.preflight.level.${item.level}`)}:</span>
          <span class="min-w-0 flex-1 {item.level === 'error' ? 'text-danger' : 'text-text'}">
            {item.message}
            {#if item.clip_ids.length > 0}
              <span class="text-muted">({clipList(item.clip_ids)})</span>
            {/if}
          </span>
        </li>
      {/each}
    </ul>
  {/if}
</section>
