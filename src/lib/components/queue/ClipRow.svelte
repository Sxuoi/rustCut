<script lang="ts" module>
  import type { Clip } from "$lib/bindings";

  /** What a row asks its queue to do (the queue owns selection, popovers and commands). */
  export interface RowActions {
    /** Row or #NN/name activation: selection by modifiers (+ seek on plain click). */
    select(clip: Clip, e: MouseEvent): void;
    startRename(clip: Clip): void;
    /** `viaKeyboard`: Enter/Escape (focus returns to the player); blur keeps focus where it went. */
    commitRename(clip: Clip, name: string, viaKeyboard: boolean): void;
    cancelRename(clip: Clip): void;
    setIncluded(clip: Clip, included: boolean, viaPointer: boolean): Promise<void>;
    pickTag(clip: Clip, anchor: HTMLElement): void;
    showFailure(clip: Clip, anchor: HTMLElement): void;
    retryFailed(clip: Clip): Promise<void>;
    cancelExport(clip: Clip): void;
    reveal(clip: Clip, path: string): void;
    remove(clip: Clip, viaPointer: boolean): void;
  }
</script>

<script lang="ts">
  // One Clip Queue row (QUE-01, QUE-05): include checkbox, #NN, editable name,
  // In > Out, duration, tag chip, status badge, flags and row actions. Fixed
  // height for the virtualised list (QUE-08).
  import { tick } from "svelte";
  import type { Tag } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { formatDuration, formatIndex, formatTimecode } from "$lib/util/time";
  import { formatPercent } from "$lib/queue/format";
  import { clipDuration, failureReasonText, type ClipFlag, type RowStatusInfo } from "$lib/queue/rows";
  import { formatClock } from "$lib/queue/timecode";
  import { btn } from "$lib/queue/ui";
  import TagChip from "$lib/components/tags/TagChip.svelte";
  import Icon from "./Icon.svelte";

  interface Props {
    clip: Clip;
    top: number;
    height: number;
    /** 1-based position in the shown list and the list length (screen readers). */
    pos: number;
    setSize: number;
    indexWidth: number;
    selected: boolean;
    primary: boolean;
    tag: Tag | undefined;
    status: RowStatusInfo;
    flags: readonly ClipFlag[];
    editing: boolean;
    revealPath: string | null;
    actions: RowActions;
    /** Read-only project (source missing, open elsewhere, newer schema): no edits. */
    readOnly?: boolean;
    /** Same explanation used by the failure-details Retry action. */
    retryBlocked?: string | null;
  }

  let {
    clip,
    top,
    height,
    pos,
    setSize,
    indexWidth,
    selected,
    primary,
    tag,
    status,
    flags,
    editing,
    revealPath,
    actions,
    readOnly = false,
    retryBlocked = null,
  }: Props = $props();

  const label = $derived(formatIndex(clip.index, indexWidth));
  const duration = $derived(clipDuration(clip));
  /** Secondary controls are tabbable in the primary row only (fewer tab stops in long lists). */
  const tab = $derived(primary ? 0 : -1);
  const failureReason = $derived(failureReasonText(clip, t("queue.failure.noReason")));

  const BADGE: Record<RowStatusInfo["kind"], string> = {
    pending: "border-line text-muted",
    exporting: "border-accent/60 text-accent",
    done: "border-success/60 text-success",
    failed: "border-danger/60 text-danger hover:bg-danger/10",
    outdated: "border-warning/60 text-warning",
  };

  let input: HTMLInputElement | undefined = $state();
  let finished = false;

  $effect(() => {
    if (!editing) return;
    finished = false;
    void tick().then(() => {
      input?.focus({ preventScroll: true });
      input?.select();
    });
  });

  function finish(commit: boolean, viaKeyboard: boolean): void {
    if (finished) return;
    finished = true;
    if (commit && input) actions.commitRename(clip, input.value, viaKeyboard);
    else actions.cancelRename(clip);
  }

  function onRenameKeydown(e: KeyboardEvent): void {
    if (e.isComposing) return;
    if (e.key === "Enter") {
      e.preventDefault();
      finish(true, true);
    } else if (e.key === "Escape") {
      e.preventDefault();
      finish(false, true);
    }
    // Typing never reaches the global shortcuts.
    e.stopPropagation();
  }

  function onRowClick(e: MouseEvent): void {
    actions.select(clip, e);
  }

  function onMainClick(e: MouseEvent): void {
    e.stopPropagation();
    const onName = e.target instanceof Element && e.target.closest("[data-clip-name]") !== null;
    const plain = !e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey;
    // Click on the name of the row that is already selected → rename (like a file manager).
    if (e.detail > 0 && onName && plain && primary && selected && !readOnly) {
      actions.startRename(clip);
      return;
    }
    actions.select(clip, e);
  }

  async function onInclude(e: MouseEvent & { currentTarget: HTMLInputElement }): Promise<void> {
    e.stopPropagation();
    const box = e.currentTarget;
    await actions.setIncluded(clip, box.checked, e.detail > 0);
    box.checked = clip.selected_for_export;
  }

  function flagText(f: ClipFlag): { text: string; title: string } {
    switch (f.kind) {
      case "duplicate":
        return {
          text: t("queue.flag.duplicate", { clip: formatIndex(f.of, indexWidth) }),
          title: t("queue.flag.duplicateTitle", { clip: formatIndex(f.of, indexWidth) }),
        };
      case "out_of_range":
        return { text: t("queue.flag.outOfRange"), title: t("queue.flag.outOfRangeTitle") };
      case "too_long":
        return {
          text: t("queue.flag.tooLong"),
          title: t("queue.flag.tooLongTitle", { max: formatClock(f.maxSec) }),
        };
    }
  }
</script>

<!-- The #NN/name button is the keyboard path; the row click only widens the mouse target. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<div
  role="listitem"
  aria-setsize={setSize}
  aria-posinset={pos}
  data-clip-id={clip.id}
  class="absolute inset-x-0 cursor-default border-b border-line/60 px-2 {selected ? 'bg-panel-3' : 'hover:bg-panel-2'}"
  style:top="{top}px"
  style:height="{height}px"
  onclick={onRowClick}
>
  {#if primary}<span class="absolute inset-y-0 left-0 w-0.5 bg-accent" aria-hidden="true"></span>{/if}
  <div class="flex h-7 items-center gap-2 pt-1">
    <input
      type="checkbox"
      class="h-4 w-4 shrink-0 accent-accent"
      checked={clip.selected_for_export}
      disabled={readOnly}
      tabindex={tab}
      aria-label={t("queue.row.include", { clip: label })}
      onclick={onInclude}
    />
    {#if editing}
      <span class="tabular shrink-0 text-xs text-muted">{label}</span>
      <input
        bind:this={input}
        type="text"
        class="h-6 min-w-0 flex-1 rounded border border-accent bg-bg px-1.5 text-sm text-text"
        value={clip.name}
        maxlength="200"
        placeholder={t("queue.row.unnamed")}
        aria-label={t("queue.row.renameLabel", { clip: label })}
        onkeydown={onRenameKeydown}
        onblur={() => finish(true, false)}
        onclick={(e) => e.stopPropagation()}
      />
    {:else}
      <button
        type="button"
        class="flex h-6 min-w-0 flex-1 items-baseline gap-2 rounded text-left"
        aria-pressed={selected}
        aria-label={t("queue.row.selectLabel", {
          clip: label,
          name: clip.name || t("queue.row.unnamed"),
          start: formatTimecode(clip.start_sec),
          end: formatTimecode(clip.end_sec),
          duration: formatDuration(duration),
        })}
        onclick={onMainClick}
      >
        <span class="tabular shrink-0 text-xs text-muted">{label}</span>
        <span data-clip-name class="truncate {clip.name ? 'text-text' : 'text-muted italic'}">
          {clip.name || t("queue.row.unnamed")}
        </span>
      </button>
    {/if}
    <TagChip
      {tag}
      tabindex={tab}
      ariaLabel={t("queue.row.tag", { clip: label, tag: tag?.label ?? t("tags.none") })}
      onclick={(e) => {
        e.stopPropagation();
        if (!readOnly) actions.pickTag(clip, e.currentTarget);
      }}
    />
    {#if status.kind === "failed"}
      <button
        type="button"
        tabindex={tab}
        class="shrink-0 rounded border px-1.5 py-0.5 text-[10px] font-semibold tracking-wide uppercase {BADGE.failed}"
        aria-haspopup="dialog"
        aria-label={t("queue.status.failedDetails", { clip: label })}
        title={failureReason ?? undefined}
        onclick={(e) => {
          e.stopPropagation();
          actions.showFailure(clip, e.currentTarget);
        }}
      >
        {t("queue.status.failed")}
      </button>
      <!-- AC-09: always-visible Retry, in the tab order even when the row is not the roving stop. -->
      <button
        type="button"
        class="{btn.secondary} shrink-0 !px-2 !py-0.5"
        data-testid="retry-clip"
        disabled={retryBlocked !== null}
        aria-label={t("queue.row.retry", { clip: label })}
        title={retryBlocked ?? t("queue.row.retry", { clip: label })}
        onclick={(e) => {
          e.stopPropagation();
          void actions.retryFailed(clip);
        }}
      >
        <Icon name="retry" size={12} />{t("queue.failure.retry")}
      </button>
    {:else}
      <span
        class="tabular shrink-0 rounded border px-1.5 py-0.5 text-[10px] font-semibold tracking-wide uppercase {BADGE[status.kind]}"
      >
        {t(`queue.status.${status.kind}`)}{status.fraction !== null ? ` ${formatPercent(status.fraction)}` : ""}
      </span>
    {/if}
  </div>
  <div class="flex h-5 items-center gap-2 pl-6 text-xs text-muted">
    <span class="tabular shrink-0" aria-hidden="true">
      {formatTimecode(clip.start_sec)} &gt; {formatTimecode(clip.end_sec)}
    </span>
    <span class="tabular shrink-0 text-text" aria-hidden="true">{formatDuration(duration)}</span>
    {#if status.kind === "failed" && failureReason}
      <span class="min-w-0 flex-1 truncate text-danger" data-testid="failure-reason" title={failureReason}>{failureReason}</span>
    {/if}
    <span class="flex min-w-0 {status.kind === 'failed' && failureReason ? '' : 'flex-1'} gap-1 overflow-hidden">
      {#each flags as f (f.kind)}
        {@const ft = flagText(f)}
        <span class="truncate rounded bg-warning/15 px-1 text-[10px] text-warning" title={ft.title}>
          {ft.text}<span class="sr-only">: {ft.title}</span>
        </span>
      {/each}
    </span>
    {#if status.kind === "exporting"}
      <button
        type="button"
        class={btn.icon}
        tabindex={tab}
        aria-label={t("queue.row.cancelExport", { clip: label })}
        title={t("queue.row.cancelExport", { clip: label })}
        onclick={(e) => {
          e.stopPropagation();
          actions.cancelExport(clip);
        }}
      >
        <Icon name="x" size={14} />
      </button>
    {/if}
    {#if revealPath}
      <button
        type="button"
        class={btn.icon}
        tabindex={tab}
        aria-label={t("queue.row.reveal", { clip: label })}
        title={t("queue.row.reveal", { clip: label })}
        onclick={(e) => {
          e.stopPropagation();
          if (revealPath) actions.reveal(clip, revealPath);
        }}
      >
        <Icon name="folder" size={14} />
      </button>
    {/if}
    <button
      type="button"
      class="{btn.icon} hover:text-danger"
      disabled={readOnly}
      tabindex={tab}
      aria-label={t("queue.row.delete", { clip: label })}
      title={t("queue.row.delete", { clip: label })}
      onclick={(e) => {
        e.stopPropagation();
        actions.remove(clip, e.detail > 0);
      }}
    >
      <Icon name="trash" size={14} />
    </button>
  </div>
</div>
