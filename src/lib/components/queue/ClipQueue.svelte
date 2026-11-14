<script lang="ts">
  // CLIP QUEUE panel (§6.1, QUE-01..08): header counts, search and sort (view
  // only, never renumbers), bulk include menu, Renumber chronologically, bulk
  // bar for multi-selection, virtualised rows, inline rename, tag picker and
  // failure details.
  // Focus convention (fe-shell): global shortcuts are suspended while a text
  // field has focus; after pointer actions focus returns to [data-focus-home].
  import { tick, untrack } from "svelte";
  import type { Clip } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { project } from "$lib/state/project.svelte";
  import { player } from "$lib/state/player.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import { exporter } from "$lib/state/exporter.svelte";
  import { detectPlatform, shortcutLabel } from "$lib/keyboard/shortcuts";
  import { formatIndex } from "$lib/util/time";
  import { indexWidth, tp } from "$lib/queue/format";
  import { SORT_KEYS, visibleClips, type SortDir, type SortKey } from "$lib/queue/search";
  import {
    clickMode,
    clipFlags,
    countIncluded,
    includeChanges,
    INCLUDE_MODES,
    rangeIds,
    revealablePath,
    rowStatus,
    wantsInclude,
    type IncludeMode,
  } from "$lib/queue/rows";
  import { presetOf } from "$lib/queue/exportOptions";
  import { revealScrollTop, virtualWindow } from "$lib/queue/virtual";
  import { focusHome, focusHomeUnlessModal } from "$lib/queue/popover";
  import { btn, field } from "$lib/queue/ui";
  import ClipRow, { type RowActions } from "./ClipRow.svelte";
  import FailureDetails from "./FailureDetails.svelte";
  import Icon from "./Icon.svelte";
  import Menu, { type MenuItem } from "./Menu.svelte";
  import TagChip from "$lib/components/tags/TagChip.svelte";
  import TagPicker from "$lib/components/tags/TagPicker.svelte";

  /** Fixed row height (QUE-08): two text lines. */
  const ROW_HEIGHT = 52;

  const uid = $props.id();
  const platform = detectPlatform(typeof navigator === "undefined" ? undefined : navigator);
  const undoKey = shortcutLabel("undo", platform);

  let query = $state("");
  let sortKey = $state<SortKey>("index");
  let sortDir = $state<SortDir>("asc");

  const clips = $derived(project.clips);
  const rows = $derived(visibleClips(clips, { query, sortKey, sortDir, tags: project.tags }));
  const width = $derived(indexWidth(clips.map((c) => c.index)));
  const included = $derived(countIncluded(clips));
  const preset = $derived(project.settings ? presetOf(project.settings) : null);
  const flags = $derived(
    clipFlags(clips, {
      sourceDurationSec: project.source?.duration_sec ?? null,
      maxDurationSec: project.settings?.max_duration_sec ?? null,
      preset,
    }),
  );
  const selectedIds = $derived([...project.selection]);
  const multi = $derived(selectedIds.length > 1);

  let scroller: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let viewportHeight = $state(0);
  const win = $derived(
    virtualWindow({ scrollTop, viewportHeight, rowHeight: ROW_HEIGHT, count: rows.length, overscan: 6 }),
  );
  const windowRows = $derived(rows.slice(win.start, win.end));

  /** Shift-click anchor (last plain or Ctrl click). */
  let anchorId: string | null = null;
  let editingId = $state<string | null>(null);
  let tagPicker = $state.raw<{ ids: string[]; anchor: HTMLElement } | null>(null);
  let failure = $state.raw<{ id: string; anchor: HTMLElement } | null>(null);
  const failureClip = $derived(failure ? project.clip(failure.id) : undefined);

  const pickerCurrent = $derived.by(() => {
    if (!tagPicker) return null;
    const tagIds = new Set(tagPicker.ids.map((id) => project.clip(id)?.tag_id ?? null));
    return tagIds.size === 1 ? [...tagIds][0] : undefined;
  });

  const bulkTag = $derived.by(() => {
    const tagIds = new Set(selectedIds.map((id) => project.clip(id)?.tag_id ?? null));
    if (tagIds.size !== 1) return { tag: undefined, mixed: true };
    return { tag: project.tag([...tagIds][0]), mixed: false };
  });
  const bulkCrop = $derived(project.primary?.crop_x_offset ?? 0);

  const includeItems = $derived<MenuItem[]>(
    INCLUDE_MODES.map((m) => ({
      id: m,
      label: t(`queue.include.${m}`),
      hint: tp("queue.include.hint", clips.filter((c) => wantsInclude(c, m)).length),
      disabled: clips.length === 0 || project.readOnly !== null,
    })),
  );

  const moreItems = $derived<MenuItem[]>([
    {
      id: "renumber",
      label: t("queue.more.renumber"),
      hint: t("queue.more.renumberHint"),
      disabled: clips.length === 0 || project.readOnly !== null,
    },
    { id: "tags", label: t("queue.more.tags") },
    { id: "timestamps", label: t("queue.more.timestamps") },
  ]);

  // ───────────── scrolling and the primary row (QUE-03) ─────────────

  function reveal(id: string): void {
    const index = rows.findIndex((c) => c.id === id);
    if (index < 0 || !scroller) return;
    const next = revealScrollTop({
      index,
      rowHeight: ROW_HEIGHT,
      scrollTop: scroller.scrollTop,
      viewportHeight: scroller.clientHeight,
    });
    if (next !== null) {
      scroller.scrollTop = next;
      scrollTop = next;
    }
  }

  // Publish the visible order so Up/Down (QUE-03/QUE-06) follow sort and filter.
  $effect(() => {
    ui.queueOrder = rows.map((c) => c.id);
    return () => {
      ui.queueOrder = null;
    };
  });

  $effect(() => {
    const id = project.primaryId;
    if (id) untrack(() => reveal(id));
  });

  function onScroll(): void {
    if (!scroller) return;
    scrollTop = scroller.scrollTop;
    tagPicker = null;
    failure = null;
  }

  // ───────────── rename (click on the selected name, or the N key) ─────────────

  let handledRename = ui.renameRequest?.nonce ?? 0;

  async function beginRename(id: string): Promise<void> {
    if (!project.clip(id) || project.readOnly !== null) return;
    if (!rows.some((c) => c.id === id)) {
      query = "";
      await tick();
    }
    editingId = id;
    reveal(id);
  }

  $effect(() => {
    const req = ui.renameRequest;
    if (!req || req.nonce <= handledRename) return;
    handledRename = req.nonce;
    untrack(() => void beginRename(req.clipId));
  });

  // ───────────── commands ─────────────

  async function deleteClips(ids: string[]): Promise<void> {
    if (ids.length === 0) return;
    const first = project.clip(ids[0]);
    const label = first ? formatIndex(first.index, width) : "";
    const before = project.revision;
    await project.deleteClips(ids);
    if (project.revision > before) ui.info(tp("queue.deleted", ids.length, { clip: label, undo: undoKey }), "deleted");
  }

  async function applyInclude(mode: IncludeMode): Promise<void> {
    const { include, exclude } = includeChanges(clips, mode);
    await project.editClipsBatch([
      { ids: include, edit: { kind: "set_included", included: true } },
      { ids: exclude, edit: { kind: "set_included", included: false } },
    ]);
  }

  async function onMore(id: string): Promise<void> {
    if (id === "renumber") {
      const before = project.revision;
      await project.renumber();
      if (project.revision > before) ui.info(t("queue.renumbered", { undo: undoKey }), "renumbered");
    } else if (id === "tags") {
      ui.tagEditorOpen = true;
    } else if (id === "timestamps") {
      ui.timestampsOpen = true;
    }
  }

  function selectAndSeek(clip: Clip): void {
    project.select(clip.id, "replace");
    anchorId = clip.id;
    player.seek(clip.start_sec);
  }

  const actions: RowActions = {
    retryFailed,
    select(clip, e) {
      const mode = clickMode(e);
      if (mode === "range") {
        const ids = rangeIds(
          rows.map((c) => c.id),
          anchorId ?? project.primaryId,
          clip.id,
        );
        project.selection.clear();
        for (const id of ids) project.selection.add(id);
        project.primaryId = clip.id;
      } else if (mode === "toggle") {
        project.select(clip.id, "toggle");
        anchorId = clip.id;
      } else {
        selectAndSeek(clip);
      }
      if (e.detail > 0) focusHome();
    },
    startRename(clip) {
      editingId = clip.id;
    },
    commitRename(clip, value, viaKeyboard) {
      editingId = null;
      const name = value.trim();
      if (name !== clip.name) void project.editClips([clip.id], { kind: "rename", name });
      if (viaKeyboard) focusHome();
    },
    cancelRename() {
      editingId = null;
      focusHome();
    },
    async setIncluded(clip, value, viaPointer) {
      await project.editClips([clip.id], { kind: "set_included", included: value });
      if (viaPointer) focusHomeUnlessModal();
    },
    pickTag(clip, anchor) {
      failure = null;
      tagPicker = { ids: [clip.id], anchor };
    },
    showFailure(clip, anchor) {
      tagPicker = null;
      failure = { id: clip.id, anchor };
    },
    cancelExport(clip) {
      void exporter.cancel(clip.id);
    },
    reveal(_clip, path) {
      void exporter.reveal(path);
    },
    remove(clip, viaPointer) {
      void deleteClips([clip.id]);
      if (viaPointer) focusHome();
    },
  };

  function onPickTag(tagId: string | null, viaKeyboard: boolean): void {
    const picker = tagPicker;
    tagPicker = null;
    if (!picker) return;
    void project.editClips(picker.ids, { kind: "set_tag", tag_id: tagId });
    if (viaKeyboard && picker.anchor.isConnected) picker.anchor.focus();
    else focusHome();
  }

  function onEditTags(): void {
    tagPicker = null;
    ui.tagEditorOpen = true;
  }

  async function retryFailed(clip: Clip): Promise<void> {
    failure = null;
    const settings = project.settings;
    if (settings) await exporter.retryClip(clip.id, settings);
  }

  function onSearchKeydown(e: KeyboardEvent): void {
    if (e.isComposing) return;
    if (e.key === "Escape") {
      e.preventDefault();
      if (query !== "") query = "";
      else focusHome();
    } else if (e.key === "Enter") {
      e.preventDefault();
      const first = rows[0];
      if (first) {
        selectAndSeek(first);
        focusHome();
      }
    }
    // Typing in the search box never triggers global shortcuts.
    e.stopPropagation();
  }

  function onBulkCrop(e: Event & { currentTarget: HTMLInputElement }): void {
    const value = Number(e.currentTarget.value);
    if (Number.isFinite(value)) void project.editClips(selectedIds, { kind: "set_crop", crop_x_offset: value });
  }
</script>

<div class="flex h-full min-h-0 flex-col">
  <div class="flex items-center justify-between gap-2 px-3 pt-2.5 pb-1.5">
    <h2 class="text-xs font-semibold tracking-wider text-muted uppercase">{t("queue.title")}</h2>
    <div class="flex items-center gap-1">
      <span class="text-xs text-muted">
        {t("queue.count", { clips: tp("queue.count.clips", clips.length), included: tp("queue.count.included", included) })}
      </span>
      <Menu label={t("queue.more.label")} items={moreItems} onselect={(id) => void onMore(id)} align="right" triggerClass={btn.icon}>
        {#snippet trigger()}<Icon name="more" />{/snippet}
      </Menu>
    </div>
  </div>

  <div class="flex items-center gap-1.5 px-3 pb-2">
    <div class="relative min-w-0 flex-1">
      <Icon name="search" size={14} class="pointer-events-none absolute top-1/2 left-2 -translate-y-1/2 text-muted" />
      <input
        type="search"
        bind:value={query}
        class="{field} h-7 w-full pl-7 text-xs"
        placeholder={t("queue.search.placeholder")}
        aria-label={t("queue.search.label")}
        onkeydown={onSearchKeydown}
      />
    </div>
    <label class="sr-only" for="{uid}-sort">{t("queue.sort.label")}</label>
    <select id="{uid}-sort" bind:value={sortKey} class="{field} h-7 text-xs">
      {#each SORT_KEYS as key (key)}
        <option value={key}>{t(`queue.sort.${key}`)}</option>
      {/each}
    </select>
    <button
      type="button"
      class={btn.icon}
      aria-label={sortDir === "asc" ? t("queue.sort.asc") : t("queue.sort.desc")}
      title={sortDir === "asc" ? t("queue.sort.asc") : t("queue.sort.desc")}
      onclick={() => (sortDir = sortDir === "asc" ? "desc" : "asc")}
    >
      <Icon name={sortDir === "asc" ? "up" : "down"} size={14} />
    </button>
    <Menu
      label={t("queue.include.label")}
      items={includeItems}
      onselect={(id) => void applyInclude(id as IncludeMode)}
      align="right"
      triggerClass="{btn.ghost} h-7 border border-line"
    >
      {#snippet trigger()}<span>{t("queue.include.button")}</span><Icon name="chevron" size={12} />{/snippet}
    </Menu>
  </div>

  {#if multi}
    <div
      class="flex flex-wrap items-center gap-1.5 border-y border-line bg-panel-2 px-3 py-1.5"
      role="toolbar"
      aria-label={tp("queue.bulk.label", selectedIds.length)}
    >
      <span class="text-xs font-semibold">{tp("queue.bulk.selected", selectedIds.length)}</span>
      <TagChip
        tag={bulkTag.tag}
        mixed={bulkTag.mixed}
        ariaLabel={tp("queue.bulk.tag", selectedIds.length)}
        onclick={(e) => {
          failure = null;
          tagPicker = { ids: selectedIds, anchor: e.currentTarget };
        }}
      />
      <button
        type="button"
        class={btn.ghost}
        disabled={project.readOnly !== null}
        onclick={() => void project.editClips(selectedIds, { kind: "set_included", included: true })}
      >
        {t("queue.bulk.include")}
      </button>
      <button
        type="button"
        class={btn.ghost}
        disabled={project.readOnly !== null}
        onclick={() => void project.editClips(selectedIds, { kind: "set_included", included: false })}
      >
        {t("queue.bulk.exclude")}
      </button>
      {#if preset === "vcrop"}
        <label class="flex items-center gap-1 text-xs text-muted">
          {t("queue.bulk.crop")}
          <input
            type="range"
            min="-1"
            max="1"
            step="0.05"
            value={bulkCrop}
            class="w-20 accent-accent"
            aria-valuetext={t("queue.bulk.cropValue", { value: bulkCrop.toFixed(2) })}
            onchange={onBulkCrop}
          />
        </label>
      {/if}
      <button type="button" class="{btn.ghost} hover:text-danger" disabled={project.readOnly !== null} onclick={() => void deleteClips(selectedIds)}>
        <Icon name="trash" size={14} />{t("queue.bulk.delete")}
      </button>
      <button
        type="button"
        class="{btn.icon} ml-auto"
        aria-label={t("queue.bulk.clear")}
        title={t("queue.bulk.clear")}
        onclick={() => project.clearSelection()}
      >
        <Icon name="x" size={14} />
      </button>
    </div>
  {/if}

  <div
    bind:this={scroller}
    bind:clientHeight={viewportHeight}
    class="relative min-h-0 flex-1 overflow-y-auto border-t border-line"
    onscroll={onScroll}
  >
    {#if clips.length === 0}
      <p class="p-4 text-sm text-muted">{t("queue.empty")}</p>
    {:else if rows.length === 0}
      <p class="p-4 text-sm text-muted" role="status">{t("queue.noMatches", { query: query.trim() })}</p>
    {:else}
      <div role="list" aria-label={t("queue.list.label")} class="relative" style:height="{win.totalHeight}px">
        {#each windowRows as clip, i (clip.id)}
          <ClipRow
            {clip}
            top={(win.start + i) * ROW_HEIGHT}
            height={ROW_HEIGHT}
            pos={win.start + i + 1}
            setSize={rows.length}
            indexWidth={width}
            selected={project.selection.has(clip.id)}
            primary={project.primaryId === clip.id}
            tag={project.tag(clip.tag_id)}
            status={rowStatus(clip, exporter.clipProgress(clip.id))}
            flags={flags.get(clip.id) ?? []}
            editing={editingId === clip.id}
            revealPath={revealablePath(clip)}
            {actions}
            readOnly={project.readOnly !== null}
            retryBlocked={exporter.running ? t("queue.failure.busy") : null}
          />
        {/each}
      </div>
    {/if}
  </div>
</div>

{#if tagPicker}
  <TagPicker
    anchor={tagPicker.anchor}
    tags={project.tags}
    current={pickerCurrent}
    count={tagPicker.ids.length}
    onpick={onPickTag}
    onedit={onEditTags}
    onclose={() => (tagPicker = null)}
  />
{/if}

{#if failure && failureClip}
  <FailureDetails
    anchor={failure.anchor}
    clip={failureClip}
    indexWidth={width}
    retryBlocked={exporter.running ? t("queue.failure.busy") : null}
    onretry={() => void retryFailed(failureClip)}
    onclose={() => (failure = null)}
  />
{/if}
