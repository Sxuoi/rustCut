<script lang="ts">
  // Tag chooser popover for one row or the bulk bar (MRK-06, QUE-07): a
  // listbox of "No tag" + the palette (colour + label), ↑/↓/Home/End to move,
  // Enter/Space to choose, plus "Edit tags…".
  import type { Tag } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { tp } from "$lib/queue/format";
  import Popover from "$lib/components/queue/Popover.svelte";
  import Icon from "$lib/components/queue/Icon.svelte";

  interface Props {
    anchor: HTMLElement;
    tags: Tag[];
    /** Current tag id; `undefined` when the clips have different tags. */
    current: string | null | undefined;
    /** Number of clips the choice applies to. */
    count: number;
    onpick: (tagId: string | null, viaKeyboard: boolean) => void;
    onedit: () => void;
    onclose: () => void;
  }

  let { anchor, tags, current, count, onpick, onedit, onclose }: Props = $props();

  const uid = $props.id();
  const options = $derived<{ id: string | null; label: string; color: string | null }[]>([
    { id: null, label: t("tags.noTag"), color: null },
    ...tags.map((tag) => ({ id: tag.id, label: tag.label, color: tag.color })),
  ]);
  let active = $state(0);

  $effect.pre(() => {
    const i = options.findIndex((o) => o.id === current);
    active = i >= 0 ? i : 0;
  });

  function onKeydown(e: KeyboardEvent): void {
    const n = options.length;
    switch (e.key) {
      case "ArrowDown":
        active = (active + 1) % n;
        break;
      case "ArrowUp":
        active = (active - 1 + n) % n;
        break;
      case "Home":
        active = 0;
        break;
      case "End":
        active = n - 1;
        break;
      case "Enter":
      case " ":
        onpick(options[active].id, true);
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  }
</script>

<Popover {anchor} label={tp("tags.picker.title", count)} width={230} initialFocus="[role=listbox]" {onclose}>
  <p class="px-3 pt-2 pb-1 text-xs text-muted">{tp("tags.picker.title", count)}</p>
  <ul
    role="listbox"
    tabindex="0"
    aria-label={tp("tags.picker.title", count)}
    aria-activedescendant="{uid}-opt-{active}"
    class="max-h-64 overflow-y-auto py-1 focus:outline-none"
    onkeydown={onKeydown}
  >
    {#each options as o, i (o.id ?? "")}
      <!-- Keyboard: the listbox handles arrows/Enter via aria-activedescendant. -->
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <li
        id="{uid}-opt-{i}"
        role="option"
        tabindex="-1"
        aria-selected={o.id === current}
        class="flex cursor-pointer items-center gap-2 px-3 py-1.5 {i === active ? 'bg-panel-3' : ''}"
        onclick={() => onpick(o.id, false)}
        onpointermove={() => (active = i)}
      >
        {#if o.color}
          <span class="h-3 w-3 shrink-0 rounded-full" style:background-color={o.color} aria-hidden="true"></span>
        {:else}
          <span class="h-3 w-3 shrink-0 rounded-full border border-dashed border-muted" aria-hidden="true"></span>
        {/if}
        <span class="min-w-0 flex-1 truncate">{o.label}</span>
        {#if o.id === current}<Icon name="check" size={14} class="text-accent" />{/if}
      </li>
    {/each}
  </ul>
  <div class="border-t border-line p-1">
    <button type="button" class="w-full rounded px-2 py-1.5 text-left text-xs text-muted hover:bg-panel-3 hover:text-text" onclick={onedit}>
      {t("tags.picker.edit")}
    </button>
  </div>
</Popover>
