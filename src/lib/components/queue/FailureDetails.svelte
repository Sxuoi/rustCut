<script lang="ts">
  // EXP-10 failure details of one clip: reason, exit code, last 20 lines of
  // FFmpeg output, Retry and Copy log.
  import type { Clip } from "$lib/bindings";
  import { t } from "$lib/i18n";
  import { failureInfo } from "$lib/queue/rows";
  import { formatIndex } from "$lib/util/time";
  import { ui } from "$lib/state/ui.svelte";
  import Popover from "./Popover.svelte";
  import Icon from "./Icon.svelte";
  import { btn } from "$lib/queue/ui";

  interface Props {
    anchor: HTMLElement;
    clip: Clip;
    indexWidth: number;
    /** Why Retry is unavailable (an export is running), else `null`. */
    retryBlocked: string | null;
    onretry: () => void;
    onclose: () => void;
  }

  let { anchor, clip, indexWidth, retryBlocked, onretry, onclose }: Props = $props();

  const info = $derived(failureInfo(clip));
  const label = $derived(formatIndex(clip.index, indexWidth));

  async function copyLog(): Promise<void> {
    const text = [info?.reason ?? "", ...(info?.logTail ?? [])].join("\n").trim();
    try {
      await navigator.clipboard.writeText(text);
      ui.info(t("queue.failure.copied"));
    } catch {
      ui.error(t("queue.failure.copyFailed"));
    }
  }
</script>

<Popover {anchor} label={t("queue.failure.title", { clip: label })} width={380} {onclose}>
  <div class="space-y-2 p-3">
    <p class="flex items-center gap-1.5 font-semibold text-danger">
      <Icon name="error" size={16} />
      {t("queue.failure.title", { clip: label })}
    </p>
    <p class="text-text">{info?.reason ?? t("queue.failure.noReason")}</p>
    {#if info?.exitCode !== null && info?.exitCode !== undefined}
      <p class="text-xs text-muted">{t("queue.failure.exitCode", { code: info.exitCode })}</p>
    {/if}
    {#if info && info.logTail.length > 0}
      <div>
        <p class="mb-1 text-xs text-muted">{t("queue.failure.logTail", { n: info.logTail.length })}</p>
        <!-- The log scrolls, so it must be keyboard-focusable to scroll it. -->
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <pre
          class="tabular max-h-40 overflow-auto rounded border border-line bg-bg p-2 text-[11px] leading-snug whitespace-pre-wrap text-muted select-text"
          tabindex="0"
          aria-label={t("queue.failure.logTail", { n: info.logTail.length })}>{info.logTail.join("\n")}</pre>
      </div>
    {/if}
    {#if info?.logPath}
      <p class="truncate text-xs text-muted" title={info.logPath}>{t("queue.failure.logPath", { path: info.logPath })}</p>
    {/if}
    <div class="flex items-center justify-end gap-2 pt-1">
      <button type="button" class={btn.secondary} onclick={copyLog}>
        <Icon name="copy" size={14} />{t("queue.failure.copy")}
      </button>
      <button
        type="button"
        class={btn.primary}
        disabled={retryBlocked !== null}
        title={retryBlocked ?? undefined}
        onclick={onretry}
      >
        <Icon name="retry" size={14} />{t("queue.failure.retry")}
      </button>
    </div>
    {#if retryBlocked}<p class="text-right text-xs text-muted">{retryBlocked}</p>{/if}
  </div>
</Popover>

