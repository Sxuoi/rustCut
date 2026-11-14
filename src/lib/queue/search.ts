// QUE-06 search and sort for the Clip Queue. Sorting changes the view only:
// clip numbers (`index`, QUE-02) are never touched here.

import type { Clip, Tag } from "$lib/bindings";
import { formatTimecode } from "$lib/util/time";
import { parseTimecode, toMs } from "./timecode";

/** Sort keys offered by the queue (QUE-06). */
export type SortKey = "index" | "time" | "duration" | "name";
export type SortDir = "asc" | "desc";

export const SORT_KEYS: readonly SortKey[] = ["index", "time", "duration", "name"];

/** What a search needs besides the clip itself. */
export interface SearchContext {
  tags: readonly Tag[];
}

/** Case- and accent-insensitive form used for name matching. */
export function normalizeText(s: string): string {
  return s.normalize("NFKD").replace(/\p{M}/gu, "").toLowerCase();
}

/** Split a query into tokens; every token must match (AND). */
export function tokenize(query: string): string[] {
  return query.trim().split(/\s+/u).filter((s) => s !== "");
}

/**
 * One token against one clip:
 * * `#12` / `#012` → clip number 12 (`#` alone matches everything);
 * * digits → clip number, else name;
 * * time-like (`15:20`, `1:02:03.5`) → In/Out timecode text, or a range that
 *   contains that time;
 * * anything else → name or tag label (case/accent-insensitive).
 */
export function matchesToken(clip: Clip, token: string, ctx: SearchContext): boolean {
  const hash = /^#(\d*)$/u.exec(token);
  if (hash) return hash[1] === "" || clip.index === Number(hash[1]);
  if (/^\d+$/u.test(token) && clip.index === Number(token)) return true;
  if (/^[\d:.]+$/u.test(token) && token.includes(":")) {
    if (formatTimecode(clip.start_sec).includes(token) || formatTimecode(clip.end_sec).includes(token)) return true;
    const at = parseTimecode(token);
    if (at !== null && toMs(clip.start_sec) <= toMs(at) && toMs(at) <= toMs(clip.end_sec)) return true;
  }
  const needle = normalizeText(token);
  if (normalizeText(clip.name).includes(needle)) return true;
  if (clip.tag_id === null) return false;
  const tag = ctx.tags.find((t) => t.id === clip.tag_id);
  return tag !== undefined && normalizeText(tag.label).includes(needle);
}

/** QUE-06: does `clip` match every token of `query`? An empty query matches. */
export function matchesQuery(clip: Clip, query: string, ctx: SearchContext): boolean {
  return tokenize(query).every((tok) => matchesToken(clip, tok, ctx));
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

function primaryCompare(a: Clip, b: Clip, key: SortKey): number {
  switch (key) {
    case "index":
      return a.index - b.index;
    case "time":
      return toMs(a.start_sec) - toMs(b.start_sec) || toMs(a.end_sec) - toMs(b.end_sec);
    case "duration":
      return toMs(a.end_sec - a.start_sec) - toMs(b.end_sec - b.start_sec);
    case "name":
      return collator.compare(a.name.trim(), b.name.trim());
  }
}

/**
 * Comparator for a sort key and direction. Unnamed clips sort after named
 * ones in both directions when sorting by name; ties fall back to the clip
 * number so the order is always deterministic.
 */
export function compareClips(key: SortKey, dir: SortDir): (a: Clip, b: Clip) => number {
  const sign = dir === "asc" ? 1 : -1;
  return (a, b) => {
    if (key === "name") {
      const ae = a.name.trim() === "";
      const be = b.name.trim() === "";
      if (ae !== be) return ae ? 1 : -1;
    }
    return sign * primaryCompare(a, b, key) || a.index - b.index;
  };
}

/** The rows the queue shows: filtered by `query`, sorted by `key`/`dir`. */
export function visibleClips(
  clips: readonly Clip[],
  opts: { query: string; sortKey: SortKey; sortDir: SortDir; tags: readonly Tag[] },
): Clip[] {
  const ctx: SearchContext = { tags: opts.tags };
  const tokens = tokenize(opts.query);
  const rows = tokens.length === 0 ? [...clips] : clips.filter((c) => tokens.every((tok) => matchesToken(c, tok, ctx)));
  return rows.sort(compareClips(opts.sortKey, opts.sortDir));
}
