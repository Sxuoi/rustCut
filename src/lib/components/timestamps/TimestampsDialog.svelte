<script lang="ts">
  // Timestamps dialog: export (EXP-16: YouTube chapters, CSV, CMX3600 EDL,
  // FCPXML; text preview, warnings, Copy, Save…) and import (EXP-17: pasted
  // lines or a CSV file, preview with per-line errors, then one undo step).
  import type { ImportPreview, TimestampExport, TimestampFormat } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { backend, BackendError } from "$lib/ipc";
  import { project } from "$lib/state/project.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import { formatDuration, formatTimecode } from "$lib/util/time";
  import { tp } from "$lib/queue/format";
  import { importableItems, sourceStem, timestampFileName, TIMESTAMP_FORMATS } from "$lib/queue/timestamps";
  import { btn, field } from "$lib/queue/ui";
  import Modal from "$lib/components/export/Modal.svelte";
  import Icon from "$lib/components/queue/Icon.svelte";

  type Tab = "export" | "import";
  const TABS: readonly Tab[] = ["export", "import"];
  const PARSE_DEBOUNCE_MS = 300;

  const uid = $props.id();
  const open = $derived(ui.timestampsOpen && project.loaded);

  let tab = $state<Tab>("export");
  let format = $state<TimestampFormat>("youtube_chapters");
  let result = $state.raw<TimestampExport | null>(null);
  let exportError = $state<string | null>(null);
  let exportLoading = $state(false);

  let text = $state("");
  let preview = $state.raw<ImportPreview | null>(null);
  let parseError = $state<string | null>(null);
  let parsing = $state(false);
  let importing = $state(false);
  let fileInput: HTMLInputElement | undefined = $state();

  const importable = $derived(preview ? importableItems(preview.items) : []);

  // Export text for the chosen format, refreshed whenever the project changes.
  $effect(() => {
    if (!open || tab !== "export") return;
    const f = format;
    void project.revision;
    let cancelled = false;
    exportLoading = true;
    backend
      .exportTimestamps(f)
      .then((r) => {
        if (cancelled) return;
        result = r;
        exportError = null;
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        result = null;
        exportError = BackendError.from(e).message;
      })
      .finally(() => {
        if (!cancelled) exportLoading = false;
      });
    return () => {
      cancelled = true;
    };
  });

  // Import preview of the pasted text (debounced).
  $effect(() => {
    if (!open || tab !== "import") return;
    const input = text;
    if (input.trim() === "") {
      preview = null;
      parseError = null;
      return;
    }
    let cancelled = false;
    const timer = setTimeout(() => {
      parsing = true;
      backend
        .parseTimestamps(input)
        .then((p) => {
          if (cancelled) return;
          preview = p;
          parseError = null;
        })
        .catch((e: unknown) => {
          if (cancelled) return;
          preview = null;
          parseError = BackendError.from(e).message;
        })
        .finally(() => {
          if (!cancelled) parsing = false;
        });
    }, PARSE_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  });

  function close(): void {
    ui.timestampsOpen = false;
  }

  function onTabKeydown(e: KeyboardEvent): void {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight" && e.key !== "Home" && e.key !== "End") return;
    e.preventDefault();
    const i = TABS.indexOf(tab);
    const next =
      e.key === "Home" ? 0 : e.key === "End" ? TABS.length - 1 : (i + (e.key === "ArrowRight" ? 1 : -1) + TABS.length) % TABS.length;
    tab = TABS[next];
    document.getElementById(`${uid}-tab-${tab}`)?.focus();
  }

  async function copy(): Promise<void> {
    if (!result) return;
    try {
      await navigator.clipboard.writeText(result.text);
      ui.info(t("timestamps.export.copied"));
    } catch {
      ui.error(t("timestamps.export.copyFailed"));
    }
  }

  async function save(): Promise<void> {
    if (!result) return;
    const name = timestampFileName(sourceStem(project.source?.path ?? ""), result.format, result.file_extension);
    try {
      const path = await backend.saveTextFile(name, result.text);
      if (path) ui.info(t("timestamps.export.saved", { path }));
    } catch (e) {
      ui.error(BackendError.from(e).message);
    }
  }

  async function loadFile(e: Event & { currentTarget: HTMLInputElement }): Promise<void> {
    const file = e.currentTarget.files?.[0];
    e.currentTarget.value = "";
    if (!file) return;
    try {
      text = await file.text();
    } catch {
      ui.error(t("timestamps.import.readFailed", { name: file.name }));
    }
  }

  async function doImport(): Promise<void> {
    if (importable.length === 0 || importing) return;
    importing = true;
    try {
      const patch = await backend.importTimestamps(importable);
      project.applyPatch(patch);
      ui.info(tp("timestamps.import.done", importable.length));
      text = "";
      preview = null;
      close();
    } catch (e) {
      ui.error(BackendError.from(e).message);
    } finally {
      importing = false;
    }
  }
</script>

<Modal {open} title={t("timestamps.title")} onclose={close} size="lg">
  <div role="tablist" aria-label={t("timestamps.tabs")} class="mb-3 flex gap-1 border-b border-line">
    {#each TABS as id (id)}
      <button
        type="button"
        role="tab"
        id="{uid}-tab-{id}"
        aria-selected={tab === id}
        aria-controls="{uid}-panel-{id}"
        tabindex={tab === id ? 0 : -1}
        data-autofocus={tab === id ? "" : undefined}
        class="-mb-px border-b-2 px-3 py-1.5 text-sm {tab === id
          ? 'border-accent text-text'
          : 'border-transparent text-muted hover:text-text'}"
        onclick={() => (tab = id)}
        onkeydown={onTabKeydown}
      >
        {t(`timestamps.tab.${id}`)}
      </button>
    {/each}
  </div>

  <div role="tabpanel" id="{uid}-panel-export" aria-labelledby="{uid}-tab-export" hidden={tab !== "export"} class="space-y-3">
    <div class="flex flex-wrap items-center gap-2">
      <label class="text-xs text-muted" for="{uid}-format">{t("timestamps.export.format")}</label>
      <select id="{uid}-format" class={field} bind:value={format}>
        {#each TIMESTAMP_FORMATS as f (f)}<option value={f}>{t(`timestamps.format.${f}`)}</option>{/each}
      </select>
      <span class="text-xs text-muted">{t(`timestamps.format.${format}.hint`)}</span>
    </div>
    {#if exportError}
      <p class="flex items-start gap-2 text-danger" role="alert"><Icon name="error" size={16} class="mt-0.5" />{exportError}</p>
    {/if}
    {#if result && result.warnings.length > 0}
      <ul class="space-y-1" aria-label={t("timestamps.export.warnings")}>
        {#each result.warnings as w, i (i)}
          <li class="flex items-start gap-2 text-warning">
            <Icon name="alert" size={16} class="mt-0.5" /><span class="sr-only">{t("timestamps.export.warning")}:</span>{w}
          </li>
        {/each}
      </ul>
    {/if}
    <textarea
      class="{field} tabular h-56 w-full resize-y text-xs"
      readonly
      aria-label={t("timestamps.export.preview")}
      aria-busy={exportLoading}
      value={result?.text ?? ""}
    ></textarea>
    <div class="flex justify-end gap-2">
      <button type="button" class={btn.secondary} disabled={!result} onclick={() => void copy()}>
        <Icon name="copy" size={14} />{t("timestamps.export.copy")}
      </button>
      <button type="button" class={btn.primary} disabled={!result} onclick={() => void save()}>
        <Icon name="save" size={14} />{t("timestamps.export.save")}
      </button>
    </div>
  </div>

  <div role="tabpanel" id="{uid}-panel-import" aria-labelledby="{uid}-tab-import" hidden={tab !== "import"} class="space-y-3">
    <div class="flex items-center justify-between gap-2">
      <label class="text-xs text-muted" for="{uid}-paste">{t("timestamps.import.label")}</label>
      <button type="button" class={btn.ghost} onclick={() => fileInput?.click()}>
        <Icon name="file" size={14} />{t("timestamps.import.loadFile")}
      </button>
      <input
        bind:this={fileInput}
        type="file"
        accept=".csv,.txt,text/csv,text/plain"
        class="hidden"
        tabindex="-1"
        aria-hidden="true"
        onchange={loadFile}
      />
    </div>
    <textarea
      id="{uid}-paste"
      class="{field} tabular h-32 w-full resize-y text-xs"
      placeholder={t("timestamps.import.placeholder")}
      spellcheck="false"
      bind:value={text}
    ></textarea>
    {#if parseError}
      <p class="flex items-start gap-2 text-danger" role="alert"><Icon name="error" size={16} class="mt-0.5" />{parseError}</p>
    {/if}
    {#if preview && preview.items.length > 0}
      <div class="max-h-56 overflow-auto rounded border border-line">
        <table class="w-full text-left text-xs">
          <caption class="sr-only">{t("timestamps.import.previewCaption")}</caption>
          <thead class="sticky top-0 bg-panel-2 text-muted">
            <tr>
              <th scope="col" class="px-2 py-1">{t("timestamps.import.col.line")}</th>
              <th scope="col" class="px-2 py-1">{t("timestamps.import.col.in")}</th>
              <th scope="col" class="px-2 py-1">{t("timestamps.import.col.out")}</th>
              <th scope="col" class="px-2 py-1">{t("timestamps.import.col.duration")}</th>
              <th scope="col" class="px-2 py-1">{t("timestamps.import.col.name")}</th>
              <th scope="col" class="px-2 py-1">{t("timestamps.import.col.status")}</th>
            </tr>
          </thead>
          <tbody>
            {#each preview.items as item (item.line)}
              <tr class="border-t border-line/60 {item.error ? 'text-muted' : ''}">
                <td class="tabular px-2 py-1">{item.line}</td>
                <td class="tabular px-2 py-1">{item.error ? "" : formatTimecode(item.start_sec)}</td>
                <td class="tabular px-2 py-1">{item.error ? "" : formatTimecode(item.end_sec)}</td>
                <td class="tabular px-2 py-1">{item.error ? "" : formatDuration(item.end_sec - item.start_sec)}</td>
                <td class="max-w-48 truncate px-2 py-1" title={item.name}>{item.name}</td>
                <td class="px-2 py-1">
                  {#if item.error}
                    <span class="flex items-start gap-1 text-danger"><Icon name="error" size={14} class="mt-px" />{item.error}</span>
                  {:else}
                    <span class="flex items-center gap-1 text-success"><Icon name="check" size={14} />{t("timestamps.import.ok")}</span>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
    <div class="flex items-center justify-between gap-2">
      <p class="text-xs text-muted" aria-live="polite">
        {#if parsing}
          {t("timestamps.import.parsing")}
        {:else if preview}
          {t("timestamps.import.summary", {
            valid: tp("timestamps.import.valid", importable.length),
            invalid: tp("timestamps.import.invalid", preview.items.length - importable.length),
          })}
        {/if}
      </p>
      <button
        type="button"
        class={btn.primary}
        disabled={importable.length === 0 || importing || parsing || project.readOnly !== null}
        onclick={() => void doImport()}
      >
        {tp("timestamps.import.button", importable.length)}
      </button>
    </div>
  </div>
</Modal>
