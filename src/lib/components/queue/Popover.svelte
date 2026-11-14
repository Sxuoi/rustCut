<script lang="ts">
  // Non-modal popover anchored to an element (tag picker, failure details).
  // Fixed-positioned so the virtualised list's overflow never clips it; closes
  // on Escape (focus back to the anchor), outside pointer-down and resize.
  import type { Snippet } from "svelte";
  import { onMount, tick } from "svelte";
  import { focusableIn, placePopover } from "$lib/queue/popover";

  interface Props {
    /** Element the popover hangs from (focus returns here on Escape). */
    anchor: HTMLElement;
    /** Accessible name of the popover. */
    label: string;
    width?: number;
    /** Selector of the element to focus first (default: first focusable). */
    initialFocus?: string;
    onclose: () => void;
    children: Snippet;
  }

  let { anchor, label, width = 260, initialFocus, onclose, children }: Props = $props();

  let el: HTMLDivElement | undefined = $state();
  let left = $state(-9999);
  let top = $state(-9999);

  function place(): void {
    if (!el) return;
    const p = placePopover(
      anchor.getBoundingClientRect(),
      { width: el.offsetWidth, height: el.offsetHeight },
      { width: window.innerWidth, height: window.innerHeight },
    );
    left = p.left;
    top = p.top;
  }

  onMount(() => {
    place();
    void tick().then(() => {
      if (!el) return;
      const first = (initialFocus ? el.querySelector<HTMLElement>(initialFocus) : null) ?? focusableIn(el)[0] ?? el;
      first.focus({ preventScroll: true });
    });
  });

  function onWindowPointerDown(e: PointerEvent): void {
    const target = e.target instanceof Node ? e.target : null;
    if (!el || !target || el.contains(target) || anchor.contains(target)) return;
    onclose();
  }

  function onKeydown(e: KeyboardEvent): void {
    if (e.key !== "Escape") return;
    e.preventDefault();
    e.stopPropagation();
    onclose();
    if (anchor.isConnected) anchor.focus({ preventScroll: true });
  }

  function onFocusOut(e: FocusEvent): void {
    const next = e.relatedTarget instanceof Node ? e.relatedTarget : null;
    // Tabbing out of the popover closes it (keyboard users never get stuck).
    if (next && el && !el.contains(next) && !anchor.contains(next)) onclose();
  }
</script>

<svelte:window onpointerdown={onWindowPointerDown} onresize={onclose} />

<div
  bind:this={el}
  role="dialog"
  aria-label={label}
  tabindex="-1"
  class="fixed z-40 rounded-md border border-line bg-panel-2 text-sm shadow-xl"
  style:left="{left}px"
  style:top="{top}px"
  style:width="{width}px"
  onkeydown={onKeydown}
  onfocusout={onFocusOut}
>
  {@render children()}
</div>
