#!/usr/bin/env node
// Generates public/sample/sample.mp4 for browser-only UI development (mock
// backend) and the in-browser UI tests: 3 min, 1280x720, 30 fps, 2 s GOP,
// H.264 + AAC (a sine tone with periodic beeps so the waveform has shape),
// with FFmpeg's testsrc counter burned in so the on-screen time is visible.
//
// Usage: node scripts/make-sample.mjs [--force]

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const out = join(root, "public", "sample", "sample.mp4");
if (existsSync(out) && !process.argv.includes("--force")) {
  console.log(`✓ ${out} exists`);
  process.exit(0);
}
mkdirSync(dirname(out), { recursive: true });
const ffmpeg = process.env.FFMPEG_DIR ? join(process.env.FFMPEG_DIR, "ffmpeg") : "ffmpeg";
const args = [
  "-hide_banner", "-nostdin", "-y",
  "-f", "lavfi", "-i", "testsrc=size=1280x720:rate=30:duration=180",
  "-f", "lavfi", "-i", "sine=frequency=220:sample_rate=48000:duration=180",
  "-f", "lavfi", "-i", "sine=frequency=880:beep_factor=4:sample_rate=48000:duration=180",
  "-filter_complex", "[1:a][2:a]amix=inputs=2:weights=1 0.6,volume=0.5[a]",
  "-map", "0:v", "-map", "[a]",
  "-c:v", "libx264", "-preset", "veryfast", "-crf", "28", "-g", "60", "-keyint_min", "60", "-sc_threshold", "0",
  "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "96k", "-movflags", "+faststart",
  out,
];
execFileSync(ffmpeg, args, { stdio: "inherit" });
console.log(`✓ wrote ${out}`);
