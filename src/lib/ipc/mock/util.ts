// Small helpers shared by the mock backend modules.

import type { CommandError, ErrorCode } from "$lib/bindings";
import { BackendError } from "../backend";

/** `rustcut_model::time::to_ms`: whole milliseconds. */
export function toMs(sec: number): number {
  return Number.isFinite(sec) ? Math.round(sec * 1000) : 0;
}

/** Rust's `f64::round` (half away from zero). */
export function roundAway(v: number): number {
  return v < 0 ? -Math.round(-v) : Math.round(v);
}

export function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}

/** Deterministic 32-bit FNV-1a over the UTF-8 bytes, as 8 hex chars. */
export function hash32(text: string): string {
  let h = 0x811c9dc5;
  for (const byte of new TextEncoder().encode(text)) {
    h ^= byte;
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h.toString(16).padStart(8, "0");
}

/** `crypto.randomUUID()` with a fallback for exotic environments. */
export function newId(): string {
  const c = (globalThis as { crypto?: Crypto }).crypto;
  if (c && typeof c.randomUUID === "function") return c.randomUUID();
  const hex = () =>
    Math.floor(Math.random() * 0x10000)
      .toString(16)
      .padStart(4, "0");
  return `${hex()}${hex()}-${hex()}-4${hex().slice(1)}-a${hex().slice(1)}-${hex()}${hex()}${hex()}`;
}

export function clone<T>(value: T): T {
  return structuredClone(value);
}

/** A BackendError with a specific, actionable message (AC-12). */
export function fail(code: ErrorCode, message: string, detail: string | null = null): BackendError {
  const e: CommandError = { code, message, detail };
  return new BackendError(e);
}

/** `0.5`, `1`, `2.25`: seconds without trailing zeros. */
export function formatSeconds(sec: number): string {
  return String(Number(sec.toFixed(3)));
}

/** Path helpers that accept both separators (mock paths may be Windows or POSIX). */
export function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? path;
}

export function dirName(path: string): string {
  const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (i < 0) return ".";
  return i === 0 ? "/" : path.slice(0, i);
}

export function stripExtension(name: string): string {
  const i = name.lastIndexOf(".");
  return i > 0 ? name.slice(0, i) : name;
}

export function joinPath(dir: string, name: string): string {
  return dir.endsWith("/") || dir.endsWith("\\") ? dir + name : `${dir}/${name}`;
}

/** The subset of `Storage` the mock uses. */
export interface KeyValueStore {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

/** In-memory store: the fallback when `localStorage` is unavailable, and the test double. */
export class MemoryStorage implements KeyValueStore {
  private readonly data = new Map<string, string>();
  getItem(key: string): string | null {
    return this.data.get(key) ?? null;
  }
  setItem(key: string, value: string): void {
    this.data.set(key, value);
  }
  removeItem(key: string): void {
    this.data.delete(key);
  }
}

/** `localStorage` when it works (not in private windows / node), else memory. */
export function defaultStorage(): KeyValueStore {
  try {
    const ls = (globalThis as { localStorage?: Storage }).localStorage;
    if (ls) {
      const probe = "rustcut.mock.probe";
      ls.setItem(probe, "1");
      ls.removeItem(probe);
      return ls;
    }
  } catch {
    // fall through to memory
  }
  return new MemoryStorage();
}

export function readJson<T>(storage: KeyValueStore, key: string): T | null {
  try {
    const raw = storage.getItem(key);
    return raw === null ? null : (JSON.parse(raw) as T);
  } catch {
    return null;
  }
}

export function writeJson(storage: KeyValueStore, key: string, value: unknown): boolean {
  try {
    storage.setItem(key, JSON.stringify(value));
    return true;
  } catch {
    return false;
  }
}
