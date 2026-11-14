<script lang="ts">
  // §6.4 Empty (drop zone, Open video, recent projects) and Probing
  // (spinner with the file name). OWNER: fe-shell.
  import { t } from "$lib/i18n";
  import { isTauri } from "$lib/ipc";
  import { ariaKeyShortcuts, shortcutLabel } from "$lib/keyboard/shortcuts";
  import { openBrowserFile, openPath, openWithDialog } from "$lib/state/session";
  import { ui } from "$lib/state/ui.svelte";
  import { basename } from "./format";

  const uid = $props.id();
  const inBrowser = !isTauri();
  let fileInput: HTMLInputElement | undefined = $state();

  function openedAt(iso: string): string {
    const ms = Date.parse(iso);
    return Number.isFinite(ms) ? new Date(ms).toLocaleString() : "";
  }

  function onFileChosen(e: Event & { currentTarget: HTMLInputElement }): void {
    const file = e.currentTarget.files?.[0];
    e.currentTarget.value = "";
    if (file) void openBrowserFile(file);
  }
</script>

{#if ui.phase === "probing"}
  <div class="flex h-full flex-col items-center justify-center gap-4" role="status" aria-live="polite">
    <div class="size-10 animate-spin rounded-full border-4 border-panel-3 border-t-accent" aria-hidden="true"></div>
    <p class="text-sm">{t("empty.probing", { name: ui.probingName ?? "" })}</p>
  </div>
{:else}
  <div class="flex h-full items-center justify-center overflow-auto p-6">
    <div class="flex w-full max-w-2xl flex-col gap-6">
      <section
        aria-labelledby="{uid}-title"
        class="flex flex-col items-center gap-3 rounded-xl border-2 border-dashed px-6 py-12 text-center transition-colors {ui.dragOver
          ? 'border-accent bg-accent/10'
          : 'border-line bg-panel'}"
      >
        <h1 id="{uid}-title" class="text-xl font-semibold">{t("empty.title")}</h1>
        <p class="text-sm text-muted">{ui.dragOver ? t("empty.dropNow") : t("empty.dropHint")}</p>
        <button
          type="button"
          class="mt-2 rounded-md bg-accent-strong px-5 py-2 font-medium text-white hover:bg-accent-hover"
          aria-keyshortcuts={ariaKeyShortcuts("open", ui.platform)}
          onclick={() => void openWithDialog()}
        >
          {t("empty.open")}
          <span class="ml-2 text-xs opacity-80">{shortcutLabel("open", ui.platform)}</span>
        </button>
        {#if inBrowser}
          <input
            bind:this={fileInput}
            id="{uid}-file"
            type="file"
            class="sr-only"
            accept="video/*,.mkv,.flv,.ts,.m2ts,.mov,.rustcut"
            onchange={onFileChosen}
          />
          <button type="button" class="text-sm text-accent underline-offset-2 hover:underline" onclick={() => fileInput?.click()}>
            {t("empty.browseLocal")}
          </button>
        {/if}
        <p class="text-xs text-muted">{t("empty.formats")}</p>
      </section>

      {#if ui.recent.length > 0}
        <section aria-labelledby="{uid}-recent">
          <h2 id="{uid}-recent" class="mb-2 text-sm font-semibold text-muted">{t("empty.recent")}</h2>
          <ul class="flex flex-col gap-1">
            {#each ui.recent as r (r.project_path)}
              <li>
                <button
                  type="button"
                  class="flex w-full items-center gap-3 rounded-md border border-line bg-panel px-3 py-2 text-left hover:bg-panel-2"
                  title={r.project_path}
                  onclick={() => void openPath(r.project_path, r.source_name || basename(r.source_path))}
                >
                  <span class="min-w-0 flex-1 truncate font-medium">{r.source_name || basename(r.source_path)}</span>
                  <span class="text-xs text-muted">{t("empty.clipCount", { count: r.clip_count })}</span>
                  {#if r.location === "app_library"}
                    <span class="rounded bg-panel-3 px-1.5 text-xs text-muted">{t("empty.inLibrary")}</span>
                  {/if}
                  <span class="text-xs text-muted">{openedAt(r.opened_at)}</span>
                </button>
              </li>
            {/each}
          </ul>
        </section>
      {/if}
    </div>
  </div>
{/if}
