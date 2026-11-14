<script lang="ts">
  // PRJ-04 Locate missing media: pick the moved file; a different file
  // (fingerprint mismatch) needs an explicit confirmation. OWNER: fe-shell.
  import type { RelinkResult } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { backend, BackendError } from "$lib/ipc";
  import { project } from "$lib/state/project.svelte";
  import { relinkSource } from "$lib/state/session";
  import { ui } from "$lib/state/ui.svelte";
  import { formatTimecode } from "$lib/util/time";
  import Dialog from "./Dialog.svelte";
  import { basename, formatBytes } from "./format";

  type Mismatch = Extract<RelinkResult, { kind: "mismatch" }>;

  let candidate = $state<string | null>(null);
  let mismatch = $state.raw<Mismatch | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);

  const source = $derived(project.source);
  const suggested = $derived(project.snapshot?.source_status.kind === "missing" ? project.snapshot.source_status.suggested_path : null);

  $effect(() => {
    if (ui.locateMediaOpen) {
      candidate = null;
      mismatch = null;
      error = null;
    }
  });

  function close(): void {
    ui.locateMediaOpen = false;
  }

  async function tryPath(path: string, accept: boolean): Promise<void> {
    busy = true;
    error = null;
    candidate = path;
    try {
      const r = await relinkSource(path, accept, true);
      if (r?.kind === "relinked") close();
      else if (r?.kind === "mismatch") mismatch = r;
    } catch (e) {
      error = BackendError.from(e).message;
    } finally {
      busy = false;
    }
  }

  async function choose(): Promise<void> {
    let path: string | null;
    try {
      path = await backend.pickMediaFile();
    } catch (e) {
      error = BackendError.from(e).message;
      return;
    }
    if (path) {
      mismatch = null;
      await tryPath(path, false);
    }
  }
</script>

<Dialog open={ui.locateMediaOpen} onclose={close} title={t("locate.title")}>
  {#if source}
    {#if !mismatch}
      <p class="text-sm">{t("locate.intro", { name: basename(source.path) })}</p>
      <dl class="mt-3 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-sm">
        <dt class="text-muted">{t("locate.lastPath")}</dt>
        <dd class="break-all select-text">{source.path}</dd>
        <dt class="text-muted">{t("locate.size")}</dt>
        <dd class="tabular">{formatBytes(source.fingerprint.size_bytes)}</dd>
        <dt class="text-muted">{t("locate.duration")}</dt>
        <dd class="tabular">{formatTimecode(source.duration_sec)}</dd>
      </dl>
      {#if suggested}
        <p class="mt-3 text-sm">{t("locate.suggested")}</p>
        <button
          type="button"
          class="mt-1 w-full truncate rounded border border-line bg-panel-2 px-3 py-1.5 text-left text-sm hover:bg-panel-3"
          disabled={busy}
          title={suggested}
          onclick={() => void tryPath(suggested, false)}
        >
          {suggested}
        </button>
      {/if}
    {:else}
      <div role="alert" class="rounded border border-warning/60 bg-warning/10 p-3 text-sm">
        <p class="font-semibold text-warning">{t("locate.mismatchTitle")}</p>
        <p class="mt-1 break-all">{candidate}</p>
        <ul class="mt-2 list-disc pl-5">
          <li>{t("locate.mismatchSize", { expected: formatBytes(mismatch.expected.size_bytes), found: formatBytes(mismatch.found.size_bytes) })}</li>
          <li>{t("locate.mismatchDuration", { expected: formatTimecode(source.duration_sec), found: formatTimecode(mismatch.found_duration_sec) })}</li>
          {#if mismatch.clips_out_of_range > 0}
            <li class="text-danger">{t("locate.outOfRange", { count: mismatch.clips_out_of_range })}</li>
          {/if}
        </ul>
        <p class="mt-2">{t("locate.mismatchQuestion")}</p>
      </div>
    {/if}
  {/if}
  {#if busy}<p role="status" class="mt-3 text-sm text-muted">{t("locate.checking")}</p>{/if}
  {#if error}<p role="alert" class="mt-3 text-sm text-danger">{error}</p>{/if}

  {#snippet footer()}
    <button type="button" class="rounded border border-line px-3 py-1.5 text-sm hover:bg-panel-3" onclick={close}>{t("dialog.cancel")}</button>
    {#if mismatch && candidate}
      {@const path = candidate}
      <button type="button" class="rounded border border-line px-3 py-1.5 text-sm hover:bg-panel-3" disabled={busy} onclick={() => void choose()}>
        {t("locate.chooseAnother")}
      </button>
      <button
        type="button"
        class="rounded bg-warning px-3 py-1.5 text-sm font-medium text-black hover:opacity-90 disabled:opacity-50"
        disabled={busy}
        onclick={() => void tryPath(path, true)}
      >
        {t("locate.useAnyway")}
      </button>
    {:else}
      <button
        type="button"
        class="rounded bg-accent-strong px-3 py-1.5 text-sm font-medium text-white hover:bg-accent-hover disabled:opacity-50"
        disabled={busy}
        onclick={() => void choose()}
      >
        {t("locate.choose")}
      </button>
    {/if}
  {/snippet}
</Dialog>
