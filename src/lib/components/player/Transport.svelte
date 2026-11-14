<script lang="ts">
  // §6.1 transport: time / duration, speed, |< -5s -1f PLAY +1f +5s >|,
  // pending In/Out readouts and + ADD (Enter). Buttons run the same actions
  // as the keys (keyboard/actions.ts). OWNER: fe-shell.
  import Menu, { type MenuItem } from "$lib/components/shell/Menu.svelte";
  import { t } from "$lib/i18n";
  import { actions } from "$lib/keyboard";
  import { ariaKeyShortcuts, shortcutLabel, type ActionId } from "$lib/keyboard/shortcuts";
  import { loopAsExportedAvailable } from "$lib/state/playback";
  import { player, SPEEDS } from "$lib/state/player.svelte";
  import { project } from "$lib/state/project.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import { timelineMessageKeys } from "$lib/timeline/messages";
  import { EMPTY_TIMECODE, formatTimecode } from "$lib/util/time";

  let speedOpen = $state(false);

  const ready = $derived(project.loaded && player.ready);
  const editable = $derived(project.loaded && !project.readOnly);
  const hasMarks = $derived(player.pendingIn !== null || player.pendingOut !== null);

  /** Mark actions read the playhead synchronously, exactly like the keys. */
  function run(action: ActionId): void {
    actions.run(action, player.now());
  }

  /** "Seek back 5 s (J)". */
  function hint(action: ActionId, labelKey: string): string {
    return `${t(labelKey)} (${shortcutLabel(action, ui.platform)})`;
  }

  /**
   * Mouse clicks do not move focus onto transport buttons, so Space and
   * Enter keep controlling playback and adding (keyboard users still Tab).
   */
  function keepFocus(e: MouseEvent): void {
    e.preventDefault();
  }

  function speedText(s: number): string {
    return t("transport.speedValue", { speed: s });
  }

  const speedItems = $derived<MenuItem[]>(
    SPEEDS.map((s) => ({
      id: String(s),
      label: speedText(s),
      checked: Math.abs(player.rate - s) < 1e-9,
      onSelect: () => player.setRate(s),
    })),
  );

  const BTN =
    "rounded border border-line bg-panel-2 px-2.5 py-1 text-sm hover:bg-panel-3 disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-panel-2";
</script>

<div class="flex flex-col gap-1.5 border-t border-line bg-panel px-3 py-2" role="group" aria-label={t("transport.region")}>
  <div class="flex items-center gap-3">
    <p class="tabular text-sm" role="timer">
      <span class="sr-only">{t("transport.position")}</span>
      {formatTimecode(player.currentTime)}
      <span class="text-muted">/ {formatTimecode(player.duration)}</span>
    </p>
    {#if player.loopClipId}
      <span class="rounded bg-accent/15 px-1.5 text-xs text-accent">{t("transport.looping")}</span>
    {/if}
    {#if loopAsExportedAvailable(project.settings)}
    <button
      type="button"
      class="rounded border border-line px-2 py-0.5 text-xs hover:bg-panel-3"
      aria-pressed={player.loopAsExported}
      title={t("transport.loopAsExportedHint")}
      onmousedown={keepFocus}
      onclick={() => (player.loopAsExported = !player.loopAsExported)}
    >{t(timelineMessageKeys.loopAsExported)}</button>
    {/if}
    <div class="ml-auto flex items-center gap-1 text-sm">
      <span class="text-muted" aria-hidden="true">{t("transport.speed")}</span>
      <Menu
        label={`${speedText(player.rate)} ▾`}
        ariaLabel={t("transport.speedAria", { speed: speedText(player.rate) })}
        title={`${shortcutLabel("speedDown", ui.platform)} / ${shortcutLabel("speedUp", ui.platform)}`}
        items={speedItems}
        open={speedOpen}
        onOpenChange={(o) => (speedOpen = o)}
        align="right"
        up
        buttonClass="tabular border border-line bg-panel-2"
      />
    </div>
  </div>

  <div class="flex flex-wrap items-center gap-1">
    <button
      type="button"
      class={BTN}
      disabled={!ready}
      aria-label={t("transport.toStart")}
      title={t("transport.toStart")}
      onmousedown={keepFocus}
      onclick={() => player.seek(0)}>|&lt;</button
    >
    <button
      type="button"
      class={BTN}
      disabled={!ready}
      aria-label={t("kb.seekBack5")}
      aria-keyshortcuts={ariaKeyShortcuts("seekBack5", ui.platform)}
      title={hint("seekBack5", "kb.seekBack5")}
      onmousedown={keepFocus}
      onclick={() => run("seekBack5")}>-5s</button
    >
    <button
      type="button"
      class={BTN}
      disabled={!ready}
      aria-label={t("kb.frameBack")}
      aria-keyshortcuts={ariaKeyShortcuts("frameBack", ui.platform)}
      title={hint("frameBack", "kb.frameBack")}
      onmousedown={keepFocus}
      onclick={() => run("frameBack")}>-1f</button
    >
    <button
      type="button"
      class="{BTN} min-w-24 font-semibold"
      disabled={!ready}
      aria-label={player.playing ? t("transport.pause") : t("transport.play")}
      aria-keyshortcuts={ariaKeyShortcuts("playPause", ui.platform)}
      title={hint("playPause", "kb.playPause")}
      onmousedown={keepFocus}
      onclick={() => run("playPause")}
    >
      {player.playing ? t("transport.pauseShort") : t("transport.playShort")}
    </button>
    <button
      type="button"
      class={BTN}
      disabled={!ready}
      aria-label={t("kb.frameForward")}
      aria-keyshortcuts={ariaKeyShortcuts("frameForward", ui.platform)}
      title={hint("frameForward", "kb.frameForward")}
      onmousedown={keepFocus}
      onclick={() => run("frameForward")}>+1f</button
    >
    <button
      type="button"
      class={BTN}
      disabled={!ready}
      aria-label={t("kb.seekForward5")}
      aria-keyshortcuts={ariaKeyShortcuts("seekForward5", ui.platform)}
      title={hint("seekForward5", "kb.seekForward5")}
      onmousedown={keepFocus}
      onclick={() => run("seekForward5")}>+5s</button
    >
    <button
      type="button"
      class={BTN}
      disabled={!ready}
      aria-label={t("transport.toEnd")}
      title={t("transport.toEnd")}
      onmousedown={keepFocus}
      onclick={() => player.seek(player.duration)}>&gt;|</button
    >
  </div>

  <div class="flex flex-wrap items-center gap-2 text-sm">
    <button
      type="button"
      class={BTN}
      disabled={!ready}
      aria-label={t("transport.setIn")}
      aria-keyshortcuts={ariaKeyShortcuts("markIn", ui.platform)}
      title={hint("markIn", "transport.setIn")}
      onmousedown={keepFocus}
      onclick={() => run("markIn")}>{t("transport.in")}</button
    >
    <span class="tabular {player.pendingIn === null ? 'text-muted' : 'text-pending-in'}">
      <span class="sr-only">{t("transport.inValue")}</span>
      {player.pendingIn === null ? EMPTY_TIMECODE : formatTimecode(player.pendingIn)}
    </span>
    <button
      type="button"
      class="{BTN} ml-2"
      disabled={!ready}
      aria-label={t("transport.setOut")}
      aria-keyshortcuts={ariaKeyShortcuts("markOut", ui.platform)}
      title={hint("markOut", "transport.setOut")}
      onmousedown={keepFocus}
      onclick={() => run("markOut")}>{t("transport.out")}</button
    >
    <span class="tabular {player.pendingOut === null ? 'text-muted' : 'text-pending-out'}">
      <span class="sr-only">{t("transport.outValue")}</span>
      {player.pendingOut === null ? EMPTY_TIMECODE : formatTimecode(player.pendingOut)}
    </span>
    {#if hasMarks}
      <button
        type="button"
        class="rounded px-1.5 text-muted hover:bg-panel-3 hover:text-text"
        aria-label={t("menu.clearMarks")}
        aria-keyshortcuts={ariaKeyShortcuts("escape", ui.platform)}
        title={hint("escape", "menu.clearMarks")}
        onmousedown={keepFocus}
        onclick={() => run("escape")}>×</button
      >
    {/if}
    <button
      type="button"
      class="ml-auto rounded bg-accent-strong px-3 py-1 font-semibold text-white hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-40"
      disabled={!ready || !editable || player.pendingIn === null}
      aria-keyshortcuts={ariaKeyShortcuts("addClip", ui.platform)}
      title={hint("addClip", "kb.addClip")}
      onmousedown={keepFocus}
      onclick={() => run("addClip")}
    >
      {t("transport.add", { key: shortcutLabel("addClip", ui.platform) })}
    </button>
  </div>
</div>
