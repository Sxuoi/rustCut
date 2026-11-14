// Colour helpers: tag colours always come with a label/letter (MRK-06).

export const NEUTRAL_BAR = "#64748b";

/** `#RRGGBB` -> [r, g, b] 0..255, or null. */
export function parseHex(hex: string): [number, number, number] | null {
  const m = /^#([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function lin(c: number): number {
  const s = c / 255;
  return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
}

/** Black or white, whichever reads better on `hex`. */
export function contrastText(hex: string): "#000000" | "#ffffff" {
  const rgb = parseHex(hex);
  if (!rgb) return "#ffffff";
  const l = 0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2]);
  return l > 0.179 ? "#000000" : "#ffffff";
}

/** First letter of a tag label ("Funny" -> "F"); "" if empty. */
export function tagLetter(label: string | undefined | null): string {
  const first = Array.from((label ?? "").trim())[0];
  return first ? first.toUpperCase() : "";
}
