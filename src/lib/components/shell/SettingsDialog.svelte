<script lang="ts">
  // App settings (AppSettings) + About. OWNER: fe-shell.
  import type { AppSettings, ProjectLocation } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { backend, BackendError } from "$lib/ipc";
  import { saveSettings } from "$lib/state/session";
  import { ui } from "$lib/state/ui.svelte";
  import { MIN_CLIP_SEC_RANGE } from "$lib/util/marks";
  import Dialog from "./Dialog.svelte";
  import { formatBytes } from "./format";

  const uid = $props.id();
  let draft = $state<AppSettings | null>(null);
  let error = $state<string | null>(null);
  let note = $state<string | null>(null);
  let busy = $state(false);

  // Fresh copy each time the dialog opens.
  $effect(() => {
    if (ui.settingsOpen) {
      draft = ui.settings ? { ...ui.settings } : null;
      error = ui.settings ? null : t("settings.unavailable");
      note = null;
    }
  });

  function close(): void {
    ui.settingsOpen = false;
  }

  async function save(): Promise<void> {
    if (!draft) return;
    busy = true;
    error = null;
    try {
      await saveSettings($state.snapshot(draft));
      close();
    } catch (e) {
      error = BackendError.from(e).message;
    } finally {
      busy = false;
    }
  }

  async function clearCache(): Promise<void> {
    busy = true;
    try {
      const freed = await backend.clearCache();
      note = t("settings.cacheCleared", { size: formatBytes(freed) });
    } catch (e) {
      error = BackendError.from(e).message;
    } finally {
      busy = false;
    }
  }

  /** §8.5: the OS setting is always honoured; this can only add reduction. */
  function setMotion(always: boolean): void {
    if (draft) draft.reduced_motion = always ? true : null;
  }

  const locations: ProjectLocation[] = ["next_to_video", "app_library"];
</script>

<Dialog open={ui.settingsOpen} onclose={close} title={t("settings.title")}>
  {#if draft}
    <form
      id="{uid}-form"
      class="flex flex-col gap-4 text-sm"
      onsubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <fieldset class="flex flex-col gap-1">
        <legend class="mb-1 font-medium">{t("settings.location")}</legend>
        {#each locations as loc (loc)}
          <label class="flex items-center gap-2">
            <input type="radio" name="{uid}-location" value={loc} bind:group={draft.project_location} />
            {t(`settings.location.${loc}`)}
          </label>
        {/each}
      </fieldset>

      <label class="flex flex-col gap-1">
        <span class="font-medium">{t("settings.minClip")}</span>
        <input
          type="number"
          class="w-32 rounded border border-line bg-panel-2 px-2 py-1"
          min={MIN_CLIP_SEC_RANGE[0]}
          max={MIN_CLIP_SEC_RANGE[1]}
          step="0.1"
          required
          bind:value={draft.min_clip_duration_sec}
          aria-describedby="{uid}-min-help"
        />
        <span id="{uid}-min-help" class="text-xs text-muted">{t("settings.minClipHelp")}</span>
      </label>

      <div class="flex flex-col gap-1">
        <label for="{uid}-cache" class="font-medium">{t("settings.cacheCap")}</label>
        <div class="flex items-center gap-2">
          <input
            id="{uid}-cache"
            type="number"
            class="w-32 rounded border border-line bg-panel-2 px-2 py-1"
            min="1"
            step="1"
            required
            bind:value={draft.cache_cap_gb}
          />
          <button type="button" class="rounded border border-line px-3 py-1 hover:bg-panel-3" disabled={busy} onclick={() => void clearCache()}>
            {t("settings.clearCache")}
          </button>
        </div>
        {#if note}<p role="status" class="text-xs text-success">{note}</p>{/if}
      </div>

      <label class="flex items-center gap-2">
        <input type="checkbox" bind:checked={draft.default_spaces_to_underscores} />
        {t("settings.spaces")}
      </label>

      <!-- Telemetry opt-in is hidden: nothing sends or reads it yet (PRD 8.4). -->
      <label class="flex items-center gap-2">
        <input type="checkbox" checked={draft.reduced_motion === true} onchange={(e) => setMotion(e.currentTarget.checked)} />
        {t("settings.motion")}
      </label>

      <label class="flex flex-col gap-1">
        <span class="font-medium">{t("settings.language")}</span>
        <select class="w-56 rounded border border-line bg-panel-2 px-2 py-1" bind:value={draft.language}>
          <option value="en">English</option>
        </select>
      </label>
    </form>
  {/if}

  {#if error}<p role="alert" class="mt-3 text-sm text-danger">{error}</p>{/if}

  {#if ui.appInfo}
    {@const info = ui.appInfo}
    <section class="mt-5 border-t border-line pt-3 text-xs text-muted" aria-labelledby="{uid}-about">
      <h3 id="{uid}-about" class="mb-1 text-sm font-medium text-text">{t("settings.about")}</h3>
      <dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5">
        <dt>{t("settings.version")}</dt>
        <dd>{info.app_version} ({t("settings.schema", { n: info.schema_version })})</dd>
        <dt>FFmpeg</dt>
        <dd>{info.ffmpeg_found ? (info.ffmpeg_version ?? t("settings.found")) : t("settings.ffmpegMissing")}</dd>
        <dt>{t("settings.dataDir")}</dt>
        <dd class="break-all select-text">{info.data_dir}</dd>
        <dt>{t("settings.logDir")}</dt>
        <dd class="break-all select-text">{info.log_dir}</dd>
        <dt>{t("settings.cacheDir")}</dt>
        <dd class="break-all select-text">{info.cache_dir}</dd>
      </dl>
    </section>
  {/if}

  {#snippet footer()}
    <button type="button" class="rounded border border-line px-3 py-1.5 text-sm hover:bg-panel-3" onclick={close}>{t("dialog.cancel")}</button>
    <button
      type="submit"
      form="{uid}-form"
      class="rounded bg-accent-strong px-3 py-1.5 text-sm font-medium text-white hover:bg-accent-hover disabled:opacity-50"
      disabled={busy || !draft}
    >
      {t("dialog.save")}
    </button>
  {/snippet}
</Dialog>
