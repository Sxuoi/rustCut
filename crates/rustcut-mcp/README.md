# rustcut-mcp

An [MCP](https://modelcontextprotocol.io) server (stdio) that lets AI agents, including small and cheap
models, clip videos with RustCut's engine without the desktop app running. It uses the same project files, the
same validation, the same pre-flight checks and the same export engine as the app. Originals are never
modified: clips are exported as new files.

## Build

```
cargo build --release -p rustcut-mcp
```

The binary is `target/release/rustcut-mcp` (`rustcut-mcp.exe` on Windows). It needs FFmpeg and FFprobe; it finds
them like the app does (`RUSTCUT_FFMPEG_DIR`, next to the executable, `PATH`, WinGet's Gyan builds).

## Register

Claude Code:

```
claude mcp add rustcut -- C:/path/to/rustcut-mcp.exe
claude mcp add rustcut -e RUSTCUT_MCP_ROOTS=D:/Videos -e RUSTCUT_WHISPER_MODEL=D:/models/ggml-base.bin -- C:/path/to/rustcut-mcp.exe
```

Other MCP clients (generic JSON; `env` is optional):

```json
{
  "mcpServers": {
    "rustcut": {
      "command": "C:/path/to/rustcut-mcp.exe",
      "args": [],
      "env": {
        "RUSTCUT_FFMPEG_DIR": "C:/ffmpeg/bin",
        "RUSTCUT_MCP_ROOTS": "D:/Videos;E:/Clips",
        "RUSTCUT_WHISPER_MODEL": "D:/models/ggml-base.bin",
        "RUSTCUT_DATA_DIR": ""
      }
    }
  }
}
```

| Variable | Meaning |
| --- | --- |
| `RUSTCUT_FFMPEG_DIR` | Folder with `ffmpeg` and `ffprobe`. |
| `RUSTCUT_MCP_ROOTS` | Folders the server may read and write (`;`-separated on Windows, `:` elsewhere). **Unset: every absolute path on the computer is readable and writable, including `output_dir` (the server logs a warning at startup). Set it for any agent you do not fully trust.** MCP roots advertised by the client are applied as well (both must allow a path). |
| `RUSTCUT_WHISPER_MODEL` | A ggml whisper model file, for `transcribe`. Never downloaded for you: get one such as `ggml-base.bin` from the whisper.cpp project's model downloads. |
| `RUSTCUT_DATA_DIR` | Relocates data, cache and logs (`data/`, `cache/`, `logs/` below it). Unset: the desktop app's folders (`app.rustcut.desktop`), so both share projects, settings, backups and the recent list. |

Logs go to stderr and `<logs>/mcp.log`; stdout carries only the protocol.

## Tools

| Tool | What it does |
| --- | --- |
| `probe_video` | Duration, size, resolution, fps, VFR, HDR, rotation, codecs, audio and subtitle tracks, sidecar subtitles, warnings. |
| `get_transcript` | Subtitle lines `[HH:MM:SS.mmm -> HH:MM:SS.mmm] text` from an embedded text track or a sidecar `.srt/.vtt/.ass`; paged. |
| `transcribe` | Speech-to-text with FFmpeg's `whisper` filter (needs `RUSTCUT_WHISPER_MODEL`); cached; same output as `get_transcript`. |
| `find_loud_moments` | Ranks moments by EBU R128 loudness above the track median (laughter, shouting, explosions) with suggested clip ranges. |
| `get_frames` | JPEG images at given times (up to 12). |
| `contact_sheet` | One tiled JPEG of evenly spaced frames with timestamps. |
| `list_clips` | Clips of the video's project (an empty list, not an error, when there is none). |
| `add_clips` | Add clips from `{start,end,name,tag}` items and/or a timestamps text (`12:10-13:05 Jump scare`); per-item errors. |
| `update_clip`, `delete_clips` | Edit or remove clips by number. |
| `preview_export` | Pre-flight report (file names, size estimate, free space, warnings, blocking errors); writes nothing. |
| `export_clips` | Exports (presets `lossless`, `accurate`, `vertical_crop`, `vertical_blur`); waits up to `wait_sec` then returns a `job_id`; progress notifications when the client sends a progress token. |
| `export_status`, `cancel_export` | Poll or cancel a running export. |

Times are seconds from the video start (number) or `"HH:MM:SS.mmm"` / `"MM:SS"` / `"12m30s"` text. `add_clips` timestamps accept `12:10-13:05 name`, `10:00 --> 10:20 name`, `[00:10:00 -> 00:10:05] text` (a `get_transcript` line), `90-100 name` (seconds), `12m30s-13m name`, CSV and chapter lists (`12:10 name`, ends at the next start). The result lists `warnings` (end clamped to the video end, clip over 10 minutes, odd name); names are limited to 100 characters. `get_transcript` strips markup, ASS drawing commands, web-address watermarks and duplicate lines. The first call of a session can take several seconds (FFmpeg and encoder detection). Tool failures are
returned as tool errors (`isError`) with an actionable message, never as protocol errors.

## Example prompt

> Clip the 5 funniest moments of D:/Videos/stream.mp4 for TikTok as vertical crops.

A typical run: `probe_video`, `find_loud_moments` and `get_transcript` (or `transcribe`) to find candidates,
`contact_sheet` to check them, `add_clips`, `preview_export` with `preset: "vertical_crop"`, `export_clips`.

## Safety model

- Absolute paths only; every path read or written must be inside `RUSTCUT_MCP_ROOTS` and the client's MCP
  roots when set. Inputs must be video files; other kinds are refused with a specific message.
- Export options passed to `preview_export`/`export_clips` (`preset`, `naming_template`, `resolution`, `on_conflict`, `output_dir`, `audio_track`) apply to that call only; they are not written to the project, and omitted ones use the documented defaults (template `{index}_{name}`, 1080x1920, rename, track 1). Only `crop_offset` is stored (on the clips).
- When both `RUSTCUT_MCP_ROOTS` and client roots are set, a path must be inside both; refusals list the effective (overlapping) folders.
- Outputs go to `<video folder>/<video name>_clips` unless `output_dir` is given. Existing files are never
  overwritten unless `on_conflict: "overwrite"` is passed explicitly (default: rename; a clip's earlier output
  is treated like any other existing file).
- Projects are written only while holding the project lock the desktop app uses. If RustCut (or another agent)
  has the project open, mutating tools return "This project is open in RustCut or another agent. Close it
  there, then retry."; nothing is written. Read-only tools need no lock.
- Saves are atomic and write the app's rolling backups; the recent list is updated so the desktop start
  screen lists the project.
- Nothing is downloaded and no network is used. Exports run through the app's engine, with `.part` files and
  process cleanup: running exports are cancelled when the client disconnects, and FFmpeg children die with
  the server, also when the server is killed hard (on Windows the server runs in a kill-on-close Job Object).

## Seeing the changes in the app

Clips and export results are stored in the video's `.rustcut` project (next to the video, or in the app
library). Open the video in RustCut to see, edit and export the same clips; exported clips show as done.
