# RustCut architecture & build contract

Source of requirements: `docs/PRD.md` (copy of *batch_video_clipper_prd_v1.3.md*).
Requirement IDs (PLY-, MRK-, QUE-, EXP-, FRM-, PRJ-, AC-) are cited in code
comments and tests so every requirement is traceable.

## 1. Layout

```
Cargo.toml                workspace (edition 2024, resolver 3)
crates/
  rustcut-model           schema v2 + IPC DTOs (ts-rs → src/lib/bindings), time, fingerprint   [done]
  rustcut-framing         §7.7 crop window, chains C/D/E/F                                     [done]
  rustcut-naming          EXP-05/06/07 naming, sanitization, collisions; EXP-16/17 timestamps
  rustcut-ffmpeg          PURE: argv builders A–F, -progress parser, ffprobe/packet parsers,
                          EXP-03 container rules, EXP-11 estimates, stderr classification, HW args
  rustcut-runner          IO: sidecar location, spawning (Job Object / CREATE_NO_WINDOW /
                          PDEATHSIG), progress streaming, stderr tail, cancellation
  rustcut-project         ProjectStore + undo/redo, atomic save, backups, lock, migration,
                          location/library, recent projects, auto-saver
  rustcut-media           probe service, media index (MP4 sample tables / packet scan), waveform,
                          playback tiers (remux/proxy LRU cache), HW encoder detection
  rustcut-export          pre-flight, scheduler, .part safe writes, journal, verification
  rustcut-testkit         ffmpeg/ffprobe lookup + synthetic golden-set fixtures (§11)
src-tauri/                Tauri v2 app crate `rustcut` (lib `rustcut_lib`): commands, state, events
src/                      Svelte 5 + TS + Tailwind v4 UI
scripts/                  setup-sidecars.mjs, make-sample.mjs, bindings-index.mjs
tests/e2e/                end-to-end tests against the real app (CDP into WebView2)
```

Dependency graph (arrows = depends on):

```
model ← framing ← ffmpeg ← runner ← media
model ← naming                      ↖
model ← project                       export ← (naming, ffmpeg, framing, runner)
everything ← src-tauri
testkit: dev-dependency only (no deps on other crates)
```

## 2. Conventions (all Rust crates)

* Edition 2024, `rust-version = 1.89` (std file locking `File::try_lock` is used for PRJ-05).
* Library code never panics on bad input (no `unwrap()` outside tests); errors are
  `thiserror` enums. Commands convert them to `rustcut_model::CommandError` with a
  specific, actionable `message` (AC-12).
* FFmpeg/FFprobe are spawned **only** through `rustcut-runner`, always with an
  argument vector (never a shell string), `-nostdin`, and only the located sidecar
  binaries (§7.9). Paths/names pass through unmodified (Unicode, spaces).
* Times are `f64` seconds relative to the source's first PTS, rounded to ms when
  stored (`rustcut_model::time::round_ms`). FFmpeg time args use `{:.3}`.
* Logging: `tracing` macros; never log clip names or paths at info level or above
  when telemetry is on (§8.4) — debug level is fine (local log files only).
* Windows is the primary dev platform but code must compile for macOS/Linux
  (`#[cfg]` platform code; CI not available here).
* Tests that need FFmpeg use `rustcut-testkit` and skip with a clear message
  (not fail) if FFmpeg cannot be found, except in `rustcut-testkit` itself.

## 3. Environment notes for agents (this machine)

* Rust: `$env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path` before cargo
  (PowerShell) — the session's PATH predates the install.
* FFmpeg 9.0.2 full (GPL) build: `$env:LOCALAPPDATA\Microsoft\WinGet\Packages\Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe\ffmpeg-9.0.2-full_build\bin`
  (`RUSTCUT_FFMPEG_DIR` may point there). AMD AMF (`h264_amf`) works on this GPU.
* Parallel agents: use your own `CARGO_TARGET_DIR=D:\RustCut\target-<agent>` to
  avoid lock contention, and build/test only your crate(s): `cargo test -p <crate>`.
* Shared fixtures are generated once into `D:\RustCut\.fixtures` (testkit).
* Node 24 / npm. Frontend checks: `npm run check`, `npm test`, `npm run build`.

## 4. IPC contract (Rust ⇄ WebView)

All commands: `#[tauri::command(rename_all = "snake_case")]`, return
`Result<T, CommandError>`, async where they touch disk/processes. TS wrappers:
`src/lib/ipc/tauri.ts` (already written — the argument names below are binding).

| Command | Args | Returns |
| :--- | :--- | :--- |
| `subscribe_events` | `on_event: Channel<AppEvent>` | `()` — replaces any previous channel |
| `get_app_info` | – | `AppInfo` |
| `get_settings` / `set_settings` | `settings: AppSettings` | `AppSettings` (normalized) |
| `list_recent_projects` | – | `Vec<RecentProject>` |
| `pick_media_file` | – | `Option<String>` (native dialog: video files + `.rustcut`) |
| `pick_output_dir` | `current: Option<String>` | `Option<String>` |
| `open_path` | `path: String` | `OpenResult` (video or `.rustcut`; PRJ-02/03/04/05/06) |
| `relink_source` | `path, accept_mismatch: bool` | `RelinkResult` |
| `close_project` | – | `()` (flush save, release lock, cancel media jobs) |
| `get_snapshot` | – | `Option<ProjectSnapshot>` |
| `add_clip` | `req: AddClipRequest` | `AddClipResult` |
| `edit_clips` | `req: EditClipsRequest` | `StatePatch` |
| `edit_clips_batch` | `batch: Vec<EditClipsRequest>` | `StatePatch` (atomic, one undo step; emits the patch) |
| `get_platform_presets` | – | `Vec<PlatformPreset>` (user file in the data dir or the bundled list; a bad file yields a toast) |
| `delete_clips` | `ids: Vec<Uuid>` | `StatePatch` |
| `undo` / `redo` | – | `Option<StatePatch>` |
| `renumber_clips` | – | `StatePatch` |
| `set_tags` | `tags: Vec<Tag>` | `StatePatch` |
| `set_export_settings` | `settings: ExportSettings` | `StatePatch` |
| `set_view_state` | `view: ViewState` | `()` (saved, not undoable, no patch) |
| `save_now` | – | `SaveState` |
| `crop_window` | `display_width, display_height: u32, offset: f64` | `CropWindow` (FRM-04) |
| `preview_export` | `clip_ids, settings` | `PreflightReport` (names, checks, estimate) |
| `start_export` | `clip_ids, settings` | `BatchInfo` (fails with `Busy` if running, `InvalidInput` if pre-flight has errors) |
| `cancel_export` | `clip_id: Option<Uuid>` | `()` (None = cancel all) |
| `resume_export` | – | `()` (after a pause) |
| `retry_failed` | – | `Option<BatchInfo>` |
| `request_playback` | `tier: PlaybackTier` | `PlaybackInfo` (starts remux/proxy if needed; progress via events) |
| `start_waveform` | `track: u32` (1-based) | `()` (chunks via events) |
| `get_media_index` | – | `Option<MediaIndexInfo>` |
| `start_media_index` | – | `()`: packet-scan index after the first frame (`MediaIndex` event) |
| `reveal_path` | `path` | `()` — only paths this project exported (or its output dir) |
| `open_output_folder` | – | `()` |
| `clear_cache` | – | `u64` bytes freed |
| `detect_hw_encoders` | `force: bool` | `Vec<HwEncoderInfo>` |
| `export_timestamps` | `format: TimestampFormat` | `TimestampExport` |
| `save_text_file` | `default_name, text` | `Option<String>` (native save dialog) |
| `parse_timestamps` | `text` | `ImportPreview` |
| `import_timestamps` | `items: Vec<ImportedClip>` | `StatePatch` (one undo step) |

**Patch rule.** Every state change yields exactly one `StatePatch` whose
`revision` is the previous revision + 1. The app sends it as
`AppEvent::Patch` on the channel **and** returns it from the command that caused
it. The UI applies a patch iff `revision == current + 1`, ignores older ones and
refetches a snapshot on a gap. Background changes (export statuses, relink) only
arrive as events. Commands that change nothing return a patch with the current
revision and no upserts.

**Mark capture** never crosses IPC (§5 rule 4): marks are UI state; only
`add_clip` sends the final range.

## 5. Crate specs (what each must provide)

The authoritative behaviour is the PRD; this lists the API shape the other
crates consume. Each crate documents its public API in rustdoc.

### rustcut-naming
* `sanitize_component(s, spaces_to_underscores) -> String` — EXP-06 steps 1–6.
* `render_name(template, &NameContext, spaces_to_underscores) -> Result<String, TemplateError>`
  — EXP-05 variables, each value sanitized, then the whole name; empty → `Clip_{index}`;
  `{name}` empty → `Clip_{index}`; `{index}` padded to `Project::index_width()`.
* `validate_template(t) -> Result<(), TemplateError>` (unknown variable, braces, extension).
* `resolve_outputs(requests, ext, dir, policy, previous_outputs, exists) -> Vec<ResolvedName>`
  — EXP-07: in-batch duplicates `_2`, `_3`… in clip-number order (case-insensitive,
  NFC), existing files per policy, a clip's own previous output → `ReplacePrevious`;
  suffixes keep the 120-byte limit; Windows 260-char path check (edge-case table).
* `timestamps::{export, parse_import}` — EXP-16 (YouTube chapter rules), CSV,
  CMX3600 EDL, FCPXML; EXP-17 parsing (`12:10-13:05 Jump scare`, CSV) with
  per-line errors; missing end = next start (or source end).
* Tests: every PRD vector verbatim; property tests (no forbidden chars, ≤120 bytes,
  never a reserved name, idempotent).

### rustcut-ffmpeg (pure, no IO)
* `args`: `ExportCommand { input, output_part, start_sec, end_sec, muxer, video: VideoPlan, audio: AudioPlan, … }`
  `.to_args() -> Vec<OsString>` and `build_export_command(&JobSpec) -> ExportCommand`
  producing **exactly** the PRD §7.6 B/C/D/E argv (golden tests), plus HEVC
  `-tag:v hvc1`, chain F + BT.709 tags, fps cap, Native, SAR normalisation,
  EXP-04 mapping rules, HW encoder args (EXP-13), `-threads` for parallel SW jobs.
  `probe_args`, `packet_index_args`, `verify_args`, `hw_test_args`.
* `progress::ProgressParser` — `-progress pipe:1` blocks (`out_time_us`, `speed`,
  `progress=continue|end`, `N/A` values).
* `probe::parse_probe_json(json, path) -> Result<Probe, ProbeError>` → `SourceInfo`
  (display dims via `rustcut_framing::display_dims`, clockwise rotation from the
  display-matrix side data or `rotate` tag, HDR from PQ/HLG transfer, VFR heuristic,
  audio tracks, `format_name`, `bit_rate`), `mime_type()` (RFC 6381 codec string),
  `probe_failure_message(stderr)` with recovery hints (truncated MP4 etc.).
* `index::{MediaIndex, parse_packet_csv}` — keyframes and frame PTS relative to the
  source start, `keyframe_at_or_before`, `lead_in`, `next_frame`/`prev_frame`, VFR.
* `compat::lossless_container(&SourceInfo, &ExportSettings) -> ContainerDecision` — EXP-03.
* `estimate` — EXP-11 formulas (bits→bytes fixed; 5 s lead-in when unknown).
* `failure::classify(stderr_tail) -> FailureKind` (disk full, output missing,
  encoder sessions exhausted, HW encoder failure, invalid data, other) +
  `first_error_line`.

### rustcut-runner (IO)
* `Sidecars::locate()` — `RUSTCUT_FFMPEG_DIR`, next to the executable (bundled
  `ffmpeg[.exe]` and dev `ffmpeg-<triple>[.exe]`), `PATH`, WinGet Gyan dir.
* `spawn` with stdin null, piped stdout/stderr, `CREATE_NO_WINDOW`, every child in a
  process-wide Job Object with `KILL_ON_JOB_CLOSE` (Windows); `PR_SET_PDEATHSIG`
  (Linux); macOS: PIDs reported to the caller for journaling.
* `run_capture(program, args, timeout) -> Output` (ffprobe), and
  `run_ffmpeg(args, opts) -> RunOutcome` streaming `ProgressUpdate`s to a callback,
  keeping the last 20 stderr lines and appending all of them to a log file,
  cancellable (kills the child), reporting exit code + whether `progress=end` arrived.
* `capabilities()` — ffmpeg version, `has_filter("zscale")`, `has_encoder(..)` (cached).
* Tests: real ffmpeg run with progress; cancel kills the process; orphan test (a
  parent process spawned via an example binary is killed → its ffmpeg dies).

### rustcut-project
* `ProjectStore` (sync; the app wraps it in a Mutex): `add_clip`, `edit_clips`,
  `delete_clips`, `undo`, `redo` (≥200 steps, `UNDO_LIMIT`), `renumber_chronologically`,
  `set_tags`, `set_export_settings`, `set_view_state`, system updates
  `mark_exporting`, `apply_export_record(ExportRecord)`, `normalize_on_load`
  (exporting→pending, outdated refresh), `relink(SourceInfo)`, `snapshot(..)`.
  Validation per MRK-01 (clamp, min duration, Out > In), stable indices (QUE-02,
  `next_index` monotonic, also across undo), duplicate hint (MRK-03), read-only mode.
  Undo/redo restore user fields (`ClipFields`) and whole clips for add/delete;
  export status is recomputed (`Clip::after_user_edit`), never rolled back.
* `persist`: atomic save (temp in same dir → `sync_all` → rename), load with
  migration v1→v2 (`.v1.bak` kept), newer schema → read-only, corrupt → newest
  valid backup + notice; `BackupStore` (10 rolling versions in app data);
  `ProjectLock` (std `File::try_lock`, holder info, second opener read-only);
  `location` (next to video if writable else app library by fingerprint; finding
  an existing project for a source); `recent` (PRJ-07).
* `autosave::AutoSaver` (tokio): debounced ≤1 s, immediate for add/delete, flush.
* Tests: AC-15 property test (10 000 random op sequences), AC-04 kill test harness
  (example binary + test; iterations via `RUSTCUT_KILL_ITERATIONS`, default 200,
  1000 in the ignored full run), lock, migration, corruption, read-only fallback.

### rustcut-testkit
* `ffmpeg()`, `ffprobe()` paths; `fixture(Fixture) -> PathBuf` generating on demand
  into `.fixtures/` (atomic, safe under parallel tests); helpers `probe_json`,
  `duration`, `frame_count`, `packet_hashes` (`-c copy -f framemd5`),
  `frame_luma_at`, `video_size`.
* Fixtures (short, synthetic): M01-short 1080p60 2 s GOP H.264/AAC MP4; M02 MKV
  H.264 + 4 audio; M03 10 s GOP; M04 VFR; M05 HEVC 10-bit HLG rotated 90°;
  M06–M10 framing ramps (1440×1080, 2560×1080, 1080×1920, 1080×2340,
  720×576 SAR 16:15) + 1920×1080 ramp; M11 FLV; M12 WebM VP9/Opus; M13 start_time≠0;
  M14 no audio; M15 truncated MP4; M16 AV1 144 fps (small); M17 MKV/MOV with PCM;
  M18 Unicode path with spaces; plus `FrameNumbered` (luma = frame number) for AC-08.

### rustcut-media, rustcut-export
Specified in the wave-2 briefs (they consume the wave-1 crates as built).

## 6. Frontend

* Stores (`src/lib/state/*.svelte.ts`): `project` (snapshot mirror, selection,
  commands), `player` (transport, synchronous `now()`, pending marks, loop),
  `ui` (phase, dialogs, toasts, rename requests), `media` (keyframes, waveform),
  `exporter` (batch, progress, pre-flight). `events.ts` routes `AppEvent`s.
* Ownership for parallel work: **fe-shell** — App.svelte, shell/*, player/Player &
  Transport, state/{project,player,ui,events}, keyboard, marks, mock backend,
  i18n/en/common.ts. **fe-timeline** — timeline/*, player/CropOverlay &
  CropControls, state/media, util/crop, i18n/en/timeline.ts. **fe-queue** —
  queue/*, export/*, tags/*, timestamps/*, state/exporter, i18n/en/{queue,export}.ts.
  Change another owner's file only to fix a compile error, minimally.
* Canvas for timelines and waveform (60 fps with 500 clips).
* Keyboard (§5): letters by `event.key`, punctuation by `event.code`; suspended in
  text fields; WebView accelerators (F5, Ctrl+R, Ctrl+P, Alt+←/→, Ctrl+F, …)
  prevented; mark keys call `player.now()` synchronously.
* Accessibility (§8.5): keyboard reachable, visible focus, accessible names,
  contrast ≥ 4.5:1, tags always labelled, reduced motion honoured. Every string via `t()`.

## 7. Out of scope for this build (needs external infrastructure)

Code signing / notarisation (R4), the custom minimal LGPL/GPL FFmpeg build and
installer-size target (§8.3, R3, R5), auto-updater endpoints, opt-in telemetry
backend (Q6), Linux/macOS packaging verification (Q7), user research items.
Phase 3 (v2.0) smart features are not built.
