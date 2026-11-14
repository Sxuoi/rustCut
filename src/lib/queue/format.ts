// Pure formatting helpers for the Clip Queue and the export UI: clip-number
// padding (QUE-01, EXP-05), sizes (EXP-11 "est. ~90 MB"), durations and ETAs
// (EXP-09), percentages and lossless lead-in (MRK-07). Every visible word goes
// through t() (§8.5).

import { t } from "$lib/i18n";

const pluralRules = new Intl.PluralRules("en");

/**
 * Plural-aware t(): looks up `${key}.one` / `${key}.other` (English plural
 * categories; other languages may add `.few`, `.many`, …) with `{count}` set.
 */
export function tp(key: string, count: number, vars: Record<string, string | number> = {}): string {
  const category = pluralRules.select(count);
  const candidate = `${key}.${category}`;
  const all = { count, ...vars };
  const text = t(candidate, all);
  return text === candidate ? t(`${key}.other`, all) : text;
}

/**
 * `{index}` / `#NN` width: max(2, digits of the highest clip number), the
 * same rule as `Project::index_width` (QUE-01, EXP-05).
 */
export function indexWidth(indices: Iterable<number>): number {
  let max = 1;
  for (const i of indices) if (Number.isFinite(i) && i > max) max = i;
  return Math.max(2, String(Math.floor(max)).length);
}

const BYTE_UNITS = ["KB", "MB", "GB", "TB"] as const;

/**
 * Human-readable size with binary multiples (as Windows Explorer shows them):
 * one decimal below 10, whole numbers above (`~90 MB`, `1.2 GB`, `412 GB`).
 * `approx` prefixes `~` for estimates (EXP-11). Invalid input → `—`.
 */
export function formatBytes(bytes: number, approx = false): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  let text: string;
  if (bytes < 1024) {
    text = t("queue.unit.B", { n: Math.round(bytes) });
  } else {
    let v = bytes / 1024;
    let u = 0;
    while (v >= 1024 && u < BYTE_UNITS.length - 1) {
      v /= 1024;
      u++;
    }
    let n = v < 10 ? Math.round(v * 10) / 10 : Math.round(v);
    // 1023.7 MB rounds to 1024 MB: show it as 1 GB instead.
    if (n >= 1024 && u < BYTE_UNITS.length - 1) {
      u++;
      n = Math.round((v / 1024) * 10) / 10;
    }
    text = t(`queue.unit.${BYTE_UNITS[u]}`, { n: Number.isInteger(n) ? String(n) : n.toFixed(1) });
  }
  return approx ? t("queue.format.approx", { value: text }) : text;
}

/**
 * Duration for estimates and ETAs (EXP-09): `4 s`, `1 min 15 s`, `12 min`,
 * `1 h 5 min`. Values under one second show as `1 s`; from 10 minutes on the
 * seconds are dropped. `approx` prefixes `~`. Invalid input → `—`.
 */
export function formatSeconds(sec: number, approx = false): string {
  if (!Number.isFinite(sec) || sec < 0) return "—";
  const s = Math.max(1, Math.round(sec));
  let text: string;
  if (s < 60) {
    text = t("queue.unit.seconds", { s });
  } else if (s < 600) {
    const m = Math.floor(s / 60);
    const r = s % 60;
    text = r === 0 ? t("queue.unit.minutes", { m }) : t("queue.unit.minutesSeconds", { m, s: r });
  } else if (s < 3600) {
    const m = Math.round(s / 60);
    text = m >= 60 ? t("queue.unit.hours", { h: 1 }) : t("queue.unit.minutes", { m });
  } else {
    const totalMin = Math.round(s / 60);
    const h = Math.floor(totalMin / 60);
    const m = totalMin % 60;
    text = m === 0 ? t("queue.unit.hours", { h }) : t("queue.unit.hoursMinutes", { h, m });
  }
  return approx ? t("queue.format.approx", { value: text }) : text;
}

/** Whole percent, rounded down so 100% only shows when finished (EXP-09). */
export function formatPercent(fraction: number): string {
  const f = Number.isFinite(fraction) ? Math.min(1, Math.max(0, fraction)) : 0;
  return t("queue.format.percent", { n: Math.floor(f * 100 + 1e-9) });
}

/** MRK-07 lead-in label: `+1.4 s`. */
export function formatLeadIn(sec: number): string {
  const v = Number.isFinite(sec) && sec > 0 ? Math.round(sec * 10) / 10 : 0;
  return t("queue.format.leadIn", { s: v.toFixed(1) });
}
