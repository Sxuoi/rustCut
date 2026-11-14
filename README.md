# RustCut

Non-destructive batch video clipper: play a long recording, mark In/Out as often
as you like, name each clip in the Clip Queue, and export every clip as its own
file in one action — losslessly at disk speed, or frame-accurately and
pre-framed for vertical platforms (9:16 crop or blur).

Built from `docs/PRD.md` (v1.3). Architecture and contracts: `docs/ARCHITECTURE.md`.

Stack: Rust (edition 2024) · Tauri v2 · Svelte 5 + TypeScript + Tailwind v4 ·
FFmpeg/FFprobe as sidecars.

## Install (Windows 10/11 x64)

In PowerShell:

```powershell
irm https://github.com/Sxuoi/rustcut/releases/latest/download/install.ps1 | iex
```

This installs RustCut for the current user, without administrator rights, in
`%LOCALAPPDATA%\RustCut`, with its own FFmpeg and FFprobe and with Start menu and desktop
shortcuts. The script checks the download against its published SHA-256 and checks that the
bundled FFmpeg runs. Run the same line again to update.

To pass options, run the script as a script block:

```powershell
& ([scriptblock]::Create((irm https://github.com/Sxuoi/rustcut/releases/latest/download/install.ps1))) -Uninstall
```

The options are:
- `-Version 0.1.0`: install that release instead of the latest one.
- `-InstallDir <folder>`: install somewhere other than `%LOCALAPPDATA%\RustCut`.
- `-NoShortcuts`: skip the Start menu and desktop shortcuts.
- `-Uninstall`: remove RustCut.

You can also uninstall from Settings > Apps > Installed apps. Either way, settings and caches are
kept in `%APPDATA%\app.rustcut.desktop` and `%LOCALAPPDATA%\app.rustcut.desktop`.

The installer isn't code-signed yet. Running it through the one-liner is fine. If you download
`RustCut-windows-x64-setup.exe` from the release page and open it, SmartScreen warns you first.

The installer includes FFmpeg's `ffmpeg.exe` and `ffprobe.exe`, which are licensed under GPL
version 3. [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) has their license and where to get the
source.

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

## Release (Windows)

```bash
npm run release:windows
```

This builds the per-user NSIS installer (`npx tauri build --bundles nsis`, sidecars included) and
puts the three release files in `target/release-assets/`:
- `RustCut-windows-x64-setup.exe`
- `RustCut-windows-x64-setup.exe.sha256`
- `install.ps1`

Upload them to a release tagged `v<version>`. The one-line installer always fetches the latest
release's files by these names.

## Automation (MCP)

`crates/rustcut-mcp` is an MCP server that lets AI agents clip videos with RustCut's engine, headlessly. It
reads and writes the same project files as the desktop app, so clips an agent adds show up when you open the
video in RustCut. See [crates/rustcut-mcp/README.md](crates/rustcut-mcp/README.md).
