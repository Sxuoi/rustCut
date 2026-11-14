<script lang="ts" module>
  /** One entry of a {@link Menu}. */
  export interface MenuItem {
    id: string;
    label: string;
    /** Second line in muted text. */
    hint?: string;
    disabled?: boolean;
  }
</script>

<script lang="ts">
  // Accessible menu button (WAI-ARIA menu pattern): ↓/↑ open and move, Home/End,
  // Enter/Space choose, Escape closes and refocuses the trigger, Tab closes.
  import type { Snippet } from "svelte";
  import { tick } from "svelte";

  interface Props {
    /** Accessible name of the trigger and the menu. */
    label: string;
    items: MenuItem[];
    onselect: (id: string) => void;
    /** Visible trigger content. */
    trigger: Snippet;
    triggerClass?: string;
    align?: "left" | "right";
    disabled?: boolean;
  }

  let { label, items, onselect, trigger, triggerClass = "", align = "left", disabled = false }: Props = $props();

  const menuId = $props.id();
  let open = $state(false);
  let button: HTMLButtonElement | undefined = $state();
  let menu: HTMLDivElement | undefined = $state();

  function entries(): HTMLElement[] {
    return Array.from(menu?.querySelectorAll<HTMLElement>('[role="menuitem"]:not([aria-disabled="true"])') ?? []);
  }

  async function show(focus: "first" | "last"): Promise<void> {
    open = true;
    await tick();
    const list = entries();
    (focus === "first" ? list[0] : list.at(-1))?.focus();
  }

  function hide(refocus: boolean): void {
    open = false;
    if (refocus) button?.focus();
  }

  function onTriggerKeydown(e: KeyboardEvent): void {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      void show(e.key === "ArrowDown" ? "first" : "last");
    }
  }

  function onMenuKeydown(e: KeyboardEvent): void {
    const list = entries();
    const i = list.indexOf(document.activeElement as HTMLElement);
    let next: HTMLElement | undefined;
    switch (e.key) {
      case "ArrowDown":
        next = list[(i + 1) % list.length];
        break;
      case "ArrowUp":
        next = list[(i - 1 + list.length) % list.length];
        break;
      case "Home":
        next = list[0];
        break;
      case "End":
        next = list.at(-1);
        break;
      case "Escape":
        e.preventDefault();
        e.stopPropagation();
        hide(true);
        return;
      case "Tab":
        hide(false);
        return;
      default:
        return;
    }
    e.preventDefault();
    next?.focus();
  }

  function choose(item: MenuItem, e: MouseEvent): void {
    if (item.disabled) return;
    hide(e.detail === 0);
    onselect(item.id);
  }

  function onWindowPointerDown(e: PointerEvent): void {
    if (!open) return;
    const target = e.target instanceof Node ? e.target : null;
    if (target && (menu?.contains(target) || button?.contains(target))) return;
    open = false;
  }
</script>

<svelte:window onpointerdown={onWindowPointerDown} />

<div class="relative inline-flex">
  <button
    bind:this={button}
    type="button"
    class={triggerClass}
    aria-label={label}
    aria-haspopup="menu"
    aria-expanded={open}
    aria-controls={open ? menuId : undefined}
    {disabled}
    onclick={() => (open ? hide(false) : void show("first"))}
    onkeydown={onTriggerKeydown}
  >
    {@render trigger()}
  </button>
  {#if open}
    <div
      bind:this={menu}
      id={menuId}
      role="menu"
      aria-label={label}
      tabindex="-1"
      class="absolute top-full z-40 mt-1 min-w-52 rounded-md border border-line bg-panel-2 py-1 text-sm shadow-xl {align ===
      'right'
        ? 'right-0'
        : 'left-0'}"
      onkeydown={onMenuKeydown}
    >
      {#each items as item (item.id)}
        <button
          type="button"
          role="menuitem"
          tabindex="-1"
          aria-disabled={item.disabled ? "true" : undefined}
          class="block w-full px-3 py-1.5 text-left hover:bg-panel-3 focus:bg-panel-3 focus:outline-none aria-disabled:cursor-not-allowed aria-disabled:text-muted"
          onclick={(e) => choose(item, e)}
        >
          <span class="block">{item.label}</span>
          {#if item.hint}<span class="block text-xs text-muted">{item.hint}</span>{/if}
        </button>
      {/each}
    </div>
  {/if}
</div>
