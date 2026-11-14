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
and process-counter samples. Run-specific hard links and project sidecars use
a fresh directory on the fixture volume; the harness also creates a fresh app
and WebView profile. These are cleaned after the owned app exits. Generated
inputs and reports remain for reuse. Do not compare numbers from busy machines
or different build modes as if they were equivalent.

## What is measured

| Measurement | Method and limits |
| :--- | :--- |
| Cold start | Spawn to the editor with a playable decoded video, using an isolated profile and no harness settling delay. Includes CDP connection and readiness polling overhead; conservative upper bound. OS file cache is not flushed. |
| Large open | A real single-instance file hand-off runs the UI's ordinary open flow. A video frame callback armed before hand-off timestamps first presentation. Includes second-launch hand-off overhead. The MP4 has a trailing `moov`; a sparse `free` atom before it leaves encoded packet offsets intact. Full mode has a real three-hour index but repeated synthetic content. |
| AC-14 disk reads | `Get-CimInstance Win32_Process` snapshots before hand-off and immediately after first presentation/readiness; difference of `ReadTransferCount` for the owned process tree, including `rustcut.exe` and descendant `msedgewebview2.exe` processes. Newly observed children contribute their lifetime reads. The report contains both snapshots. This is process I/O transfer, including cache/non-media reads, not physical disk sectors. CIM sampling adds time beyond first frame; short-lived children that exit between samples cannot be counted. Use this as an approximation, not proof of physical disk traffic. |
| Seek | Thirty reproducible pseudorandom seeks across a 2 s GOP H.264 seed, from assignment of `video.currentTime` to a newly presented video frame; p50 and p95. CDP round trips are excluded. |
| Frame step | Ten real Period and Comma key presses in each direction, from keydown to new presented frame, reported as p95. |
| Mark capture | Paused transport timestamp captured in the real I keydown handler compared with pending In. Equality is checked; the absence of IPC in the capture implementation remains covered by frontend tests/code review. Reported as INFO. |
| Timeline | Exactly 500 clips created with `import_timestamps`, after an actual waveform completes. Dispatch wheel zooms over five seconds and collect `requestAnimationFrame` intervals; assert that the timeline summary changes. Report mean scheduling fps and p95 interval. This measures responsiveness while exercising redraws, not GPU presentation rate; display refresh limits the result. |
| Memory | `PrivatePageCount` (private committed bytes) from Win32_Process for the owned Rust PID and all descendants during playback. Five samples, report peak core and tree totals. Compare small-file and 20 GB playback tree totals; the size delta is INFO because PRD defines no numeric flatness tolerance. This is not working-set/RSS memory. |
| Lossless | From `start_export` invocation through persisted completed statuses for all ten clips. Includes pre-flight, encoder detection if performed by the app, child-process creation and output verification; no encoder warmup call precedes it. |
| Vertical | Media seconds divided by wall seconds, same completion endpoint, 1080×1920 output with 60 fps cap. Measure explicit libx264 and an available hardware encoder separately; no hardware yields INFO. |
| Waveform | Fresh source/cache: open request to first nonempty waveform event, then final `done` event. Includes open/probe. Observes the app's existing Tauri channel callbacks and forwards them unchanged, so the UI also receives the waveform. |
| Auto-save | Start a backend rename and poll the actual `.rustcut` JSON until the new name is on disk (10 ms polling, generous timeout). |

The long synthetic source and sparse padding test bounded opening and index
access, not the entropy or disk layout of a real recording. OS caches may be warm
from generation; record cache conditions when reviewing a run. Endpoint overhead
and machine load can cause a failure that needs investigation; never rewrite it
as a passing result. End-to-end and performance execution remains a separate
verification step from JavaScript syntax checks.

## Results to fill after verification

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
