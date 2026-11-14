<script lang="ts" module>
  /** One entry of a dropdown menu. */
  export interface MenuItem {
    id: string;
    label: string;
    /** Shortcut shown on the right ("Ctrl+O"), from the shortcut table. */
    shortcut?: string;
    /** `aria-keyshortcuts` value. */
    keys?: string;
    disabled?: boolean;
    /** Radio item state (preset, speed); omit for plain commands. */
    checked?: boolean;
    separatorBefore?: boolean;
    onSelect: () => void;
  }
</script>

<script lang="ts">
  // Accessible menu button (WAI-ARIA menu pattern): ↑/↓/Home/End move, Enter
  // or Space activates, Esc closes and returns focus, ←/→ switch menus in the
  // menu bar, typing a letter jumps to an item. OWNER: fe-shell.
  import { tick } from "svelte";

  interface Props {
    /** Visible button text. */
    label: string;
    /** Accessible name when the visible label is not enough. */
    ariaLabel?: string;
    items: MenuItem[];
    open: boolean;
    onOpenChange: (open: boolean) => void;
    /** ←/→ inside the open menu (menu bar navigation). */
    onNavigate?: (dir: 1 | -1) => void;
    buttonClass?: string;
    align?: "left" | "right";
    /** Open upwards (menus at the bottom of the window). */
    up?: boolean;
    disabled?: boolean;
    title?: string;
  }

  let {
    label,
    ariaLabel,
    items,
    open,
    onOpenChange,
    onNavigate,
    buttonClass = "",
    align = "left",
    up = false,
    disabled = false,
    title,
  }: Props = $props();

  const uid = $props.id();
  let root: HTMLDivElement | undefined = $state();
  let button: HTMLButtonElement | undefined = $state();
  let menu: HTMLDivElement | undefined = $state();
  let focusLastOnOpen = false;

  function enabledItems(): HTMLElement[] {
    return menu ? Array.from(menu.querySelectorAll<HTMLElement>('[data-menu-item]:not([aria-disabled="true"])')) : [];
  }

  function focusAt(index: number): void {
    const els = enabledItems();
    if (els.length === 0) {
      menu?.focus();
      return;
    }
    els[((index % els.length) + els.length) % els.length].focus();
  }

  $effect(() => {
    if (!open) return;
    void tick().then(() => {
      const els = enabledItems();
      const checked = els.findIndex((el) => el.getAttribute("aria-checked") === "true");
      focusAt(focusLastOnOpen ? els.length - 1 : checked >= 0 ? checked : 0);
      focusLastOnOpen = false;
    });
    const onPointerDown = (e: PointerEvent): void => {
      if (root && e.target instanceof Node && !root.contains(e.target)) onOpenChange(false);
    };
    window.addEventListener("pointerdown", onPointerDown, true);
    return () => window.removeEventListener("pointerdown", onPointerDown, true);
  });

  function close(refocus: boolean): void {
    onOpenChange(false);
    if (refocus) button?.focus();
  }

  function activate(item: MenuItem): void {
    if (item.disabled) return;
    close(true);
    item.onSelect();
  }

  function onButtonKeydown(e: KeyboardEvent): void {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      focusLastOnOpen = e.key === "ArrowUp";
      onOpenChange(true);
    }
  }

  function typeAhead(char: string, from: number): void {
    const els = enabledItems();
    const c = char.toLowerCase();
    for (let k = 1; k <= els.length; k++) {
      const el = els[(from + k) % els.length];
      if ((el.dataset.label ?? "").toLowerCase().startsWith(c)) {
        el.focus();
        return;
      }
    }
  }

  function onMenuKeydown(e: KeyboardEvent): void {
    const els = enabledItems();
    const i = els.indexOf(document.activeElement as HTMLElement);
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        focusAt(i + 1);
        return;
      case "ArrowUp":
        e.preventDefault();
        focusAt(i - 1);
        return;
      case "Home":
        e.preventDefault();
        focusAt(0);
        return;
      case "End":
        e.preventDefault();
        focusAt(els.length - 1);
        return;
      case "Escape":
        e.preventDefault();
        close(true);
        return;
      case "Tab":
        close(false);
        return;
      case "ArrowLeft":
      case "ArrowRight":
        e.preventDefault();
        onNavigate?.(e.key === "ArrowRight" ? 1 : -1);
        return;
      case "Enter":
      case " ":
        e.preventDefault();
        if (i >= 0) els[i].click();
        return;
      default:
        // Printable keys never reach the global shortcuts while a menu is open.
        if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
          e.preventDefault();
          typeAhead(e.key, i);
        }
    }
  }
</script>

<div class="relative" bind:this={root}>
  <button
    bind:this={button}
    type="button"
    id="{uid}-button"
    class="rounded px-2 py-1 text-sm hover:bg-panel-3 aria-expanded:bg-panel-3 disabled:opacity-50 {buttonClass}"
    aria-haspopup="menu"
    aria-expanded={open}
    aria-controls={open ? `${uid}-menu` : undefined}
    aria-label={ariaLabel}
    {title}
    {disabled}
    onclick={() => onOpenChange(!open)}
    onkeydown={onButtonKeydown}
  >
    {label}
  </button>
  {#if open}
    <div
      bind:this={menu}
      id="{uid}-menu"
      role="menu"
      tabindex="-1"
      aria-labelledby="{uid}-button"
      class="absolute z-50 min-w-60 rounded-md border border-line bg-panel-2 py-1 shadow-xl {align === 'right'
        ? 'right-0'
        : 'left-0'} {up ? 'bottom-full mb-1' : 'top-full mt-1'}"
      onkeydown={onMenuKeydown}
    >
      {#each items as item (item.id)}
        {#if item.separatorBefore}
          <div role="separator" class="my-1 border-t border-line"></div>
        {/if}
        <button
          type="button"
          data-menu-item
          data-label={item.label}
          role={item.checked === undefined ? "menuitem" : "menuitemradio"}
          aria-checked={item.checked === undefined ? undefined : item.checked}
          aria-disabled={item.disabled ? "true" : undefined}
          aria-keyshortcuts={item.keys || undefined}
          tabindex="-1"
          class="flex w-full items-center gap-4 px-3 py-1.5 text-left text-sm outline-none focus-visible:bg-panel-3 focus:bg-panel-3 hover:bg-panel-3 aria-disabled:cursor-default aria-disabled:text-muted aria-disabled:hover:bg-transparent"
          onclick={() => activate(item)}
        >
          <span class="w-3 text-accent" aria-hidden="true">{item.checked ? "✓" : ""}</span>
          <span class="flex-1">{item.label}</span>
          {#if item.shortcut}<span class="tabular text-xs text-muted">{item.shortcut}</span>{/if}
        </button>
      {/each}
    </div>
  {/if}
</div>
