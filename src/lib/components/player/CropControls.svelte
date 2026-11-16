<script lang="ts">
  // PRD 6.3 / FRM-03 / FRM-04: controls under the player while a framed
  // preset is active. Presets with a crop window (Vertical Crop, Square on
  // Blur) get the "Crop position" slider [-1..+1] and Reset to centre; every
  // framed preset gets the "9:16 only" / "1:1 only" toggle that shows just
  // what the export contains (shared state: state/framing.svelte.ts). The
  // slider previews live while dragging and commits one undoable edit on change.
  import { t } from "$lib/i18n";
  import { presetOf } from "$lib/queue/exportOptions";
  import { framing } from "$lib/state/framing.svelte";
  import { project } from "$lib/state/project.svelte";
  import { windowShape } from "$lib/util/crop";

  const settings = $derived(project.settings);
  const kind = $derived(settings && presetOf(settings) !== "lossless" ? settings.framing : "original");
  const shape = $derived(windowShape(kind));
  const outputShape = $derived(kind === "square_blur" ? "1:1" : "9:16");
  const clip = $derived(project.primary);
  const value = $derived(framing.offsetFor(clip));
  const disabled = $derived(clip === null || !!project.readOnly);

  function commit(v: number): void {
    const c = clip;
    if (!c) return;
    const next = Math.round(Math.min(1, Math.max(-1, v)) * 1000) / 1000;
    if (Math.abs(next - c.crop_x_offset) < 1e-6) {
      framing.clearPreview();
      return;
    }
    framing.setPreview(c.id, next);
    void project.editClips([c.id], { kind: "set_crop", crop_x_offset: next }).finally(() => framing.clearPreview());
  }

  function onInput(e: Event): void {
    if (!clip) return;
    framing.setPreview(clip.id, Number((e.currentTarget as HTMLInputElement).value));
  }

  function onChange(e: Event): void {
    commit(Number((e.currentTarget as HTMLInputElement).value));
  }
</script>

<div class="flex flex-wrap items-center gap-x-3 gap-y-1 px-2 py-1 text-xs text-muted">
  {#if shape}
    <label for="crop-position" class="whitespace-nowrap">{t("crop.position")}</label>
    <span aria-hidden="true">-1</span>
    <input
      id="crop-position"
      type="range"
      min="-1"
      max="1"
      step="0.05"
      class="min-w-32 flex-1 accent-accent"
      {disabled}
      value={value}
      aria-valuetext={t("crop.window.valuetext", { offset: value.toFixed(2) })}
      oninput={onInput}
      onchange={onChange}
    />
    <span aria-hidden="true">+1</span>
    <output class="w-10 text-right font-mono text-text" for="crop-position">{value.toFixed(2)}</output>
    <button
      type="button"
      class="rounded border border-line px-2 py-0.5 text-text hover:bg-panel-2 focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-50"
      disabled={disabled || Math.abs(value) < 1e-6}
      onclick={() => commit(0)}
    >
      {t("crop.reset")}
    </button>
  {/if}
  <label class="flex items-center gap-1 text-text" title={t("crop.outputOnly.title")}>
    <input type="checkbox" class="accent-accent" bind:checked={framing.outputOnly} />
    {t("crop.outputOnly", { shape: outputShape })}
  </label>
  {#if shape && framing.offsetIgnored}
    <span class="text-warning" role="status">{t("crop.hint.ignored", { shape })}</span>
  {:else if shape && !clip}
    <span role="status">{t("crop.hint.noClip", { shape })}</span>
  {/if}
</div>
