<script lang="ts">
  // MRK-06 tag palette editor: edit labels and colours, add, reorder and
  // remove tags (new tags get slugified unique ids; existing ids are kept
  // because clips reference them). Colour and label are always shown together.
  // Saved as one change via project.setTags.
  import { tick, untrack } from "svelte";
  import { t } from "$lib/i18n";
  import { project } from "$lib/state/project.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import { tp } from "$lib/queue/format";
  import {
    finalizeTags,
    MAX_TAG_LABEL,
    nextTagColor,
    tagUsage,
    validateTags,
    type TagDraft,
    type TagProblem,
  } from "$lib/queue/tags";
  import { btn, field } from "$lib/queue/ui";
  import Modal from "$lib/components/export/Modal.svelte";
  import Icon from "$lib/components/queue/Icon.svelte";
  import TagChip from "./TagChip.svelte";

  type Row = TagDraft & { key: number };

  const uid = $props.id();
  const open = $derived(ui.tagEditorOpen && project.loaded);

  let rows = $state<Row[]>([]);
  let keySeq = 0;
  let showErrors = $state(false);
  let saving = $state(false);
  let list: HTMLUListElement | undefined = $state();

  const usage = $derived(tagUsage(project.clips));
  const issues = $derived(validateTags(rows));
  const removedInUse = $derived(
    project.tags.filter((tag) => !rows.some((r) => r.id === tag.id)).reduce((n, tag) => n + (usage.get(tag.id) ?? 0), 0),
  );

  $effect(() => {
    if (!open) return;
    untrack(() => {
      rows = project.tags.map((tag) => ({ id: tag.id, label: tag.label, color: tag.color, key: ++keySeq }));
      showErrors = false;
    });
  });

  function problems(index: number): TagProblem[] {
    return issues.filter((i) => i.index === index).map((i) => i.problem);
  }

  async function focusRow(index: number): Promise<void> {
    await tick();
    list?.querySelector<HTMLInputElement>(`[data-row="${index}"] input[type="text"]`)?.focus();
  }

  function add(): void {
    rows.push({ id: null, label: "", color: nextTagColor(rows), key: ++keySeq });
    void focusRow(rows.length - 1);
  }

  function remove(index: number): void {
    rows.splice(index, 1);
    if (rows.length > 0) void focusRow(Math.min(index, rows.length - 1));
  }

  function move(index: number, delta: -1 | 1): void {
    const to = index + delta;
    if (to < 0 || to >= rows.length) return;
    const [r] = rows.splice(index, 1);
    rows.splice(to, 0, r);
  }

  async function save(): Promise<void> {
    showErrors = true;
    if (issues.length > 0) {
      void focusRow(issues[0].index);
      return;
    }
    saving = true;
    const before = project.revision;
    await project.setTags(finalizeTags(rows));
    saving = false;
    if (project.revision > before) ui.tagEditorOpen = false;
  }

  function close(): void {
    ui.tagEditorOpen = false;
  }

  function onKeydown(e: KeyboardEvent): void {
    if (e.key !== "Enter" || e.isComposing || !(e.target instanceof HTMLInputElement) || e.target.type !== "text") return;
    e.preventDefault();
    void save();
  }
</script>

<Modal {open} title={t("tags.editor.title")} onclose={close} onkeydown={onKeydown} size="md">
  <p class="mb-3 text-xs text-muted">{t("tags.editor.intro")}</p>
  {#if rows.length === 0}
    <p class="mb-3 text-muted">{t("tags.editor.empty")}</p>
  {/if}
  <ul bind:this={list} class="space-y-2" aria-label={t("tags.editor.listLabel")}>
    {#each rows as row, i (row.key)}
      {@const rowProblems = showErrors ? problems(i) : []}
      <li data-row={i} class="space-y-1">
        <div class="flex items-center gap-2">
          <input
            type="color"
            bind:value={row.color}
            class="h-7 w-9 shrink-0 cursor-pointer rounded border border-line bg-bg p-0.5"
            aria-label={t("tags.editor.color", { n: i + 1 })}
          />
          <input
            type="text"
            bind:value={row.label}
            maxlength={MAX_TAG_LABEL}
            class="{field} min-w-0 flex-1"
            placeholder={t("tags.editor.placeholder")}
            aria-label={t("tags.editor.label", { n: i + 1 })}
            aria-invalid={rowProblems.length > 0}
            aria-describedby={rowProblems.length > 0 ? `${uid}-err-${row.key}` : undefined}
          />
          <TagChip tag={{ id: row.id ?? "", label: row.label.trim() || t("tags.editor.unnamed"), color: row.color }} />
          <span class="w-16 shrink-0 text-right text-xs text-muted">
            {row.id !== null ? tp("tags.editor.usage", usage.get(row.id) ?? 0) : t("tags.editor.new")}
          </span>
          <button
            type="button"
            class={btn.icon}
            disabled={i === 0}
            aria-label={t("tags.editor.moveUp", { n: i + 1 })}
            onclick={() => move(i, -1)}
          >
            <Icon name="up" size={14} />
          </button>
          <button
            type="button"
            class={btn.icon}
            disabled={i === rows.length - 1}
            aria-label={t("tags.editor.moveDown", { n: i + 1 })}
            onclick={() => move(i, 1)}
          >
            <Icon name="down" size={14} />
          </button>
          <button
            type="button"
            class="{btn.icon} hover:text-danger"
            aria-label={t("tags.editor.remove", { n: i + 1 })}
            title={t("tags.editor.remove", { n: i + 1 })}
            onclick={() => remove(i)}
          >
            <Icon name="trash" size={14} />
          </button>
        </div>
        {#if rowProblems.length > 0}
          <p id="{uid}-err-{row.key}" class="pl-11 text-xs text-danger">
            {rowProblems.map((p) => t(`tags.editor.problem.${p}`)).join(" ")}
          </p>
        {/if}
      </li>
    {/each}
  </ul>
  <button type="button" class="{btn.secondary} mt-3" onclick={add}>
    <Icon name="plus" size={14} />{t("tags.editor.add")}
  </button>
  {#if removedInUse > 0}
    <p class="mt-3 text-xs text-warning" role="status">{tp("tags.editor.removedInUse", removedInUse)}</p>
  {/if}

  {#snippet footer()}
    <div class="flex justify-end gap-2">
      <button type="button" class={btn.secondary} onclick={close}>{t("tags.editor.cancel")}</button>
      <button type="button" class={btn.primary} disabled={saving} onclick={() => void save()}>
        {t("tags.editor.save")}
      </button>
    </div>
  {/snippet}
</Modal>
