<script lang="ts">
  // Tag chip: colour swatch + label (or its letter when compact). Never colour
  // alone (§8.5, MRK-06). Clickable when `onclick` is given (opens a picker).
  import type { Tag } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { tagLetter } from "$lib/queue/tags";

  interface Props {
    tag: Tag | null | undefined;
    /** Show the label's first letter instead of the full label. */
    compact?: boolean;
    /** Several selected clips with different tags. */
    mixed?: boolean;
    onclick?: (e: MouseEvent & { currentTarget: HTMLButtonElement }) => void;
    /** Accessible name of the button (defaults to the label). */
    ariaLabel?: string;
    tabindex?: number;
    class?: string;
  }

  let { tag, compact = false, mixed = false, onclick, ariaLabel, tabindex, class: klass = "" }: Props = $props();

  const text = $derived(
    mixed ? t("tags.mixed") : tag ? (compact ? tagLetter(tag.label) : tag.label) : t("tags.add"),
  );
  const title = $derived(tag ? tag.label : mixed ? t("tags.mixed") : t("tags.noneTitle"));
  const base =
    "inline-flex h-5 max-w-28 shrink-0 items-center gap-1 rounded-full border px-1.5 text-[11px] leading-none";
</script>

{#snippet body()}
  {#if tag && !mixed}
    <span class="h-2.5 w-2.5 shrink-0 rounded-full" style:background-color={tag.color} aria-hidden="true"></span>
  {/if}
  <span class="truncate">{text}</span>
{/snippet}

{#if onclick}
  <button
    type="button"
    class="{base} {tag ? 'border-line bg-panel-3 text-text' : 'border-dashed border-line text-muted'} hover:border-accent {klass}"
    aria-label={ariaLabel ?? title}
    aria-haspopup="dialog"
    {title}
    {tabindex}
    {onclick}
  >
    {@render body()}
  </button>
{:else}
  <span class="{base} {tag ? 'border-line bg-panel-3 text-text' : 'border-dashed border-line text-muted'} {klass}" {title}>
    {@render body()}
  </span>
{/if}
