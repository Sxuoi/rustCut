<script lang="ts">
  // §6.1 header: File/Edit/View/Help menus (listing shortcuts), preset
  // selector, source name, save state. OWNER: fe-shell.
  import type { Preset } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { actions } from "$lib/keyboard";
  import { ariaKeyShortcuts, shortcutLabel, type ActionId } from "$lib/keyboard/shortcuts";
  import { loopAsExportedAvailable } from "$lib/state/playback";
  import { player } from "$lib/state/player.svelte";
  import { project } from "$lib/state/project.svelte";
  import { closeProject } from "$lib/state/session";
  import { ui } from "$lib/state/ui.svelte";
  import { timelineMessageKeys } from "$lib/timeline/messages";
  import Menu, { type MenuItem } from "./Menu.svelte";
  import { applyPreset, formatSaveState, PRESETS, presetLabelKey, presetOf, stem } from "./format";

  let openMenu = $state<number | null>(null);
  let presetOpen = $state(false);
  let nowMs = $state(Date.now());

  // "saved 2s ago" ticks.
  $effect(() => {
    const id = setInterval(() => (nowMs = Date.now()), 1000);
    return () => clearInterval(id);
  });

  interface CmdOpts {
    disabled?: boolean;
    label?: string;
    separatorBefore?: boolean;
  }

  /** A menu entry running a §5 action, labelled from the shortcut table. */
  function cmd(action: ActionId, labelKey: string, opts: CmdOpts = {}): MenuItem {
    return {
      id: action,
      label: opts.label ?? t(labelKey),
      shortcut: shortcutLabel(action, ui.platform),
      keys: ariaKeyShortcuts(action, ui.platform),
      disabled: opts.disabled,
      separatorBefore: opts.separatorBefore,
      // Marks read the playhead synchronously, like the keys do.
      onSelect: () => actions.run(action, player.now()),
    };
  }

  const loaded = $derived(project.loaded);
  const ready = $derived(project.loaded && player.ready);
  const editable = $derived(project.loaded && !project.readOnly);
  const hasClips = $derived(project.clips.length > 0);
  const missing = $derived(project.snapshot?.source_status.kind === "missing");
  const history = $derived(project.history);

  const menus = $derived<{ id: string; label: string; items: MenuItem[] }[]>([
    {
      id: "file",
      label: t("menu.file"),
      items: [
        cmd("open", "menu.open"),
        cmd("saveNow", "menu.save", { disabled: !loaded }),
        cmd("exportDialog", "menu.export", { disabled: !loaded }),
        { id: "timestamps", label: t("menu.timestamps"), disabled: !loaded, separatorBefore: true, onSelect: () => (ui.timestampsOpen = true) },
        { id: "locate", label: t("menu.locate"), disabled: !missing, onSelect: () => (ui.locateMediaOpen = true) },
        { id: "settings", label: t("menu.settings"), separatorBefore: true, onSelect: () => (ui.settingsOpen = true) },
        { id: "close", label: t("menu.close"), disabled: !loaded, onSelect: () => void closeProject() },
      ],
    },
    {
      id: "edit",
      label: t("menu.edit"),
      items: [
        cmd("undo", "menu.undo", {
          disabled: !editable || !history.can_undo,
          label: history.undo_label ? t("menu.undoLabel", { label: history.undo_label }) : undefined,
        }),
        cmd("redo", "menu.redo", {
          disabled: !editable || !history.can_redo,
          label: history.redo_label ? t("menu.redoLabel", { label: history.redo_label }) : undefined,
        }),
        cmd("markIn", "kb.markIn", { disabled: !ready, separatorBefore: true }),
        cmd("markOut", "kb.markOut", { disabled: !ready }),
        cmd("addClip", "kb.addClip", { disabled: !ready || !editable || player.pendingIn === null }),
        cmd("escape", "menu.clearMarks", { disabled: player.pendingIn === null && player.pendingOut === null }),
        cmd("rename", "kb.rename", { disabled: !editable || !hasClips, separatorBefore: true }),
        cmd("deleteSelection", "kb.deleteSelection", { disabled: !editable || project.targetIds().length === 0 }),
        cmd("retimeIn", "kb.retimeIn", { disabled: !ready || !editable || !project.primary }),
        cmd("retimeOut", "kb.retimeOut", { disabled: !ready || !editable || !project.primary }),
        { id: "tags", label: t("menu.tags"), disabled: !loaded, separatorBefore: true, onSelect: () => (ui.tagEditorOpen = true) },
        { id: "renumber", label: t("menu.renumber"), disabled: !editable || !hasClips, onSelect: () => void project.renumber() },
      ],
    },
    {
      id: "view",
      label: t("menu.view"),
      items: [
        cmd("playPause", "kb.playPause", { disabled: !ready }),
        cmd("loopToggle", "kb.loopToggle", { disabled: !ready || !project.primary }),
        { id: "loopAsExported", label: t(timelineMessageKeys.loopAsExported), checked: player.loopAsExported, disabled: !loopAsExportedAvailable(project.settings), onSelect: () => (player.loopAsExported = !player.loopAsExported) },
        cmd("selectPrevious", "kb.selectPrevious", { disabled: !hasClips }),
        cmd("selectNext", "kb.selectNext", { disabled: !hasClips }),
        cmd("goToIn", "kb.goToIn", { disabled: !ready, separatorBefore: true }),
        cmd("goToOut", "kb.goToOut", { disabled: !ready }),
        cmd("speedDown", "kb.speedDown", { separatorBefore: true }),
        cmd("speedUp", "kb.speedUp"),
      ],
    },
    {
      id: "help",
      label: t("menu.help"),
      items: [cmd("cheatSheet", "menu.shortcuts"), { id: "about", label: t("menu.about"), onSelect: () => (ui.settingsOpen = true) }],
    },
  ]);

  const preset = $derived(project.settings ? presetOf(project.settings) : null);
  const presetItems = $derived<MenuItem[]>(
    PRESETS.map((p) => ({ id: p, label: t(presetLabelKey(p)), checked: p === preset, onSelect: () => choosePreset(p) })),
  );

  function choosePreset(p: Preset): void {
    const s = project.settings;
    if (!s || presetOf(s) === p) return;
    void project.setExportSettings(applyPreset(s, p));
  }

  const save = $derived(formatSaveState(project.save, nowMs));
  const TONE = { muted: "text-muted", busy: "text-accent", error: "text-danger" } as const;
</script>

<header class="flex h-10 shrink-0 items-center gap-3 border-b border-line bg-panel px-2">
  <nav aria-label={t("menu.main")}>
    <ul class="flex items-center">
      {#each menus as m, i (m.id)}
        <li>
          <Menu
            label={m.label}
            items={m.items}
            open={openMenu === i}
            onOpenChange={(o) => (openMenu = o ? i : openMenu === i ? null : openMenu)}
            onNavigate={(dir) => (openMenu = (i + dir + menus.length) % menus.length)}
          />
        </li>
      {/each}
    </ul>
  </nav>

  {#if loaded && preset}
    <div class="flex items-center gap-1 text-sm">
      <span class="text-muted" aria-hidden="true">{t("header.preset")}</span>
      <Menu
        label={`${t(presetLabelKey(preset))} ▾`}
        ariaLabel={t("header.presetAria", { preset: t(presetLabelKey(preset)) })}
        items={presetItems}
        open={presetOpen}
        onOpenChange={(o) => (presetOpen = o)}
        disabled={!editable}
        buttonClass="border border-line bg-panel-2"
      />
    </div>
  {/if}

  <div class="ml-auto flex min-w-0 items-center gap-3 text-sm">
    {#if project.source}
      <span class="min-w-0 truncate font-medium" title={project.source.path}>{stem(project.source.path)}</span>
    {/if}
    {#if loaded && save.text}
      <span class="shrink-0 {TONE[save.tone]}" title={save.title ?? undefined}>{save.text}</span>
    {/if}
    {#if loaded && save.location}
      <span class="shrink-0 rounded bg-panel-3 px-1.5 text-xs text-muted" title={project.save?.project_path ?? undefined}>{save.location}</span>
    {/if}
  </div>
</header>
