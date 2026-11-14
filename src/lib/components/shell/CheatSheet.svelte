<script lang="ts">
  // `?` cheat sheet, generated from the shortcut table (single source of
  // truth, §5). OWNER: fe-shell.
  import { t } from "$lib/i18n";
  import { SHORTCUT_GROUPS, SHORTCUTS, shortcutLabels } from "$lib/keyboard/shortcuts";
  import { ui } from "$lib/state/ui.svelte";
  import Dialog from "./Dialog.svelte";

  const groups = $derived(
    SHORTCUT_GROUPS.map((g) => ({
      ...g,
      rows: SHORTCUTS.filter((s) => s.group === g.group).map((s) => ({
        def: s,
        keys: shortcutLabels(s.action, ui.platform),
      })),
    })),
  );
</script>

<Dialog open={ui.cheatSheetOpen} onclose={() => (ui.cheatSheetOpen = false)} title={t("cheat.title")} width="max-w-3xl">
  <div class="grid gap-x-8 gap-y-4 sm:grid-cols-2">
    {#each groups as g (g.group)}
      <section aria-labelledby="cheat-{g.group}">
        <h3 id="cheat-{g.group}" class="mb-1 text-xs font-semibold tracking-wide text-muted uppercase">{t(g.labelKey)}</h3>
        <dl class="text-sm">
          {#each g.rows as row (row.def.action)}
            <div class="flex items-baseline justify-between gap-3 border-b border-line/60 py-1">
              <dt>{t(row.def.labelKey)}</dt>
              <dd class="flex shrink-0 flex-wrap justify-end gap-1">
                {#each row.keys as k, i (k)}
                  {#if i > 0}<span class="text-muted" aria-hidden="true">/</span>{/if}
                  <kbd class="tabular rounded border border-line bg-panel-2 px-1.5 text-xs">{k}</kbd>
                {/each}
              </dd>
            </div>
          {/each}
        </dl>
      </section>
    {/each}
  </div>
  <p class="mt-4 text-xs text-muted">{t("cheat.rules", { primary: ui.platform === "macos" ? "⌘" : "Ctrl" })}</p>
</Dialog>
