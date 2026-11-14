// Shared Tailwind class strings of the queue, export, tag and timestamp
// components, so buttons and fields look the same everywhere. Colours come
// from the theme tokens in app.css; text on accent backgrounds uses the dark
// background colour (≥ 4.5:1, §8.5).

export const btn = {
  primary:
    "inline-flex items-center justify-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-semibold text-bg hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-50",
  secondary:
    "inline-flex items-center justify-center gap-1.5 rounded-md border border-line px-3 py-1.5 text-xs text-text hover:bg-panel-3 disabled:cursor-not-allowed disabled:opacity-50",
  danger:
    "inline-flex items-center justify-center gap-1.5 rounded-md border border-danger/60 px-3 py-1.5 text-xs text-danger hover:bg-danger/10 disabled:cursor-not-allowed disabled:opacity-50",
  ghost:
    "inline-flex items-center justify-center gap-1 rounded px-1.5 py-1 text-xs text-muted hover:bg-panel-3 hover:text-text disabled:cursor-not-allowed disabled:opacity-50",
  icon: "inline-flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted hover:bg-panel-3 hover:text-text disabled:cursor-not-allowed disabled:opacity-40",
} as const;

export const field =
  "rounded-md border border-line bg-bg px-2 py-1 text-sm text-text placeholder:text-muted focus:border-accent disabled:opacity-50";

export const label = "text-xs font-medium text-muted";
