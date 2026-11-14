<script lang="ts">
  // Export dialog (§6.2, EXP-01): preset, per-preset options (Lossless:
  // container, lead-in, AAC offer, snap In; re-encode: resolution, frame rate,
  // audio track / mix, encoder, parallel jobs; vertical: platform maximum),
  // folder, file-name template with live preview, If-exists policy and the
  // pre-flight list whose errors block Start (EXP-11). Settings persist via
  // project.setExportSettings as they change; Enter starts, Escape closes.
  import type { ConflictPolicy, EncoderChoice, ExportSettings, OutputResolution, Preset } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { backend, BackendError } from "$lib/ipc";
  import { project } from "$lib/state/project.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import { exporter } from "$lib/state/exporter.svelte";
  import { formatBytes, formatLeadIn, formatSeconds, tp } from "$lib/queue/format";
  import { formatClock, parseTimecode } from "$lib/queue/timecode";
  import { includedClips } from "$lib/queue/rows";
  import {
    applyPlatform,
    applyPreset,
    clampJobs,
    CONFLICT_POLICIES,
    CUSTOM_PLATFORM,
    defaultParallelJobs,
    encoderOptions,
    FPS_CAPS,
    insertToken,
    isReencodePreset,
    isVerticalPreset,
    leadInSummary,
    PARALLEL_JOBS,
    platformPresets,
    platformById,
    PRESETS,
    presetOf,
    previewNames,
    resolveAutoEncoder,
    RESOLUTIONS,
    setMaxDuration,
    TEMPLATE_VARIABLES,
    templateHasExtension,
    unknownVariables,
    type EncoderOption,
  } from "$lib/queue/exportOptions";
  import { btn, field, label } from "$lib/queue/ui";
  import Modal from "./Modal.svelte";
  import PreflightList from "./PreflightList.svelte";
  import Icon from "$lib/components/queue/Icon.svelte";

  /** Debounces: pre-flight after a change, persisting typed text. */
  const PREVIEW_DEBOUNCE_MS = 250;
  const PERSIST_DEBOUNCE_MS = 400;

  const uid = $props.id();
  const open = $derived(ui.exportDialogOpen && project.loaded);

  /** Local copy of the settings; replaced on every change. */
  let draft = $state.raw<ExportSettings | null>(null);
  /** Local edits not yet sent to the backend (keeps the draft from being overwritten). */
  let dirty = false;
  let persistTimer: ReturnType<typeof setTimeout> | null = null;
  let previewSeq = 0;
  let previewPending = $state(false);
  let templateInput: HTMLInputElement | undefined = $state();
  let maxText = $state("");
  let maxError = $state(false);
  let nativeSize = $state<string | null>(null);

  const ids = $derived(includedClips(project.clips).map((c) => c.id));
  const preset = $derived<Preset>(draft ? presetOf(draft) : "lossless");
  const vertical = $derived(isVerticalPreset(preset));
  const reencode = $derived(isReencodePreset(preset));
  const hw = $derived(exporter.hwEncoders ?? []);
  const encoders = $derived(draft ? encoderOptions(hw, draft.encoder) : []);
  const autoEncoder = $derived(resolveAutoEncoder(hw));
  const jobsDefault = $derived(
    draft ? defaultParallelJobs({ preset, encoder: draft.encoder, hw, cpuCount: exporter.cpuCount }) : 1,
  );
  const report = $derived(exporter.report);
  const names = $derived(report ? previewNames(report.outputs) : { shown: [], more: 0 });
  const leadIn = $derived(report ? leadInSummary(report.outputs) : null);
  const audioTracks = $derived(project.source?.audio_tracks ?? []);
  const unknownVars = $derived(draft ? unknownVariables(draft.naming_template) : []);
  const hasExtension = $derived(draft ? templateHasExtension(draft.naming_template) : false);
  const checking = $derived(previewPending || exporter.reportLoading);
  const canStart = $derived(
    draft !== null &&
      report !== null &&
      report.can_start &&
      !checking &&
      !exporter.running &&
      !exporter.starting &&
      ids.length > 0,
  );
  const startBlocker = $derived(
    exporter.running
      ? t("export.dialog.busy")
      : ids.length === 0
        ? t("export.dialog.nothing")
        : checking
          ? t("export.preflight.checking")
          : report && !report.can_start
            ? t("export.dialog.blocked")
            : null,
  );

  // Mirror the project's settings while there are no local edits in flight.
  $effect(() => {
    const s = project.settings;
    if (!open || !s || dirty) return;
    draft = s;
    maxText = s.max_duration_sec !== null ? formatClock(s.max_duration_sec) : "";
    maxError = false;
  });

  // Encoder list (EXP-13) and CPU count (EXP-12 defaults), loaded once.
  $effect(() => {
    if (!open) return;
    void exporter.loadEncoders();
    void exporter.loadCpuCount();
  });

  // FRM-06 Native size = crop window size (same Rust function as the export).
  $effect(() => {
    const video = project.source?.video;
    if (!open || !vertical || !video) return;
    let cancelled = false;
    backend
      .cropWindow(video.display_width, video.display_height, 0)
      .then((w) => {
        if (!cancelled) nativeSize = `${w.w}×${w.h}`;
      })
      .catch(() => {
        if (!cancelled) nativeSize = null;
      });
    return () => {
      cancelled = true;
    };
  });

  // Pre-flight (EXP-11) for the included clips with the draft settings.
  $effect(() => {
    const settings = draft;
    const clipIds = ids;
    if (!open || !settings) return;
    const seq = ++previewSeq;
    previewPending = true;
    const timer = setTimeout(() => {
      void exporter.preview(clipIds, settings).finally(() => {
        if (seq === previewSeq) previewPending = false;
      });
    }, PREVIEW_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  });

  function update(patch: Partial<ExportSettings>, delay = 0): void {
    if (!draft) return;
    draft = { ...draft, ...patch };
    dirty = true;
    if (persistTimer) clearTimeout(persistTimer);
    persistTimer = setTimeout(() => void persist(), delay);
  }

  async function persist(): Promise<void> {
    if (persistTimer) {
      clearTimeout(persistTimer);
      persistTimer = null;
    }
    if (!dirty || !draft) return;
    const settings = draft;
    dirty = false;
    await project.setExportSettings(settings);
  }

  function setPreset(p: Preset): void {
    if (!draft) return;
    const next = applyPreset(draft, p);
    update({
      ...next,
      parallel_jobs: defaultParallelJobs({ preset: p, encoder: next.encoder, hw, cpuCount: exporter.cpuCount }),
    });
  }

  function setEncoder(value: string): void {
    const option = encoders.find((o) => o.value === value);
    if (!option) return;
    const encoder: EncoderChoice = option.value;
    update({ encoder, parallel_jobs: defaultParallelJobs({ preset, encoder, hw, cpuCount: exporter.cpuCount }) });
  }

  function encoderLabel(o: EncoderOption): string {
    if (o.value === "auto") {
      const resolved = hw.find((e) => e.name === autoEncoder)?.label ?? t("export.encoder.libx264");
      return t("export.encoder.auto", { encoder: resolved });
    }
    if (o.value === "libx264") return t("export.encoder.libx264");
    const name = o.label ?? o.value;
    return o.available ? name : t("export.encoder.unavailable", { encoder: name });
  }

  function fpsLabel(cap: number | null): string {
    return cap === null ? t("export.fps.source") : t("export.fps.cap", { fps: cap });
  }

  function resolutionLabel(r: OutputResolution): string {
    if (r === "native") return nativeSize ? t("export.resolution.nativeSize", { size: nativeSize }) : t("export.resolution.native");
    return r.replace("x", "×");
  }

  function trackLabel(i: number): string {
    const track = audioTracks[i];
    const name = track.title ?? track.language ?? track.codec;
    return t("export.audio.track", { n: i + 1, name, channels: track.channels });
  }

  function onTemplateInput(e: Event & { currentTarget: HTMLInputElement }): void {
    update({ naming_template: e.currentTarget.value }, PERSIST_DEBOUNCE_MS);
  }

  function insertVariable(name: string): void {
    const input = templateInput;
    if (!input) return;
    const end = input.value.length;
    const r = insertToken(input.value, input.selectionStart ?? end, input.selectionEnd ?? end, `{${name}}`);
    input.value = r.text;
    update({ naming_template: r.text }, PERSIST_DEBOUNCE_MS);
    input.focus();
    input.setSelectionRange(r.caret, r.caret);
  }

  function onFolderInput(e: Event & { currentTarget: HTMLInputElement }): void {
    const value = e.currentTarget.value.trim();
    update({ output_dir: value === "" ? null : value }, PERSIST_DEBOUNCE_MS);
  }

  async function browse(): Promise<void> {
    if (!draft) return;
    try {
      const dir = await backend.pickOutputDir(draft.output_dir ?? report?.output_dir ?? null);
      if (dir) update({ output_dir: dir });
    } catch (e) {
      ui.error(BackendError.from(e).message);
    }
  }

  function onPlatformChange(e: Event & { currentTarget: HTMLSelectElement }): void {
    if (!draft) return;
    const value = e.currentTarget.value;
    const next = applyPlatform(draft, value === "" ? null : value);
    maxText = next.max_duration_sec !== null ? formatClock(next.max_duration_sec) : "";
    maxError = false;
    update(next);
  }

  function onMaxInput(e: Event & { currentTarget: HTMLInputElement }): void {
    if (!draft) return;
    maxText = e.currentTarget.value;
    const text = maxText.trim();
    if (text === "") {
      maxError = false;
      update(setMaxDuration(draft, null), PERSIST_DEBOUNCE_MS);
      return;
    }
    const sec = parseTimecode(text);
    maxError = sec === null || sec < 1;
    if (!maxError && sec !== null) update(setMaxDuration(draft, Math.round(sec)), PERSIST_DEBOUNCE_MS);
  }

  /** Enter pressed before the pre-flight finished: start as soon as it passes. */
  let pendingStart = $state(false);
  const waiting = $derived(
    draft !== null &&
      ids.length > 0 &&
      !exporter.running &&
      !exporter.starting &&
      (report === null || checking) &&
      !exporter.reportError,
  );

  $effect(() => {
    if (!pendingStart) return;
    if (canStart) {
      pendingStart = false;
      void start();
    } else if (!waiting) {
      // Blocking result, error, or batch already running: drop the queued start.
      pendingStart = false;
    }
  });

  async function start(): Promise<void> {
    if (!canStart || !draft) return;
    const settings = draft;
    await persist();
    const batch = await exporter.start(ids, settings);
    if (batch) ui.exportDialogOpen = false;
  }

  function close(): void {
    pendingStart = false;
    void persist();
    ui.exportDialogOpen = false;
  }

  /** Enter starts when allowed (buttons, selects and textareas keep their own Enter). */
  function onKeydown(e: KeyboardEvent): void {
    if (e.key !== "Enter" || e.isComposing || e.defaultPrevented) return;
    const target = e.target instanceof HTMLElement ? e.target : null;
    if (target && ["BUTTON", "SELECT", "TEXTAREA", "A"].includes(target.tagName)) return;
    e.preventDefault();
    if (canStart) void start();
    else if (waiting) pendingStart = true;
  }

  const PRESET_KEYS: Record<Preset, string> = {
    lossless: "export.preset.lossless",
    accurate: "export.preset.accurate",
    vcrop: "export.preset.vcrop",
    vblur: "export.preset.vblur",
  };
  const POLICY_KEYS: Record<ConflictPolicy, string> = {
    rename: "export.conflict.rename",
    overwrite: "export.conflict.overwrite",
    skip: "export.conflict.skip",
  };
  const row = "grid grid-cols-[7.5rem_minmax(0,1fr)] items-start gap-x-3 gap-y-1";
</script>

<Modal open={open && draft !== null} title={tp("export.dialog.title", ids.length)} onclose={close} onkeydown={onKeydown} size="xl">
  {#if draft}
    <div class="space-y-4">
      <!-- Preset -->
      <div class={row} role="radiogroup" aria-labelledby="{uid}-preset-label">
        <span id="{uid}-preset-label" class="{label} pt-1">{t("export.preset.label")}</span>
        <div class="grid grid-cols-2 gap-x-4 gap-y-1">
          {#each PRESETS as p (p)}
            <label class="flex items-center gap-2 py-0.5">
              <input
                type="radio"
                name="{uid}-preset"
                value={p}
                checked={preset === p}
                class="accent-accent"
                data-autofocus={preset === p ? "" : undefined}
                onchange={() => setPreset(p)}
              />
              <span>{t(PRESET_KEYS[p])}</span>
            </label>
          {/each}
          <p class="col-span-2 text-xs text-muted">{t(`export.preset.${preset}.hint`)}</p>
        </div>
      </div>

      {#if preset === "lossless"}
        <!-- Lossless: container + lead-in information instead of resolution and encoder -->
        <div class={row}>
          <span class="{label} pt-0.5">{t("export.lossless.label")}</span>
          <div class="space-y-1.5">
            <p>
              {t("export.lossless.container", { container: (report?.extension ?? "…").toUpperCase() })}
              <span class="text-muted">{t("export.lossless.containerHint")}</span>
            </p>
            <p class="text-muted">
              {#if leadIn && leadIn.maxSec !== null}
                {t("export.lossless.leadIn", { max: formatLeadIn(leadIn.maxSec) })}
              {:else}
                {t("export.lossless.leadInUnknown")}
              {/if}
              {#if leadIn && leadIn.unknown > 0 && leadIn.maxSec !== null}
                {tp("export.lossless.leadInSomeUnknown", leadIn.unknown)}
              {/if}
            </p>
            {#if report?.offer_aac_audio || draft.lossless_audio_to_aac}
              <label class="flex items-center gap-2">
                <input
                  type="checkbox"
                  class="accent-accent"
                  checked={draft.lossless_audio_to_aac}
                  onchange={(e) => update({ lossless_audio_to_aac: e.currentTarget.checked })}
                />
                {t("export.lossless.aac")}
              </label>
            {/if}
            <label class="flex items-start gap-2">
              <input
                type="checkbox"
                class="mt-0.5 accent-accent"
                checked={draft.snap_in_to_keyframe}
                onchange={(e) => update({ snap_in_to_keyframe: e.currentTarget.checked })}
              />
              <span>
                {t("export.lossless.snap")}
                <span class="block text-xs text-muted">{t("export.lossless.snapHint")}</span>
              </span>
            </label>
          </div>
        </div>
      {/if}

      {#if reencode}
        <div class={row}>
          <span class="{label} pt-1.5">{t("export.output.label")}</span>
          <div class="flex flex-wrap items-center gap-x-4 gap-y-2">
            {#if vertical}
              <label class="flex items-center gap-1.5">
                <span class="text-xs text-muted">{t("export.resolution.label")}</span>
                <select
                  class={field}
                  value={draft.output_resolution}
                  onchange={(e) => update({ output_resolution: e.currentTarget.value as OutputResolution })}
                >
                  {#each RESOLUTIONS as r (r)}<option value={r}>{resolutionLabel(r)}</option>{/each}
                </select>
              </label>
            {/if}
            <label class="flex items-center gap-1.5">
              <span class="text-xs text-muted">{t("export.fps.label")}</span>
              <select
                class={field}
                value={draft.fps_cap === null ? "" : String(draft.fps_cap)}
                onchange={(e) => update({ fps_cap: e.currentTarget.value === "" ? null : Number(e.currentTarget.value) })}
              >
                {#each FPS_CAPS as cap (cap)}
                  <option value={cap === null ? "" : String(cap)}>{fpsLabel(cap)}</option>
                {/each}
                {#if draft.fps_cap !== null && !FPS_CAPS.includes(draft.fps_cap)}
                  <option value={String(draft.fps_cap)}>{fpsLabel(draft.fps_cap)}</option>
                {/if}
              </select>
            </label>
            {#if vertical}
              <label class="flex items-center gap-1.5">
                <span class="text-xs text-muted">{t("export.audio.label")}</span>
                {#if audioTracks.length === 0}
                  <span class="text-muted">{t("export.audio.none")}</span>
                {:else}
                  <select
                    class={field}
                    value={String(draft.audio_track)}
                    disabled={draft.mix_all_audio}
                    onchange={(e) => update({ audio_track: Number(e.currentTarget.value) })}
                  >
                    {#each audioTracks as _track, i (i)}<option value={String(i + 1)}>{trackLabel(i)}</option>{/each}
                  </select>
                {/if}
              </label>
              {#if audioTracks.length > 1}
                <label class="flex items-center gap-1.5">
                  <input
                    type="checkbox"
                    class="accent-accent"
                    checked={draft.mix_all_audio}
                    onchange={(e) => update({ mix_all_audio: e.currentTarget.checked })}
                  />
                  {t("export.audio.mixAll")}
                </label>
              {/if}
            {/if}
          </div>
        </div>

        <div class={row}>
          <span class="{label} pt-1.5">{t("export.encoder.label")}</span>
          <div class="flex flex-wrap items-center gap-2">
            <select
              class={field}
              aria-label={t("export.encoder.label")}
              value={draft.encoder}
              onchange={(e) => setEncoder(e.currentTarget.value)}
            >
              {#each encoders as o (o.value)}
                <option value={o.value} disabled={!o.available} title={o.reason ?? undefined}>{encoderLabel(o)}</option>
              {/each}
            </select>
            <button
              type="button"
              class={btn.ghost}
              disabled={exporter.hwLoading}
              onclick={() => void exporter.loadEncoders(true)}
            >
              <Icon name="retry" size={12} />{exporter.hwLoading ? t("export.encoder.detecting") : t("export.encoder.redetect")}
            </button>
          </div>
        </div>
      {/if}

      <div class={row}>
        <label class="{label} pt-1.5" for="{uid}-jobs">{t("export.jobs.label")}</label>
        <div class="flex items-center gap-2">
          <select
            id="{uid}-jobs"
            class={field}
            value={String(clampJobs(draft.parallel_jobs))}
            onchange={(e) => update({ parallel_jobs: clampJobs(Number(e.currentTarget.value)) })}
          >
            {#each PARALLEL_JOBS as n (n)}<option value={String(n)}>{n}</option>{/each}
          </select>
          <span class="text-xs text-muted">{t(`export.jobs.hint.${preset === "lossless" ? "lossless" : "reencode"}`, { n: jobsDefault })}</span>
        </div>
      </div>

      {#if vertical}
        <!-- FRM-08 platform maximum duration -->
        <div class={row}>
          <label class="{label} pt-1.5" for="{uid}-platform">{t("export.platform.label")}</label>
          <div class="space-y-1">
            <div class="flex flex-wrap items-center gap-2">
              <select id="{uid}-platform" class={field} value={draft.platform_preset ?? ""} onchange={onPlatformChange}>
                <option value="">{t("export.platform.none")}</option>
                {#each $platformPresets as p (p.id)}
                  <option value={p.id}>{p.maxDurationSec === null ? p.label : t("export.platform.option", { name: p.label, max: formatClock(p.maxDurationSec) })}</option>
                {/each}
                {#if draft.platform_preset !== null && !platformById(draft.platform_preset)}
                  <option value={draft.platform_preset}>{t("export.platform.custom")}</option>
                {:else}
                  <option value={CUSTOM_PLATFORM}>{t("export.platform.custom")}</option>
                {/if}
              </select>
              <label class="flex items-center gap-1.5">
                <span class="text-xs text-muted">{t("export.platform.max")}</span>
                <input
                  type="text"
                  inputmode="numeric"
                  class="{field} w-20"
                  value={maxText}
                  placeholder={t("export.platform.maxPlaceholder")}
                  aria-invalid={maxError}
                  aria-describedby="{uid}-max-hint"
                  oninput={onMaxInput}
                />
              </label>
            </div>
            <p id="{uid}-max-hint" class="text-xs {maxError ? 'text-danger' : 'text-muted'}">
              {maxError ? t("export.platform.maxInvalid") : t("export.platform.hint")}
            </p>
          </div>
        </div>
      {/if}

      <!-- Folder -->
      <div class={row}>
        <label class="{label} pt-1.5" for="{uid}-folder">{t("export.folder.label")}</label>
        <div class="space-y-1">
          <div class="flex items-center gap-2">
            <input
              id="{uid}-folder"
              type="text"
              class="{field} min-w-0 flex-1"
              value={draft.output_dir ?? ""}
              placeholder={report?.output_dir ?? t("export.folder.defaultPlaceholder")}
              oninput={onFolderInput}
            />
            <button type="button" class={btn.secondary} onclick={() => void browse()}>
              <Icon name="folder" size={14} />{t("export.folder.browse")}
            </button>
            {#if draft.output_dir !== null}
              <button type="button" class={btn.ghost} onclick={() => update({ output_dir: null })}>
                {t("export.folder.reset")}
              </button>
            {/if}
          </div>
          <p class="text-xs text-muted">{t("export.folder.hint")}</p>
        </div>
      </div>

      <!-- File name template (EXP-05) -->
      <div class={row}>
        <label class="{label} pt-1.5" for="{uid}-template">{t("export.template.label")}</label>
        <div class="space-y-1.5">
          <div class="flex items-center gap-3">
            <input
              bind:this={templateInput}
              id="{uid}-template"
              type="text"
              class="{field} tabular min-w-0 flex-1"
              value={draft.naming_template}
              spellcheck="false"
              aria-describedby="{uid}-names"
              oninput={onTemplateInput}
            />
            <label class="flex shrink-0 items-center gap-1.5">
              <input
                type="checkbox"
                class="accent-accent"
                checked={draft.spaces_to_underscores}
                onchange={(e) => update({ spaces_to_underscores: e.currentTarget.checked })}
              />
              {t("export.template.spaces")}
            </label>
          </div>
          <div class="flex flex-wrap gap-1" role="group" aria-label={t("export.template.variables")}>
            {#each TEMPLATE_VARIABLES as v (v)}
              <button
                type="button"
                class="tabular rounded border border-line px-1.5 py-0.5 text-[11px] text-muted hover:border-accent hover:text-text"
                title={t(`export.var.${v}`)}
                aria-label={t("export.template.insert", { variable: `{${v}}`, description: t(`export.var.${v}`) })}
                onclick={() => insertVariable(v)}
              >
                {`{${v}}`}
              </button>
            {/each}
          </div>
          <div id="{uid}-names" class="text-xs" aria-live="polite">
            {#if unknownVars.length > 0}
              <p class="text-warning">{t("export.template.unknown", { list: unknownVars.map((n) => `{${n}}`).join(", ") })}</p>
            {/if}
            {#if hasExtension}
              <p class="text-warning">{t("export.template.extension")}</p>
            {/if}
            {#if names.shown.length > 0}
              <p class="text-muted">
                {t("export.template.preview")}
                {#each names.shown as o, i (o.clip_id)}
                  <span class="tabular text-text">{o.file_name}</span>{#if o.action !== "write"}<span class="text-muted"> ({t(`export.action.${o.action}`)})</span>{/if}{i < names.shown.length - 1 ? ", " : ""}
                {/each}
                {#if names.more > 0}{tp("export.template.more", names.more)}{/if}
              </p>
            {/if}
          </div>
        </div>
      </div>

      <!-- If exists (EXP-07) -->
      <div class={row} role="radiogroup" aria-labelledby="{uid}-conflict-label">
        <span id="{uid}-conflict-label" class="{label} pt-0.5">{t("export.conflict.label")}</span>
        <div class="space-y-1">
          <div class="flex flex-wrap gap-x-4 gap-y-1">
            {#each CONFLICT_POLICIES as policy (policy)}
              <label class="flex items-center gap-2">
                <input
                  type="radio"
                  name="{uid}-conflict"
                  value={policy}
                  class="accent-accent"
                  checked={draft.on_conflict === policy}
                  onchange={() => update({ on_conflict: policy })}
                />
                {t(POLICY_KEYS[policy])}
              </label>
            {/each}
          </div>
          <p class="text-xs text-muted">{t("export.conflict.hint")}</p>
        </div>
      </div>

      <div class="border-t border-line pt-3">
        <PreflightList items={report?.items ?? []} loading={checking} error={exporter.reportError} />
      </div>
    </div>
  {/if}

  {#snippet footer()}
    <div class="flex flex-wrap items-center justify-between gap-3">
      <p class="text-xs text-muted">
        {#if report}
          {t("export.dialog.estimate", { size: formatBytes(report.estimate_bytes, true) })}
          {#if report.estimate_seconds !== null}· {formatSeconds(report.estimate_seconds, true)}{/if}
          {#if report.free_bytes !== null}· {t("export.dialog.free", { size: formatBytes(report.free_bytes) })}{/if}
        {/if}
        {#if startBlocker}<span class="ml-2 text-warning">{startBlocker}</span>{/if}
      </p>
      <div class="flex gap-2">
        <button type="button" class={btn.secondary} onclick={close}>{t("export.dialog.cancel")}</button>
        <button
          type="button"
          class={btn.primary}
          disabled={!canStart}
          aria-keyshortcuts="Enter"
          onclick={() => void start()}
        >
          {exporter.starting ? t("export.dialog.starting") : t("export.dialog.start")}
        </button>
      </div>
    </div>
  {/snippet}
</Modal>
