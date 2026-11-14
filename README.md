# RustCut

Non-destructive batch video clipper: play a long recording, mark In/Out as often
as you like, name each clip in the Clip Queue, and export every clip as its own
file in one action — losslessly at disk speed, or frame-accurately and
pre-framed for vertical platforms (9:16 crop or blur).

Built from `docs/PRD.md` (v1.3). Architecture and contracts: `docs/ARCHITECTURE.md`.

Stack: Rust (edition 2024) · Tauri v2 · Svelte 5 + TypeScript + Tailwind v4 ·
FFmpeg/FFprobe as sidecars.

## Prerequisites

* Rust stable (≥ 1.89) with the MSVC toolchain on Windows (Visual Studio C++ Build Tools)
* Node.js ≥ 20 and npm
* FFmpeg ≥ 6.1 with libx264 (and `zscale` for HDR tone-mapping), e.g. `winget install Gyan.FFmpeg`
* Windows: WebView2 runtime (preinstalled on Windows 11)

## Setup

```bash
npm install
npm run sidecars      # copies ffmpeg/ffprobe into src-tauri/binaries/
npm run bindings      # regenerates src/lib/bindings from rustcut-model
```

## Run

```bash
npm run tauri dev     # desktop app with hot reload
npm run dev           # UI only, in a browser, against the in-memory mock backend
npm run tauri build   # installers in target/release/bundle/
```

## Test

```bash
cargo test            # engine crates (FFmpeg-based tests generate fixtures into .fixtures/)
npm test              # UI unit tests (vitest)
npm run check         # svelte-check / TypeScript
```

## Automation (MCP)

`crates/rustcut-mcp` is an MCP server that lets AI agents clip videos with RustCut's engine, headlessly. It
reads and writes the same project files as the desktop app, so clips an agent adds show up when you open the
video in RustCut. See [crates/rustcut-mcp/README.md](crates/rustcut-mcp/README.md).
