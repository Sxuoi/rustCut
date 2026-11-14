// TypeScript port of crates/rustcut-naming (EXP-05, EXP-06, EXP-07), good
// enough for the mock: template rendering, sanitisation (PRD vectors),
// in-batch duplicate suffixes and existing-file policies.

import type { ConflictPolicy, Preset } from "$lib/bindings";
import { joinPath } from "./util";

/** Maximum file name stem length in UTF-8 bytes (EXP-06 step 6). */
export const MAX_NAME_BYTES = 120;

const RESERVED = new Set([
  "CON",
  "PRN",
  "AUX",
  "NUL",
  ...Array.from({ length: 9 }, (_, i) => `COM${i + 1}`),
  ...Array.from({ length: 9 }, (_, i) => `LPT${i + 1}`),
]);

const VARIABLES = ["index", "name", "source_name", "start", "end", "duration", "date", "tag", "preset"] as const;
const MEDIA_EXTENSIONS = [".mp4", ".mkv", ".mov", ".webm", ".avi", ".flv", ".ts", ".m4v"];

/** An invalid naming template (unknown variable, unbalanced braces). */
export class TemplateError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "TemplateError";
  }
}

const utf8Length = (s: string) => new TextEncoder().encode(s).length;

/** Truncate to `maxBytes` UTF-8 bytes on a character boundary. */
export function truncateBytes(s: string, maxBytes: number): string {
  let out = "";
  let used = 0;
  for (const ch of s) {
    const n = utf8Length(ch);
    if (used + n > maxBytes) break;
    out += ch;
    used += n;
  }
  return out;
}

const trimEnds = (s: string) => s.replace(/^[_. ]+/, "").replace(/[_. ]+$/, "");

/** EXP-06 steps 1-6. An empty result is returned as "" (callers apply step 7). */
export function sanitizeComponent(input: string, spacesToUnderscores: boolean): string {
  let s = input.normalize("NFC");
  // eslint-disable-next-line no-control-regex
  s = s.replace(/[<>:"/\\|?*\u0000-\u001f\u007f-\u009f]/g, "_");
  if (spacesToUnderscores) s = s.replace(/\s/g, "_");
  s = s.replace(/_+/g, "_");
  s = trimEnds(truncateBytes(trimEnds(s), MAX_NAME_BYTES));
  const dot = s.indexOf(".");
  const stem = dot < 0 ? s : s.slice(0, dot);
  // Appended after truncation so the trailing "_" is not trimmed again.
  if (RESERVED.has(stem.toUpperCase())) s = `${stem}_${dot < 0 ? "" : s.slice(dot)}`;
  return s;
}

type Segment = { literal: string } | { variable: string };

function parseTemplate(template: string): Segment[] {
  if (template.trim() === "") throw new TemplateError("The naming template is empty. Use e.g. {index}_{name}.");
  const segments: Segment[] = [];
  let literal = "";
  for (let i = 0; i < template.length; i++) {
    const ch = template[i];
    if (ch === "{") {
      const close = template.indexOf("}", i + 1);
      const inner = close < 0 ? "" : template.slice(i + 1, close);
      if (close < 0 || inner.includes("{")) {
        throw new TemplateError("The naming template has an unbalanced or nested brace. Variables look like {name}.");
      }
      if (!(VARIABLES as readonly string[]).includes(inner)) {
        throw new TemplateError(
          `Unknown variable {${inner}} in the naming template. Available: ${VARIABLES.map((v) => `{${v}}`).join(", ")}.`,
        );
      }
      if (literal) segments.push({ literal });
      literal = "";
      segments.push({ variable: inner });
      i = close;
    } else if (ch === "}") {
      throw new TemplateError("The naming template has an unbalanced or nested brace. Variables look like {name}.");
    } else {
      literal += ch;
    }
  }
  if (literal) segments.push({ literal });
  return segments;
}

/** Errors for bad templates; warnings when it ends with a media extension. */
export function validateTemplate(template: string): { warnings: string[] } {
  parseTemplate(template);
  const lower = template.trimEnd().toLowerCase();
  const ext = MEDIA_EXTENSIONS.find((e) => lower.endsWith(e));
  return {
    warnings: ext ? [`The template ends with "${ext}", but the extension is appended automatically.`] : [],
  };
}

/** Values the template variables resolve to (EXP-05). */
export interface NameContext {
  index: number;
  /** Zero-padding of `{index}` (`Project::index_width`). */
  indexWidth: number;
  name: string;
  sourceName: string;
  startSec: number;
  endSec: number;
  /** Export date. */
  date: Date;
  tagLabel: string | null;
  preset: Preset;
}

export function paddedIndex(ctx: Pick<NameContext, "index" | "indexWidth">): string {
  return String(ctx.index).padStart(Math.max(2, ctx.indexWidth), "0");
}

const p2 = (n: number) => String(n).padStart(2, "0");

/** `HH-MM-SS`, whole seconds truncated (920.12 -> 00-15-20). */
export function formatHmsDashed(sec: number): string {
  const s = Number.isFinite(sec) && sec > 0 ? Math.floor(sec) : 0;
  return `${p2(Math.floor(s / 3600))}-${p2(Math.floor(s / 60) % 60)}-${p2(s % 60)}`;
}

/** Whole seconds, rounded (35.68 -> 36). */
export function formatWholeSeconds(sec: number): string {
  return String(Number.isFinite(sec) && sec > 0 ? Math.round(sec) : 0);
}

/** EXP-05/06: render the base file name (no extension) for one clip. */
export function renderName(template: string, ctx: NameContext, spacesToUnderscores: boolean): string {
  const segments = parseTemplate(template);
  const san = (s: string) => sanitizeComponent(s, spacesToUnderscores);
  const fallback = `Clip_${paddedIndex(ctx)}`;
  let out = "";
  for (const seg of segments) {
    if ("literal" in seg) {
      out += seg.literal;
      continue;
    }
    switch (seg.variable) {
      case "index":
        out += paddedIndex(ctx);
        break;
      case "name": {
        const n = san(ctx.name);
        out += n === "" ? fallback : n;
        break;
      }
      case "source_name":
        out += san(ctx.sourceName);
        break;
      case "start":
        out += formatHmsDashed(ctx.startSec);
        break;
      case "end":
        out += formatHmsDashed(ctx.endSec);
        break;
      case "duration":
        out += formatWholeSeconds(ctx.endSec - ctx.startSec);
        break;
      case "date":
        out += `${ctx.date.getFullYear()}-${p2(ctx.date.getMonth() + 1)}-${p2(ctx.date.getDate())}`;
        break;
      case "tag":
        out += san(ctx.tagLabel ?? "");
        break;
      case "preset":
        out += ctx.preset;
        break;
    }
  }
  const whole = san(out);
  return whole === "" ? fallback : whole;
}

// ------------------------------------------------------------ EXP-07 resolve

export type ResolvedAction = "write" | "overwrite" | "skip" | "replace_previous";

export interface OutputRequest {
  clipId: string;
  index: number;
  /** Rendered base name (no extension). */
  baseName: string;
  /** Output path this project exported for this clip before, if any. */
  previousOutput: string | null;
}

export interface ResolvedName {
  clipId: string;
  index: number;
  /** Final file name with extension. */
  fileName: string;
  outputPath: string;
  action: ResolvedAction;
}

const key = (s: string) => s.normalize("NFC").toLowerCase();

function withSuffix(base: string, n: number): string {
  const suffix = `_${n}`;
  return truncateBytes(base, MAX_NAME_BYTES - utf8Length(suffix)) + suffix;
}

/**
 * EXP-07: in-batch duplicates get `_2`, `_3`, ... in clip-number order
 * (case-insensitive, NFC); a clip's own previous output is replaced; other
 * existing files follow `policy`.
 */
export function resolveOutputs(
  requests: OutputRequest[],
  ext: string,
  dir: string,
  policy: ConflictPolicy,
  exists: (path: string) => boolean,
): ResolvedName[] {
  const taken = new Set<string>();
  const result = new Map<string, ResolvedName>();
  const ordered = [...requests].sort((a, b) => a.index - b.index);
  for (const req of ordered) {
    const pathFor = (stem: string) => joinPath(dir, `${stem}.${ext}`);
    let stem = req.baseName;
    let n = 1;
    while (taken.has(key(stem))) stem = withSuffix(req.baseName, ++n);
    let action: ResolvedAction = "write";
    const ownPrevious = req.previousOutput !== null && key(req.previousOutput) === key(pathFor(stem));
    if (ownPrevious) {
      action = "replace_previous";
    } else if (exists(pathFor(stem))) {
      if (policy === "overwrite") action = "overwrite";
      else if (policy === "skip") action = "skip";
      else {
        while (taken.has(key(stem)) || exists(pathFor(stem))) stem = withSuffix(req.baseName, ++n);
      }
    }
    taken.add(key(stem));
    result.set(req.clipId, {
      clipId: req.clipId,
      index: req.index,
      fileName: `${stem}.${ext}`,
      outputPath: pathFor(stem),
      action,
    });
  }
  return requests.map((r) => result.get(r.clipId) as ResolvedName);
}
