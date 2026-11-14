<script lang="ts">
  // Modal dialog on the native <dialog> element: focus is trapped, the page
  // behind is inert, Esc closes it and focus returns to the opener.
  // OWNER: fe-shell.
  import type { Snippet } from "svelte";
  import { t } from "$lib/i18n";

  interface Props {
    open: boolean;
    /** Called when the user closes it (Esc, close button, footer). */
    onclose: () => void;
    title: string;
    children: Snippet;
    footer?: Snippet;
    /** Tailwind max-width class. */
    width?: string;
  }

  let { open, onclose, title, children, footer, width = "max-w-lg" }: Props = $props();

  const uid = $props.id();
  let dialog: HTMLDialogElement | undefined = $state();

  $effect(() => {
    const d = dialog;
    if (!d) return;
    if (open && !d.open) d.showModal();
    else if (!open && d.open) d.close();
  });
</script>

<dialog
  bind:this={dialog}
  aria-labelledby="{uid}-title"
  class="m-auto max-h-[85vh] w-[calc(100%-2rem)] {width} overflow-hidden rounded-lg border border-line bg-panel p-0 text-text shadow-2xl backdrop:bg-black/60"
  onclose={() => {
    if (open) onclose();
  }}
>
  <div class="flex max-h-[85vh] flex-col">
    <header class="flex items-center justify-between gap-4 border-b border-line px-4 py-3">
      <h2 id="{uid}-title" class="text-base font-semibold">{title}</h2>
      <button
        type="button"
        class="rounded px-2 py-0.5 text-lg leading-none text-muted hover:bg-panel-3 hover:text-text"
        aria-label={t("dialog.close")}
        onclick={() => onclose()}
      >
        ×
      </button>
    </header>
    <div class="min-h-0 flex-1 overflow-auto px-4 py-3">
      {@render children()}
    </div>
    {#if footer}
      <footer class="flex justify-end gap-2 border-t border-line px-4 py-3">
        {@render footer()}
      </footer>
    {/if}
  </div>
</dialog>
