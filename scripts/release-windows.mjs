#!/usr/bin/env node
// Windows x64 release: builds the per-user NSIS installer (rustcut.exe plus the ffmpeg/ffprobe
// sidecars from src-tauri/binaries) and collects what a release uploads in
// <target>/release-assets/:
//
//   RustCut-windows-x64-setup.exe         the installer, under a name that stays the same across
//                                         versions so install.ps1 can fetch
//                                         .../releases/latest/download/RustCut-windows-x64-setup.exe
//   RustCut-windows-x64-setup.exe.sha256  its checksum, which install.ps1 verifies
//   install.ps1                           the one-line installer: irm <url>/install.ps1 | iex
//
// Usage: node scripts/release-windows.mjs [--skip-build]
//
// makensis stages the whole uncompressed payload (about 460 MB with the stock FFmpeg build) in
// TEMP, so the build points TEMP/TMP into the target folder instead of the system drive.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, createReadStream, existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ASSET = "RustCut-windows-x64-setup.exe";
const TRIPLE = "x86_64-pc-windows-msvc";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const target = resolve(process.env.CARGO_TARGET_DIR ?? join(root, "target"));
const fail = (message) => {
  console.error(`✗ ${message}`);
  process.exit(1);
};

if (process.platform !== "win32") fail("Run this on Windows: it builds the Windows x64 installer.");

const { productName, version } = JSON.parse(readFileSync(join(root, "src-tauri", "tauri.conf.json"), "utf8"));
for (const name of ["ffmpeg", "ffprobe"]) {
  const sidecar = join(root, "src-tauri", "binaries", `${name}-${TRIPLE}.exe`);
  if (!existsSync(sidecar)) fail(`${sidecar} is missing: run \`npm run sidecars\` first.`);
}

if (!process.argv.includes("--skip-build")) {
  const temp = join(target, "nsis-temp");
  mkdirSync(temp, { recursive: true });
  const build = spawnSync("npx tauri build --bundles nsis", {
    cwd: root,
    stdio: "inherit",
    shell: true,
    env: { ...process.env, TEMP: temp, TMP: temp },
  });
  rmSync(temp, { recursive: true, force: true });
  if (build.status !== 0) fail(`tauri build failed (exit code ${build.status}).`);
}

const installer = join(target, "release", "bundle", "nsis", `${productName}_${version}_x64-setup.exe`);
if (!existsSync(installer)) fail(`${installer} is missing: run without --skip-build.`);

const out = join(target, "release-assets");
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
copyFileSync(installer, join(out, ASSET));
const hash = createHash("sha256");
for await (const chunk of createReadStream(join(out, ASSET))) hash.update(chunk);
writeFileSync(join(out, `${ASSET}.sha256`), `${hash.digest("hex")}  ${ASSET}\n`);
copyFileSync(join(root, "install.ps1"), join(out, "install.ps1"));

console.log(`\n${productName} ${version} release assets in ${out}:`);
for (const file of [ASSET, `${ASSET}.sha256`, "install.ps1"]) {
  const size = statSync(join(out, file)).size;
  console.log(`  ${file}  ${size >= 1 << 20 ? `${(size / (1 << 20)).toFixed(1)} MB` : `${size} bytes`}`);
}
console.log(`\nUpload all three to the release tagged v${version}, for example:`);
const files = [ASSET, `${ASSET}.sha256`, "install.ps1"].map((file) => `"${join(out, file)}"`).join(" ");
console.log(`  gh release create v${version} ${files} --title "${productName} ${version}"`);
