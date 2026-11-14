<script lang="ts" module>
  /** Icons used by the queue, export, tag and timestamp components. */
  export type IconName =
    | "search"
    | "trash"
    | "folder"
    | "more"
    | "chevron"
    | "x"
    | "check"
    | "alert"
    | "error"
    | "retry"
    | "up"
    | "down"
    | "copy"
    | "save"
    | "plus"
    | "info"
    | "file";

  const PATHS: Record<IconName, string> = {
    search: "M17 10.5a6.5 6.5 0 1 1-13 0a6.5 6.5 0 0 1 13 0z M20 20l-4.8-4.8",
    trash: "M4 7h16 M10 11v6 M14 11v6 M6 7l1 13h10l1-13 M9 7V4h6v3",
    folder: "M3 6.5h6l2 2h10V19H3z",
    more: "M5 12h.01 M12 12h.01 M19 12h.01",
    chevron: "M6 9l6 6 6-6",
    x: "M6 6l12 12 M18 6L6 18",
    check: "M5 12.5l4.5 4.5L19 7",
    alert: "M12 3.5l9.5 16.5h-19z M12 10v4.5 M12 17.5h.01",
    error: "M21 12a9 9 0 1 1-18 0a9 9 0 0 1 18 0z M9 9l6 6 M15 9l-6 6",
    retry: "M4 12a8 8 0 0 1 13.7-5.6L20 8.5 M20 4v4.5h-4.5 M20 12a8 8 0 0 1-13.7 5.6L4 15.5 M4 20v-4.5h4.5",
    up: "M12 19V5 M6 11l6-6 6 6",
    down: "M12 5v14 M18 13l-6 6-6-6",
    copy: "M9 9h11v11H9z M15 9V4H4v11h5",
    save: "M12 4v11 M7 10.5l5 5 5-5 M5 20h14",
    plus: "M12 5v14 M5 12h14",
    info: "M21 12a9 9 0 1 1-18 0a9 9 0 0 1 18 0z M12 11v5 M12 7.5h.01",
    file: "M6 3h8l4 4v14H6z M14 3v4h4",
  };
</script>

<script lang="ts">
  let { name, size = 16, class: klass = "" }: { name: IconName; size?: number; class?: string } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width={name === "more" ? 3 : 2}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  focusable="false"
  class="shrink-0 {klass}"
>
  <path d={PATHS[name]} />
</svg>
