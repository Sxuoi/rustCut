// Every UI string is externalised (PRD §8.5). Each area owns one message file
// so parallel work never collides: common (fe-shell), timeline (fe-timeline),
// queue + export (fe-queue). Keys are namespaced: "shell.open", "queue.empty".

import common from "./en/common";
import exportMessages from "./en/export";
import queue from "./en/queue";
import timeline from "./en/timeline";

type Messages = Record<string, string>;

const en: Messages = { ...common, ...timeline, ...queue, ...exportMessages };

let active: Messages = en;

/** Look up `key`, interpolating `{name}` placeholders from `vars`. */
export function t(key: string, vars?: Record<string, string | number>): string {
  const template = active[key] ?? key;
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (m, name: string) => (name in vars ? String(vars[name]) : m));
}

export function setLanguage(lang: string): void {
  // Only English ships at launch (Q9); unknown languages fall back to English.
  active = lang === "en" ? en : en;
}
