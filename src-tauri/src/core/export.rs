//! Export commands: pre-flight, start, cancel, resume, retry, reveal (EXP-01..14, PRD 7.8).

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use rustcut_export::{
    BatchRequest, EngineConfig, ExportEngine, ExportSink, PreflightInput, RealFs, preflight,
};
use rustcut_model::{
    AppEvent, BatchInfo, BatchState, BatchSummary, CheckLevel, ClipFinished, ClipOutcome,
    CommandError, EncoderChoice, ErrorCode, ExportProgress, ExportRecord, ExportSettings,
    ExportStatus, PreflightReport, Preset, compute_fingerprint, now_utc,
};
use uuid::Uuid;

use super::session::Session;
use super::{AppCore, lk};
use crate::events::EventSink;
use crate::platform::Platform;

/// Applies engine events to the project store and forwards them to the WebView.
struct Bridge {
    session: Arc<Session>,
    sink: Arc<dyn EventSink>,
    platform: Arc<dyn Platform>,
}

impl ExportSink for Bridge {
    fn on_clip_started(&self, batch_id: &str, clip_id: Uuid, _output_path: &Path) {
        tracing::debug!(batch_id, %clip_id, "clip export started");
    }

    fn on_progress(&self, progress: ExportProgress) {
        self.sink.send(AppEvent::ExportProgress { progress });
    }

    fn on_clip_finished(&self, record: ExportRecord, finished: ClipFinished) {
        {
            let mut store = lk(&self.session.store);
            if let Some(patch) = store.apply_export_record(&record) {
                self.session.schedule_save(&mut store);
                self.sink.send(AppEvent::Patch { patch });
            }
        }
        self.sink.send(AppEvent::ExportClipFinished { finished });
    }

    fn on_batch_state(&self, state: BatchState) {
        if let BatchState::Finished { summary } = &state
            && let Some((title, body)) = finish_notification(summary)
        {
            self.platform.notify(&title, &body);
        }
        self.sink.send(AppEvent::ExportBatch { state });
    }
}

/// EXP-14: the OS notification text of a finished batch (`None` when nothing ran to an end).
fn finish_notification(summary: &BatchSummary) -> Option<(String, String)> {
    let (done, failed) = (summary.done.len(), summary.failed.len());
    if done == 0 && failed == 0 {
        return None;
    }
    let noun = |n: usize| if n == 1 { "clip" } else { "clips" };
    let body = if failed == 0 {
        format!("{done} {} exported to {}", noun(done), summary.output_dir)
    } else {
        format!(
            "{done} {} exported, {failed} failed. Open RustCut to see why.",
            noun(done)
        )
    };
    Some(("RustCut export finished".to_string(), body))
}

/// Lexical normalisation for comparing paths (no file system access).
fn normalized(path: &Path) -> Option<Vec<String>> {
    let abs = std::path::absolute(path).ok()?;
    let mut parts = Vec::new();
    for c in abs.components() {
        match c {
            Component::ParentDir | Component::CurDir => return None,
            other => {
                let s = other.as_os_str().to_string_lossy().into_owned();
                parts.push(if cfg!(windows) {
                    s.to_lowercase().replace("\\\\?\\", "")
                } else {
                    s
                });
            }
        }
    }
    Some(parts)
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (normalized(a), normalized(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// Whether an export with these settings may pick a hardware encoder.
fn settings_need_hw(settings: &ExportSettings) -> bool {
    settings.preset() != Preset::Lossless && settings.encoder != EncoderChoice::Libx264
}

/// PRD 7.9: the output folder comes from a validated absolute path, never a relative one
/// (which would resolve against the app's working directory or look like an option).
pub(crate) fn validate_output_dir(settings: &ExportSettings) -> Result<(), CommandError> {
    match settings.output_dir.as_deref() {
        Some(dir) if !dir.trim().is_empty() && !Path::new(dir).is_absolute() => {
            Err(CommandError::new(
                ErrorCode::InvalidInput,
                "The output folder must be an absolute path. Choose the folder again.",
            ))
        }
        _ => Ok(()),
    }
}

impl AppCore {
    /// Build or reuse the engine. Hardware detection is awaited only when `needs_hw` (a
    /// re-encode that may pick a hardware encoder): Lossless never waits on the probe.
    async fn engine(&self, needs_hw: bool) -> Result<ExportEngine, CommandError> {
        let hw = if needs_hw {
            self.hw_encoders().await
        } else {
            lk(&self.inner.hw).clone().unwrap_or_default()
        };
        let hw_known = lk(&self.inner.hw).is_some();
        {
            let mut cached = lk(&self.inner.engine);
            if let Some(engine) = cached.clone() {
                // Built before the list was known and it is known now: rebuild once.
                let stale = hw_known && !self.inner.engine_hw_known.load(Ordering::Acquire);
                if !stale || engine.is_running() {
                    return Ok(engine);
                }
                *cached = None;
            }
        }
        let sidecars = self.sidecars()?.clone();
        let zscale = self.caps().await.is_some_and(|c| c.has_filter("zscale"));
        let engine = ExportEngine::new(EngineConfig {
            sidecars,
            log_dir: self.inner.paths.log.clone(),
            data_dir: self.inner.paths.data.clone(),
            cpu_count: self.inner.cpu_count,
            hw_encoders: hw,
            zscale_available: zscale,
        });
        let mut cached = lk(&self.inner.engine);
        if cached.is_none() {
            self.inner
                .engine_hw_known
                .store(hw_known, Ordering::Release);
        }
        Ok(cached.get_or_insert(engine).clone())
    }

    /// Pre-flight against the session's current state (EXP-11).
    async fn run_preflight(
        &self,
        session: &Arc<Session>,
        clip_ids: &[Uuid],
        settings: &ExportSettings,
    ) -> PreflightReport {
        let project = lk(&session.store).project().clone();
        let source_path = lk(&session.source_path).clone();
        let fingerprint =
            tokio::task::spawn_blocking(move || compute_fingerprint(&source_path).ok())
                .await
                .unwrap_or(None);
        let zscale = self.caps().await.is_some_and(|c| c.has_filter("zscale"));
        let index = lk(&session.index).as_ref().map(|e| e.index.clone());
        preflight(&PreflightInput {
            project: &project,
            clip_ids,
            settings,
            media_index: index.as_ref(),
            current_fingerprint: fingerprint,
            zscale_available: zscale,
            today: chrono::Local::now().date_naive(),
            fs: &RealFs,
        })
    }

    /// `preview_export`.
    pub async fn preview_export(
        &self,
        clip_ids: &[Uuid],
        settings: &ExportSettings,
    ) -> Result<PreflightReport, CommandError> {
        let session = self.session()?;
        Ok(self.run_preflight(&session, clip_ids, settings).await)
    }

    /// `start_export`: re-run the pre-flight, refuse on blocking items or a running batch,
    /// save the settings into the project, mark the clips Exporting and start the engine.
    ///
    /// # Errors
    /// [`ErrorCode::InvalidInput`] listing the blocking pre-flight messages,
    /// [`ErrorCode::Busy`] while another batch runs, [`ErrorCode::Io`] when the output
    /// folder cannot be created.
    pub async fn start_export(
        &self,
        clip_ids: &[Uuid],
        settings: &ExportSettings,
    ) -> Result<BatchInfo, CommandError> {
        let _starting = self.inner.start_lock.lock().await;
        validate_output_dir(settings)?;
        let session = self.session()?;
        let engine = self.engine(settings_need_hw(settings)).await?;
        if engine.is_running() {
            return Err(CommandError::new(
                ErrorCode::Busy,
                "An export is already running. Wait for it to finish or cancel it first.",
            ));
        }
        let report = self.run_preflight(&session, clip_ids, settings).await;
        if !report.can_start {
            let reasons: Vec<String> = report
                .items
                .iter()
                .filter(|i| i.level == CheckLevel::Error)
                .map(|i| i.message.clone())
                .collect();
            let message = if reasons.is_empty() {
                "The export cannot start.".to_string()
            } else {
                format!("The export cannot start: {}", reasons.join(" "))
            };
            return Err(CommandError::new(ErrorCode::InvalidInput, message));
        }
        let settings = settings.clone().normalized();
        let planned: Vec<Uuid> = report.outputs.iter().map(|o| o.clip_id).collect();
        let (project, index) = {
            let mut store = lk(&session.store);
            if store.read_only().is_none() {
                let before = store.revision();
                let patch = store.set_export_settings(settings.clone())?;
                if store.revision() != before {
                    self.emit(AppEvent::Patch { patch });
                }
            }
            let before = store.revision();
            let patch = store.mark_exporting(&planned);
            if store.revision() != before {
                self.emit(AppEvent::Patch { patch });
            }
            session.schedule_save(&mut store);
            (
                store.project().clone(),
                lk(&session.index).as_ref().map(|e| e.index.clone()),
            )
        };
        let bridge = Arc::new(Bridge {
            session: session.clone(),
            sink: self.inner.sink.clone(),
            platform: self.inner.platform.clone(),
        });
        match engine.start(
            BatchRequest {
                project,
                settings,
                report,
                media_index: index,
            },
            bridge,
        ) {
            Ok(info) => Ok(info),
            Err(e) => {
                self.revert_exporting(&session, &planned);
                Err(e.into())
            }
        }
    }

    /// The engine refused after the clips were marked: restore their previous statuses.
    fn revert_exporting(&self, session: &Session, ids: &[Uuid]) {
        let mut store = lk(&session.store);
        for id in ids {
            let record = ExportRecord {
                clip_id: *id,
                outcome: ClipOutcome::Cancelled,
                exported_at: now_utc(),
                preset: store.project().export_settings.preset(),
                settings_hash: String::new(),
                content_hash: String::new(),
            };
            if let Some(patch) = store.apply_export_record(&record) {
                self.emit(AppEvent::Patch { patch });
            }
        }
        session.schedule_save(&mut store);
    }

    /// `cancel_export`: one clip, or the whole batch when `clip_id` is `None`.
    pub fn cancel_export(&self, clip_id: Option<Uuid>) {
        let engine = lk(&self.inner.engine).clone();
        if let Some(engine) = engine {
            match clip_id {
                Some(id) => engine.cancel_clip(id),
                None => engine.cancel_all(),
            }
        }
    }

    /// `resume_export`: continue a batch paused for a full disk or a missing folder.
    ///
    /// # Errors
    /// [`ErrorCode::InvalidInput`] when nothing is paused, or the problem persists.
    pub fn resume_export(&self) -> Result<(), CommandError> {
        let engine = lk(&self.inner.engine).clone();
        match engine {
            Some(engine) => Ok(engine.resume()?),
            None => Err(CommandError::new(
                ErrorCode::InvalidInput,
                "No export is paused.",
            )),
        }
    }

    /// `retry_failed`: a new batch of the clips whose last export failed (project settings).
    pub async fn retry_failed(&self) -> Result<Option<BatchInfo>, CommandError> {
        let session = self.session()?;
        let (ids, settings) = {
            let store = lk(&session.store);
            let project = store.project();
            let mut failed: Vec<_> = project
                .clips
                .iter()
                .filter(|c| c.export_status == ExportStatus::Failed)
                .collect();
            failed.sort_by_key(|c| c.index);
            (
                failed.iter().map(|c| c.id).collect::<Vec<_>>(),
                project.export_settings.clone(),
            )
        };
        if ids.is_empty() {
            return Ok(None);
        }
        self.start_export(&ids, &settings).await.map(Some)
    }

    /// The output folder of the open project.
    fn output_dir(&self) -> Result<PathBuf, CommandError> {
        let session = self.session()?;
        let dir = lk(&session.store).project().output_dir();
        Ok(PathBuf::from(dir))
    }

    /// `open_output_folder`.
    ///
    /// # Errors
    /// [`ErrorCode::NotFound`] while the folder does not exist yet.
    pub fn open_output_folder(&self) -> Result<(), CommandError> {
        let dir = self.output_dir()?;
        if !dir.is_dir() {
            return Err(CommandError::new(
                ErrorCode::NotFound,
                format!(
                    "The output folder \"{}\" does not exist yet. Export a clip first, or choose another folder in the export settings.",
                    dir.display()
                ),
            ));
        }
        self.inner.platform.open_dir(&dir).map_err(|e| {
            CommandError::new(
                ErrorCode::Io,
                format!("Could not open \"{}\": {e}", dir.display()),
            )
        })
    }

    /// `reveal_path` (PRD 7.9): only the output folder of this project or a file recorded in a
    /// clip's last export may be revealed.
    ///
    /// # Errors
    /// [`ErrorCode::InvalidInput`] for any other path, [`ErrorCode::NotFound`] when the file
    /// was moved or deleted.
    pub fn reveal_path(&self, path: &str) -> Result<(), CommandError> {
        let session = self.session()?;
        let requested = PathBuf::from(path);
        let allowed = {
            let store = lk(&session.store);
            let project: &rustcut_model::Project = store.project();
            same_path(&requested, Path::new(&project.output_dir()))
                || project
                    .clips
                    .iter()
                    .filter_map(|c| c.last_export.as_ref()?.output_path.as_deref())
                    .any(|p| same_path(&requested, Path::new(p)))
        };
        if !allowed {
            return Err(CommandError::new(
                ErrorCode::InvalidInput,
                "Only the output folder of this project and the files it exported can be shown in the file manager.",
            ));
        }
        if !requested.exists() {
            return Err(CommandError::new(
                ErrorCode::NotFound,
                format!(
                    "\"{}\" no longer exists. It may have been moved or deleted; export the clip again.",
                    requested.display()
                ),
            ));
        }
        self.inner.platform.reveal(&requested).map_err(|e| {
            CommandError::new(
                ErrorCode::Io,
                format!("Could not show \"{}\": {e}", requested.display()),
            )
        })
    }

    /// Block until the engine is idle (used by tests and shutdown paths).
    pub async fn wait_export_idle(&self, timeout: std::time::Duration) -> bool {
        let engine = lk(&self.inner.engine).clone();
        let Some(engine) = engine else { return true };
        let end = std::time::Instant::now() + timeout;
        while engine.is_running() {
            if std::time::Instant::now() >= end {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        true
    }
}
