# Product Requirements Document (PRD)
**Product Name (Working Title):** RustCut / QuickClip (final name pending trademark check, Q2)  
**Category:** Desktop Video Utility / Non-Destructive Batch Video Clipper  
**Core Tech Stack:** Rust (core), Tauri v2 (desktop shell), FFmpeg & FFprobe (media engine, bundled as sidecars)  
**Target Platforms:** Windows, macOS, Linux (minimum versions in §8.6)  
**Document Version:** 1.3.0 · **Status:** Draft for review · **Last updated:** 2026-11-13  
**Owner:** TBD (Product) · **Reviewers:** Engineering lead, Design lead

---

## Revision History

| Version | Date | Summary |
| :--- | :--- | :--- |
| 1.2.0 | — | 16:9 working canvas, adjustable crop position, blurred background. |
| 1.3.0 | 2026-11-13 | Consistency pass, decisions on open behaviours, FFmpeg commands verified, playback-compatibility and data-safety requirements, new sections (goals, metrics, acceptance criteria, test media, risks, open questions). |

### What changed in 1.3.0
1. **Contradictions resolved.** Auto-save and crash recovery were P0 (§4.5 of v1.2) but scheduled for Phase 2; naming templates were P0 but "custom templates" were Phase 2; the journey's export choice (*Fast Lossless or Vertical Auto-Crop*) didn't match the two modes in §4.4; `J`/`L` both seeked 5 s *and* "increased playback speed"; `{clip_name}` vs `{name}`; `crop_x_offset` existed per clip *and* in export settings; Out < In was "swap **or** reject"; the example source had three different names. Each now has one answer.
2. **16:9 working canvas replaced by source-space framing (§4.5).** Measured on test footage, v1.2's canvas put letterbox bars into every 21:9 vertical export (25% of the frame), and panning to the edge on 4:3 / 1:1 / 9:16 sources produced 39% / 69% / 100% black output. Cropping the largest 9:16 window that fits inside the source never leaves the picture for any aspect ratio, needs no special cases, and keeps *Original* re-encodes framed exactly like Lossless exports.
3. **Vertical output is 1080×1920 by default.** v1.2's commands produced 608×1080 while the requirement promised 1080×1920; output resolution is now an explicit setting.
4. **FFmpeg commands verified and hardened (§7.6)** on FFmpeg 6.1.1: explicit stream mapping (default mapping silently dropped 2 of 3 OBS audio tracks in testing), `-fps_mode passthrough` (default re-encode turned a 180-frame VFR clip into 240 frames by duplicating), `.part` temp files with atomic rename, `-nostdin`, machine-readable progress, `hvc1` tag for HEVC in MP4, and a blurred background that renders ~3× faster.
5. **Lossless keyframe behaviour quantified.** A lossless cut starts at the keyframe *before* Mark In: 1.0 s of lead-in with a 2 s GOP in testing, and real recordings range from ~1 s to 10 s+ (v1.2 said 0.5–1 s). The UI now shows each clip's effective start (MRK-07).
6. **Disk-space formula fixed.** v1.2's `Bitrate × Σ duration` gives bits, not bytes (8× too high). The corrected formula matched the measured output size within 1%.
7. **Playback compatibility strategy added (§7.3).** Tauri plays video in the OS WebView, which cannot play every format listed as P0 (FLV in WebView2 and WKWebView, MKV in WKWebView, HEVC without OS decoders). Added direct-play → remux → proxy tiers and a Phase 0 spike.
8. **Data safety added.** Atomic saves with backups, a fallback location for read-only folders, a single-writer lock, source fingerprinting for relink, undo/redo.
9. **Engineering corrections.** `rayon` removed (export jobs are child processes → `tokio` + semaphore); orphan-process prevention; measurable NFRs (memory measured per process, reference hardware).
10. **Format.** Requirement IDs for traceability; LaTeX removed from plain numbers (`$1\text{--}5$` shows up raw in many Markdown viewers).

---

## 1. Executive Summary

### 1.1 Problem Statement
Short-form creators (livestream clippers, podcast editors, TikTok/Shorts/Reels creators) routinely pull 10–30 short clips out of a single 1–5 hour recording.

Conventional NLEs (Premiere Pro, CapCut, DaVinci Resolve) make this slow and risky:
1. Split the timeline and **delete** everything except the moment.
2. Export, then wait for the render.
3. Undo (`Ctrl+Z`) to bring the deleted footage back.
4. Scrub for the next moment, losing track of what was already clipped or overwriting earlier markers.
5. A crash, or an accidental save while footage is deleted, can permanently lose the remaining timestamps.

### 1.2 Product Vision
A lightweight, Rust-powered desktop app for **non-destructive multi-trimming and batch export**. The source is never cut. Users play the video, mark In and Out as often as they like (10–50+ segments), name each segment in the **Clip Queue**, and export every segment as its own file in one action: losslessly at disk speed, or frame-accurately and pre-framed for vertical platforms.

### 1.3 Goals
| ID | Goal |
| :--- | :--- |
| G1 | Mark, queue and name a clip without the mouse: two keystrokes minimum (`I`, `Enter`) plus typing. |
| G2 | Never lose work: markers survive crashes, power loss, moved files and accidental deletes. |
| G3 | Export any number of clips in one action; lossless exports run at disk speed; vertical exports are upload-ready (1080×1920). |
| G4 | Stay responsive with 1–5 h, > 20 GB sources on mid-range hardware. |

### 1.4 Non-Goals (v1)
- Not an NLE: no multi-track timeline, transitions, titles, effects beyond framing, colour grading or audio mixing (beyond choosing a track).
- One source video per project; multi-source batches are future scope.
- No downloading from URLs (YouTube/Twitch); users bring their own files.
- No uploading or publishing to platforms.
- No clipping of files that are still being recorded.
- No audio-only sources.
- No cloud sync or collaboration.

### 1.5 Success Metrics
Initial targets, to calibrate during beta. Telemetry is opt-in (§8.4), so usage figures come from opted-in users and moderated tests.

| Metric | Target | Measured by |
| :--- | :--- | :--- |
| Time to first export (new user, 2 h sample VOD, 3 clips) | ≤ 5 min | Moderated usability test |
| Per-clip overhead (mark → queued → named), keyboard only | ≤ 10 s median | Usability test, opt-in telemetry |
| Batch wait: 10 × 60 s clips, Lossless | < 10 s on reference hardware | Automated benchmark (§8.1) |
| Clip export success rate (excluding corrupt sources) | ≥ 99.5% | Opt-in telemetry, beta logs |
| Projects with lost markers | 0 | Kill-test suite (AC-04), support tickets |
| Crash-free sessions | ≥ 99.5% | Opt-in crash reports |
| Beta users still active in week 4 | ≥ 40% | Opt-in telemetry |

---

## 2. Target Audience & User Personas

When personas conflict, MVP trade-offs favour **The Livestream Clipper**.

| Persona | Description | Core Needs | Current Pain Points |
| :--- | :--- | :--- | :--- |
| **The Livestream Clipper** (primary) | Extracts funny or viral moments from 3-hour YouTube/Twitch VODs for TikTok/Shorts. | Keyboard-driven marking for dozens of clips; instant export without re-rendering; vertical output. | Forgetting which timestamps were already clipped; waiting on one export at a time. |
| **Podcast / Webinar Editor** | Cuts Q&A sessions or speaker highlights from long recordings. | Clean per-topic file names; resume work the next day; frame-accurate cuts for further editing. | Nested sequences and sub-clips in heavy NLEs are tedious. |
| **Gamer (Highlight Collector)** | Records long sessions, keeps only "kill" or "clutch" moments, then deletes the raw footage. | Trim > 20 GB files without RAM spikes; keep OBS multi-track audio; handle 120/144 fps and HEVC/AV1 recordings. | Editors lag or crash on multi-gigabyte files. |

---

## 3. Core User Journeys

Example data used throughout this document: source `livestream_2026_10_21.mp4` (2:30:00, 1920×1080, 60 fps, ~8 Mbps) with three clips.

| # | Name | In | Out | Duration |
| :--- | :--- | :--- | :--- | :--- |
| 01 | Opening Moment | 00:02:10.500 | 00:03:00.000 | 0:49.5 |
| 02 | Jump Scare Reaction | 00:15:20.120 | 00:15:55.800 | 0:35.7 |
| 03 | Patch Discussion | 00:42:00.000 | 00:43:30.000 | 1:30.0 |

### 3.1 First session
1. **Import:** Drag and drop, or `Ctrl+O`. The app probes the file (≤ 2 s), picks a playback tier (§7.3) and starts background jobs (waveform, keyframe index).
2. **Scan:** Watch at 1.5–2×; seek with `J`/`L` (±5 s) and `←`/`→` (±1 s).
3. **Mark:** At 00:15:20.120 press `I`; at 00:15:55.800 press `O`, or skip `O` and press `Enter` when the moment ends (Out defaults to the playhead). The pending range is drawn on both timelines.
4. **Queue:** `Enter` adds it as **Clip #02**: a coloured bar on the timeline and a row in the Clip Queue. Press `N` to name it now, or name it later.
5. **Repeat** until the queue holds every moment worth keeping.
6. **Export:** `Ctrl+E` opens the Export dialog (§6.2): preset (*Lossless*, *Frame-Accurate*, *Vertical Crop 9:16*, *Vertical Blur 9:16*), output folder, file-name template with a live preview, and pre-flight results. `Enter` starts the batch.
7. **Completion:** Clips export in the background with per-clip progress while the queue stays editable. A summary lists successes and failures with *Retry failed* and *Open folder*.

### 3.2 Resume the next day
Opening the same video (or its `.rustcut` file) restores every clip, name, tag and export status, plus the playhead and zoom. Clips edited after their export show **Outdated** and can be re-exported in one action.

### 3.3 When something goes wrong
- One clip fails (e.g., a corrupt segment): the batch continues; the failed row shows the reason and *Retry*.
- The source was moved: *Locate Missing Media* relinks it, and the fingerprint check confirms it is the same file.

---

## 4. Functional Requirements

Priorities: **P0** = MVP, required for the first release · **P1** = should-have (Phase 2) · **P2** = future.  
Requirement IDs are used by the acceptance criteria (§10) and the roadmap (§12).

### 4.1 Media Player & Navigation (PLY)
- **PLY-01 [P0] Format support.** Containers: MP4, MOV, MKV, WebM, FLV (MPEG-TS/M2TS: P1). Video: H.264, HEVC, VP9, AV1. Audio: AAC, MP3, Opus, AC-3, PCM. Preview goes through the playback tiers in §7.3. Export always reads the original file with FFmpeg, so any file FFprobe can read is exportable even when its preview needs a remux or proxy.
- **PLY-02 [P0] Transport.** Play/pause; seek ±1 s and ±5 s; step ±1 frame (pauses playback). Holding a key repeats at the decoder's pace.
- **PLY-03 [P0] Speed.** 0.5×, 1×, 1.25×, 1.5×, 2×, 3×, pitch preserved.
- **PLY-04 [P0] Time display & mark accuracy.** Time is shown as `HH:MM:SS.mmm`. While paused, the displayed time and any mark use the media time of the frame actually on screen (`requestVideoFrameCallback` where available), so In/Out land on the visible frame. Frame stepping uses the stream's frame duration; for VFR sources it uses the real next/previous frame timestamp once the media index (PLY-06) exists.
- **PLY-05 [P1] Audio waveform.** Generated in the background from the selected audio track, cached per source fingerprint, and drawn progressively (usable before it finishes). Peaks are stored at ≥ 50 points per second for the zoom timeline.
- **PLY-06 [P1] Zoom timeline & media index.** A detail timeline around the playhead (window 2 s–10 min, mouse-wheel zoom) with frame ticks, keyframe ticks, pending In/Out and clip edges. The media index (keyframe and frame timestamps) is read instantly from MP4/MOV sample tables, or built by a background packet scan for other containers, then cached.
- **PLY-07 [P2] Hover thumbnails** on the overview timeline (sprite sheet generated in the background).

### 4.2 Non-Destructive Marking (MRK)
- **MRK-01 [P0] Pending range.** `I` and `O` set a pending In/Out at the playhead; the source is never modified.
  - Marks can be set in either order, which supports "that was funny, go back for the start" (Out first, then In).
  - A new mark that conflicts with the other pending mark (In ≥ Out) **clears the other mark** and shows a toast ("Out point cleared"). Marks are never swapped.
  - `Enter`/`A` adds the pending range. If only In is set, Out = playhead (when valid). After adding, the pending marks clear and the new clip is selected.
  - Minimum clip duration 0.5 s (setting: 0.1–5 s). Marks clamp to [0, source duration].
- **MRK-02 [P0] Timeline visualisation.** Every clip is a coloured bar on the overview timeline, so it is always clear what has already been clipped. Overlapping clips stack into lanes (up to 4 visible, then a "+N" badge). The pending range is drawn hatched. Exported clips carry a check mark; hover shows name and times; click selects and seeks.
- **MRK-03 [P0] Overlapping clips** are allowed (e.g., a 10:00–12:00 cut for YouTube and a 10:30–11:00 cut for TikTok). Identical ranges are allowed but flagged with a "duplicate" hint.
- **MRK-04 [P0] Undo/redo** for every queue change (add, delete, rename, retime, tag, include toggle, crop position), at least 200 steps per session. Deletes need no confirmation because they can be undone.
- **MRK-05 [P1] Trim handles.** Drag clip edges on either timeline; holding `Shift` snaps to the playhead, keyframes and other clip edges; a tooltip shows the new time and duration. Keyboard equivalent: `Alt+I` / `Alt+O` set the selected clip's In/Out to the playhead.
- **MRK-06 [P1] Colour tags.** A per-project palette with labels (defaults: Red = Funny, Blue = Educational, Green = Highlight), editable. Tags always show a label or letter, never colour alone.
- **MRK-07 [P1] Keyframe awareness for Lossless.** With a Lossless preset active, each clip shows its *effective start* (the keyframe at or before In) and its lead-in (e.g., "+1.4 s"). *Loop preview* can play "as exported". An optional *Snap In to keyframe* makes lossless cuts start exactly where marked.

### 4.3 Clip Queue (QUE)
- **QUE-01 [P0] Queue rows** show the include checkbox, clip number, editable name, In, Out, duration (`m:ss.s`, where duration = Out − In), tag and export status.
- **QUE-02 [P0] Stable clip numbers.** Each new clip takes the next number (`index` in the schema). Numbers are never reused after a delete, so `{index}` file names never collide with files already exported. *Renumber chronologically* (P1) reassigns numbers on request.
- **QUE-03 [P0] Jump & loop.** Selecting a row (click, or `↑`/`↓`) seeks to its In; *Loop preview* (`P`) plays only that range on repeat.
- **QUE-04 [P0] Include, exclude, delete.** Per-row checkbox; bulk *Select all / none / not exported / failed*; delete with the row button or `Delete` (undoable).
- **QUE-05 [P0] Export status** per clip: Pending, Exporting, Done, Failed (with reason), Outdated (edited since its last export). Done rows offer *Show in folder*.
- **QUE-06 [P1] Search & sort** by number, time, duration or name. Sorting changes the view only, never the clip numbers.
- **QUE-07 [P1] Bulk edit** of tag, include flag and crop position for multi-selected rows.
- **QUE-08 [P1] Scale.** The list stays smooth (virtualised) at 500 clips.

### 4.4 Batch Export (EXP)
- **EXP-01 [P0] One-action export** of all included clips through the Export dialog (§6.2). The queue stays editable during export; editing a clip that is exporting or exported marks it Outdated.
- **EXP-02 [P0] Two cut modes:**

| | Lossless (stream copy) | Frame-Accurate (re-encode) |
| :--- | :--- | :--- |
| Start | Keyframe at or before In: lead-in up to one GOP (≈ 1–5 s typical, 10 s+ on some recorders) | Exact frame at In |
| End | Last frame before Out | Last frame before Out |
| Quality | Bit-identical packets | Visually lossless (CRF 18; vertical presets CRF 20) |
| Speed | Disk-bound (seconds) | Encoder-bound (≈ 1–5× realtime) |
| Framing | Original only | Original; Vertical Crop / Vertical Blur (P1, §4.5) |
| Output | MP4 when compatible, else the source container (EXP-03) | MP4 (H.264 + AAC) |

Measured on FFmpeg 6.1.1 with a 2 s GOP: a lossless cut marked 3.000–8.000 s started at the 2.000 s keyframe (audio and video) and ended on the frame before 8.000 s; the re-encoded cut started exactly on the 3.000 s frame and lasted 5.000 s.

- **EXP-03 [P0] Lossless output container.** MP4 when every mapped stream is MP4-compatible (covers the common OBS MKV → MP4 case); otherwise the source container. If only the audio is incompatible (e.g., PCM), the dialog offers *MP4 with AAC audio, video stays lossless*. HEVC copied into MP4 is tagged `hvc1` (FFmpeg's default `hev1` is rejected by Apple players).
- **EXP-04 [P0] Stream mapping.**
  - Lossless: first video stream + **all** audio tracks (keeps OBS multi-track).
  - Frame-Accurate (Original): first video + all audio tracks, re-encoded to AAC.
  - Vertical presets: first video + one audio track (default track 1, selectable), downmixed to stereo. P1: *Mix all tracks*.
  - Subtitle, data and attachment streams are dropped. Sources without audio export video-only.
- **EXP-05 [P0] File naming.** Template variables:

| Variable | Value | Example |
| :--- | :--- | :--- |
| `{index}` | Stable clip number, zero-padded to max(2, digits of the highest number) | `02` |
| `{name}` | Sanitized clip name; empty → `Clip_{index}` | `Jump_Scare_Reaction` |
| `{source_name}` | Source file name without extension | `livestream_2026_10_21` |
| `{start}` / `{end}` | `HH-MM-SS` | `00-15-20` |
| `{duration}` | Whole seconds | `36` |
| `{date}` | Export date, `YYYY-MM-DD` | `2026-11-13` |
| `{tag}` | Tag label (empty if none) | `Funny` |
| `{preset}` | `lossless`, `accurate`, `vcrop`, `vblur` | `vcrop` |

The extension comes from the output container and is appended automatically; templates never contain one. Examples: `{index}_{name}` (default) → `02_Jump_Scare_Reaction.mp4`; `{source_name}_{start}_{name}` → `livestream_2026_10_21_00-15-20_Jump_Scare_Reaction.mp4`. `{preset}` keeps a YouTube and a TikTok export of the same clip apart in one folder. The dialog previews the first three names.

- **EXP-06 [P0] Sanitization**, identical on every OS so files stay portable, applied to each variable value and then to the rendered name:
  1. Unicode NFC normalisation.
  2. `< > : " / \ | ? *` and control characters → `_`.
  3. Whitespace → `_` (setting *Spaces → `_`*, default on; turn it off where the file name becomes the upload title).
  4. Collapse repeated `_`; trim `_`, `.` and spaces from both ends.
  5. Windows reserved names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`, also before a dot) get `_` appended.
  6. Truncate to 120 UTF-8 bytes on a character boundary.
  7. Empty result → `Clip_{index}`.

| Input | Output |
| :--- | :--- |
| `Jump Scare Reaction` | `Jump_Scare_Reaction` |
| `What?! Epic/Win: 100%` | `What_!_Epic_Win_100%` |
| `  ...Trailing dots...  ` | `Trailing_dots` |
| `con` · `aux.final` | `con_` · `aux_.final` |
| `Rekaman: Bahas Patch 2.0 🔥` | `Rekaman_Bahas_Patch_2.0_🔥` |
| `???` (clip 7) | `Clip_07` |

(v1.2's example output `What_Epic_Win_100%` silently dropped the legal `!`; the vectors above are the contract.)

- **EXP-07 [P0] Collisions.** Duplicate names within a batch get `_2`, `_3`, … in clip-number order. Existing files follow the dialog policy *Auto-rename* (default) / *Overwrite* / *Skip*, except a file this project previously exported for the same clip, which is replaced on re-export.
- **EXP-08 [P0] Safe writes.** Each clip is written to `<final name>.part` (muxer set explicitly with `-f`) and renamed when FFmpeg succeeds. Failed or cancelled clips leave nothing behind; stale `.part` files listed in the project's export journal are removed on next launch. FFmpeg is spawned with an argument vector (never a shell string) and `-nostdin`.
- **EXP-09 [P0] Progress & cancel.** Progress comes from `-progress pipe:1` (`out_time_us`, `speed`, `progress=end`) and is weighted by clip duration into an overall bar and ETA. *Cancel clip* and *Cancel all* kill the process and delete its `.part`. FFmpeg processes never outlive the app (§7.8).
- **EXP-10 [P0] Fault tolerance.** A failed clip never stops the batch: its status becomes Failed with the exit code and the last 20 lines of FFmpeg output (full log in the log folder), and the next clip starts. Exception: a full disk or a vanished output folder (e.g., USB unplugged) **pauses** the batch with a prompt, because every remaining clip would fail too. *Retry failed* re-queues only failures.
- **EXP-11 [P0] Pre-flight**, shown in the dialog; red items block *Start*: source present and fingerprint unchanged; output folder writable; free space ≥ 1.2 × estimate; names resolved; container compatibility; HDR source with a re-encode preset (warning, FRM-07); clips longer than the preset's maximum duration (warning, FRM-08).
  - Lossless estimate: `bytes ≈ source_bitrate_bps / 8 × Σ (duration + lead_in)`, with lead-in from the media index (5 s per clip when unknown).
  - Re-encode estimate: `bytes ≈ ((video_maxrate_bps + audio_bitrate_bps) × Σ duration + bufsize_bits × clip_count) / 8`, an upper bound because presets cap the bitrate; the `bufsize` term covers the encoder's initial burst (a 35.7 s test clip averaged 16.7 Mbps against a 16 Mbps cap).
- **EXP-12 [P1] Parallel export.** A scheduler caps concurrent FFmpeg processes. Defaults: Lossless 1 (disk-bound; parallel reads slow HDDs down), software re-encode 2 on ≥ 8 cores (else 1), hardware encode 2. User range 1–4. Software jobs split CPU threads (`-threads` = cores ÷ jobs). An "out of encoder sessions" error re-queues the clip and lowers concurrency by one.
- **EXP-13 [P1] Hardware encoders** (NVENC, AMF, Quick Sync, VideoToolbox, VAAPI), detected at first launch with a 1-second test encode each and cached. Quality presets map to each encoder's rate-control mode. If a hardware encode fails, that clip retries with libx264 and the log records why.
- **EXP-14 [P1] Desktop integration.** OS notification when a batch finishes; system sleep is blocked while an export runs.
- **EXP-15 [P2] Smart cut.** Re-encode only the head of a clip up to the first keyframe after In and stream-copy the rest. Experimental; falls back to Frame-Accurate when codec parameters can't be matched.
- **EXP-16 [P2] Timestamp export.** YouTube chapters (plain text), CSV, CMX3600 EDL and FCPXML (for Resolve, Premiere, Final Cut). Chapters follow YouTube's rules (first timestamp `00:00`, at least three in ascending order, each ≥ 10 s): the export inserts `00:00 Intro` when needed and warns about violations.
- **EXP-17 [P2] Timestamp import** from CSV or pasted lines such as `12:10-13:05 Jump scare` (e.g., community timestamp comments), with a preview before clips are created.

### 4.5 Framing & Vertical Output (FRM)
Framing applies to Frame-Accurate exports only; choosing a vertical preset switches the cut mode automatically.

- **FRM-01 [P1] Framing options:** *Original* (source framing, same as Lossless) · *Vertical Crop 9:16* · *Vertical Blur 9:16*.
- **FRM-02 [P1] Source-space crop window.** Framing math works on the source's **display frame**: rotation metadata applied, non-square pixels converted to square. The crop window is the **largest 9:16 rectangle that fits inside that frame**:
  - Source at least as wide as 9:16 (all landscape and square footage): full height, width = height × 9/16, slides horizontally.
  - Source taller than 9:16 (e.g., phone screen recordings): full width, height = width × 16/9, centred vertically; crop position ignored.
  - Width and height round to the nearest even number without exceeding the frame; position rounds down to an even number. Algorithm and test vectors: §7.7.
  - The window can never leave the picture, so no source produces black bars, whatever its aspect ratio.
- **FRM-03 [P1] Crop position per clip.** `crop_x_offset` ∈ [−1.0, +1.0] (−1 = left edge, 0 = centre, +1 = right edge), mapped to `x = (frame_width − window_width) × (1 + offset) / 2`. New clips default to 0; *Reset to centre* restores it. Controls: drag the window on the preview, the *Crop position* slider, or `Alt+←` / `Alt+→` (±0.05; with `Shift` ±0.25). Ignored by Vertical Blur and when the window already spans the full width (e.g., 9:16 sources).
- **FRM-04 [P1] WYSIWYG preview.** In Vertical Crop mode the player shows the 9:16 window over the source with the outside dimmed (§6.3), plus a *9:16 only* toggle. Overlay and export call the same Rust function over IPC, so preview and output cannot drift apart.
- **FRM-05 [P1] Vertical Blur.** The full display frame is fitted inside the 9:16 output over a blurred, cover-scaled copy of itself. The blur runs at quarter resolution and is scaled up (~3× faster than full-resolution blur in testing, visually equivalent).
- **FRM-06 [P1] Output resolution & frame rate.** *1080×1920* (default), *720×1280*, or *Native* (window size, no upscaling; e.g., 608×1080 from a 1080p source). Scaling uses Lanczos. Frame rate: the source rate, capped at 60 fps by default (120/144 fps gameplay is resampled); otherwise timing passes through untouched, so VFR stays VFR.
- **FRM-07 [P1] HDR sources.** HDR (PQ or HLG transfer, e.g., iPhone footage) is detected at probe time. P0: a pre-flight warning that re-encoded colours will look washed out (Lossless is unaffected). P1: tone-map to SDR BT.709 in re-encode presets (needs an FFmpeg build with `zscale`).
- **FRM-08 [P1] Preset maximum duration.** A vertical preset can define a maximum clip duration (example: 3:00 for YouTube Shorts); longer clips are flagged in the queue and in pre-flight. Platform limits change, so values live in an updatable preset file.

**Why not v1.2's 16:9 working canvas.** It padded every source to 16:9 before cropping. On test footage this put letterbox bars into 21:9 vertical exports (25% of the frame), filled 39–100% of the output with black when panning narrower sources to the edge, and made Frame-Accurate *Original* exports differ from Lossless ones. The source-space window keeps v1.2's guarantee (no out-of-range offsets, no per-aspect special cases) without those artefacts.

### 4.6 Project Files & Persistence (PRJ)
- **PRJ-01 [P0] Auto-save.** Every change is saved within 1 s (debounced; adds and deletes immediately). Writes are atomic: write a temp file, flush it to disk, rename it over the project. The last 10 versions are kept as rolling backups in the app data folder.
- **PRJ-02 [P0] Location.** Default: `<source name>.rustcut` next to the source. If that folder isn't writable (read-only drive, camera card, permissions), the project is stored in the app data folder, indexed by source fingerprint, and the status bar says where. Setting: *Next to video* / *App library*.
- **PRJ-03 [P0] Crash recovery & resume.** Opening a source finds its project (next to the video, else by fingerprint in the app library) and restores clips, names, tags, statuses, playhead, zoom and selection. Clips found in the Exporting state reset to Pending.
- **PRJ-04 [P0] Missing or changed source.** A missing source opens *Locate Missing Media*, which also suggests a file in the same folder with a matching fingerprint. A fingerprint mismatch (different, re-recorded or truncated file) warns before markers are reused; clips beyond the new duration are flagged *Out of range* and excluded from export.
- **PRJ-05 [P0] Single writer.** A lock file stops two windows or instances from auto-saving the same project; the second opens it read-only.
- **PRJ-06 [P0] Schema versioning.** Integer `schema_version`. Older files are migrated on open (original kept as `.v<N>.bak`); newer files open read-only with a warning. A corrupt project falls back to the newest valid backup, with a notice.
- **PRJ-07 [P1] Recent projects** on the start screen.

---

## 5. Keyboard Shortcuts

Keyboard-first: every P0 action works without a mouse. On macOS, `Ctrl` = `Cmd`.

| Key | Action | Priority |
| :--- | :--- | :--- |
| `Space` or `K` | Play / pause | P0 |
| `J` / `L` | Seek −5 s / +5 s | P0 |
| `←` / `→` | Seek −1 s / +1 s | P0 |
| `,` / `.` | Step −1 / +1 frame (pauses) | P0 |
| `Shift+,` / `Shift+.` (`<` / `>`) | Speed down / up (0.5 → 1 → 1.25 → 1.5 → 2 → 3) | P0 |
| `I` or `[` | Set pending In at the playhead | P0 |
| `O` or `]` | Set pending Out at the playhead | P0 |
| `Enter` or `A` | Add the pending range to the queue (Out = playhead if not set) | P0 |
| `Esc` | Clear pending marks / leave a text field / close a dialog | P0 |
| `N` | Rename the selected clip (else the most recent one) | P0 |
| `↑` / `↓` | Select previous / next clip and seek to its In | P0 |
| `P` | Loop preview of the selected clip (toggle) | P0 |
| `Delete` / `Backspace` | Delete the selected clip(s) (undoable) | P0 |
| `Ctrl+Z` / `Ctrl+Shift+Z` (also `Ctrl+Y` on Windows/Linux) | Undo / redo | P0 |
| `Ctrl+E` | Open the Export dialog; `Enter` inside it starts the export | P0 |
| `Ctrl+S` | Save now (auto-save is always on) | P0 |
| `Ctrl+O` | Open a video or project | P0 |
| `Shift+I` / `Shift+O` | Go to the pending (or selected clip's) In / Out | P1 |
| `Alt+I` / `Alt+O` | Set the selected clip's In / Out to the playhead | P1 |
| `Alt+←` / `Alt+→` | Nudge the selected clip's crop position (±0.05; `Shift` ±0.25) | P1 |
| `?` | Shortcut cheat sheet | P1 |

Rules:
1. Shortcuts are suspended while a text field has focus. `Enter` commits a rename and returns focus to the player; `Esc` cancels it.
2. Punctuation shortcuts bind to physical key positions (`KeyboardEvent.code`) so they work on non-US layouts; letter shortcuts follow the typed character. Full remapping is P2.
3. WebView browser accelerators (e.g., `Alt+←` back navigation, `F5`/`Ctrl+R` reload, `Ctrl+P` print) are disabled so they can never fire.
4. Mark keys read the player time synchronously inside the key handler; there is no IPC round-trip before the time is captured.

---

## 6. UI/UX

Dark theme by default to reduce eye strain in long sessions (light theme: P2).

### 6.1 Main window

```
+--------------------------------------------------------------------------------------------------+
| File  Edit  View  Help   Preset: [Lossless (Original) v]   livestream_2026_10_21  saved 2s ago   |
+---------------------------------------------------------+----------------------------------------+
|                                                         | CLIP QUEUE         3 clips, 2 included |
|                                                         | [Search clips...      ]  [Sort: # v]   |
|                                                         +----------------------------------------+
|                                                         | [x] #01 Opening Moment            DONE |
|                     VIDEO PLAYER                        |     00:02:10.500 > 00:03:00.000 0:49.5 |
|         (9:16 window overlay in Vertical Crop)          | [x] #02 Jump Scare Reaction    PENDING |
|                                                         |     00:15:20.120 > 00:15:55.800 0:35.7 |
|                                                         | [ ] #03 Patch Discussion        FAILED |
|                                                         |     00:42:00.000 > 00:43:30.000 1:30.0 |
+---------------------------------------------------------+                                        |
| 00:15:24.120 / 02:30:00.000           Speed [1.5x v]    |                                        |
| [|<] [-5s] [-1f]  [ PLAY ]  [+1f] [+5s] [>|]            |                                        |
| In 00:15:20.120  Out --:--:--.---  [ + ADD (Enter) ]    |                                        |
+---------------------------------------------------------+----------------------------------------+
| OVERVIEW 00:00:00                            02:30:00   | 2 included    est. ~90 MB    ~4 s      |
| |-[01]-------[02]------------------[03]-------------|   | [     EXPORT 2 CLIPS  (Ctrl+E)     ]   |
| |~~~~~~~~~~~~~~~~~~~~ waveform ~~~~~~~~~~~~~~~~~~~~~|   | [##########..........]  50%  1 / 2     |
| ZOOM  +/-10 s          K = keyframe   | = playhead      |                                        |
| |  '  K  '  '  [I==========|          '  K  '  '  '|    |                                        |
+--------------------------------------------------------------------------------------------------+
```

The header shows the save state ("saved 2s ago", or where the project lives when it fell back to the app library). The progress row appears only while exporting. The overview timeline marks every queued clip; the zoom timeline shows the pending In (`[I`), playhead, frame and keyframe ticks.

### 6.2 Export dialog

```
+----------------------------- Export 2 clips -----------------------------+
| Preset     ( ) Lossless (Original)        (*) Vertical Crop 9:16         |
|            ( ) Frame-Accurate (Original)  ( ) Vertical Blur 9:16         |
| Output     [1080x1920 v]   Frame rate [Source, max 60 v]   Audio [1 v]   |
| Encoder    [Auto (NVENC H.264) v]                Parallel jobs [2 v]     |
| Folder     [D:\Recordings\livestream_2026_10_21_clips   ] [Browse]       |
| File name  [{index}_{name}_{preset}             ] [x] Spaces -> _        |
|            02_Jump_Scare_Reaction_vcrop.mp4, ...                         |
| If exists  (*) Auto-rename   ( ) Overwrite   ( ) Skip                    |
+--------------------------------------------------------------------------+
| Pre-flight  OK  Source found, unchanged                                  |
|             OK  Free space 412 GB (needs ~0.2 GB)                        |
|             OK  File names resolved, no conflicts                        |
|                                       [ Cancel ]   [ Start (Enter) ]     |
+--------------------------------------------------------------------------+
```

The default output folder is `<source folder>/<source name>_clips`, created if missing and remembered per project. Options that don't apply to the chosen preset are hidden (Lossless shows container and lead-in information instead of resolution and encoder).

### 6.3 Vertical Crop overlay

```
+------------------------------------------------------+
|::::::::::::::::::::::::|              |::::::::::::::|
|::::::::::::::::::::::::|  9:16 window |::::::::::::::|
|::::::::::::::::::::::::|  (drag <->)  |::::::::::::::|
|::::::::::::::::::::::::|              |::::::::::::::|
+------------------------------------------------------+
 Crop position  [-1 ===========|========= +1]  [Reset]
```

Dimmed areas (`:`) are outside the exported window. The slider and the drag handle edit the selected clip's `crop_x_offset` (FRM-03).

### 6.4 States

| State | What the user sees |
| :--- | :--- |
| Empty | Drop zone, *Open video*, recent projects |
| Probing | Spinner with the file name (≤ 2 s typical) |
| Preparing preview (Tier 2/3) | Progress bar with ETA; marking starts once the preview plays; exporting existing clips works immediately |
| Ready | Player, timelines, queue |
| Exporting | Per-row status and overall progress; queue stays editable |
| Source missing | Banner with *Locate…*; queue read-only until relinked |
| Read-only project | Banner with the reason (open elsewhere, newer schema version) |

---

## 7. Technical Architecture

### 7.1 Stack
- **Core:** Rust, edition 2024.
- **Shell:** Tauri v2: Rust backend plus the OS WebView for the UI (WebView2 on Windows, WKWebView on macOS, WebKitGTK on Linux).
- **Frontend:** Svelte or SolidJS with Tailwind CSS (decision: Q3). The timelines and waveform draw on `<canvas>`, not DOM nodes, so they stay smooth at 500 clips.
- **Media engine:** FFmpeg and FFprobe as Tauri sidecars, pinned to one release (≥ 6.1, the version §7.6 was verified on) and built with only the components the app needs (§8.3, R3). Users never install FFmpeg.

### 7.2 Overview

```
+----------------------------- WebView (UI) -----------------------------+
|  <video> player    timelines + waveform (<canvas>)    clip queue       |
|  keyboard controller (captures mark times synchronously)               |
+-------------------^-------------------------------------+--------------+
                    | state patches, progress (Channel)   | commands (invoke)
+-------------------+-------------------------------------v--------------+
|  Rust core (tokio)                                                     |
|  ProjectStore: state, undo/redo, auto-save, lock, backups              |
|  MediaProbe     MediaIndex (keyframes, frames)     WaveformService     |
|  PlaybackCompat (remux / proxy cache)              ExportScheduler     |
+--------------------------------------+---------------------------------+
                                       | argv spawn, -progress pipe:1
                             +---------v---------+
                             |  ffmpeg / ffprobe |  (sidecars)
                             +-------------------+
```

- Rust owns the project state. The UI sends commands (`add_clip`, `update_clip`, `undo`, …) and renders the state patches it gets back. Undo/redo is an inverse-command stack inside `ProjectStore`, so every change, including an undo, is auto-saved the same way.
- Progress streams to the UI over Tauri `Channel`s; long jobs never block the UI thread.

### 7.3 Playback compatibility
The OS WebView decides what can play, and support differs: FLV plays in neither WebView2 nor WKWebView, WKWebView has no MKV support, HEVC and AV1 depend on OS/hardware decoders, and WebKitGTK depends on which GStreamer plugins are installed. The app therefore picks a tier per file:

| Tier | When | How | Cost |
| :--- | :--- | :--- | :--- |
| 1 Direct | The WebView plays the container and codecs | Stream the file over the asset protocol with HTTP byte ranges | None |
| 2 Remux | Codecs play, container doesn't (e.g., MKV/FLV with H.264 + AAC) | Background stream copy to MP4 in the cache. P1: on-demand fragmented-MP4 transmux through Media Source Extensions, no full copy | Disk I/O ≈ file size, temporary disk space |
| 3 Proxy (P1) | Codec not decodable (e.g., HEVC without an OS decoder) | Background transcode to 540p H.264 with a short GOP, timestamps preserved | Minutes of CPU/GPU |

- **Selection:** `canPlayType` with the exact codec string from FFprobe, confirmed by a decoded first frame within 3 s; any player error falls back to the next tier automatically.
- **Cache:** remux and proxy files live in an LRU cache (default cap 50 GB, user-clearable). **Export always reads the original file.**
- **MVP without Tier 3:** an undecodable codec shows a specific message (which codec, why, what to do), never a black player.
- **Time origin:** all times are relative to the source's first presentation timestamp, the same reference FFmpeg's `-ss` uses by default. Each tier is checked with a fixture whose `start_time` ≠ 0 (M13); if a WebView reports a different origin, a per-source offset is stored.
- **Decision gate:** if Tiers 1–2 miss the bar on two or more OSes in Phase 0, switch to a native player (libmpv) or WebCodecs (Q4).

### 7.4 Rust crates
| Crate | Purpose |
| :--- | :--- |
| `tauri` + plugins `shell`, `dialog`, `single-instance`, `updater` | App shell, sidecar spawning, file dialogs, single instance, signed updates |
| `tokio` | Async runtime: child processes, progress parsing, a `Semaphore` capping concurrent exports |
| `serde`, `serde_json` | Project file and IPC types |
| `uuid` (v7) | Time-sortable clip IDs |
| `xxhash-rust` | Source fingerprint (xxh3 over size + first and last 1 MiB) |
| `fs4` | Free disk space, project lock files |
| `tracing`, `tracing-appender` | Structured, rotated logs |
| `thiserror` | Typed errors surfaced to the UI |

Removed from v1.2: `rayon`, because export jobs are FFmpeg child processes that wait on I/O; a CPU thread pool adds nothing and would tie up its threads waiting. `sanitize-filename` is no longer named as the solution: EXP-06 specifies the rules and test vectors, and the implementation may use any crate that passes them.

### 7.5 Project file schema (`.rustcut`, JSON)

```json
{
  "schema_version": 2,
  "app_version": "1.3.0",
  "created_at": "2026-10-21T19:05:11Z",
  "updated_at": "2026-10-21T21:47:03Z",
  "source": {
    "path": "D:/Recordings/livestream_2026_10_21.mp4",
    "relative_path": "livestream_2026_10_21.mp4",
    "fingerprint": { "size_bytes": 9123456789, "xxh3_head_tail": "9f2c41d07b3e88a1" },
    "duration_sec": 9000.0,
    "start_time_sec": 0.0,
    "video": {
      "codec": "h264",
      "coded_width": 1920, "coded_height": 1080,
      "display_width": 1920, "display_height": 1080,
      "rotation": 0, "sar": "1:1",
      "frame_rate": "60/1", "vfr": false, "hdr": false
    },
    "audio_tracks": [
      { "stream_index": 1, "codec": "aac", "channels": 2, "title": "Mix" }
    ]
  },
  "tags": [
    { "id": "funny", "label": "Funny", "color": "#EF4444" },
    { "id": "edu", "label": "Educational", "color": "#3B82F6" },
    { "id": "highlight", "label": "Highlight", "color": "#22C55E" }
  ],
  "export_settings": {
    "output_dir": "D:/Recordings/livestream_2026_10_21_clips",
    "naming_template": "{index}_{name}",
    "spaces_to_underscores": true,
    "on_conflict": "rename",
    "cut_mode": "lossless",
    "framing": "original",
    "output_resolution": "1080x1920",
    "fps_cap": 60,
    "audio_track": 1,
    "encoder": "auto",
    "parallel_jobs": 1
  },
  "next_index": 4,
  "clips": [
    {
      "id": "0192b7e4-5c1a-7d3e-9f10-2a4b6c8d0e1f",
      "index": 1,
      "name": "Opening Moment",
      "start_sec": 130.5,
      "end_sec": 180.0,
      "tag_id": "edu",
      "crop_x_offset": -0.4,
      "selected_for_export": true,
      "export_status": "completed",
      "last_export": {
        "output_path": "D:/Recordings/livestream_2026_10_21_clips/01_Opening_Moment.mp4",
        "exported_at": "2026-10-21T21:40:12Z",
        "settings_hash": "c41f09aa",
        "error": null
      }
    },
    {
      "id": "0192b7f9-0b2c-7a44-8e21-5d6f7a8b9c0d",
      "index": 2,
      "name": "Jump Scare Reaction",
      "start_sec": 920.12,
      "end_sec": 955.8,
      "tag_id": "funny",
      "crop_x_offset": 0.3,
      "selected_for_export": true,
      "export_status": "pending",
      "last_export": null
    },
    {
      "id": "0192b81c-3e4d-7f56-a0b1-c2d3e4f5a6b7",
      "index": 3,
      "name": "Patch Discussion",
      "start_sec": 2520.0,
      "end_sec": 2610.0,
      "tag_id": null,
      "crop_x_offset": 0.0,
      "selected_for_export": false,
      "export_status": "failed",
      "last_export": {
        "output_path": null,
        "exported_at": "2026-10-21T21:40:15Z",
        "settings_hash": "7be2d310",
        "error": "ffmpeg exit 1: Invalid data found when processing input"
      }
    }
  ],
  "view_state": {
    "playhead_sec": 924.12,
    "zoom_window_sec": 20,
    "selected_clip_id": "0192b7f9-0b2c-7a44-8e21-5d6f7a8b9c0d"
  }
}
```

| Field | Notes |
| :--- | :--- |
| `schema_version` | Integer; migration rules in PRJ-06 (v1.2's `"project_version": "1.0"` string = version 1). |
| `source.relative_path` | Relative to the project file; tried first when the whole folder moved. |
| `source.fingerprint` | Identifies the file across renames and moves; a mismatch triggers PRJ-04. |
| `source.video.display_*` | Size after rotation and SAR; all framing math uses these. |
| `source.video.frame_rate` | Rational string, never a float (59.94 fps = `60000/1001`). |
| `index`, `next_index` | Stable clip numbers (QUE-02), never reused. |
| `start_sec`, `end_sec` | Seconds from the first presentation timestamp, millisecond precision. |
| `tag_id` | Replaces v1.2's per-clip `color`; points into `tags`. |
| `crop_x_offset` | Per clip only (v1.2 also had a project-level copy). |
| `export_status` | `pending`, `exporting`, `completed`, `failed`, `outdated`; `exporting` resets to `pending` on load. |
| `last_export.settings_hash` | Hash of the clip's times and crop plus the export settings used; when it no longer matches, the clip becomes `outdated`. |

### 7.6 FFmpeg command generation
Rules for every command:
- Built in Rust as an argument vector, never through a shell (quotes below are for readability only). Paths and names pass through unmodified, Unicode included.
- Always `-hide_banner -nostdin -y`, `-progress pipe:1 -nostats`, and an explicit muxer (`-f mp4`), because the output path ends in `.part`.
- Times are seconds with millisecond precision, relative to the first presentation timestamp. `-ss` and `-to` are input options (before `-i`): FFmpeg seeks using the index, and `-to` is an absolute position on the source timeline (verified: `-ss 3.0 -to 8.0` yields a 5.000 s re-encode).
- Re-encodes add `-fps_mode passthrough`; without it, FFmpeg's MP4 default duplicated frames to force constant frame rate (180 → 240 frames on a VFR test clip).
- Verified on FFmpeg 6.1.1 with synthetic fixtures (1080p with 2 s GOP; ramps at 16:9, 21:9, 4:3, 1:1, 9:16, 1080×2340 and 720×576 SAR 16:15; a rotated file; 3-track MKV; HEVC; 10-bit PQ HDR).

**A. Probe**
```bash
ffprobe -v error -show_format -show_streams -of json "livestream_2026_10_21.mp4"
```
Media index (background job, P1): one line per video packet; `K` marks keyframes.
```bash
ffprobe -v error -select_streams v:0 -show_entries packet=pts_time,flags -of csv=p=0 "livestream_2026_10_21.mp4"
# 0.000000,K__
# 0.016667,___
```

**B. Lossless, clip #02**
```bash
ffmpeg -hide_banner -nostdin -y \
  -ss 920.120 -to 955.800 -i "livestream_2026_10_21.mp4" \
  -map 0:v:0 -map "0:a?" -c copy \
  -avoid_negative_ts make_zero \
  -progress pipe:1 -nostats \
  -f mp4 "02_Jump_Scare_Reaction.mp4.part"
```
- Starts on the keyframe at or before 920.120 (lead-in, EXP-02); `make_zero` shifts timestamps so the file starts at 0.
- `-map "0:a?"` keeps every audio track; FFmpeg's default mapping kept 1 of 3 in testing.
- HEVC sources add `-tag:v hvc1`.
- On exit code 0 the app renames `.part` → `.mp4`.

**C. Frame-Accurate, original framing**
```bash
ffmpeg -hide_banner -nostdin -y \
  -ss 920.120 -to 955.800 -i "livestream_2026_10_21.mp4" \
  -map 0:v:0 -map "0:a?" \
  -c:v libx264 -preset fast -crf 18 -pix_fmt yuv420p -fps_mode passthrough \
  -c:a aac -b:a 192k \
  -movflags +faststart \
  -progress pipe:1 -nostats \
  -f mp4 "02_Jump_Scare_Reaction_accurate.mp4.part"
```

**D. Vertical Crop 9:16 → 1080×1920, clip #02 (`crop_x_offset` = +0.3)**  
Rust computes the window from the display frame (§7.7): 1920×1080 → 608×1080 at x = 852.
```bash
ffmpeg -hide_banner -nostdin -y \
  -ss 920.120 -to 955.800 -i "livestream_2026_10_21.mp4" \
  -map 0:v:0 -map "0:a:0?" \
  -vf "crop=608:1080:852:0,scale=1080:1920:flags=lanczos,setsar=1" \
  -c:v libx264 -preset fast -crf 20 -maxrate 16M -bufsize 32M -pix_fmt yuv420p -fps_mode passthrough \
  -c:a aac -b:a 192k -ac 2 \
  -movflags +faststart \
  -progress pipe:1 -nostats \
  -f mp4 "02_Jump_Scare_Reaction_vcrop.mp4.part"
```
- Literal integers from Rust replace v1.2's runtime expression, so the preview overlay and the export use identical numbers.
- Non-square pixels are normalised first, e.g., 720×576 SAR 16:15: `scale=768:576,setsar=1,crop=324:576:222:0,…`.
- Rotation needs nothing extra: FFmpeg auto-rotates before filters (verified: a 1920×1080 file flagged 90° reaches the filters as 1080×1920), so the window is computed on display dimensions.
- When the source exceeds the frame-rate cap, `fps=60` is inserted before `scale`. *Native* resolution drops the `scale` step (→ 608×1080 here). HDR sources prepend chain F.

**E. Vertical Blur 9:16 → 1080×1920**
```bash
ffmpeg -hide_banner -nostdin -y \
  -ss 920.120 -to 955.800 -i "livestream_2026_10_21.mp4" \
  -filter_complex "[0:v]split=2[bg][fg];[bg]scale=270:480:force_original_aspect_ratio=increase,crop=270:480,boxblur=10:2,scale=1080:1920,setsar=1[bgv];[fg]scale=1080:1920:force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1[fgv];[bgv][fgv]overlay=(W-w)/2:(H-h)/2,format=yuv420p[vout]" \
  -map "[vout]" -map "0:a:0?" \
  -c:v libx264 -preset fast -crf 20 -maxrate 16M -bufsize 32M -fps_mode passthrough \
  -c:a aac -b:a 192k -ac 2 \
  -movflags +faststart \
  -progress pipe:1 -nostats \
  -f mp4 "02_Jump_Scare_Reaction_vblur.mp4.part"
```
- Background: cover-scale to quarter resolution (270×480), blur, scale up. Foreground: fit inside 1080×1920, centred.
- `-filter_complex` output must be mapped explicitly; `"0:a:0?"` exports video-only when there is no audio.

**F. HDR → SDR tone-map (P1, needs `zscale`).** Insert at the start of the video chain (before `crop` in D, before `split` in E) and tag the output `-color_primaries bt709 -color_trc bt709 -colorspace bt709`:
```text
zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p
```
Verified on a 10-bit PQ fixture (output 8-bit BT.709).

Examples C–E use the template `{index}_{name}_{preset}`; with the default template all presets share one name and EXP-07 resolves the collision.

### 7.7 Crop window algorithm
```rust
/// Largest 9:16 window inside the display frame (rotation applied, square pixels).
/// Returns (w, h, x, y) in pixels, all even.
fn crop_window(dw: u32, dh: u32, offset: f64) -> (u32, u32, u32, u32) {
    let even_round = |v: f64| ((v / 2.0).round() as u32) * 2; // nearest even
    let floor_even = |v: f64| ((v / 2.0).floor() as u32) * 2;
    let off = offset.clamp(-1.0, 1.0);
    if dw as u64 * 16 >= dh as u64 * 9 {
        // at least as wide as 9:16: full height, slide on X
        let h = dh & !1;
        let w = even_round(dh as f64 * 9.0 / 16.0).min(dw & !1);
        let x = floor_even((dw - w) as f64 * (1.0 + off) / 2.0);
        (w, h, x, 0)
    } else {
        // taller than 9:16: full width, centred on Y, offset ignored
        let w = dw & !1;
        let h = even_round(dw as f64 * 16.0 / 9.0).min(dh & !1);
        (w, h, 0, floor_even((dh - h) as f64 / 2.0))
    }
}
```

Test vectors (the UI overlay and any TypeScript port must reproduce them):

| Display frame | Window | x at offset −1 / 0 / +0.3 / +1 | y |
| :--- | :--- | :--- | :--- |
| 1920×1080 (16:9) | 608×1080 | 0 / 656 / 852 / 1312 | 0 |
| 3840×2160 (4K) | 1216×2160 | 0 / 1312 / 1704 / 2624 | 0 |
| 1280×720 | 406×720 | 0 / 436 / 568 / 874 | 0 |
| 2560×1080 (21:9) | 608×1080 | 0 / 976 / 1268 / 1952 | 0 |
| 1440×1080 (4:3) | 608×1080 | 0 / 416 / 540 / 832 | 0 |
| 1080×1080 (1:1) | 608×1080 | 0 / 236 / 306 / 472 | 0 |
| 768×576 (720×576, SAR 16:15) | 324×576 | 0 / 222 / 288 / 444 | 0 |
| 1080×1920 (9:16) | 1080×1920 | 0 (offset ignored) | 0 |
| 1080×2340 (taller than 9:16) | 1080×1920 | 0 (offset ignored) | 210 |

Verified: for each 1080p-class, 720p, 21:9, 4:3, 1:1, 9:16, 1080×2340 and SAR 16:15 fixture at offsets −1, 0 and +1, the exported 1080×1920 frame had no black rows or columns, and its mean brightness matched the source region it was cut from.

### 7.8 Export scheduler & process management
- Jobs run on tokio; a `Semaphore` with `parallel_jobs` permits caps concurrent FFmpeg processes (EXP-12).
- FFmpeg processes must not outlive the app. Windows: each child joins a Job Object with kill-on-close. Linux: `PR_SET_PDEATHSIG`. macOS: child PIDs are journaled and terminated on next launch. On every OS, stale `.part` files from the journal are removed at startup.
- Windows children are spawned without a console window (`CREATE_NO_WINDOW`).
- stderr is captured to a per-clip log; the last 20 lines are stored with a failure.
- A clip succeeds when FFmpeg exits with code 0, reported `progress=end`, and the output probes with a video stream of the expected duration (±1 s, plus lead-in for Lossless).

### 7.9 Security
- Tauri capabilities expose only the app's own commands to the frontend: no generic shell or filesystem access from JavaScript.
- Only the bundled `ffmpeg`/`ffprobe` sidecars can be spawned, with arguments built in Rust from validated values (enums, numbers, canonical paths), never strings passed in from the UI.
- The asset-protocol scope is narrowed at runtime to the opened source and the cache folder.
- No core feature needs network access (§8.4).

---

## 8. Non-Functional Requirements

**Reference hardware (R):** Windows 11, 6-core/12-thread desktop CPU (2021 or newer), 16 GB RAM, NVMe SSD, RTX 3060-class GPU.  
**Minimum hardware (M):** 4-core CPU (2018 or newer), 8 GB RAM, SATA SSD, integrated GPU; on macOS, Apple M1 with 8 GB.

### 8.1 Performance
| Metric | Target | Conditions |
| :--- | :--- | :--- |
| Cold start to interactive | ≤ 1.5 s | R |
| Open a 3 h / 20 GB MP4 to first frame | ≤ 2 s, without reading the whole file | Tier 1; R and M |
| Seek latency | ≤ 300 ms p95 | 1080p H.264, 2 s GOP, Tier 1, R |
| Frame step | ≤ 100 ms forward; ≤ 500 ms backward | Same; long-GOP sources may exceed the backward target |
| Mark capture | Mark = time of the frame on screen when paused; no IPC in the capture path | All |
| Timeline | 60 fps pan/zoom with 500 clips and a waveform | R |
| Memory | Rust core ≤ 100 MB; whole process tree incl. WebView ≤ 400 MB during 1080p playback; flat regardless of file size | R |
| Lossless export | 10 × 60 s clips in < 10 s total, process start included | 1080p60 H.264 ≈ 8 Mbps, NVMe, R |
| Vertical export | ≥ 1× realtime (software), ≥ 4× realtime (hardware encoder) | 1080×1920, 60 fps, R |
| Waveform | First view ≤ 5 s; full 3 h track ≤ 60 s | SSD, R |
| Auto-save | On disk ≤ 1 s after a change | All |

v1.2's single "< 150 MB RAM" target is unrealistic once the WebView's own renderer and GPU processes are counted, so memory is now budgeted per process.

### 8.2 Reliability
- **Zero marker loss:** a kill test (1,000 random force-kills during scripted edits and saves) never leaves a corrupt project and loses at most the last second of edits.
- **Export success** ≥ 99.5% of clips across the golden test set (§11), excluding fixtures that are corrupt by design.
- **No orphaned FFmpeg processes** after a crash, cancel or quit, verified on each OS.

### 8.3 Installer size
≤ 60 MB per platform and architecture (v1.2's 35–60 MB range kept). This needs a custom FFmpeg build with only the required demuxers, decoders, encoders and filters, built as shared libraries so `ffmpeg` and `ffprobe` don't each carry a full copy; stock "full" static builds are far larger. macOS ships separate Apple Silicon and Intel downloads rather than one universal binary.

### 8.4 Privacy & offline
- Every core feature works offline; media never leaves the machine.
- Crash reports and usage metrics are opt-in (off by default) and never include file names, paths or clip names.
- The network is used only for opt-in update checks and future opt-in downloads (e.g., Whisper models).

### 8.5 Accessibility & localisation
- Every control is keyboard-reachable with a visible focus ring and has an accessible name for screen readers.
- Text contrast ≥ 4.5:1; colour tags always carry a label; the OS reduced-motion setting is honoured.
- English at launch, with every UI string externalised for translation (launch languages: Q9).

### 8.6 Platform support (proposed; confirm in Phase 0)
| OS | Minimum | Notes |
| :--- | :--- | :--- |
| Windows | 10 and 11, x64 (ARM64: P2) | WebView2 runtime via the bundled bootstrapper |
| macOS | 12 Monterey or later; Apple Silicon and Intel | App and sidecars signed and notarised |
| Linux | x86_64; Ubuntu 22.04 / Fedora 39 class | WebKitGTK video needs GStreamer codec plugins; packaging decision Q7 |

---

## 9. Edge Cases & Error Handling

| Area | Scenario | Resolution |
| :--- | :--- | :--- |
| Marking | New mark conflicts with the other (In ≥ Out) | The other mark is cleared with a toast; marks are never swapped (MRK-01) |
| Marking | Range shorter than the minimum (0.5 s) | Not added; the toast states the minimum |
| Marking | Mark before 0 or after the end | Clamped |
| Marking | Typing in a text field | Shortcuts suspended (§5) |
| Source | Container the WebView can't play | Tier 2 remux, automatic (§7.3) |
| Source | Codec the WebView can't decode | Tier 3 proxy (P1); the MVP shows a specific message, and export still works |
| Source | Variable frame rate | Frame step uses the media index; re-encodes keep the original timing unless a frame-rate cap applies |
| Source | Rotation metadata (phones) | Display orientation used everywhere; Lossless keeps the rotation flag (verified) |
| Source | Non-square pixels | Display width used for framing (FRM-02) |
| Source | HDR (PQ/HLG) | Pre-flight warning (P0); tone-map (P1, FRM-07) |
| Source | Multiple audio tracks | Lossless and Frame-Accurate keep all; vertical presets use the chosen track; preview plays the default track |
| Source | No audio | Video-only output |
| Source | `start_time` ≠ 0 | Times are relative to the first frame; verified per tier (§7.3) |
| Source | Truncated or corrupt file (e.g., MP4 missing its index after a recorder crash) | Probe fails with a specific message and recovery hint; MKV plays up to the last valid cluster |
| Source | File changed on disk, or a different file at the same path | Fingerprint mismatch warning (PRJ-04) |
| Source | Moved or deleted | *Locate Missing Media* (PRJ-04) |
| Source | File still growing (recording in progress) | Not supported in v1; a size change during the session shows a warning |
| Source | Network or slow drive | Works; Lossless stays sequential; the ETA uses measured throughput |
| Export | Lossless clip starts before In | Expected (keyframes): effective start shown (MRK-07); use Frame-Accurate or *Snap In to keyframe* |
| Export | Codec not allowed in MP4 | Source container, or audio-only transcode (EXP-03) |
| Export | Duplicate names or existing files | `_2`, `_3`, …; dialog policy (EXP-07) |
| Export | Illegal characters, reserved names | Sanitization (EXP-06) |
| Export | Path longer than 260 characters on Windows without long-path support | Name truncated; if still too long, a pre-flight error before the batch starts |
| Export | Not enough disk space | Pre-flight estimate (EXP-11); a full disk mid-batch pauses it (EXP-10) |
| Export | Output folder disappears | Batch pauses with a prompt (EXP-10) |
| Export | Hardware encoder fails | Clip retried with libx264 (EXP-13) |
| Export | Cancel, quit or crash mid-export | Processes killed, `.part` removed, statuses reset to Pending on next launch |
| Export | Clip extends past the end of the source | Out clamped to the duration; a clip whose In is past the end is flagged *Out of range* |
| Project | Source folder is read-only | Project stored in the app library (PRJ-02) |
| Project | Same project opened twice | Second copy opens read-only (PRJ-05) |
| Project | Newer schema version, or corrupt file | Read-only, or newest valid backup (PRJ-06) |

---

## 10. Acceptance Criteria

### MVP (P0)
| ID | Covers | Criterion |
| :--- | :--- | :--- |
| AC-01 | MRK-01, PRJ-01 | With a source loaded, `I` at t1, `O` at t2 (t2 − t1 ≥ 0.5 s) and `Enter` add a clip [t1, t2] that appears on the timeline within 100 ms and is on disk within 1 s. |
| AC-02 | MRK-01 | `O`, then `I` at an earlier time, then `Enter` creates a valid clip (Out-first); `I` then `Enter` creates [In, playhead]. |
| AC-03 | MRK-01 | No input sequence creates a clip with Out ≤ In; a conflicting mark clears the other and shows a toast. |
| AC-04 | PRJ-01 | Kill test (§8.2): 1,000 random force-kills during a scripted editing session → 0 corrupt projects, ≤ 1 s of edits lost. |
| AC-05 | PRJ-03 | Reopening a source restores clips, names, tags, statuses, playhead and zoom exactly. |
| AC-06 | PRJ-04 | After moving the source, reopening offers relink; the moved file restores all markers; a different file triggers the mismatch warning. |
| AC-07 | EXP-02 | Lossless outputs contain packets bit-identical to the source for the exported range (packet-hash comparison, `-c copy -f framemd5`). |
| AC-08 | EXP-02 | Frame-Accurate outputs start on the source frame at In for CFR fixtures (decoded-frame comparison) and within 1 frame for VFR fixtures, with the expected frame count. |
| AC-09 | EXP-10 | In a 10-clip batch where one clip is built to fail, the other 9 complete and the failed row shows a readable reason and *Retry*. |
| AC-10 | EXP-08, EXP-09 | Cancelling mid-clip leaves no `.part` file and no running FFmpeg process. |
| AC-11 | EXP-06, EXP-07 | The sanitization vectors pass on Windows, macOS and Linux; no export overwrites a file outside the chosen policy. |
| AC-12 | PLY-01 | Every golden-set fixture (§11) either plays or shows a specific, actionable message within 5 s; nothing hangs. |
| AC-13 | §5 | Every P0 action in §5 works without a mouse; no shortcut fires while typing in a text field. |
| AC-14 | §8.1 | Opening the 20 GB fixture stays within the memory target and reads < 50 MB from disk before the first frame. |
| AC-15 | MRK-04 | Undo/redo returns the exact prior state across 10,000 random operation sequences (property test). |
| AC-16 | EXP-11 | Pre-flight blocks export when free space < 1.2 × estimate or the output folder isn't writable. |

### Phase 2 (P1) examples
| ID | Covers | Criterion |
| :--- | :--- | :--- |
| AC-17 | FRM-02, FRM-03 | For fixtures M06–M10 at offsets −1, 0 and +1, vertical exports are 1080×1920, contain no black rows or columns, and match the §7.7 vectors. |
| AC-18 | FRM-04 | The preview overlay rectangle equals the exported window for every §7.7 vector. |
| AC-19 | EXP-12 | With 4 parallel jobs, a batch never exceeds 4 FFmpeg processes and finishes with the same outputs as a sequential run. |

---

## 11. Test Media Matrix (golden set)

Fixtures are short or generated unless size is the point.

| ID | Fixture | Exercises |
| :--- | :--- | :--- |
| M01 | 1080p60 H.264/AAC MP4, 2 s GOP, 3 h (~10–20 GB) | Baseline, performance, memory (AC-14) |
| M02 | OBS-style MKV, H.264 + 4 audio tracks | Tier 2 remux, multi-track mapping |
| M03 | MP4 with a 10 s GOP (screen recording) | Lossless lead-in, keyframe snap |
| M04 | VFR phone recording (Android, iPhone) | Frame stepping, timing (AC-08) |
| M05 | iPhone HEVC 10-bit HLG, rotated 90° | HDR warning/tone-map, rotation, HEVC tiers |
| M06 | 4:3, 1440×1080 | Framing |
| M07 | 21:9, 2560×1080 | Framing without letterbox |
| M08 | 9:16, 1080×1920 | Framing, offset ignored |
| M09 | 1080×2340 screen recording | Taller than 9:16 |
| M10 | 720×576, SAR 16:15 | Non-square pixels |
| M11 | FLV, H.264/AAC | Tier 2 |
| M12 | WebM, VP9/Opus | Container mapping |
| M13 | MP4 with `start_time` ≠ 0 (remuxed from TS) | Time origin |
| M14 | MP4 without audio | Mapping |
| M15 | Truncated MP4 (missing index) | Error handling |
| M16 | 4K 144 fps AV1 (NVENC gameplay) | Frame-rate cap, AV1 decode tiers |
| M17 | MOV or MKV with PCM audio | MP4 incompatibility → audio-only transcode |
| M18 | Path with spaces and non-Latin characters | Path handling on every OS |

---

## 12. Development Roadmap (proposed)

Durations are placeholders until team size is known; re-plan at the end of Phase 0. v1.2 scheduled two P0 items (auto-save, crash recovery) after the MVP; every P0 item now ships in Phase 1.

### Phase 0: Technical spike (Week 1)
- Playback matrix: golden-set fixtures on WebView2, WKWebView and WebKitGTK → tier strategy confirmed, or switch to libmpv/WebCodecs (Q4).
- Byte-range streaming of a 20 GB file through the asset protocol: flat memory, working seeks.
- Measure mark-capture accuracy (`requestVideoFrameCallback`) and frame-step latency.
- FFmpeg build configuration and licensing decision (R3); sidecars signed and notarised in CI on all three OSes.
- **Exit:** decision records for playback and the FFmpeg build; go/no-go.

### Phase 1: MVP, all P0 (Weeks 2–5)
- Probe, Tier 1–2 playback, transport, speed, time display (PLY-01–04).
- Marking, overview timeline, overlaps, undo/redo (MRK-01–04).
- Clip queue with statuses (QUE-01–05).
- Export engine: Lossless and Frame-Accurate (libx264, original framing), naming, sanitization, collisions, safe writes, progress and cancel, fault tolerance, pre-flight (EXP-01–11).
- Auto-save, backups, recovery, relink, lock, schema versioning (PRJ-01–06).
- P0 shortcuts; signed packages for all three OSes.
- **Exit:** AC-01–AC-16 pass on the golden set.

### Phase 2: Clipper Pro, P1 (Weeks 6–9)
- Waveform, zoom timeline and media index, keyframe awareness, trim handles, colour tags (PLY-05–06, MRK-05–07).
- Search and sort, bulk edit, 500-clip scale (QUE-06–08).
- Framing: Vertical Crop with pan, Vertical Blur, output presets, frame-rate cap, HDR tone-map, preset maximum duration (FRM-01–08).
- Parallel scheduler, hardware encoders, notifications and sleep blocking (EXP-12–14).
- Tier 3 proxy, MPEG-TS input, recent projects (PRJ-07), auto-updater.
- **Exit:** AC-17–AC-19 pass.

### Phase 3: Smart features (v2.0, TBD)
- **Audio peak detection:** suggest loud moments (laughter, shouting, action) as candidate clips the user accepts or dismisses.
- **Local Whisper.cpp captions:** `.srt` per clip or burned in, fully offline; multilingual models are downloaded on demand (opt-in) to keep the installer small.
- Smart cut, timestamp import/export, hover thumbnails, shortcut remapping (EXP-15–17, PLY-07).
- To validate with users first: chat-activity or stream-marker import, auto-reframe (subject tracking for the 9:16 window), multi-source projects.

---

## 13. Risks & Mitigations

| ID | Risk | Likelihood | Impact | Mitigation |
| :--- | :--- | :--- | :--- | :--- |
| R1 | WebViews can't play key formats (FLV, MKV on macOS, HEVC without decoders; Linux needs GStreamer plugins) | High | High | Playback tiers (§7.3), Phase 0 spike, native-player fallback (Q4) |
| R2 | HTML5 seeking and frame stepping are imprecise on long-GOP or VFR sources | Medium | High | On-screen-frame time capture, media index, short-GOP proxy |
| R3 | FFmpeg licensing: libx264/libx265 make the build GPL, and H.264/HEVC/AAC encoders raise patent-licensing questions in some markets | Medium | High | Decide with counsel before Phase 1: (a) GPL FFmpeg shipped as a separate sidecar with a source offer, or (b) LGPL FFmpeg using OS encoders (Media Foundation, VideoToolbox, VAAPI) with Cisco's OpenH264 binary, downloaded at first use, as fallback. Tied to Q1 |
| R4 | Unsigned sidecars blocked by Gatekeeper or SmartScreen | Medium | Medium | Sign every binary in CI, notarise every macOS build, Windows code-signing certificate |
| R5 | Installer exceeds 60 MB | High | Low | Minimal shared-library FFmpeg build, per-architecture packages |
| R6 | Lossless lead-in surprises users ("my clip starts early") | High | Medium | Effective start in the UI, *Snap In to keyframe*, clear preset descriptions |
| R7 | Data loss from non-atomic writes or two writers | Low | High | Atomic saves, backups, lock file, kill tests |
| R8 | Hardware encoders vary (quality, session limits, drivers) | Medium | Medium | Capability probe, per-clip software fallback, conservative defaults |
| R9 | MVP scope vs. schedule | High | Medium | Re-plan after Phase 0; cut by priority, not by date |
| R10 | Working names clash with existing products | Medium | Low | Trademark search before branding (Q2) |

---

## 14. Open Questions

| ID | Question | Owner | Needed by |
| :--- | :--- | :--- | :--- |
| Q1 | Business model: free/open source, paid, or freemium? It drives the FFmpeg licensing choice (R3). | Product | Phase 0 |
| Q2 | Final product name (RustCut, QuickClip or other) after a trademark search | Product | Beta |
| Q3 | Frontend framework: Svelte or SolidJS (or React)? | Eng lead | Phase 0 |
| Q4 | If WebView playback misses the bar: libmpv embedding or WebCodecs? | Eng lead | End of Phase 0 |
| Q5 | Lossless default container: MP4 when compatible (proposed) or always the source container? | Product | Phase 1 |
| Q6 | Telemetry scope and consent flow | Product | Beta |
| Q7 | Linux packaging: AppImage, Flatpak (codec extensions), or both? | Eng | Phase 1 |
| Q8 | Which platform presets (Shorts, TikTok, Reels) and duration limits ship, and how they update | Product | Phase 2 |
| Q9 | UI languages at launch | Product | Beta |

---

## 15. Glossary

| Term | Meaning |
| :--- | :--- |
| In / Out | Start and end marks of a clip on the source timeline |
| Pending range | In/Out marks not yet added to the queue |
| Keyframe (I-frame, IDR) | A frame decodable on its own; lossless cuts can only start on one |
| GOP | Group of pictures: the keyframe interval (e.g., 2 s) |
| Lead-in | Extra footage before In in a lossless cut (In minus the preceding keyframe) |
| Stream copy | Copying compressed packets without decoding (Lossless mode) |
| Re-encode | Decode and encode again; enables exact cuts and framing |
| VFR / CFR | Variable / constant frame rate |
| SAR | Sample (pixel) aspect ratio; anything other than 1:1 means non-square pixels |
| Display frame | The picture as shown: rotation applied, square pixels |
| Tier 1 / 2 / 3 | Direct play / remux / proxy preview paths (§7.3) |
| Fingerprint | Size plus a hash of the first and last 1 MiB; identifies a source file |
| `.part` file | Temporary export output, renamed when the export succeeds |
| Outdated | A clip changed since its last export |
