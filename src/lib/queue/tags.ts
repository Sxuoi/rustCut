// MRK-06 tag palette editing: slugified unique ids, default colours,
// validation and the chip letter. Tags always show a label next to their
// colour (§8.5: never colour alone). Pure: no Svelte, no IPC.

import type { Tag } from "$lib/bindings";

/** Colours offered to new tags, in order (all readable on the dark theme). */
export const TAG_PALETTE: readonly string[] = [
  "#ef4444",
  "#f59e0b",
  "#22c55e",
  "#3b82f6",
  "#a855f7",
  "#ec4899",
  "#14b8a6",
  "#f97316",
  "#84cc16",
];

/** Longest label the editor accepts. */
export const MAX_TAG_LABEL = 32;

/**
 * Id from a label: lower-case ASCII-folded letters and digits joined by `-`
 * (`Funny Moments!` → `funny-moments`). Labels without letters or digits → `tag`.
 */
export function slugify(label: string): string {
  const s = label
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, "-")
    .replace(/^-+|-+$/g, "");
  return s === "" ? "tag" : s;
}

/** A slugified id not in `taken`: `funny`, `funny-2`, `funny-3`, … */
export function uniqueTagId(label: string, taken: Iterable<string>): string {
  const used = new Set(taken);
  const base = slugify(label);
  if (!used.has(base)) return base;
  for (let n = 2; ; n++) {
    const id = `${base}-${n}`;
    if (!used.has(id)) return id;
  }
}

/** First palette colour no tag uses yet (cycling when all are used). */
export function nextTagColor(tags: readonly Pick<Tag, "color">[]): string {
  const used = new Set(tags.map((t) => t.color.toLowerCase()));
  return TAG_PALETTE.find((c) => !used.has(c)) ?? TAG_PALETTE[tags.length % TAG_PALETTE.length];
}

/** `#RRGGBB`, the only colour format the schema stores. */
export function isHexColor(s: string): boolean {
  return /^#[0-9a-fA-F]{6}$/.test(s);
}

/** A problem that blocks saving the palette. */
export type TagProblem = "empty_label" | "duplicate_label" | "bad_color";

export interface TagIssue {
  /** Position in the edited list. */
  index: number;
  problem: TagProblem;
}

/** Validate an edited palette: non-empty unique labels (case-insensitive) and `#RRGGBB` colours. */
export function validateTags(tags: readonly Pick<Tag, "label" | "color">[]): TagIssue[] {
  const issues: TagIssue[] = [];
  const seen = new Map<string, number>();
  tags.forEach((t, index) => {
    const label = t.label.trim();
    if (label === "") issues.push({ index, problem: "empty_label" });
    else {
      const key = label.toLocaleLowerCase();
      if (seen.has(key)) issues.push({ index, problem: "duplicate_label" });
      else seen.set(key, index);
    }
    if (!isHexColor(t.color)) issues.push({ index, problem: "bad_color" });
  });
  return issues;
}

/** Edited row of the tag editor; `id` is `null` for tags added in this session. */
export interface TagDraft {
  id: string | null;
  label: string;
  color: string;
}

/**
 * Final tags from the editor rows: existing ids are kept (clips reference
 * them), new tags get slugified ids unique across the whole palette.
 */
export function finalizeTags(drafts: readonly TagDraft[]): Tag[] {
  const taken = new Set(drafts.flatMap((d) => (d.id === null ? [] : [d.id])));
  return drafts.map((d) => {
    const label = d.label.trim();
    if (d.id !== null) return { id: d.id, label, color: d.color.toLowerCase() };
    const id = uniqueTagId(label, taken);
    taken.add(id);
    return { id, label, color: d.color.toLowerCase() };
  });
}

/** Single-letter form of a label for compact chips (always shown with the colour). */
export function tagLetter(label: string): string {
  const first = Array.from(label.trim())[0];
  return first === undefined ? "?" : first.toLocaleUpperCase();
}

/** How many clips use each tag id. */
export function tagUsage(clips: readonly { tag_id: string | null }[]): Map<string, number> {
  const out = new Map<string, number>();
  for (const c of clips) if (c.tag_id !== null) out.set(c.tag_id, (out.get(c.tag_id) ?? 0) + 1);
  return out;
}
