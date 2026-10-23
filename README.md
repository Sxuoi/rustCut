# RustCut / QuickClip

Non-destructive batch video clipper. Mark In/Out on a long VOD, queue named
clips, export them all in one pass. Desktop shell: Tauri v2 + FFmpeg sidecar.

Product spec: `C:\Anna\batch_video_clipper_prd.md` (v1.2.0).

## Layout

```
rustcut/
  Cargo.toml              workspace
  crates/
    rustcut-core/         pure logic: no Tauri, no async, fully unit-tested
      src/
        timecode.rs       HH:MM:SS.mmm parse/format
        clip.rs           clip segment model (non-destructive markers)
        naming.rs         naming templates + filename sanitization
        ffmpeg.rs         FFmpeg command generation, 16:9 canvas + 9:16 crop math
        project.rs        .rustcut JSON schema, atomic save, crash recovery
  src-tauri/              Tauri v2 shell (Phase 1 of the build)
  ui/                     React + TypeScript + Vite + Tailwind (Phase 1 of the build)
```

## Status

- [x] `rustcut-core`: models, naming, FFmpeg args, `.rustcut` persistence, ffprobe parsing, export planning, waveform peaks — 44 unit tests green
- [x] Tauri v2 shell: project IPC, `probe_media`, `start_export` (live `export://` events), `waveform_peaks`
- [x] UI: player, clip queue, timeline with audio waveform, keyboard map, per-clip crop sliders + drag overlay, 3 export presets, live progress bar
- [x] End-to-end FFmpeg verification (5 integration tests): lossless cut, 9:16 center crop at 608x1080, blurred-background 9:16, waveform peaks, template naming — 51 tests total
- [ ] Installer bundle (bundled FFmpeg sidecar, 35–60 MB target)

## Build & test

```bash
cargo test --workspace          # core logic tests
cargo build --release           # optimized build (LTO + strip)
```

## Key invariants (from the PRD)

- The source video is never modified; clips are timestamp ranges only.
- All framing math runs on the **16:9 working canvas**: the source is scaled to
  fit (contain) and centered, so the 9:16 crop X range is always valid.
- Lossless mode (`-c copy`) trims at keyframes with no re-encode; canvas and
  crop only apply in re-encode mode.
- Project state saves atomically (temp file + rename) on every change.
