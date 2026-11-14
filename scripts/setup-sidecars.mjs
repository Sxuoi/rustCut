#!/usr/bin/env node
// Copies ffmpeg/ffprobe into src-tauri/binaries/<name>-<target-triple><ext>
// so Tauri can bundle them as sidecars (bundle.externalBin).
//
// Lookup order: $FFMPEG_DIR, PATH, WinGet Gyan.FFmpeg package (Windows).
// Usage: node scripts/setup-sidecars.mjs [--force]

import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outDir = join(root, "src-tauri", "binaries");
const isWin = process.platform === "win32";
const ext = isWin ? ".exe" : "";
const force = process.argv.includes("--force");

function targetTriple() {
  try {
    const out = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
    const m = out.match(/^host:\s*(\S+)/m);
    if (m) return m[1];
  } catch {
    /* fall through */
  }
  const arch = { x64: "x86_64", arm64: "aarch64" }[process.arch] ?? process.arch;
  if (isWin) return `${arch}-pc-windows-msvc`;
  if (process.platform === "darwin") return `${arch}-apple-darwin`;
  return `${arch}-unknown-linux-gnu`;
}

function onPath(name) {
  try {
    const cmd = isWin ? "where" : "which";
    const out = execFileSync(cmd, [name], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
    const first = out.split(/\r?\n/).find((l) => l.trim());
    return first ? first.trim() : null;
  } catch {
    return null;
  }
}

function wingetDir() {
  if (!isWin || !process.env.LOCALAPPDATA) return null;
  const pkgs = join(process.env.LOCALAPPDATA, "Microsoft", "WinGet", "Packages");
  if (!existsSync(pkgs)) return null;
  for (const pkg of readdirSync(pkgs)) {
    if (!pkg.startsWith("Gyan.FFmpeg")) continue;
    for (const sub of readdirSync(join(pkgs, pkg))) {
      const bin = join(pkgs, pkg, sub, "bin");
      if (existsSync(join(bin, `ffmpeg${ext}`))) return bin;
    }
  }
  return null;
}

function locate(name) {
  const file = `${name}${ext}`;
  if (process.env.FFMPEG_DIR && existsSync(join(process.env.FFMPEG_DIR, file))) {
    return join(process.env.FFMPEG_DIR, file);
  }
  const p = onPath(name);
  // WinGet "Links" entries are symlinks; resolve to the real binary when possible.
  if (p && existsSync(p) && statSync(p).size > 1024 * 1024) return p;
  const w = wingetDir();
  if (w) return join(w, file);
  return p;
}

const triple = targetTriple();
mkdirSync(outDir, { recursive: true });
let failed = false;
for (const name of ["ffmpeg", "ffprobe"]) {
  const src = locate(name);
  if (!src || !existsSync(src)) {
    console.error(`✗ ${name} not found (set FFMPEG_DIR or put it on PATH)`);
    failed = true;
    continue;
  }
  const dst = join(outDir, `${name}-${triple}${ext}`);
  const same = existsSync(dst) && statSync(dst).size === statSync(src).size;
  if (same && !force) {
    console.log(`✓ ${name}: up to date (${dst})`);
    continue;
  }
  copyFileSync(src, dst);
  console.log(`✓ ${name}: ${src} → ${dst}`);
}
process.exit(failed ? 1 : 0);
