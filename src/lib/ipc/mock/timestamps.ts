// Timestamp export (EXP-16: YouTube chapters, CSV, CMX3600 EDL, FCPXML) and
// import (EXP-17: `12:10-13:05 Jump scare`, chapter lines, CSV).

import type { Clip, ImportPreview, ImportedClip, Project, TimestampExport, TimestampFormat } from "$lib/bindings";
import { formatTimecode, roundMs } from "$lib/util/time";
import { frameRate } from "./media";
import { formatSeconds, baseName, stripExtension, toMs } from "./util";

/** YouTube chapter rule: each chapter at least this long (EXP-16). */
export const MIN_CHAPTER_SEC = 10;

const p2 = (n: number) => String(n).padStart(2, "0");

function clipLabel(c: Clip): string {
  return c.name.trim() === "" ? `Clip ${p2(c.index)}` : c.name.trim();
}

/** Included clips by In, then number (the order every timestamp format uses). */
function ordered(project: Project): Clip[] {
  return project.clips
    .filter((c) => c.selected_for_export)
    .sort((a, b) => toMs(a.start_sec) - toMs(b.start_sec) || a.index - b.index);
}

function chapterStamp(sec: number, withHours: boolean): string {
  const s = Math.floor(Math.max(0, sec));
  const h = Math.floor(s / 3600);
  return withHours ? `${h}:${p2(Math.floor(s / 60) % 60)}:${p2(s % 60)}` : `${p2(Math.floor(s / 60))}:${p2(s % 60)}`;
}

function youtubeChapters(project: Project): TimestampExport {
  const warnings: string[] = [];
  const entries = ordered(project).map((c) => ({ t: c.start_sec, name: clipLabel(c) }));
  if (entries.length === 0) {
    warnings.push("No clips are included, so there are no chapters to export.");
    return { format: "youtube_chapters", text: "", file_extension: "txt", warnings };
  }
  const first = entries[0];
  if (first && toMs(first.t) > 0) entries.unshift({ t: 0, name: "Intro" });
  if (entries.length < 3) warnings.push("YouTube needs at least three chapters; only " + entries.length + " would be listed.");
  const end = project.source.duration_sec;
  entries.forEach((e, i) => {
    const next = entries[i + 1];
    const len = (next ? next.t : end) - e.t;
    if (next && Math.floor(next.t) <= Math.floor(e.t)) {
      warnings.push(`Chapters "${e.name}" and "${next.name}" start at the same second; YouTube needs ascending times.`);
    } else if (len < MIN_CHAPTER_SEC) {
      warnings.push(`Chapter "${e.name}" is ${formatSeconds(Math.max(0, len))} s long; YouTube needs at least ${MIN_CHAPTER_SEC} s.`);
    }
  });
  const withHours = entries.some((e) => e.t >= 3600);
  const text = entries.map((e) => `${chapterStamp(e.t, withHours)} ${e.name}`).join("\n") + "\n";
  return { format: "youtube_chapters", text, file_extension: "txt", warnings };
}

function csvCell(s: string): string {
  return /[",\r\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

function csvExport(project: Project): TimestampExport {
  const tagLabel = (c: Clip) => project.tags.find((t) => t.id === c.tag_id)?.label ?? "";
  const rows = [["index", "name", "start", "end", "duration_sec", "tag"]];
  for (const c of ordered(project)) {
    rows.push([
      String(c.index),
      c.name,
      formatTimecode(c.start_sec),
      formatTimecode(c.end_sec),
      formatSeconds(roundMs(c.end_sec - c.start_sec)),
      tagLabel(c),
    ]);
  }
  const warnings = rows.length === 1 ? ["No clips are included, so the CSV only has a header row."] : [];
  return { format: "csv", text: rows.map((r) => r.map(csvCell).join(",")).join("\r\n") + "\r\n", file_extension: "csv", warnings };
}

function frameTc(sec: number, fps: number): string {
  const total = Math.round(sec * fps);
  const f = total % fps;
  const s = Math.floor(total / fps);
  return `${p2(Math.floor(s / 3600))}:${p2(Math.floor(s / 60) % 60)}:${p2(s % 60)}:${p2(f)}`;
}

function edlExport(project: Project): TimestampExport {
  const fps = Math.max(1, Math.round(frameRate(project.source)));
  const title = stripExtension(baseName(project.source.path));
  const lines = [`TITLE: ${title}`, "FCM: NON-DROP FRAME", ""];
  let record = 0;
  ordered(project).forEach((c, i) => {
    const dur = c.end_sec - c.start_sec;
    lines.push(
      `${String(i + 1).padStart(3, "0")}  AX       AA/V  C        ${frameTc(c.start_sec, fps)} ${frameTc(c.end_sec, fps)} ${frameTc(record, fps)} ${frameTc(record + dur, fps)}`,
      `* FROM CLIP NAME: ${clipLabel(c)}`,
      "",
    );
    record += dur;
  });
  const warnings = ["Frame counts assume a constant frame rate of " + fps + " fps."];
  return { format: "edl", text: lines.join("\n"), file_extension: "edl", warnings };
}

const xml = (s: string) =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

function fcpxmlExport(project: Project): TimestampExport {
  const fps = Math.max(1, Math.round(frameRate(project.source)));
  const t = (sec: number) => `${Math.round(sec * fps)}/${fps}s`;
  const { video } = project.source;
  const title = xml(stripExtension(baseName(project.source.path)));
  let offset = 0;
  const clips = ordered(project).map((c) => {
    const dur = c.end_sec - c.start_sec;
    const line = `          <asset-clip ref="r2" name="${xml(clipLabel(c))}" offset="${t(offset)}" start="${t(c.start_sec)}" duration="${t(dur)}" format="r1"/>`;
    offset += dur;
    return line;
  });
  const text = [
    '<?xml version="1.0" encoding="UTF-8"?>',
    "<!DOCTYPE fcpxml>",
    '<fcpxml version="1.9">',
    "  <resources>",
    `    <format id="r1" name="FFVideoFormat${video.display_height}p${fps}" frameDuration="1/${fps}s" width="${video.display_width}" height="${video.display_height}"/>`,
    `    <asset id="r2" name="${title}" start="0s" duration="${t(project.source.duration_sec)}" hasVideo="1" hasAudio="1" format="r1">`,
    `      <media-rep kind="original-media" src="${xml(encodeURI(`file:///${project.source.path.replace(/\\/g, "/")}`))}"/>`,
    "    </asset>",
    "  </resources>",
    "  <library>",
    `    <event name="${title}">`,
    `      <project name="${title} clips">`,
    `        <sequence format="r1" duration="${t(offset)}" tcStart="0s" tcFormat="NDF">`,
    "          <spine>",
    ...clips,
    "          </spine>",
    "        </sequence>",
    "      </project>",
    "    </event>",
    "  </library>",
    "</fcpxml>",
    "",
  ].join("\n");
  return { format: "fcpxml", text, file_extension: "fcpxml", warnings: [] };
}

/** EXP-16: export the included clips in `format`. */
export function exportTimestamps(project: Project, format: TimestampFormat): TimestampExport {
  switch (format) {
    case "youtube_chapters":
      return youtubeChapters(project);
    case "csv":
      return csvExport(project);
    case "edl":
      return edlExport(project);
    case "fcpxml":
      return fcpxmlExport(project);
  }
}

// -------------------------------------------------------------------- import

/** Parse `SS`, `MM:SS`, `H:MM:SS` (optionally with `.fff`) into seconds. */
export function parseTimecode(input: string): number | null {
  const s = input.trim().replace(",", ".");
  if (!/^\d+(?::\d{1,2}){0,2}(?:\.\d+)?$/.test(s)) return null;
  const parts = s.split(":");
  const last = Number(parts[parts.length - 1]);
  const nums = parts.slice(0, -1).map(Number);
  if (parts.length > 1 && last >= 60) return null;
  const value = parts.length === 1 ? last : parts.length === 2 ? (nums[0] ?? 0) * 60 + last : (nums[0] ?? 0) * 3600 + (nums[1] ?? 0) * 60 + last;
  return roundMs(value);
}

const T = String.raw`(\d+(?::\d{1,2}){0,2}(?:[.,]\d+)?)`;
const RANGE = new RegExp(String.raw`^\s*[\[(]?${T}\s*[-–—]\s*${T}[\])]?\s*[-–—:|,]?\s*(.*)$`);
const CHAPTER = new RegExp(String.raw`^\s*[\[(]?${T}[\])]?\s*[-–—:|]?\s*(.*)$`);
const HELP = "Use 12:10-13:05 Name, 12:10 Name, or CSV with start,end,name.";

function splitCsv(line: string): string[] {
  const cells: string[] = [];
  let cur = "";
  let quoted = false;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (quoted) {
      if (ch === '"' && line[i + 1] === '"') {
        cur += '"';
        i++;
      } else if (ch === '"') quoted = false;
      else cur += ch;
    } else if (ch === '"') quoted = true;
    else if (ch === ",") {
      cells.push(cur);
      cur = "";
    } else cur += ch;
  }
  cells.push(cur);
  return cells.map((c) => c.trim());
}

interface RawItem {
  line: number;
  start: number | null;
  /** `null` = "until the next start" (chapter lines). */
  end: number | null;
  name: string;
  error: string | null;
  text: string;
}

function parseLine(line: number, text: string, header: string[] | null): RawItem {
  const bad = (error: string): RawItem => ({ line, start: null, end: null, name: text.trim(), error, text });
  if (header) {
    const cells = splitCsv(text);
    const col = (names: string[]) => header.findIndex((h) => names.includes(h));
    const [si, ei, ni] = [col(["start", "in"]), col(["end", "out"]), col(["name", "title", "label"])];
    const start = si >= 0 ? parseTimecode(cells[si] ?? "") : null;
    const end = ei >= 0 ? parseTimecode(cells[ei] ?? "") : null;
    if (start === null) return bad(`Line ${line}: the start time "${cells[si] ?? ""}" is not a time. ${HELP}`);
    return { line, start, end, name: ni >= 0 ? (cells[ni] ?? "") : "", error: null, text };
  }
  const range = RANGE.exec(text);
  if (range) {
    const [s, e] = [parseTimecode(range[1] ?? ""), parseTimecode(range[2] ?? "")];
    if (s !== null && e !== null) return { line, start: s, end: e, name: (range[3] ?? "").trim(), error: null, text };
  }
  if (text.includes(",")) {
    const cells = splitCsv(text);
    const [s, e] = [parseTimecode(cells[0] ?? ""), parseTimecode(cells[1] ?? "")];
    if (s !== null && e !== null) return { line, start: s, end: e, name: cells.slice(2).join(", "), error: null, text };
  }
  const chapter = CHAPTER.exec(text);
  const start = chapter ? parseTimecode(chapter[1] ?? "") : null;
  if (chapter && start !== null) return { line, start, end: null, name: (chapter[2] ?? "").trim(), error: null, text };
  return bad(`Line ${line}: no time found. ${HELP}`);
}

/**
 * EXP-17: parse pasted text or CSV into a preview. Missing ends become the
 * next start (or the source end); each item is validated like `add_clip`
 * (ends clamp to the source; In past the end, Out <= In and too-short clips
 * carry an `error` and are skipped on import).
 */
export function parseTimestampText(text: string, durationSec: number, minClipSec: number): ImportPreview {
  const lines = text.split(/\r?\n/);
  let header: string[] | null = null;
  const raw: RawItem[] = [];
  lines.forEach((l, i) => {
    if (l.trim() === "") return;
    if (header === null && raw.length === 0 && l.includes(",")) {
      const cells = splitCsv(l).map((c) => c.toLowerCase());
      if (cells.includes("start") || cells.includes("in")) {
        header = cells;
        return;
      }
    }
    raw.push(parseLine(i + 1, l, header));
  });
  const items: ImportedClip[] = raw.map((r, i) => {
    const fail = (error: string): ImportedClip => ({
      line: r.line,
      start_sec: r.start ?? 0,
      end_sec: r.end ?? r.start ?? 0,
      name: r.name,
      error,
    });
    if (r.error || r.start === null) return fail(r.error ?? `Line ${r.line}: no time found. ${HELP}`);
    let end = r.end;
    if (end === null) {
      const next = raw.slice(i + 1).find((n) => n.start !== null && n.start > (r.start as number));
      end = next && next.start !== null ? next.start : durationSec;
    }
    end = Math.min(end, durationSec);
    if (toMs(r.start) >= toMs(durationSec)) {
      return fail(`Line ${r.line}: ${formatTimecode(r.start)} is past the end of the video (${formatTimecode(durationSec)}).`);
    }
    if (toMs(end) <= toMs(r.start)) {
      return fail(`Line ${r.line}: Out (${formatTimecode(end)}) must be after In (${formatTimecode(r.start)}).`);
    }
    if (toMs(end) - toMs(r.start) < toMs(minClipSec)) {
      return fail(`Line ${r.line}: clips must be at least ${formatSeconds(minClipSec)} s long.`);
    }
    return { line: r.line, start_sec: r.start, end_sec: end, name: r.name, error: null };
  });
  return { items, valid_count: items.filter((i) => i.error === null).length };
}
