# FFmpeg sidecars

Tauri bundles `ffmpeg` and `ffprobe` from this folder (`bundle.externalBin`).
Files must be named with the Rust target triple, e.g.:

```
ffmpeg-x86_64-pc-windows-msvc.exe
ffprobe-x86_64-pc-windows-msvc.exe
```

Populate them with `npm run sidecars` (copies the `ffmpeg`/`ffprobe` found on
`PATH`, in `FFMPEG_DIR`, or in the WinGet Gyan.FFmpeg package). The binaries
are git-ignored.

Development uses a stock GPL "full" build (gyan.dev). Shipping needs the
custom minimal build and licensing decision in PRD §8.3 / R3 / Q1.
