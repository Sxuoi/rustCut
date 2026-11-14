# Performance measurements

`scripts/perf.mjs` measures PRD §8.1 and AC-14 against the embedded Windows app via
the shared CDP harness. It generates synthetic media only, under `.fixtures/perf`
(already git-ignored). It never scans user media. Run it on an idle Windows 11
machine with an SSD/NVMe drive, a 60 Hz or faster display, and no other RustCut
instance. It refuses to start while `rustcut.exe` is running. Close the app yourself;
the script stops only its own processes. Do not run it alongside the e2e suite:
both use CDP port 9333 and the app is single-instance.

## Run

Install npm dependencies and FFmpeg sidecars first. Set `RUSTCUT_FFMPEG_DIR` and
`FFMPEG_DIR` to a directory containing both tools if necessary; the script also
searches the harness's sidecar directory and PATH. Full mode needs about 11 GB
for actual source packets, plus output files; its logical 20 GB file uses NTFS
sparse padding. Sparse-file support is required (`fsutil sparse setflag`); the
script fails if it cannot create the sparse fixture.

```powershell
npm.cmd ci
$env:CARGO_TARGET_DIR = 'D:\RustCut\target-cx-tests'
npm.cmd run e2e:build
npm.cmd run perf -- --quick

# Embedded release build; the normal build command also embeds the frontend.
$env:CARGO_TARGET_DIR = 'D:\RustCut\target-cx-tests'
npm.cmd run tauri -- build --no-bundle
npm.cmd run perf -- --release
```

`--quick --release` is also supported. `node scripts/perf.mjs --help` prints usage
without importing the harness or launching anything. A plain Cargo debug build
uses Vite's development URL; use `e2e:build` for an embedded debug frontend.

A release build runs only the FFmpeg sidecars in its own folder (PRD §7.9); it ignores
`RUSTCUT_FFMPEG_DIR` and `PATH`. `tauri build --no-bundle` does not put them there, so copy
`src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe` and `ffprobe-x86_64-pc-windows-msvc.exe`
next to `<target>\release\rustcut.exe` as `ffmpeg.exe` and `ffprobe.exe` before `--release`.

Quick mode uses 640×360 at 30 fps, a 6 s seed repeated to 60 s, a logical 128 MiB
tail-index MP4, ten 6 s lossless clips, and two minutes of audio. It exercises all
measurement paths, usually in a few minutes. Its numeric results are `INFO`,
because these workloads cannot establish the full PRD targets. Measurement
errors are `FAIL` even in quick mode. Full mode generates a 1080p60 H.264 seed
at approximately 8 Mbps, stream-copies it to three hours, pads to exactly
20,000,000,000 bytes, measures ten 60 s lossless clips and a 60 s vertical clip,
and processes three hours of audio. Initial fixture creation can take time and
is excluded from every reported metric. Existing complete fixtures are reused.

The selected build is recorded in the report. Debug timings are diagnostic;
use an embedded release build for target comparisons. Full mode reports
`PASS`/`FAIL` against numeric targets, and `INFO` for nonnumeric or unavailable
measurements (including missing hardware encoders). It exits nonzero on a failed
measurement or target. Hardware results name the actual available encoder.

JSON and a Markdown table are printed and saved as
`.fixtures/perf/{quick,full}-v1-results.{json,md}`. Each report includes the build,
fixture dimensions, durations, bitrate, raw latency samples, frame intervals,
and process-counter samples. Run-specific hard links, project sidecars and the exported
files use a fresh directory on the fixture volume; the harness also creates a fresh app
and WebView profile. These are cleaned after the owned app exits. Generated
inputs and reports remain for reuse. Do not compare numbers from busy machines
or different build modes as if they were equivalent.

## What is measured

| Measurement | Method and limits |
| :--- | :--- |
| Cold start | Spawn to the editor with a playable decoded video, using an isolated profile and no harness settling delay. Includes CDP connection and readiness polling overhead; conservative upper bound. OS file cache is not flushed. |
| Large open | A real single-instance file hand-off runs the UI's ordinary open flow. A video frame callback armed before hand-off timestamps first presentation. Includes second-launch hand-off overhead. The MP4 has a trailing `moov`; a sparse `free` atom before it leaves encoded packet offsets intact. Full mode has a real three-hour index but repeated synthetic content. |
| AC-14 disk reads | A helper PowerShell process samples `ReadTransferCount` of the owned process tree about every 30 ms (Toolhelp snapshot and `GetProcessIoCounters`). It keeps a handle to every process it has seen, so a child that exits early (ffprobe) still reports its final count. The judged value is the reads of `rustcut.exe` and its FFmpeg/ffprobe children, the only processes that open the media file, from the last sample before the hand-off to the first sample after the first presented frame: an upper bound, reported with the last sample before the frame. Processes created after the first frame are left out. An INFO row adds the WebView2 processes, which receive the file's bytes from `rustcut.exe` over IPC. This is process I/O transfer, including cached and non-media reads, not physical disk sectors: an approximation, not proof of physical disk traffic. The report keeps CIM snapshots before the hand-off and after `waitReady` for context. |
| Seek | Thirty reproducible pseudorandom seeks across a 2 s GOP H.264 seed, from assignment of `video.currentTime` to a newly presented video frame; p50 and p95. CDP round trips are excluded. |
| Frame step | Ten real Period and Comma key presses in each direction, from keydown to new presented frame, reported as p95. |
| Mark capture | Paused transport timestamp captured in the real I keydown handler compared with pending In. Equality is checked; the absence of IPC in the capture implementation remains covered by frontend tests/code review. Reported as INFO. |
| Timeline | Exactly 500 clips created with `import_timestamps`, after an actual waveform completes. Dispatch wheel zooms over five seconds and collect `requestAnimationFrame` intervals; assert that the timeline summary changes. Report mean scheduling fps and p95 interval. This measures responsiveness while exercising redraws, not GPU presentation rate; display refresh limits the result. |
| Memory | `PrivatePageCount` (private committed bytes) from Win32_Process for the owned Rust PID and all descendants during playback. Five samples, report peak core and tree totals. The 20 GB stage starts once the launch warm-up has written its hardware-encoder cache, so its test encodes are not counted as playback. Compare small-file and 20 GB playback tree totals; the size delta is INFO because PRD defines no numeric flatness tolerance. This is not working-set/RSS memory. |
| Lossless | From `start_export` invocation through persisted completed statuses for all ten clips. Includes pre-flight, encoder detection if performed by the app, child-process creation and output verification; no encoder warmup call precedes it. |
| Vertical | Media seconds divided by wall seconds, same completion endpoint, 1080×1920 output with 60 fps cap. Measure explicit libx264 and an available hardware encoder separately; no hardware yields INFO. |
| Waveform | Fresh source/cache: open request to first nonempty waveform event, then final `done` event. Includes open/probe and the first presented frame, which the waveform job waits for (AC-14). Observes the app's existing Tauri channel callbacks and forwards them unchanged, so the UI also receives the waveform. |
| Auto-save | Start a backend rename and poll the actual `.rustcut` JSON until the new name is on disk (10 ms polling, generous timeout). |

The long synthetic source and sparse padding test bounded opening and index
access, not the entropy or disk layout of a real recording. OS caches may be warm
from generation; record cache conditions when reviewing a run. Endpoint overhead
and machine load can cause a failure that needs investigation; never rewrite it
as a passing result. End-to-end and performance execution remains a separate
verification step from JavaScript syntax checks.

## Results to fill after verification

### Full run, release build, idle machine

Date: 2026-11-14 · Build: release (`npx tauri build --no-bundle`, embedded frontend) at commit
eaf5d8a, with the bundled FFmpeg 9.0.2 sidecars copied next to `target\release\rustcut.exe` ·
Command: `npm run perf -- --release` (full mode).

Machine: AMD Ryzen 7 8845HS laptop CPU (8 cores, 16 threads) on AC power with the Windows
"Balanced" power plan; integrated AMD Radeon 780M (driver 32.0.21030.1005); 16 GB LPDDR5x-6400
(13.8 GB visible to Windows); SK hynix HFS512GEJ4X112N PCIe NVMe (fixtures on D:); Windows 11
Home 10.0.26200; WebView2 Runtime 157.0.4341.46; Node 24.15.0. The app window opened on an
external 1920×1080 75 Hz monitor. This is not the PRD's reference hardware (R): the GPU is
integrated, so the hardware encoder is AMD AMF rather than NVENC and GPU memory is system RAM.

Conditions: total CPU 4–10 % before each run (other Claude Code sessions) and no FFmpeg process.
An orphaned `find` that had kept one core busy since the previous evening and a debug RustCut
window were stopped first. Run 1 created the fixtures (about 22 GB written; excluded from the
timings, but Windows was still flushing those writes while it measured). Run 2 started after the
disk had settled and reused them; it is the recorded result. The OS file cache was not flushed:
the 11.1 GB source is larger than free RAM, but run 1 had read the 20 GB file's 16.1 MB `moov`.
Fixtures: 1080p60 H.264 at 8.23 Mbps with AAC, three hours, 11.12 GB, plus the 20 GB tail-index
copy. Raw reports: `.fixtures/perf/full-v1-run{1,2,3,4}-results.json` (git-ignored copies;
every run overwrites `full-v1-results.json`).

| PRD §8.1 / AC-14 target | Target | Run 2 (recorded) | Result | Run 1 |
| :--- | :--- | :--- | :--- | :--- |
| Cold start to interactive | ≤1.5 s | 1.61 s | FAIL | 3.86 s |
| 3 h / 20 GB MP4 to first frame | ≤2 s | 1.17 s | PASS | 2.09 s (FAIL) |
| AC-14 process-tree read transfer (snapshot after first frame) | <50 MB | 543.2 MB | FAIL | 337.0 MB |
| Seek p50 (informational) / p95 | p95 ≤300 ms | 55.9 / 98.9 ms | PASS | 55.7 / 112.1 ms |
| Frame step forward / backward (p95) | ≤100 / ≤500 ms | 91.6 / 89.1 ms | PASS | 95.8 / 93.7 ms |
| Paused mark capture | Exact displayed frame | Exact (00:00:19.767 both) | Meets it (INFO) | Exact |
| Timeline, 500 clips + waveform | 60 fps | 75.0 fps (p95 interval 13.6 ms) | PASS | 75.0 fps |
| Rust core private bytes, 1080p playback | ≤100 MB | 13.6 MB | PASS | 13.1 MB |
| Process-tree private bytes, 1080p playback | ≤400 MB | 662.1 MB | FAIL | 651.1 MB |
| AC-14 20 GB playback, Rust core | ≤100 MB | 20.7 MB | PASS | 20.0 MB |
| AC-14 20 GB playback, process tree | ≤400 MB; flat vs small file | 844.5 MB (+182.4 MB) | FAIL | 1,049.4 MB |
| Lossless, 10 × 60 s, process start included | <10 s | 8.02 s | PASS | 9.45 s |
| Vertical 1080×1920 60 fps, software | ≥1× realtime | 1.78× (libx264) | PASS | 1.81× |
| Vertical 1080×1920 60 fps, hardware | ≥4× realtime | 0.99× (h264_amf) | FAIL | 0.97× |
| Waveform first chunk / full 3 h track | ≤5 / ≤60 s | 0.38 / 7.61 s | PASS | 0.41 / 8.84 s |
| Auto-save on disk | ≤1 s | 0.29 s | PASS | 0.30 s |

Two passes have little margin: frame step forward (each step is a seek that decodes from the
keyframe, 86–92 ms per step with the 2 s GOP) and lossless export (9.45 s in run 1).

#### Causes of the failures

Found with probes run after run 2 on the same build and machine: the e2e CDP harness, process
command lines and Windows counters. The probe scripts are not in the repository. Nothing here
changes the results above.

**Cold start (1.61 s).** perf.mjs measures a first-ever launch: a fresh WebView2 profile and
empty app data. Traced launches (IPC requests timed in the page, `<video>` events, the app log):

- The document starts 450–540 ms after spawn. Tauri's `setup` takes 2–7 ms; the rest is process
  start and WebView2 (environment, browser process, controller).
- DOMContentLoaded follows about 50 ms later. `App.svelte` then waits for `loadAppContext()`,
  whose `get_app_info` awaits the FFmpeg capability probe that `warm_up` starts in `setup`
  (`ffmpeg -version`, `-filters`, `-encoders`): 70–170 ms. Only then do `get_launch_path` and
  `open_path` run (104–125 ms, mostly ffprobe).
- From the open result to the first decoded frame: about 300 ms with a fresh WebView2 profile,
  about 160 ms with a reused one.
- With empty app data, hardware detection runs a 640×360 AMF test encode (196 MB private) that
  is still running at the first frame.

Spawn to ready was 1.12–1.20 s with a fresh profile and empty app data, 1.05–1.17 s once the
hardware-encoder cache exists, 0.89–0.96 s with a reused profile, and 1.44–1.61 s for the first
launch after the heavy file I/O of a run, most likely because the binaries had to be read from
disk again. Fixed afterwards (see "After the cold-start fix" below): the launch file no longer
waits for app info, and the warm-up starts 3 s after setup instead of at once. Not done:
probing the launch path already in `setup`. WebView2's own start and the fresh-profile cost
are outside the app.

**AC-14 reads (543 MB).** A real failure, not only the late snapshot. `startMediaJobs()` in
`src/lib/state/session.ts` starts the waveform job as soon as `open_path` returns, before the
`<video>` element has loaded. Its FFmpeg (`-i <file> -map 0:a:0 -vn … -f s16le pipe:1`)
demuxes the whole interleaved file at up to 1.5 GB/s and accounts for 465.7 MB of run 2's total.
A probe that sampled the app's per-process `ReadTransferCount` about every 0.2 s saw the waveform
process start 324 ms after the hand-off and read 842 MB by the last sample before the first
frame (which came at 1,173 ms). The rest: rustcut.exe 36–43 MB (the 2 × 1 MB fingerprint, one
pass over the 16.1 MB `moov` for the sample-table index, and the asset protocol serving the
`moov` and first media to the WebView), and the WebView2 browser process 20–33 MB, mostly the
same bytes received from rustcut.exe (WebView processes never open the file). Without the
waveform, about 20 MB of the file is needed before the first frame, although the `moov` is read
three times (ffprobe, the index, the WebView). Fixed after this run: the waveform now waits
for the first frame, and perf.mjs now counts the reads at the first frame instead of about 1 s
later (see "After the waveform fix" below).

**Process-tree memory during 1080p playback (662 MB).** WebView2, not the Rust core (13.6 MB).
In a clean session (open the 60 s seed, play) the tree was 403–475 MB over eight samples: GPU
process 255–300 MB (Windows reports 61 MB of shared GPU memory for it), renderer 93–99 MB,
browser 39–40 MB, network, storage and audio utilities 28–29 MB, crashpad 3 MB, Rust core
12 MB. perf.mjs plays the small file after the 20 GB stage in the same app. That order alone
gave 481–495 MB, with the renderer still at 192 MB after `close_project`; in the perf run the
renderer reached 325 MB after its seeks and steps. Even a clean session is 1–19 % over the
budget. Options to evaluate, none confirmed: measure on R (discrete GPU), release the `<video>`
source on close so the renderer can purge, WebView2's `MemoryUsageTargetLevel`.

**AC-14 20 GB playback memory (845 MB).** The Rust core stays flat (20.7 MB). A probe saw
650–676 MB while the 20 GB file played: renderer 275 MB, the waveform FFmpeg 98 MB (a child of
rustcut.exe for the 8 s it runs), GPU process 212 MB, browser 40 MB. The renderer holds 175 MB
more than with the seed, although the file is a stream copy of it that differs only in length,
so this is most likely Chromium's demuxer state for 1.15 million samples. perf.mjs adds the
stage-order effect above. Playing a three-hour MP4 in the WebView's `<video>` grows with
duration; a flat tree needs a different playback path or an accepted per-hour allowance.

**Hardware vertical export (0.99×, h264_amf).** AMF's QVBR rate control, as set in
`crates/rustcut-ffmpeg/src/hw.rs` (`-quality quality -rc qvbr -qvbr_quality_level 20` plus the
16M cap), runs at 55–63 frames per second on this Radeon 780M with FFmpeg 9.0.2, whatever the
frame size (640×360: 60 fps), input frame rate (30/60/120), `-quality` preset, `-usage`,
`-async_depth` (1–42) or `-preencode 0`. QVBR and HQVBR need pre-analysis (`-preanalysis 0`
fails to initialise). The same encode with `vbr_peak`, `vbr_latency`, `cbr` or `cqp` runs at
316–400 fps, and decoding with the crop and lanczos upscale alone runs at 1,372 fps. The full
export command with `-rc vbr_peak -b:v 12M -maxrate 16M -bufsize 32M` reached 5.05× (11.9 s for
60 s) but wrote 12 Mbps where QVBR wrote 2.6 Mbps for this clip. Choosing is a product decision:
QVBR (quality-targeted, about 1× here) or `vbr_peak` with a target derived from the preset (fast;
needs a quality check such as VMAF). The startup detection encode uses the same QVBR mode. The
PRD target refers to an RTX 3060-class NVENC, which could not be measured here.

#### Measurement notes for perf.mjs

- AC-14 (changed after run 3): the snapshot used to follow `waitReady` and a CIM call, about
  1 s after the first frame, and could not see ffprobe, which exits before it. perf.mjs now
  samples at the first frame and judges only rustcut.exe and its FFmpeg children, the
  processes that read the file (see "What is measured").
- Memory (not changed): the small-file stage runs after the 20 GB stage in the same app. A
  fresh app per stage, or per-process samples for both stages, would separate the two budgets.
- Cold start (not changed): report a reused-profile launch next to the first-ever one. It is a
  single launch per run, so it varies by a few hundred milliseconds between runs.
- Frame step (not changed): the p95 of 10 presses is the slowest press. In runs 3, 5 and 6 the
  first step after the seek series took 100–109 ms while the median stayed at 88–95 ms; more
  presses would make the p95 meaningful.
- Exports and warm-up (changed with the cold-start fix): exports go to the run folder on the
  fixture volume instead of `%TEMP%`, where three runs whose cleanup was blocked by WebView2
  had left 1.7 GB on a nearly full system drive. The 20 GB stage waits for the launch warm-up.

#### After the waveform fix and the AC-14 measurement change (working tree on top of eaf5d8a)

`scripts/perf.mjs` now measures AC-14 at the first frame as described in "What is measured".
`startMediaJobs()` (`src/lib/state/session.ts`) now starts the waveform job when the first
frame is presented (`player.firstFrame()`, settled by `player.ready`), or after 3 s
(`MEDIA_JOBS_START_TIMEOUT_MS`) when no frame arrives; a project closed or replaced before then
gets no waveform job. The media index request is still immediate. Unit tests:
`src/lib/state/session.test.ts`, `src/lib/state/player.test.ts`; Vitest 531/531 and the e2e
suite 31/31 (debug build) pass with the change.

Later the same day, after the real-video runs below, the packet-scan index got the same
treatment. The open now builds only the quick index: the cache or the MP4/MOV sample tables
(`MediaService::quick_index`). A file that needs the FFprobe packet scan, such as Matroska,
waits for the new `start_media_index` command, which `startMediaJobs()` sends with the
waveform after the first frame. Tests: `crates/rustcut-media/tests/integration.rs`
(`quick_index` never scans), `src-tauri/tests/e2e.rs` (an MKV gets no index until
`start_media_index`; an MP4 gets its index from the open), `src/lib/state/session.test.ts`;
the media and app crate tests, Vitest 532/532 and the e2e suite 31/31 pass with it.

Measured on a new release build the same day:

- The first-frame probe: the waveform FFmpeg started 7 ms after the first frame (1,222 ms vs.
  1,215 ms after the hand-off). At the last sample before the first frame the app had read
  42.9 MB (rustcut.exe 22.6 MB, WebView2 browser 19.5 MB), against 898.5 MB before the fix.
- perf.mjs run 3, still with the old AC-14 measurement, on a busier machine (total CPU 12–16 %,
  with the Windows Camera app and a browser running): AC-14 read transfer 229.7 MB, still
  **FAIL**. 152.0 MB of it is the waveform job reading between the first frame and the late
  snapshot; rustcut.exe read 40.2 MB and the WebView2 processes 37.4 MB.
- perf.mjs run 4, with the AC-14 measurement at the first frame, on an idle machine again
  (total CPU 7–9 %): **39.9 MB, PASS** (upper bound at the first sample, 24 ms after the first
  frame; 37.8 MB at the last sample, 23 ms before it; 41 samples about 31 ms apart). That is
  rustcut.exe 23.6 MB and ffprobe 16.3 MB, its pass over the 16.1 MB `moov`, which the old
  snapshots never saw. With the WebView2 processes: 62.1 MB (INFO). The old endpoint, after
  `waitReady` and a CIM call, would have counted 360.9 MB. Other rows of run 4: cold start
  1.42 s (warm OS cache; the cold run 2 measured 1.61 s), 20 GB open 0.97 s, seek p95 111 ms,
  frame step 93.3 / 89.3 ms, timeline 75 fps, Rust core 13.4 / 20.8 MB, lossless 5.44 s,
  vertical software 1.94×, waveform 1.11 / 7.86 s and auto-save 0.32 s all PASS; process tree
  654 MB, 20 GB tree 843 MB and vertical h264_amf 1.00× still **FAIL**.
- Waveform first chunk: 1.13 s, up from 0.38 s, because it now waits for the first frame
  (target ≤5 s). Full three-hour track: 8.07 s.
- Run 3's other rows: cold start 1.30 s (PASS, but run 2's 1.61 s was the first launch after
  run 1's heavy I/O, so this does not show an effect of the fix), 20 GB open 1.39 s, seek p95
  116 ms, frame step 99.7 / 99.1 ms (at the limit on the busier machine), timeline 75 fps,
  tree 678 MB (FAIL), 20 GB tree 793 MB (FAIL), Rust core 12.5 / 19.0 MB, lossless 6.76 s,
  vertical 1.68× software and 1.01× h264_amf (FAIL), auto-save 0.30 s.

#### After the cold-start fix (working tree on top of 4031fb7)

`loadAppContext()` no longer fetches app info, which waits for the backend's FFmpeg capability
probe. `startUp()` (`src/lib/state/session.ts`) loads settings, recent projects and a project
kept across a WebView reload, opens the launch file, and fetches app info only after its first
frame (or at once without a video); the Settings dialog loads it if it is still missing. The
launch warm-up (`AppCore::warm_up`: capability probe and hardware test encodes) now starts
`WARM_UP_DELAY` (3 s) after setup; anything that needs it sooner runs it on demand. Tests:
`src/lib/state/session.test.ts` (the launch file opens before app info is requested),
`src-tauri/tests/e2e.rs` (`warm_up` still runs the detection); the app crate tests, Vitest
535/535 and the e2e suite 31/31 pass with it.

Startup traces on the release build, three launches per variant with the 60 s seed (medians).
`open_path` now starts about 40 ms after DOMContentLoaded instead of about 150 ms:

| Variant | DOMContentLoaded to first frame, before → after | Spawn to ready, before → after |
| :--- | :--- | :--- |
| Fresh profile and app data (as perf.mjs) | 565 → 449 ms | 1,141 → 1,017 ms |
| Fresh profile, hardware cache present | 516 → 435 ms | 1,058 → 1,039 ms |
| Reused profile | 398 → 337 ms | 913 → 885 ms |

WebView2's own start before DOMContentLoaded varies by about 100 ms between runs, so the
first column shows the app's part more reliably.

- perf.mjs cold start: 1.08 s (run 5), 1.04 s (a rerun) and 1.43 s (run 6), all PASS; before
  the fix 1.30–1.61 s. Run 5's exports failed because drive C: had 107 MB free (the backend's
  pre-flight refused the export); the export folders that earlier runs had left in `%TEMP%`
  were removed and perf.mjs now writes exports to its run folder.
- Run 6's other rows: 20 GB open 1.15 s, AC-14 reads 39.9 MB, seek p95 101 ms, frame step
  backward 100.3 ms, timeline 75 fps, Rust core 13.2 / 20.5 MB, lossless 5.63 s, vertical
  software 1.98×, waveform 1.15 / 7.58 s and auto-save 0.29 s PASS; frame step forward
  104.1 ms (the first step after the seek series; median 90 ms), process tree 659 MB, 20 GB
  tree 800 MB and vertical h264_amf 1.11× **FAIL**.
- Real clips, three fresh-profile launches each: DJI 4K60 1.19–1.23 s (before 1.54–2.31 s),
  anime MKV 1.03–1.07 s (before 1.41–1.56 s), iPhone HEVC 10-bit HDR 1.43–1.96 s with a median
  of 1.67 s (before 1.83–2.04 s). The HEVC clip still exceeds 1.5 s although its open in a
  running app takes 0.28 s; the extra time on a cold start was not investigated further.

#### Real videos (informational)

The same measurements on the three real clips in `.fixtures\real` (local files, not in the
repository), with the fixed release build on the idle machine. Each file got a fresh app profile
and was hard-linked twice into a run folder, so no project file was written next to the
original: a cold start with the first link, then a first open of the second link in the running
app after its caches were emptied. The script, `real.mjs`, uses perf.mjs's methods but is not in
the repository. These clips do not match the PRD's test conditions (1080p60 H.264 with a 2 s
GOP, three hours, reference hardware), so nothing here is a PASS or FAIL; ⚠ marks values beyond
a §8.1 number. Ranges cover two passes.

| | DJI drone clip | iPhone clip | Anime episode |
| :--- | :--- | :--- | :--- |
| Source | 4K59.94 H.264 High, 159 Mbps, 15.5 s, no audio, 0.5 s GOP | 1080p24 HEVC Main 10, HLG HDR, 17.5 Mbps, 34 s, AAC + Apple APAC | 720p23.976 H.264 in MKV, 0.8 Mbps, 23.7 min, AAC, GOP up to 8 s |
| Playback tier | Direct | Direct | Direct (WebView2 plays this MKV) |
| Cold start, fresh profile | 1.54–1.80 s ⚠ | 1.86–2.04 s ⚠ | 1.55–1.56 s ⚠ |
| Open to first frame | 0.60–0.62 s | 0.28 s | 0.47 s |
| Reads before first frame: rustcut.exe + FFmpeg / with WebView2 | 13.3 / 22.9 MB | 5.1 / 7.5 MB | 78.8 ⚠ / 85.3 MB |
| Waveform first chunk / done | No audio | 0.95 / 1.00 s | 0.66 / 2.07 s |
| Seek p50 / p95 | 70–89 / 397–417 ms ⚠ | 24 / 200 ms | 51 / 107 ms |
| Frame step forward / backward (p95) | 38–63 / 37 ms | 35 / 50 ms | 36 / 27 ms |
| Playback memory: process tree / Rust core | 1,023 MB ⚠ / 12.9 MB | 668 MB ⚠ / 12.0 MB | 510 MB ⚠ / 12.2 MB |
| Lossless, 10 clips | 5.75–6.05 s (1.5 s clips) | 3.24 s (3.3 s clips) | 3.29 s (60 s clips) |
| Vertical 1080×1920: libx264 / h264_amf | 0.97–0.99× ⚠ / 0.93× ⚠ | 1.70× / 1.77× ⚠ | 6.37× / 2.49× ⚠ |

What the real clips add:

- MP4 and MOV open fast and read little before the first frame (5–13 MB). The MKV read 79 MB:
  for containers other than MP4/MOV, the keyframe index comes from an ffprobe packet scan of the
  whole file (`crates/rustcut-media/src/index.rs`), which starts with the open and had read
  69 MB of the 131 MB file by the first frame. A long MKV would read far more before its first
  frame. Fixed afterwards: the scan now starts after the first frame (see "After the waveform
  fix" above). A third pass on the new release build measured the MKV at 9.4 MB before the
  first frame (rustcut.exe 8.3 MB, ffprobe 1.0 MB for the probe), with the first frame at
  0.41 s and the waveform at 1.09 s (first chunk) and 2.41 s (done). A separate check found
  no index at the first frame and the packet-scan index (316 keyframes) about 1.0 s later.
  Player.svelte takes 0 as the first keyframe for the time origin when there is no index yet,
  as it already did whenever the scan finished after the first frame. DJI and iPhone were
  unchanged in that pass (13.3 and 6.2 MB); cold start ranged from 1.41 s (MKV) to 2.31 s
  (DJI).
- Cold start exceeded 1.5 s for all three clips, most for 10-bit HEVC.
- Playback memory grows with resolution and bitrate, almost all of it in the WebView: 1 GB for
  4K60 at 159 Mbps (renderer 555 MB, GPU process 393 MB), 668 MB for 1080p 10-bit HEVC
  (renderer 326 MB, GPU process 250 MB). The Rust core stays at 12–13 MB.
- 4K60 at 159 Mbps seeks at about 400 ms p95 and exports vertically at about 1× with either
  encoder. FFmpeg decodes this clip at 3.1× and decodes, crops and scales it at 2.8×, so
  libx264 is limited by encoding the detailed footage at 1080×1920 60 fps, and AMF by its QVBR
  cap.
- The AMF speed follows that cap of about 60 frames per second: 2.49× for the 24 fps episode,
  0.93× for the 60 fps clip. libx264 was 2.6 times faster than AMF on the 720p episode.
- On the HDR clip both encoders reached about 1.7×. The HDR-to-SDR conversion is the costly
  filter (FFmpeg decodes the clip at 13×, and at 4.0× with the conversion); what limits the full
  export command below that was not measured.

### First recorded run: quick mode, debug build, busy machine (not representative)

Build: debug (`npm run e2e:build`), no commit yet · Machine: AMD Ryzen 7 8845HS, Radeon 780M
(integrated), Windows 11 · Date: 2026-11-14 · FFmpeg 9.0.2 (bundled sidecars) · Command:
`node scripts/perf.mjs --quick` (87 s).

Conditions that make these numbers indicative only: a debug build, quick-mode workloads
(smaller files than the targets name), and a long CPU-bound AV1 encode running on the same
machine (CPU near 100 %). All results are `INFO` by design in quick mode. Re-run full mode
with `--release` on an idle machine before judging any target. Raw samples:
`.fixtures/perf/quick-v1-results.json` (git-ignored).

| PRD §8.1 target | Target | Measured (quick, debug, busy CPU) | Reading |
| :--- | :--- | :--- | :--- |
| Cold start to interactive | ≤1.5 s | 2.76 s | Above target; re-measure on a release build, idle machine |
| 3 h / 20 GB MP4 first frame | ≤2 s | 0.62 s (128 MiB tail-index file) | Within target for the quick file |
| AC-14 process-tree reads to first frame | <50 MB | 8.0 MB (quick file) | Within target for the quick file |
| Seek p50 (informational) / p95 | p95 ≤300 ms | 28.9 / 34.2 ms | Within target |
| Frame step forward / backward | ≤100 / ≤500 ms | 26 / 54 ms (p95) | Within target |
| Paused mark capture | Exact displayed frame; no IPC before capture | Exact (presented 1.967 s = pending In 1.967 s) | Meets it |
| Timeline, 500 clips + waveform | 60 fps | 59.9 fps | At the display refresh rate |
| Rust core / process-tree private bytes | ≤100 / ≤400 MB | 12.6 / 461 MB | Core well within; process tree above, almost all WebView2 processes |
| AC-14 20 GB playback memory | Same budget; flat vs small file | 434 MB tree, 12.3 MB core (quick file) | Core flat; tree above the 400 MB budget |
| Lossless, 10 × 60 s, process start included | <10 s | 6.9 s for 10 × 6 s clips | Quick workload only |
| Vertical, software / hardware | ≥1× / ≥4× realtime | 1.55× / 0.54× (h264_amf) | Hardware figure dominated by encoder start-up on the short quick clips; measure in full mode |
| Waveform, first chunk / full 3 h | ≤5 / ≤60 s | 0.58 / 0.84 s for a 2 min track | Quick workload only |
| Auto-save to disk | ≤1 s | 0.32 s | Within target |

### Crash safety (AC-04, PRD §8.2): 1,000 random force-kills

`cargo test -p rustcut-project --test kill -- --ignored kill_test_full_1000` on the same machine
and conditions (debug build, busy CPU), 2026-11-14: **passed**. 1,000 iterations with kills after
5–400 ms (seed 1794592321188216100), 602 of them mid-save, 10,417 edits in total; maximum edit
loss 824 ms (PRD: ≤ 1 s; the test's assertion bound is 1.3 s, see `MAX_LOSS` in the test), zero
transient save failures, every project file readable after every kill. Run time 597 s.
