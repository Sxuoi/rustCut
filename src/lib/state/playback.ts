// Pure playback helpers (PRD §7.3 tiers and time origin, PLY-04 frame
// stepping, FRM-04 overlay geometry). OWNER: fe-shell.

import type { ExportSettings, PlaybackInfo, PlaybackTier } from "$lib/bindings";
import { t } from "$lib/i18n";
import { roundMs } from "$lib/util/time";

/** §7.3: a tier must show its first decoded frame within this time. */
export const FIRST_FRAME_TIMEOUT_MS = 3000;

/** How long the first-frame check waits for the decoded frame to be painted (to read its media time). */
export const PRESENT_GRACE_MS = 500;

/** MRK-07: "Loop preview as exported" only means something for the Lossless preset. */
export function loopAsExportedAvailable(settings: Pick<ExportSettings, "cut_mode"> | null): boolean {
  return settings?.cut_mode === "lossless";
}

/** MRK-07: preview the lossless lead-in only when explicitly requested. */
export function loopPreviewRange(
  range: { start: number; end: number },
  asExported: boolean,
  settings: Pick<ExportSettings, "cut_mode" | "snap_in_to_keyframe"> | null,
  keyframeAtOrBefore: (time: number) => number | null,
): { start: number; end: number } {
  if (!asExported || settings?.cut_mode !== "lossless") return range;
  return { start: keyframeAtOrBefore(range.start) ?? range.start, end: range.end };
}

/** Superseded preparation events must not invalidate the current load attempt. */
export function ignorePlaybackInfo(info: PlaybackInfo, latestTier: PlaybackTier | null): boolean {
  return (latestTier !== null && info.tier !== latestTier) ||
    info.unsupported_reason === "Preparing the preview was cancelled.";
}

/** Each fallback tier is requested at most once per project, including preparation failures. */
export function nextPlaybackTier(info: PlaybackInfo, tried: ReadonlySet<PlaybackTier>, requested: ReadonlySet<PlaybackTier>): PlaybackTier | null {
  const next = info.next_tier;
  return next && !tried.has(next) && !requested.has(next) ? next : null;
}

/** What the first-frame check reads from a `<video>` element (tests pass a fake). */
export interface FirstFrameTarget extends EventTarget {
  readonly readyState: number;
  readonly videoWidth: number;
  readonly videoHeight: number;
  readonly currentTime: number;
  readonly error: { readonly code: number } | null;
  requestVideoFrameCallback?(callback: (now: number, meta: { mediaTime: number }) => void): number;
}

export type FirstFrame = { ok: true; mediaTime: number } | { ok: false; reason: string };

/**
 * §7.3 tier selection: confirm a decoded first frame (`loadeddata` with a
 * picture) within `timeoutMs`. Once a picture is decoded the budget is met and
 * the timer stops: the presented frame is then awaited for at most
 * PRESENT_GRACE_MS only to learn its media time (time origin). On a busy
 * machine that paint can land after the budget; that is still a success, not a
 * reason to fall back to a slower tier.
 */
export function confirmFirstFrame(el: FirstFrameTarget, timeoutMs: number = FIRST_FRAME_TIMEOUT_MS): Promise<FirstFrame> {
  return new Promise((resolve) => {
    let settled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let grace: ReturnType<typeof setTimeout> | undefined;
    const finish = (r: FirstFrame): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      clearTimeout(grace);
      el.removeEventListener("loadeddata", onLoaded);
      el.removeEventListener("error", onError);
      resolve(r);
    };
    const onLoaded = (): void => {
      // Audio-only decode of an unsupported video stream: no picture.
      if (el.videoWidth === 0 || el.videoHeight === 0) {
        finish({ ok: false, reason: t("player.reason.noVideo") });
        return;
      }
      clearTimeout(timer);
      if (typeof el.requestVideoFrameCallback === "function") {
        el.requestVideoFrameCallback((_now, meta) => finish({ ok: true, mediaTime: meta.mediaTime }));
        // A paused element may not present again: the loaded frame counts.
        grace = setTimeout(() => finish({ ok: true, mediaTime: el.currentTime }), PRESENT_GRACE_MS);
      } else {
        finish({ ok: true, mediaTime: el.currentTime });
      }
    };
    const onError = (): void => finish({ ok: false, reason: mediaErrorReason(el.error?.code) });
    timer = setTimeout(() => finish({ ok: false, reason: t("player.reason.timeout", { s: timeoutMs / 1000 }) }), timeoutMs);
    el.addEventListener("loadeddata", onLoaded);
    el.addEventListener("error", onError);
    if (el.readyState >= 2) onLoaded();
  });
}

/** Fallback frame duration when the probe has no usable rate (30 fps). */
export const DEFAULT_FRAME_DURATION = 1 / 30;

/**
 * FFprobe rational frame rate (`"60000/1001"`, `"25/1"`, `"30"`) in frames
 * per second; `null` when unknown or invalid (`"0/0"`).
 */
export function parseFrameRate(rate: string | null | undefined): number | null {
  if (!rate) return null;
  const m = /^\s*(\d+(?:\.\d+)?)\s*(?:\/\s*(\d+(?:\.\d+)?))?\s*$/.exec(rate);
  if (!m) return null;
  const num = Number(m[1]);
  const den = m[2] === undefined ? 1 : Number(m[2]);
  if (!(num > 0) || !(den > 0)) return null;
  const fps = num / den;
  return Number.isFinite(fps) && fps > 0 && fps <= 1000 ? fps : null;
}

/** Frame duration (s) for a rational rate, with a 30 fps fallback. */
export function frameDurationOf(rate: string | null | undefined): number {
  const fps = parseFrameRate(rate);
  return fps ? 1 / fps : DEFAULT_FRAME_DURATION;
}

/**
 * §7.3 time origin: offset added to the element's media time to get source
 * time. `firstPresented` is the media time the WebView reports for the first
 * frame; `expectedFirst` is that frame's source-relative time (0, or the
 * first keyframe from the media index; negative edit-list times count as 0).
 * Differences under 1.5 frames are the same origin (decoders disagree by up
 * to a frame on edit lists); a real origin difference (a container start
 * time the WebView did not subtract) is larger and becomes the offset.
 */
export function computeTimeOffset(firstPresented: number, expectedFirst: number, frameDuration: number): number {
  if (!Number.isFinite(firstPresented) || !Number.isFinite(expectedFirst)) return 0;
  const off = Math.max(0, expectedFirst) - firstPresented;
  const frame = Number.isFinite(frameDuration) && frameDuration > 0 ? frameDuration : DEFAULT_FRAME_DURATION;
  return Math.abs(off) < Math.max(0.001, 1.5 * frame) ? 0 : roundMs(off);
}

/** A box in CSS px relative to the video element's top-left corner. */
export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/**
 * The content box of a `object-fit: contain` video: where the picture of
 * `contentW x contentH` (display size, FRM-02) sits inside a `boxW x boxH`
 * element. `null` until both sizes are known.
 */
export function containRect(boxW: number, boxH: number, contentW: number, contentH: number): Rect | null {
  if (!(boxW > 0 && boxH > 0 && contentW > 0 && contentH > 0)) return null;
  const scale = Math.min(boxW / contentW, boxH / contentH);
  const width = contentW * scale;
  const height = contentH * scale;
  return { left: (boxW - width) / 2, top: (boxH - height) / 2, width, height };
}

/** `MediaError.code` -> readable reason (§7.3 "any player error falls back"). */
export function mediaErrorReason(code: number | null | undefined): string {
  switch (code) {
    case 1:
      return t("player.reason.aborted");
    case 2:
      return t("player.reason.network");
    case 3:
      return t("player.reason.decode");
    case 4:
      return t("player.reason.notSupported");
    default:
      return t("player.reason.unknown");
  }
}

/** Codec-specific advice for an undecodable source (MVP without Tier 3). */
export function codecAdvice(codec: string): string {
  const c = codec.toLowerCase();
  if (c === "hevc" || c === "h265") return t("player.advice.hevc");
  if (c === "av1") return t("player.advice.av1");
  if (c === "vp9") return t("player.advice.vp9");
  return t("player.advice.generic");
}

/**
 * §7.3 "an undecodable codec shows a specific message (which codec, why,
 * what to do), never a black player".
 */
export function unsupportedMessage(codec: string, reason: string): string {
  const name = codec ? codec.toUpperCase() : t("player.unknownCodec");
  return t("player.unsupported", { codec: name, reason, advice: codecAdvice(codec) });
}

/** Tier name for status text. */
export function tierLabel(tier: PlaybackTier): string {
  return t(`player.tier.${tier}`);
}

/** "about 2 min left" style ETA for the Preparing preview bar (§6.4). */
export function formatEta(sec: number | null | undefined): string {
  if (sec === null || sec === undefined || !Number.isFinite(sec) || sec < 0) return t("player.eta.unknown");
  if (sec < 5) return t("player.eta.almost");
  if (sec < 90) return t("player.eta.seconds", { n: Math.round(sec) });
  return t("player.eta.minutes", { n: Math.round(sec / 60) });
}
