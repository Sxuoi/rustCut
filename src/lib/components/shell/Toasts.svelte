<script lang="ts">
  // Toasts (ui.toasts): announced politely; errors as alerts (§8.5).
  // OWNER: fe-shell.
  import { t } from "$lib/i18n";
  import { ui } from "$lib/state/ui.svelte";

  const TONE: Record<string, string> = {
    info: "border-line",
    success: "border-success/60",
    warning: "border-warning/70",
    error: "border-danger/80",
  };
</script>

<section aria-label={t("toast.region")} class="pointer-events-none fixed right-4 bottom-4 z-40 flex w-96 max-w-[calc(100vw-2rem)] flex-col gap-2">
  <div aria-live="polite" aria-relevant="additions text" class="flex flex-col gap-2">
    {#each ui.toasts as toast (toast.id)}
      <div
        role={toast.level === "error" ? "alert" : "status"}
        class="pointer-events-auto flex items-start gap-3 rounded-md border-l-4 bg-panel-2 px-3 py-2 text-sm shadow-lg {TONE[toast.level] ??
          'border-line'}"
      >
        <span class="sr-only">{t(`toast.level.${toast.level}`)}</span>
        <p class="flex-1 break-words">{toast.message}</p>
        <button
          type="button"
          class="-mr-1 rounded px-1.5 text-muted hover:bg-panel-3 hover:text-text"
          aria-label={t("toast.dismiss")}
          onclick={() => ui.dismissToast(toast.id)}
        >
          ×
        </button>
      </div>
    {/each}
  </div>
</section>
