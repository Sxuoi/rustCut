//! The batch export engine (EXP-01, 08, 09, 10, 12, 13, 14; PRD 7.8).
//!
//! One batch runs at a time. A driver task pops clips in clip-number order,
//! takes a permit from a `Semaphore` (`parallel_jobs` permits, 1..=4) and runs
//! each clip as its own task. See [`ExportEngine`] for the public surface.

use crate::error::ExportError;
use crate::journal::Journal;
use crate::preflight::lossless_lead_in;
use crate::sleep::SleepGuard;
use rustcut_ffmpeg::probe::parse_verify_json;
use rustcut_ffmpeg::{
    ContainerDecision, ExportCommand, FailureKind, JobSpec, MediaIndex, ProgressParser,
    ProgressUpdate, ResolvedEncoder, build_export_command, classify, first_error_line, fraction,
    lossless_container, verify_args,
};
use rustcut_model::{
    BatchInfo, BatchState, BatchSummary, Clip, ClipFinished, ClipOutcome, ClipProgress,
    EncoderChoice, ExportProgress, ExportRecord, ExportSettings, HwEncoderInfo, PauseReason,
    PlannedAction, PlannedOutput, PreflightReport, Preset, Project, SourceInfo, now_utc,
    settings_hash,
};
use rustcut_naming::sanitize_component;
use rustcut_runner::{CancelToken, Sidecars, StreamOptions, run_capture, run_streaming};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

/// Verification tolerance of the output duration (PRD 7.8).
pub const VERIFY_TOLERANCE_SEC: f64 = 1.0;
/// Upper bound on the lossless lead-in when the media index is unknown.
const UNKNOWN_LEAD_IN_MAX_SEC: f64 = 60.0;
/// "Out of encoder sessions" re-queues per clip before falling back to libx264.
const MAX_SESSION_RETRIES: u32 = 2;
/// Minimum interval between progress events.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
/// Timeout of the output probe.
const VERIFY_TIMEOUT: Duration = Duration::from_secs(60);

/// Static configuration of the engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Located `ffmpeg`/`ffprobe`.
    pub sidecars: Sidecars,
    /// Log root; per-clip logs go to `<log_dir>/export/<batch_id>/<index>_<name>.log`.
    pub log_dir: PathBuf,
    /// App data folder holding `export-journal.json`.
    pub data_dir: PathBuf,
    /// Logical CPU count (software jobs get `cpu_count / jobs` threads, EXP-12).
    pub cpu_count: u32,
    /// Detected hardware encoders (EXP-13).
    pub hw_encoders: Vec<HwEncoderInfo>,
    /// The FFmpeg build has `zscale` (HDR tone-mapping).
    pub zscale_available: bool,
}

/// Everything [`ExportEngine::start`] needs; the project is a snapshot, so edits
/// made during the export never change what is written (EXP-01).
#[derive(Debug, Clone)]
pub struct BatchRequest {
    /// Snapshot of the project.
    pub project: Project,
    /// Settings of this export.
    pub settings: ExportSettings,
    /// The pre-flight report the user confirmed (carries the planned outputs).
    pub report: PreflightReport,
    /// Keyframe index (lead-in for progress/verification, *Snap In*).
    pub media_index: Option<MediaIndex>,
}

/// Receives engine events. Callbacks run on engine tasks: keep them quick.
pub trait ExportSink: Send + Sync {
    /// A clip is about to be written to `output_path` (its final path).
    fn on_clip_started(&self, batch_id: &str, clip_id: Uuid, output_path: &Path);
    /// Throttled progress (at most about 4 per second) and after every clip.
    fn on_progress(&self, progress: ExportProgress);
    /// A clip reached a final outcome. The app applies the record to its store.
    fn on_clip_finished(&self, record: ExportRecord, finished: ClipFinished);
    /// Running, Paused or Finished (exactly one `Finished` per batch).
    fn on_batch_state(&self, state: BatchState);
    /// FFmpeg was spawned for `clip_id` (default: ignored).
    fn on_process_spawned(&self, _clip_id: Uuid, _pid: u32) {}
    /// The FFmpeg process of `clip_id` has exited and was reaped (default: ignored).
    fn on_process_exited(&self, _clip_id: Uuid, _pid: u32) {}
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

struct Shared {
    config: EngineConfig,
    journal: Journal,
    active: Mutex<Option<Arc<Batch>>>,
}

/// Batch export engine. Cheap to clone (shared state).
#[derive(Clone)]
pub struct ExportEngine {
    shared: Arc<Shared>,
}

impl ExportEngine {
    /// Create an engine. Call [`crate::cleanup_stale`] before, at app start.
    pub fn new(config: EngineConfig) -> Self {
        let journal = Journal::new(&config.data_dir);
        ExportEngine {
            shared: Arc::new(Shared {
                config,
                journal,
                active: Mutex::new(None),
            }),
        }
    }

    /// Start a batch and return immediately; events go to `sink`.
    ///
    /// Must be called from inside a Tokio runtime. The output folder is created
    /// here. Skip-planned clips finish as `Skipped` without running FFmpeg.
    ///
    /// # Errors
    /// * [`ExportError::Busy`] while another batch runs.
    /// * [`ExportError::PreflightFailed`] when the report has a blocking item or no outputs.
    /// * [`ExportError::UnknownClip`] when the report plans a clip the project lacks.
    /// * [`ExportError::OutputDir`] when the output folder cannot be created.
    /// * [`ExportError::NoRuntime`] outside a Tokio runtime.
    pub fn start(
        &self,
        request: BatchRequest,
        sink: Arc<dyn ExportSink>,
    ) -> Result<BatchInfo, ExportError> {
        let handle = tokio::runtime::Handle::try_current().map_err(|_| ExportError::NoRuntime)?;
        let BatchRequest {
            project,
            settings,
            report,
            media_index,
        } = request;
        if !report.can_start {
            let reason = report
                .items
                .iter()
                .find(|i| i.level == rustcut_model::CheckLevel::Error)
                .map_or_else(
                    || "the pre-flight report does not allow starting".to_string(),
                    |i| i.message.clone(),
                );
            return Err(ExportError::PreflightFailed(reason));
        }
        if report.outputs.is_empty() {
            return Err(ExportError::PreflightFailed(
                "there is nothing to export".into(),
            ));
        }
        let mut active = lock(&self.shared.active);
        if let Some(b) = active.as_ref() {
            return Err(ExportError::Busy {
                batch_id: b.id.clone(),
            });
        }
        let settings = settings.normalized();
        let batch = Batch::prepare(
            self.shared.clone(),
            project,
            settings,
            &report,
            media_index,
            sink,
        )?;
        std::fs::create_dir_all(&batch.info.output_dir).map_err(|source| {
            ExportError::OutputDir {
                path: batch.info.output_dir.clone(),
                source,
            }
        })?;
        let info = batch.info.clone();
        *active = Some(batch.clone());
        drop(active);
        handle.spawn(batch.drive());
        Ok(info)
    }

    /// Cancel one clip: kill FFmpeg, delete its `.part` (EXP-09). A queued clip
    /// becomes Cancelled without running. Unknown or finished clips are ignored.
    pub fn cancel_clip(&self, clip_id: Uuid) {
        if let Some(b) = self.current() {
            b.cancel_clip(clip_id);
        }
    }

    /// Cancel the whole batch: kill every running FFmpeg, mark queued clips
    /// Cancelled (EXP-09). A paused batch finishes immediately.
    pub fn cancel_all(&self) {
        if let Some(b) = self.current() {
            b.cancel_everything();
        }
    }

    /// Resume a paused batch (EXP-10). Re-creates the output folder first.
    ///
    /// # Errors
    /// [`ExportError::NotPaused`] when no batch is paused;
    /// [`ExportError::StillUnavailable`] when the output folder still cannot be created.
    pub fn resume(&self) -> Result<(), ExportError> {
        let b = self.current().ok_or(ExportError::NotPaused)?;
        b.resume()
    }

    /// Whether a batch is running (or paused).
    pub fn is_running(&self) -> bool {
        lock(&self.shared.active).is_some()
    }

    fn current(&self) -> Option<Arc<Batch>> {
        lock(&self.shared.active).clone()
    }
}

// ───────────────────────────── pure helpers ─────────────────────────────

/// Pick the video encoder (EXP-13): `Auto` -> best available hardware encoder
/// (nvenc > amf > qsv > videotoolbox > vaapi) else libx264; an explicit hardware
/// choice if available, else libx264 plus a note for the log.
pub fn resolve_encoder(
    choice: EncoderChoice,
    hw: &[HwEncoderInfo],
) -> (ResolvedEncoder, Option<String>) {
    let available = |c: EncoderChoice| {
        hw.iter()
            .any(|h| h.available && Some(h.name.as_str()) == c.ffmpeg_name())
    };
    match choice {
        EncoderChoice::Libx264 => (ResolvedEncoder::Libx264, None),
        EncoderChoice::Auto => EncoderChoice::HARDWARE
            .iter()
            .copied()
            .find(|c| available(*c))
            .map_or((ResolvedEncoder::Libx264, None), |c| {
                (ResolvedEncoder::Hardware(c), None)
            }),
        explicit => {
            if available(explicit) {
                (ResolvedEncoder::Hardware(explicit), None)
            } else {
                let name = explicit.ffmpeg_name().unwrap_or("hardware encoder");
                (
                    ResolvedEncoder::Libx264,
                    Some(format!(
                        "{name} is not available on this machine; using libx264"
                    )),
                )
            }
        }
    }
}

/// `<final name>.part` (EXP-08): the suffix is appended to the whole file name.
pub fn part_path(final_path: &Path) -> PathBuf {
    let mut s: OsString = final_path.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

/// First `<stem>_N.<ext>` (N >= 2) that neither exists nor is planned for another clip.
pub fn fresh_path(planned: &Path, taken: &HashSet<PathBuf>) -> PathBuf {
    let stem = planned
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = planned
        .extension()
        .map(|s| s.to_string_lossy().into_owned());
    let dir = planned.parent().map(Path::to_path_buf).unwrap_or_default();
    (2u32..)
        .map(|n| {
            let name = match &ext {
                Some(e) => format!("{stem}_{n}.{e}"),
                None => format!("{stem}_{n}"),
            };
            dir.join(name)
        })
        .find(|p| !p.exists() && !taken.contains(p))
        .unwrap_or_else(|| planned.to_path_buf())
}

fn append_log(path: &Path, text: &str) {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "[rustcut] {text}");
    }
}

// ───────────────────────────── batch state ─────────────────────────────

#[derive(Debug, Clone)]
struct WorkItem {
    clip: Clip,
    planned: PlannedOutput,
    /// Lossless lead-in (known from the index), 0 for re-encodes.
    lead_in: Option<f64>,
    /// `end - start` after clamping (no lead-in).
    nominal_sec: f64,
    session_retries: u32,
}

impl WorkItem {
    /// Expected output duration, lead-in included (progress weight/denominator).
    fn expected_sec(&self) -> f64 {
        self.nominal_sec + self.lead_in.unwrap_or(0.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum St {
    Queued,
    Running,
    Done,
    /// Failed, cancelled or skipped: removed from the progress denominator.
    Excluded,
}

#[derive(Debug, Clone)]
struct ClipState {
    weight: f64,
    fraction: f64,
    out_time: f64,
    speed: Option<f64>,
    state: St,
}

#[derive(Default)]
struct Book {
    clips: HashMap<Uuid, ClipState>,
    order: Vec<Uuid>,
    last_emit: Option<Instant>,
    done: u32,
    failed: u32,
}

#[derive(Default)]
struct Control {
    tokens: HashMap<Uuid, CancelToken>,
    cancelled: HashSet<Uuid>,
}

#[derive(Default)]
struct Results {
    done: Vec<Uuid>,
    failed: Vec<Uuid>,
    skipped: Vec<Uuid>,
    cancelled: Vec<Uuid>,
}

struct Batch {
    id: String,
    info: BatchInfo,
    shared: Arc<Shared>,
    sink: Arc<dyn ExportSink>,
    settings: ExportSettings,
    source: SourceInfo,
    source_path: PathBuf,
    decision: ContainerDecision,
    index: Option<MediaIndex>,
    jobs: usize,
    index_width: usize,
    taken: HashSet<PathBuf>,
    queue: Mutex<VecDeque<WorkItem>>,
    sem: Arc<Semaphore>,
    limit: AtomicUsize,
    active: AtomicUsize,
    wake: Notify,
    cancel_all: CancelToken,
    control: Mutex<Control>,
    paused: Mutex<Option<PauseReason>>,
    resume: Notify,
    book: Mutex<Book>,
    results: Mutex<Results>,
    started: Instant,
}

enum Attempt {
    Done { path: PathBuf, size: u64 },
    Cancelled,
    Skipped(String),
    Failed(Failure),
}

struct Failure {
    kind: FailureKind,
    exit_code: Option<i32>,
    message: String,
    tail: Vec<String>,
}

enum Step {
    Finish(ClipOutcome),
    Requeue,
    Pause(PauseReason, String),
}

impl Batch {
    fn prepare(
        shared: Arc<Shared>,
        project: Project,
        settings: ExportSettings,
        report: &PreflightReport,
        index: Option<MediaIndex>,
        sink: Arc<dyn ExportSink>,
    ) -> Result<Arc<Batch>, ExportError> {
        let jobs = settings.parallel_jobs.clamp(1, 4) as usize;
        let decision = lossless_container(&project.source, &settings);
        let mut batch = Batch {
            id: Uuid::now_v7().to_string(),
            info: BatchInfo {
                batch_id: String::new(),
                clip_ids: Vec::new(),
                total_duration_sec: 0.0,
                output_dir: report.output_dir.clone(),
            },
            source_path: PathBuf::from(&project.source.path),
            source: project.source.clone(),
            decision,
            index,
            jobs,
            index_width: project.index_width(),
            taken: report
                .outputs
                .iter()
                .map(|o| PathBuf::from(&o.output_path))
                .collect(),
            shared,
            sink,
            settings,
            queue: Mutex::new(VecDeque::new()),
            sem: Arc::new(Semaphore::new(jobs)),
            limit: AtomicUsize::new(jobs),
            active: AtomicUsize::new(0),
            wake: Notify::new(),
            cancel_all: CancelToken::new(),
            control: Mutex::new(Control::default()),
            paused: Mutex::new(None),
            resume: Notify::new(),
            book: Mutex::new(Book::default()),
            results: Mutex::new(Results::default()),
            started: Instant::now(),
        };
        batch.info.batch_id = batch.id.clone();
        let mut items = VecDeque::new();
        let mut book = Book::default();
        let scratch = part_path(Path::new("scratch.mp4"));
        for out in &report.outputs {
            let clip = project
                .clip(out.clip_id)
                .cloned()
                .ok_or(ExportError::UnknownClip(out.clip_id))?;
            let mut item = WorkItem {
                clip,
                planned: out.clone(),
                lead_in: None,
                nominal_sec: 0.0,
                session_retries: 0,
            };
            let preset = batch.settings.preset();
            item.lead_in = if preset == Preset::Lossless {
                lossless_lead_in(&item.clip, &batch.settings, batch.index.as_ref())
            } else {
                Some(0.0)
            };
            item.nominal_sec = batch.command_for(&item, &scratch, false).0.duration_sec();
            let skip = out.action == PlannedAction::Skip;
            book.order.push(out.clip_id);
            book.clips.insert(
                out.clip_id,
                ClipState {
                    weight: item.expected_sec().max(0.001),
                    fraction: 0.0,
                    out_time: 0.0,
                    speed: None,
                    state: if skip { St::Excluded } else { St::Queued },
                },
            );
            batch.info.clip_ids.push(out.clip_id);
            batch.info.total_duration_sec += item.nominal_sec;
            items.push_back(item);
        }
        *lock(&batch.queue) = items;
        *lock(&batch.book) = book;
        Ok(Arc::new(batch))
    }

    /// Build the FFmpeg command of a clip (and the resolved encoder + log note).
    fn command_for(
        &self,
        item: &WorkItem,
        part: &Path,
        force_software: bool,
    ) -> (ExportCommand, ResolvedEncoder, Option<String>) {
        let preset = self.settings.preset();
        let (encoder, note) = if preset.is_reencode() && !force_software {
            resolve_encoder(self.settings.encoder, &self.shared.config.hw_encoders)
        } else {
            (ResolvedEncoder::Libx264, None)
        };
        let threads = (preset.is_reencode() && encoder == ResolvedEncoder::Libx264)
            .then(|| (self.shared.config.cpu_count / self.jobs as u32).max(1));
        let snap = (preset == Preset::Lossless && self.settings.snap_in_to_keyframe)
            .then(|| {
                self.index
                    .as_ref()
                    .and_then(|i| i.keyframe_at_or_before(item.clip.start_sec))
            })
            .flatten();
        let spec = JobSpec {
            input: &self.source_path,
            output_part: part,
            source: &self.source,
            start_sec: item.clip.start_sec,
            end_sec: item.clip.end_sec,
            crop_x_offset: item.clip.crop_x_offset,
            settings: &self.settings,
            container: &self.decision,
            encoder,
            threads,
            zscale_available: self.shared.config.zscale_available,
            snap_in_to: snap,
        };
        (build_export_command(&spec), encoder, note)
    }

    fn uses_hardware(&self) -> bool {
        self.settings.preset().is_reencode()
            && matches!(
                resolve_encoder(self.settings.encoder, &self.shared.config.hw_encoders).0,
                ResolvedEncoder::Hardware(_)
            )
    }

    fn log_path(&self, item: &WorkItem) -> PathBuf {
        let name = sanitize_component(&item.clip.name, true);
        let name = if name.is_empty() {
            "clip".to_string()
        } else {
            name
        };
        self.shared
            .config
            .log_dir
            .join("export")
            .join(&self.id)
            .join(format!(
                "{:0w$}_{name}.log",
                item.clip.index,
                w = self.index_width
            ))
    }

    // ───────────── control ─────────────

    fn register(&self, id: Uuid) -> CancelToken {
        let token = CancelToken::new();
        let mut c = lock(&self.control);
        c.tokens.insert(id, token.clone());
        if c.cancelled.contains(&id) || self.cancel_all.is_cancelled() {
            token.cancel();
        }
        token
    }

    fn unregister(&self, id: Uuid) {
        lock(&self.control).tokens.remove(&id);
    }

    fn is_marked_cancelled(&self, id: Uuid) -> bool {
        self.cancel_all.is_cancelled() || lock(&self.control).cancelled.contains(&id)
    }

    fn cancel_clip(&self, id: Uuid) {
        let finished = lock(&self.book)
            .clips
            .get(&id)
            .is_none_or(|s| matches!(s.state, St::Done))
            || self.is_finished(id);
        if finished {
            return;
        }
        let mut c = lock(&self.control);
        c.cancelled.insert(id);
        if let Some(t) = c.tokens.get(&id) {
            t.cancel();
        }
        drop(c);
        self.wake.notify_one();
    }

    fn is_finished(&self, id: Uuid) -> bool {
        let r = lock(&self.results);
        r.done.contains(&id)
            || r.failed.contains(&id)
            || r.skipped.contains(&id)
            || r.cancelled.contains(&id)
    }

    fn cancel_everything(&self) {
        self.cancel_all.cancel();
        for t in lock(&self.control).tokens.values() {
            t.cancel();
        }
        self.wake.notify_one();
        self.resume.notify_waiters();
    }

    fn resume(&self) -> Result<(), ExportError> {
        if lock(&self.paused).is_none() {
            return Err(ExportError::NotPaused);
        }
        std::fs::create_dir_all(&self.info.output_dir).map_err(|source| {
            ExportError::StillUnavailable {
                path: self.info.output_dir.clone(),
                source,
            }
        })?;
        *lock(&self.paused) = None;
        self.sink.on_batch_state(BatchState::Running {
            batch: self.info.clone(),
        });
        self.resume.notify_waiters();
        self.wake.notify_one();
        Ok(())
    }

    fn is_paused(&self) -> bool {
        lock(&self.paused).is_some()
    }

    fn pause(&self, reason: PauseReason, message: String) {
        {
            let mut p = lock(&self.paused);
            if p.is_some() {
                return;
            }
            *p = Some(reason);
        }
        self.sink.on_batch_state(BatchState::Paused {
            batch_id: self.id.clone(),
            reason,
            message,
        });
    }

    async fn wait_while_paused(&self) {
        loop {
            let notified = self.resume.notified();
            let mut notified = std::pin::pin!(notified);
            notified.as_mut().enable();
            if !self.is_paused() || self.cancel_all.is_cancelled() {
                return;
            }
            tokio::select! {
                () = notified => {}
                () = self.cancel_all.cancelled() => {}
            }
        }
    }

    // ───────────── driver ─────────────

    async fn drive(self: Arc<Self>) {
        let sleep = SleepGuard::acquire();
        self.sink.on_batch_state(BatchState::Running {
            batch: self.info.clone(),
        });
        self.emit_progress();
        loop {
            self.wait_while_paused().await;
            if self.cancel_all.is_cancelled() {
                self.drain_cancelled();
                if self.active.load(Ordering::SeqCst) == 0 {
                    break;
                }
                self.wake.notified().await;
                continue;
            }
            let next = lock(&self.queue).pop_front();
            let Some(item) = next else {
                if self.active.load(Ordering::SeqCst) == 0 {
                    if lock(&self.queue).is_empty() {
                        break;
                    }
                    continue;
                }
                self.wake.notified().await;
                continue;
            };
            if self.is_marked_cancelled(item.clip.id) {
                self.finish(&item, ClipOutcome::Cancelled);
                continue;
            }
            if item.planned.action == PlannedAction::Skip {
                self.finish(
                    &item,
                    ClipOutcome::Skipped {
                        reason: "A file with this name already exists and the policy is Skip"
                            .into(),
                    },
                );
                continue;
            }
            let permit = tokio::select! {
                p = self.sem.clone().acquire_owned() => p,
                () = self.wake.notified() => { self.push_front(item); continue; }
                () = self.cancel_all.cancelled() => { self.push_front(item); continue; }
            };
            let Ok(permit) = permit else {
                self.push_front(item);
                break;
            };
            if self.is_paused() || self.is_marked_cancelled(item.clip.id) {
                drop(permit);
                self.push_front(item);
                continue;
            }
            self.active.fetch_add(1, Ordering::SeqCst);
            let this = self.clone();
            tokio::spawn(async move {
                let it = item.clone();
                let inner = tokio::spawn(this.clone().run_item(item, permit));
                if let Err(e) = inner.await {
                    this.finish(
                        &it,
                        ClipOutcome::Failed {
                            exit_code: None,
                            message: format!("Internal error while exporting this clip: {e}"),
                            log_tail: Vec::new(),
                            log_path: Some(this.log_path(&it).to_string_lossy().into_owned()),
                        },
                    );
                }
                this.active.fetch_sub(1, Ordering::SeqCst);
                this.wake.notify_one();
            });
        }
        // Finish: release sleep blocking, free the engine, announce once.
        drop(sleep);
        let summary = self.summary();
        {
            let mut a = lock(&self.shared.active);
            if a.as_ref().is_some_and(|b| b.id == self.id) {
                *a = None;
            }
        }
        self.sink.on_batch_state(BatchState::Finished { summary });
    }

    fn push_front(&self, item: WorkItem) {
        lock(&self.queue).push_front(item);
    }

    fn drain_cancelled(&self) {
        loop {
            let item = lock(&self.queue).pop_front();
            let Some(item) = item else { break };
            self.finish(&item, ClipOutcome::Cancelled);
        }
    }

    fn summary(&self) -> BatchSummary {
        let order: HashMap<Uuid, usize> = lock(&self.book)
            .order
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect();
        let sorted = |v: &Vec<Uuid>| {
            let mut v = v.clone();
            v.sort_by_key(|id| order.get(id).copied().unwrap_or(usize::MAX));
            v
        };
        let r = lock(&self.results);
        BatchSummary {
            batch_id: self.id.clone(),
            done: sorted(&r.done),
            failed: sorted(&r.failed),
            skipped: sorted(&r.skipped),
            cancelled: sorted(&r.cancelled),
            output_dir: self.info.output_dir.clone(),
            elapsed_sec: self.started.elapsed().as_secs_f64(),
        }
    }

    // ───────────── per-clip task ─────────────

    async fn run_item(self: Arc<Self>, mut item: WorkItem, permit: OwnedSemaphorePermit) {
        let id = item.clip.id;
        let token = self.register(id);
        let log = self.log_path(&item);
        let mut force_software = false;
        let step = loop {
            match self.run_attempt(&item, &token, force_software, &log).await {
                Attempt::Done { path, size } => {
                    break Step::Finish(ClipOutcome::Done {
                        output_path: path.to_string_lossy().into_owned(),
                        size_bytes: size,
                    });
                }
                Attempt::Cancelled => break Step::Finish(ClipOutcome::Cancelled),
                Attempt::Skipped(reason) => break Step::Finish(ClipOutcome::Skipped { reason }),
                Attempt::Failed(f) => {
                    if token.is_cancelled() {
                        break Step::Finish(ClipOutcome::Cancelled);
                    }
                    let dir_gone = !Path::new(&self.info.output_dir).is_dir();
                    if f.kind.pauses_batch() || dir_gone {
                        break self.pause_step(&item, &f, dir_gone);
                    }
                    if f.kind.requeue_with_less_concurrency()
                        && item.session_retries < MAX_SESSION_RETRIES
                    {
                        append_log(
                            &log,
                            &format!(
                                "out of encoder sessions ({}); re-queueing with lower concurrency",
                                f.message
                            ),
                        );
                        break Step::Requeue;
                    }
                    let hw_problem =
                        f.kind.retry_with_software() || f.kind.requeue_with_less_concurrency();
                    if hw_problem && !force_software && self.uses_hardware() {
                        let enc = self
                            .settings
                            .encoder
                            .ffmpeg_name()
                            .unwrap_or("the hardware encoder");
                        append_log(
                            &log,
                            &format!(
                                "hardware encoder ({enc}) failed: {}; retrying this clip with libx264 (EXP-13)",
                                f.message
                            ),
                        );
                        force_software = true;
                        continue;
                    }
                    break Step::Finish(ClipOutcome::Failed {
                        exit_code: f.exit_code,
                        message: f.message,
                        log_tail: f.tail,
                        log_path: Some(log.to_string_lossy().into_owned()),
                    });
                }
            }
        };
        self.unregister(id);
        match step {
            Step::Finish(outcome) => {
                drop(permit);
                self.finish(&item, outcome);
            }
            Step::Requeue => {
                item.session_retries += 1;
                self.set_state(id, St::Queued);
                if self.limit.load(Ordering::SeqCst) > 1 {
                    self.limit.fetch_sub(1, Ordering::SeqCst);
                    permit.forget();
                } else {
                    drop(permit);
                }
                self.push_front(item);
            }
            Step::Pause(reason, message) => {
                // Flag first: the driver must not pop this clip again before it sees the pause.
                self.pause(reason, message);
                self.set_state(id, St::Queued);
                self.push_front(item);
                drop(permit);
            }
        }
    }

    fn pause_step(&self, item: &WorkItem, f: &Failure, dir_gone: bool) -> Step {
        let dir = &self.info.output_dir;
        if dir_gone || f.kind == FailureKind::OutputMissing {
            Step::Pause(
                PauseReason::OutputFolderMissing,
                format!(
                    "The output folder {dir} is no longer available while writing {} (drive disconnected or folder removed). \
                     Reconnect it and resume, or cancel the export.",
                    item.planned.file_name
                ),
            )
        } else {
            Step::Pause(
                PauseReason::DiskFull,
                format!(
                    "The disk is full while writing {}. Free up space on the drive of {dir}, then resume, or cancel the export.",
                    item.planned.file_name
                ),
            )
        }
    }

    async fn run_attempt(
        self: &Arc<Self>,
        item: &WorkItem,
        token: &CancelToken,
        force_software: bool,
        log: &Path,
    ) -> Attempt {
        let id = item.clip.id;
        let final_planned = PathBuf::from(&item.planned.output_path);
        if !Path::new(&self.info.output_dir).is_dir() {
            return Attempt::Failed(Failure {
                kind: FailureKind::OutputMissing,
                exit_code: None,
                message: format!("The output folder {} does not exist", self.info.output_dir),
                tail: Vec::new(),
            });
        }
        let part = part_path(&final_planned);
        let (cmd, _encoder, note) = self.command_for(item, &part, force_software);
        if let Some(n) = note {
            append_log(log, &n);
        }
        if let Err(e) = self.shared.journal.record_start(&part) {
            return Attempt::Failed(Failure {
                kind: FailureKind::Other,
                exit_code: None,
                message: e.to_string(),
                tail: Vec::new(),
            });
        }
        self.set_state(id, St::Running);
        self.sink.on_clip_started(&self.id, id, &final_planned);
        if token.is_cancelled() {
            self.discard_part(&part).await;
            return Attempt::Cancelled;
        }

        let pid = Arc::new(AtomicU32::new(0));
        let (pid_cb, part_cb, this, sink_clip) = (pid.clone(), part.clone(), self.clone(), id);
        let on_spawn = move |p: u32| {
            pid_cb.store(p, Ordering::SeqCst);
            if let Err(e) = this.shared.journal.set_pid(&part_cb, p) {
                tracing::warn!(error = %e, "could not journal the FFmpeg PID");
            }
            this.sink.on_process_spawned(sink_clip, p);
        };
        let (this2, expected) = (self.clone(), item.expected_sec());
        let mut parser = ProgressParser::new();
        let on_line = move |line: &str| {
            if let Some(u) = parser.feed(line) {
                this2.on_update(id, expected, &u);
            }
        };
        let options = StreamOptions {
            log_file: Some(log.to_path_buf()),
            ..StreamOptions::default()
        };
        let args = cmd.to_args();
        let run = run_streaming(
            &self.shared.config.sidecars.ffmpeg,
            &args,
            options,
            on_spawn,
            on_line,
            token.clone(),
        )
        .await;
        let spawned = pid.load(Ordering::SeqCst);
        if spawned != 0 {
            self.sink.on_process_exited(id, spawned);
        }
        let outcome = match run {
            Ok(o) => o,
            Err(e) => {
                self.discard_part(&part).await;
                return Attempt::Failed(Failure {
                    kind: FailureKind::Other,
                    exit_code: None,
                    message: e.to_string(),
                    tail: Vec::new(),
                });
            }
        };
        if outcome.cancelled || token.is_cancelled() {
            self.discard_part(&part).await;
            return Attempt::Cancelled;
        }
        if !(outcome.success && outcome.progress_end) {
            self.discard_part(&part).await;
            let text = outcome.stderr_tail.join("\n");
            let (kind, message) = if outcome.success {
                (FailureKind::Other, "FFmpeg exited normally but never reported that the clip was finished (progress=end)".to_string())
            } else {
                let msg = first_error_line(&text).unwrap_or_else(|| match outcome.exit_code {
                    Some(c) => format!("FFmpeg failed with exit code {c}"),
                    None => "FFmpeg was terminated unexpectedly".to_string(),
                });
                (classify(&text), msg)
            };
            return Attempt::Failed(Failure {
                kind,
                exit_code: outcome.exit_code,
                message,
                tail: outcome.stderr_tail,
            });
        }
        if let Err(message) = self.verify(&part, item).await {
            self.discard_part(&part).await;
            return Attempt::Failed(Failure {
                kind: FailureKind::Other,
                exit_code: outcome.exit_code,
                message,
                tail: outcome.stderr_tail,
            });
        }
        match self.finalize(&part, &final_planned, item.planned.action) {
            Ok(Some(path)) => {
                let _ = self.shared.journal.remove(&part);
                let size = std::fs::metadata(&path).map_or(0, |m| m.len());
                Attempt::Done { path, size }
            }
            Ok(None) => {
                self.discard_part(&part).await;
                Attempt::Skipped(
                    "A file with this name appeared during the export and the policy is Skip"
                        .into(),
                )
            }
            Err(e) => {
                self.discard_part(&part).await;
                let kind = if Path::new(&self.info.output_dir).is_dir() {
                    FailureKind::Other
                } else {
                    FailureKind::OutputMissing
                };
                Attempt::Failed(Failure {
                    kind,
                    exit_code: outcome.exit_code,
                    message: format!(
                        "Cannot move the finished clip to {}: {e}. Is the file open in another program?",
                        final_planned.display()
                    ),
                    tail: outcome.stderr_tail,
                })
            }
        }
    }

    /// Success verification (PRD 7.8): a video stream and the expected duration.
    async fn verify(&self, part: &Path, item: &WorkItem) -> Result<(), String> {
        let args = verify_args(part);
        let out = run_capture(&self.shared.config.sidecars.ffprobe, &args, VERIFY_TIMEOUT)
            .await
            .map_err(|e| format!("The exported file could not be checked: {e}"))?;
        if !out.success() {
            let err = out.stderr_lossy();
            return Err(format!(
                "The exported file is unreadable: {}",
                first_error_line(&err).unwrap_or_else(|| "ffprobe rejected it".to_string())
            ));
        }
        let report = parse_verify_json(&out.stdout_lossy(), part)
            .map_err(|e| format!("The exported file could not be checked: {e}"))?;
        if report.video_codec.is_none() {
            return Err("The exported file contains no video stream".into());
        }
        let actual = report
            .duration_sec
            .filter(|d| *d > 0.0)
            .ok_or("The exported file has no duration")?;
        let expected = item.expected_sec();
        let upper = if item.lead_in.is_some() {
            VERIFY_TOLERANCE_SEC
        } else {
            UNKNOWN_LEAD_IN_MAX_SEC
        };
        if actual < expected - VERIFY_TOLERANCE_SEC || actual > expected + upper {
            return Err(format!(
                "The exported file is {actual:.1} s long but about {expected:.1} s were expected (tolerance 1 s)"
            ));
        }
        Ok(())
    }

    /// Atomic `.part` -> final rename honouring the conflict policy. `Ok(None)`:
    /// the clip must be skipped (a file appeared and the policy is Skip).
    fn finalize(
        &self,
        part: &Path,
        planned: &Path,
        action: PlannedAction,
    ) -> std::io::Result<Option<PathBuf>> {
        use rustcut_model::ConflictPolicy as P;
        let mut target = planned.to_path_buf();
        if matches!(action, PlannedAction::Write) && target.exists() {
            match self.settings.on_conflict {
                P::Rename => target = fresh_path(planned, &self.taken),
                P::Overwrite => {}
                P::Skip => return Ok(None),
            }
        }
        std::fs::rename(part, &target)?;
        Ok(Some(target))
    }

    async fn discard_part(&self, part: &Path) {
        for _ in 0..40 {
            match std::fs::remove_file(part) {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        if let Err(e) = self.shared.journal.remove(part) {
            tracing::warn!(error = %e, "could not update the export journal");
        }
    }

    // ───────────── progress & results ─────────────

    fn set_state(&self, id: Uuid, state: St) {
        if let Some(s) = lock(&self.book).clips.get_mut(&id) {
            s.state = state;
            if state == St::Queued {
                s.fraction = 0.0;
                s.out_time = 0.0;
            }
        }
    }

    fn on_update(&self, id: Uuid, expected: f64, u: &ProgressUpdate) {
        let emit = {
            let mut b = lock(&self.book);
            if let Some(s) = b.clips.get_mut(&id) {
                if let Some(t) = u.out_time_sec {
                    s.out_time = t;
                    s.fraction = s.fraction.max(fraction(t, expected));
                }
                if u.speed.is_some() {
                    s.speed = u.speed;
                }
            }
            let now = Instant::now();
            let due = u.end
                || b.last_emit
                    .is_none_or(|l| now.duration_since(l) >= PROGRESS_INTERVAL);
            if due {
                b.last_emit = Some(now);
            }
            due
        };
        if emit {
            self.emit_progress();
        }
    }

    fn emit_progress(&self) {
        let p = {
            let b = lock(&self.book);
            build_progress(&self.id, &b, self.started.elapsed().as_secs_f64())
        };
        self.sink.on_progress(p);
    }

    fn finish(&self, item: &WorkItem, outcome: ClipOutcome) {
        let id = item.clip.id;
        let preset = self.settings.preset();
        {
            let mut r = lock(&self.results);
            let mut b = lock(&self.book);
            let (list, st) = match &outcome {
                ClipOutcome::Done { .. } => {
                    b.done += 1;
                    (&mut r.done, St::Done)
                }
                ClipOutcome::Failed { .. } => {
                    b.failed += 1;
                    (&mut r.failed, St::Excluded)
                }
                ClipOutcome::Skipped { .. } => (&mut r.skipped, St::Excluded),
                ClipOutcome::Cancelled => (&mut r.cancelled, St::Excluded),
            };
            list.push(id);
            if let Some(s) = b.clips.get_mut(&id) {
                s.state = st;
                if st == St::Done {
                    s.fraction = 1.0;
                }
            }
        }
        let record = ExportRecord {
            clip_id: id,
            outcome: outcome.clone(),
            exported_at: now_utc(),
            preset,
            settings_hash: settings_hash(&item.clip, &self.settings),
            content_hash: item.clip.content_hash(preset),
        };
        self.sink.on_clip_finished(
            record,
            ClipFinished {
                batch_id: self.id.clone(),
                clip_id: id,
                outcome,
            },
        );
        self.emit_progress();
    }
}

/// Overall fraction weighted by expected clip duration (lossless includes the
/// lead-in) and an ETA from measured throughput. Failed, cancelled and skipped
/// clips leave the denominator so the bar still reaches 100 %.
fn build_progress(batch_id: &str, b: &Book, elapsed_sec: f64) -> ExportProgress {
    let mut total_w = 0.0;
    let mut done_w = 0.0;
    let mut running = Vec::new();
    for id in &b.order {
        let Some(s) = b.clips.get(id) else { continue };
        if s.state == St::Excluded {
            continue;
        }
        total_w += s.weight;
        done_w += s.weight * if s.state == St::Done { 1.0 } else { s.fraction };
        if s.state == St::Running {
            running.push(ClipProgress {
                clip_id: *id,
                fraction: s.fraction,
                out_time_sec: s.out_time,
                speed: s.speed,
            });
        }
    }
    let overall = if total_w <= 0.0 {
        1.0
    } else {
        (done_w / total_w).clamp(0.0, 1.0)
    };
    let eta = (done_w > 0.0 && elapsed_sec > 0.5 && overall < 1.0)
        .then(|| ((total_w - done_w) / (done_w / elapsed_sec)).max(0.0));
    ExportProgress {
        batch_id: batch_id.to_string(),
        overall_fraction: overall,
        eta_sec: eta,
        done: b.done,
        failed: b.failed,
        total: b.order.len() as u32,
        running,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hw(name: &str, available: bool) -> HwEncoderInfo {
        HwEncoderInfo {
            name: name.into(),
            label: name.into(),
            available,
            error: None,
        }
    }

    #[test]
    fn exp13_auto_prefers_nvenc_then_amf_else_libx264() {
        let all = vec![
            hw("h264_vaapi", true),
            hw("h264_amf", true),
            hw("h264_nvenc", true),
            hw("h264_qsv", true),
        ];
        assert_eq!(
            resolve_encoder(EncoderChoice::Auto, &all).0,
            ResolvedEncoder::Hardware(EncoderChoice::H264Nvenc)
        );
        let amf = vec![
            hw("h264_nvenc", false),
            hw("h264_amf", true),
            hw("h264_vaapi", true),
        ];
        assert_eq!(
            resolve_encoder(EncoderChoice::Auto, &amf).0,
            ResolvedEncoder::Hardware(EncoderChoice::H264Amf)
        );
        let order = vec![hw("h264_vaapi", true), hw("h264_videotoolbox", true)];
        assert_eq!(
            resolve_encoder(EncoderChoice::Auto, &order).0,
            ResolvedEncoder::Hardware(EncoderChoice::H264Videotoolbox)
        );
        assert_eq!(
            resolve_encoder(EncoderChoice::Auto, &[]).0,
            ResolvedEncoder::Libx264
        );
    }

    #[test]
    fn exp13_explicit_choice_uses_hw_if_available_else_libx264_with_note() {
        let list = vec![hw("h264_amf", true), hw("h264_nvenc", false)];
        assert_eq!(
            resolve_encoder(EncoderChoice::H264Amf, &list),
            (ResolvedEncoder::Hardware(EncoderChoice::H264Amf), None)
        );
        let (e, note) = resolve_encoder(EncoderChoice::H264Nvenc, &list);
        assert_eq!(e, ResolvedEncoder::Libx264);
        assert!(note.unwrap().contains("h264_nvenc"));
        assert_eq!(
            resolve_encoder(EncoderChoice::Libx264, &list).0,
            ResolvedEncoder::Libx264
        );
    }

    #[test]
    fn part_path_appends_to_the_whole_name() {
        assert_eq!(
            part_path(Path::new("out/02_X.mp4")),
            PathBuf::from("out/02_X.mp4.part")
        );
    }

    #[test]
    fn fresh_path_skips_existing_and_planned_names() {
        let dir = tempfile::tempdir().unwrap();
        let planned = dir.path().join("a.mp4");
        std::fs::write(&planned, b"1").unwrap();
        std::fs::write(dir.path().join("a_2.mp4"), b"1").unwrap();
        let mut taken = HashSet::new();
        taken.insert(dir.path().join("a_3.mp4"));
        assert_eq!(fresh_path(&planned, &taken), dir.path().join("a_4.mp4"));
    }

    fn book(entries: &[(f64, f64, St)]) -> Book {
        let mut b = Book::default();
        for (w, f, st) in entries {
            let id = Uuid::now_v7();
            b.order.push(id);
            b.clips.insert(
                id,
                ClipState {
                    weight: *w,
                    fraction: *f,
                    out_time: 0.0,
                    speed: Some(2.0),
                    state: *st,
                },
            );
        }
        b
    }

    #[test]
    fn overall_progress_is_weighted_by_duration() {
        // 10 s clip done, 30 s clip half way, 60 s clip queued: (10 + 15) / 100
        let b = book(&[
            (10.0, 1.0, St::Done),
            (30.0, 0.5, St::Running),
            (60.0, 0.0, St::Queued),
        ]);
        let p = build_progress("b", &b, 5.0);
        assert!((p.overall_fraction - 0.25).abs() < 1e-9);
        assert_eq!(p.running.len(), 1);
        assert_eq!(p.total, 3);
        // throughput 25 weight-seconds in 5 s -> 75 remaining takes 15 s
        assert!((p.eta_sec.unwrap() - 15.0).abs() < 1e-9);
    }

    #[test]
    fn excluded_clips_leave_the_denominator_and_bar_reaches_one() {
        let b = book(&[(10.0, 1.0, St::Done), (50.0, 0.0, St::Excluded)]);
        let p = build_progress("b", &b, 3.0);
        assert!((p.overall_fraction - 1.0).abs() < 1e-9);
        assert_eq!(p.eta_sec, None);
        let none = build_progress("b", &book(&[(5.0, 0.0, St::Excluded)]), 1.0);
        assert!((none.overall_fraction - 1.0).abs() < 1e-9);
    }

    #[test]
    fn eta_needs_measured_progress() {
        let b = book(&[(10.0, 0.0, St::Running)]);
        assert_eq!(build_progress("b", &b, 10.0).eta_sec, None);
        let b = book(&[(10.0, 0.5, St::Running)]);
        assert_eq!(build_progress("b", &b, 0.1).eta_sec, None);
    }

    #[test]
    fn expected_duration_includes_lossless_lead_in() {
        let clip = Clip {
            id: Uuid::now_v7(),
            index: 1,
            name: "x".into(),
            start_sec: 3.0,
            end_sec: 8.0,
            tag_id: None,
            crop_x_offset: 0.0,
            selected_for_export: true,
            export_status: rustcut_model::ExportStatus::Pending,
            last_export: None,
        };
        let planned = PlannedOutput {
            clip_id: clip.id,
            index: 1,
            file_name: "x.mp4".into(),
            output_path: "x.mp4".into(),
            action: PlannedAction::Write,
            lead_in_sec: Some(1.0),
        };
        let item = WorkItem {
            clip,
            planned,
            lead_in: Some(1.0),
            nominal_sec: 5.0,
            session_retries: 0,
        };
        assert!((item.expected_sec() - 6.0).abs() < 1e-9);
    }
}
