<script lang="ts">
  // Modal dialog shell for the Export, Tag and Timestamps dialogs (§8.5):
  // aria-modal + labelled title, focus trap, Escape and backdrop close, focus
  // restored to where it was (or the player) on close.
  import type { Snippet } from "svelte";
  import { tick } from "svelte";
  import { t } from "$lib/i18n";
  import { focusableIn, focusHome } from "$lib/queue/popover";
  import Icon from "$lib/components/queue/Icon.svelte";

  interface Props {
    open: boolean;
    title: string;
    onclose: () => void;
    children: Snippet;
    footer?: Snippet;
    size?: "md" | "lg" | "xl";
    /** Extra key handling inside the dialog (e.g. Enter starts the export). */
    onkeydown?: (e: KeyboardEvent) => void;
  }

  let { open, title, onclose, children, footer, size = "lg", onkeydown }: Props = $props();

  const uid = $props.id();
  const WIDTH = { md: "max-w-md", lg: "max-w-2xl", xl: "max-w-4xl" } as const;
  let panel: HTMLDivElement | undefined = $state();

  $effect(() => {
    if (!open) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    void tick().then(() => {
      if (!panel) return;
      const first = panel.querySelector<HTMLElement>("[data-autofocus]") ?? focusableIn(panel)[0] ?? panel;
      first.focus({ preventScroll: true });
    });
    return () => {
      if (previous && previous.isConnected && previous !== document.body) previous.focus({ preventScroll: true });
      else focusHome();
    };
  });

  function onKeydown(e: KeyboardEvent): void {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onclose();
      return;
    }
    if (e.key === "Tab" && panel) {
      const list = focusableIn(panel);
      if (list.length === 0) {
        e.preventDefault();
        return;
      }
      const first = list[0];
      const last = list[list.length - 1];
      const active = document.activeElement;
      if (e.shiftKey && (active === first || active === panel)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
      return;
    }
    onkeydown?.(e);
  }

  function onBackdropPointerDown(e: PointerEvent): void {
    if (e.target === e.currentTarget) onclose();
  }
</script>

{#if open}
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
    role="presentation"
    onpointerdown={onBackdropPointerDown}
  >
    <div
      bind:this={panel}
      role="dialog"
      aria-modal="true"
      aria-labelledby="{uid}-title"
      tabindex="-1"
      class="flex max-h-full w-full flex-col rounded-lg border border-line bg-panel text-sm shadow-2xl {WIDTH[size]}"
      onkeydown={onKeydown}
    >
      <header class="flex items-center justify-between gap-3 border-b border-line px-4 py-3">
        <h2 id="{uid}-title" class="text-base font-semibold">{title}</h2>
        <button
          type="button"
          class="rounded p-1 text-muted hover:bg-panel-3 hover:text-text"
          aria-label={t("export.modal.close")}
          onclick={onclose}
        >
          <Icon name="x" />
        </button>
      </header>
      <div class="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        {@render children()}
      </div>
      {#if footer}
        <footer class="border-t border-line px-4 py-3">{@render footer()}</footer>
      {/if}
    </div>
  </div>
{/if}
