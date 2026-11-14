// Pre-flight (EXP-11) for the mock: names (EXP-05..07), the checks of the
// Export dialog and the size / time estimates.

import type {
  Clip,
  ExportSettings,
  PlannedOutput,
  PreflightItem,
  PreflightReport,
  Preset,
  Project,
  SourceStatus,
} from "$lib/bindings";
import { formatTimecode } from "$lib/util/time";
import { leadInFor } from "./media";
import { indexWidth, outOfRangeIds } from "./store";
import {
  TemplateError,
  renderName,
  resolveOutputs,
  type NameContext,
  type OutputRequest,
} from "./naming";
import { isVertical, normalizeExportSettings, presetOf } from "./model";
import { defaultOutputDir } from "./sources";
import { baseName, fail, formatSeconds, stripExtension, toMs } from "./util";

/** Audio bitrate assumed for re-encodes (AAC, bits per second). */
export const AUDIO_BITRATE_BPS = 192_000;
/** Fake free space of the output drive. */
export const DEFAULT_FREE_BYTES = 120 * 1024 ** 3;
/** Windows MAX_PATH (edge-case table). */
export const MAX_PATH_CHARS = 259;

/** Video `maxrate` of the re-encode presets (§7.6): 16 Mbps at 1080p, else 8 Mbps. */
export function videoMaxrateBps(preset: Preset, settings: ExportSettings, sourceHeight: number): number {
  if (isVertical(preset)) {
    if (settings.output_resolution === "1080x1920") return 16_000_000;
    if (settings.output_resolution === "720x1280") return 8_000_000;
  }
  return sourceHeight >= 1080 ? 16_000_000 : 8_000_000;
}

/** Everything `previewExport` / `startExport` need from the backend. */
export interface PlanEnv {
  project: Project;
  sourceStatus: SourceStatus;
  /** Does a file exist in the (simulated) output folder? */
  exists: (path: string) => boolean;
  now: Date;
  /** Free bytes of the output drive (`null` = unknown). */
  freeBytes: (dir: string) => number | null;
}

export interface ExportPlan {
  report: PreflightReport;
  /** Clips that will actually be processed (action != skip, in range), in clip-number order. */
  runnable: Clip[];
  /** Clips skipped by the conflict policy. */
  policySkipped: string[];
  settings: ExportSettings;
  preset: Preset;
}

const item = (check: PreflightItem["check"], level: PreflightItem["level"], message: string, clip_ids: string[] = []): PreflightItem => ({
  check,
  level,
  message,
  clip_ids,
});

/** Build the report and the list of clips to run. Unknown clip ids are `not_found`. */
export function planExport(env: PlanEnv, clipIds: string[], rawSettings: ExportSettings): ExportPlan {
  const { project } = env;
  const settings = normalizeExportSettings(rawSettings);
  const preset = presetOf(settings);
  const clips: Clip[] = [];
  for (const id of new Set(clipIds)) {
    const c = project.clips.find((x) => x.id === id);
    if (!c) throw fail("not_found", "A clip chosen for export no longer exists. Refresh the queue and try again.");
    clips.push(c);
  }
  clips.sort((a, b) => a.index - b.index);

  const dir = settings.output_dir ?? project.export_settings.output_dir ?? defaultOutputDir(project.source);
  const items: PreflightItem[] = [];
  const outOfRange = new Set(outOfRangeIds(project));
  const inRange = clips.filter((c) => !outOfRange.has(c.id));
  const excluded = clips.filter((c) => outOfRange.has(c.id)).map((c) => c.id);

  // --- nothing_to_export / out_of_range
  if (inRange.length === 0) {
    items.push(
      item(
        "nothing_to_export",
        "error",
        clips.length === 0
          ? "No clips are included. Tick at least one clip in the queue."
          : "Every chosen clip starts after the end of the source, so nothing can be exported.",
      ),
    );
  }
  if (excluded.length > 0) {
    items.push(
      item(
        "out_of_range",
        "warning",
        `${excluded.length} clip${excluded.length === 1 ? " starts" : "s start"} after the end of the source (${formatTimecode(project.source.duration_sec)}) and will be skipped. Relink the correct video to export ${excluded.length === 1 ? "it" : "them"}.`,
        excluded,
      ),
    );
  }

  // --- source
  if (env.sourceStatus.kind === "missing") {
    items.push(item("source_present", "error", "The source video is missing. Use Locate… to relink it before exporting."));
  } else {
    items.push(item("source_present", "ok", "Source video found."));
    items.push(item("source_unchanged", "ok", "Source video matches the project (size and fingerprint unchanged)."));
  }
  items.push(item("output_writable", "ok", `Output folder is writable: ${dir}`));

  // --- names (EXP-05..07)
  const ext = "mp4";
  const sourceName = stripExtension(baseName(project.source.path));
  const width = indexWidth(project);
  const tagLabel = (c: Clip) => project.tags.find((t) => t.id === c.tag_id)?.label ?? null;
  const requests: OutputRequest[] = [];
  let templateError: string | null = null;
  try {
    for (const c of inRange) {
      const ctx: NameContext = {
        index: c.index,
        indexWidth: width,
        name: c.name,
        sourceName,
        startSec: c.start_sec,
        endSec: c.end_sec,
        date: env.now,
        tagLabel: tagLabel(c),
        preset,
      };
      requests.push({
        clipId: c.id,
        index: c.index,
        baseName: renderName(settings.naming_template, ctx, settings.spaces_to_underscores),
        previousOutput: c.last_export?.output_path ?? null,
      });
    }
  } catch (e) {
    templateError = e instanceof TemplateError ? e.message : String(e);
  }
  const resolved = templateError ? [] : resolveOutputs(requests, ext, dir, settings.on_conflict, env.exists);
  if (templateError) {
    items.push(item("names_resolved", "error", templateError));
  } else if (inRange.length > 0) {
    const renamed = resolved.filter((r, i) => r.fileName !== `${requests[i]?.baseName}.${ext}`);
    items.push(
      item(
        "names_resolved",
        "ok",
        renamed.length > 0
          ? `File names resolved; ${renamed.length} got a numeric suffix to avoid a clash.`
          : "File names resolved.",
      ),
    );
  }
  const tooLong = resolved.filter((r) => r.outputPath.length > MAX_PATH_CHARS);
  if (tooLong.length > 0) {
    items.push(
      item(
        "path_length",
        "error",
        `${tooLong.length} output path${tooLong.length === 1 ? " is" : "s are"} longer than ${MAX_PATH_CHARS} characters, which Windows can't write. Choose a shorter output folder or naming template.`,
        tooLong.map((r) => r.clipId),
      ),
    );
  }
  const skipped = resolved.filter((r) => r.action === "skip").map((r) => r.clipId);
  const runnable = inRange.filter((c) => !skipped.includes(c.id));
  if (skipped.length > 0) {
    items.push(item("names_resolved", "warning", `${skipped.length} file${skipped.length === 1 ? "" : "s"} already exist and will be skipped.`, skipped));
  }

  // --- source-dependent warnings
  if (project.source.video.hdr && preset !== "lossless") {
    items.push(item("hdr_reencode", "warning", "The source is HDR. Re-encoding converts it to SDR (BT.709) and colours may look different."));
  }
  if (settings.max_duration_sec !== null) {
    const max = settings.max_duration_sec;
    const long = inRange.filter((c) => toMs(c.end_sec) - toMs(c.start_sec) > toMs(max));
    if (long.length > 0) {
      items.push(
        item(
          "max_duration",
          "warning",
          `${long.length} clip${long.length === 1 ? " is" : "s are"} longer than the platform maximum of ${formatSeconds(max)} s.`,
          long.map((c) => c.id),
        ),
      );
    }
  }
  const leadIns = new Map<string, number>();
  const outputs: PlannedOutput[] = resolved.map((r) => {
    const clip = inRange.find((c) => c.id === r.clipId) as Clip;
    const leadIn = preset === "lossless" && !settings.snap_in_to_keyframe ? leadInFor(clip.start_sec) : 0;
    leadIns.set(clip.id, leadIn);
    return {
      clip_id: r.clipId,
      index: r.index,
      file_name: r.fileName,
      output_path: r.outputPath,
      action: r.action,
      lead_in_sec: preset === "lossless" ? leadIn : null,
    };
  });
  if (preset === "lossless" && [...leadIns.values()].some((v) => v > 0)) {
    items.push(item("lossless_lead_in", "ok", "Lossless cuts start at the keyframe at or before In, so some clips begin with extra footage (see the lead-in column)."));
  }

  // --- estimate (EXP-11)
  const sumDur = runnable.reduce((s, c) => s + (c.end_sec - c.start_sec), 0);
  let estimate: number;
  if (preset === "lossless") {
    const bitrate = project.source.bit_rate ?? 8_000_000;
    const withLeadIn = runnable.reduce((s, c) => s + (c.end_sec - c.start_sec) + (leadIns.get(c.id) ?? 0), 0);
    estimate = (bitrate / 8) * withLeadIn;
  } else {
    const maxrate = videoMaxrateBps(preset, settings, project.source.video.display_height);
    estimate = ((maxrate + AUDIO_BITRATE_BPS) * sumDur + maxrate * 2 * runnable.length) / 8;
  }
  estimate = Math.ceil(estimate);
  const free = env.freeBytes(dir);
  if (free !== null && runnable.length > 0) {
    if (free < estimate * 1.2) {
      items.push(item("free_space", "error", `Not enough free space in ${dir}: about ${fmtBytes(estimate)} needed (plus 20% headroom), ${fmtBytes(free)} free.`));
    } else {
      items.push(item("free_space", "ok", `Enough free space: ${fmtBytes(free)} free, about ${fmtBytes(estimate)} needed.`));
    }
  }
  const seconds =
    runnable.length === 0
      ? 0
      : preset === "lossless"
        ? Math.max(1, Math.ceil(estimate / (200 * 1024 * 1024)))
        : Math.max(1, Math.ceil(sumDur / 2 / settings.parallel_jobs));

  const report: PreflightReport = {
    items,
    can_start: !items.some((i) => i.level === "error"),
    outputs,
    output_dir: dir,
    extension: ext,
    preset,
    estimate_bytes: estimate,
    free_bytes: free,
    estimate_seconds: seconds,
    offer_aac_audio: false,
    excluded_clip_ids: [...excluded, ...skipped],
  };
  return { report, runnable, policySkipped: skipped, settings, preset };
}

function fmtBytes(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v.toFixed(u === 0 ? 0 : 1)} ${units[u]}`;
}
