//! The application core: every IPC command as a plain method over [`AppCore`]
//! (ARCHITECTURE section 4). No Tauri types appear here; events leave through an
//! [`EventSink`] and OS integration through a [`Platform`], so the whole flow runs in
//! `cargo test` without a WebView.

mod export;
mod media;
mod open;
mod session;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use rustcut_model::{
    APP_VERSION, AddClipRequest, AddClipResult, AppEvent, AppInfo, AppSettings, CommandError,
    CropWindow, EditClipsRequest, ErrorCode, ExportSettings, HwEncoderInfo, ImportPreview,
    ImportedClip, MediaIndexInfo, Notice, NoticeLevel, ProjectSnapshot, RecentProject,
    SCHEMA_VERSION, SaveState, StatePatch, Tag, TimestampExport, TimestampFormat, ViewState,
};
use rustcut_project::{ProjectStore, RecentStore, StoreError, StoreOptions};
use rustcut_runner::{Capabilities, Sidecars, probe_capabilities};
use uuid::Uuid;

use crate::error::{ffmpeg_missing, io_error, no_project};
use crate::events::EventSink;
use crate::paths::AppPaths;
use crate::platform::Platform;
pub use session::Session;

pub(crate) fn lk<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What [`AppCore::new`] needs.
pub struct CoreConfig {
    /// App folders (already created).
    pub paths: AppPaths,
    /// FFmpeg/FFprobe, or the reason they were not found (the app still starts).
    pub sidecars: Result<Sidecars, String>,
    /// Event delivery.
    pub sink: Arc<dyn EventSink>,
    /// OS integration.
    pub platform: Arc<dyn Platform>,
}

pub(crate) struct Inner {
    pub paths: AppPaths,
    pub sink: Arc<dyn EventSink>,
    pub platform: Arc<dyn Platform>,
    pub settings: Mutex<AppSettings>,
    pub sidecars: Option<Sidecars>,
    pub sidecar_error: Option<String>,
    pub media: Option<rustcut_media::MediaService>,
    pub caps: tokio::sync::OnceCell<Option<Arc<Capabilities>>>,
    pub hw: Mutex<Option<Vec<HwEncoderInfo>>>,
    pub engine: Mutex<Option<rustcut_export::ExportEngine>>,
    /// Whether the cached engine was built with a known hardware-encoder list.
    pub engine_hw_known: std::sync::atomic::AtomicBool,
    /// Serializes start_export so overlapping starts cannot both mark clips.
    pub start_lock: tokio::sync::Mutex<()>,
    pub recents: RecentStore,
    pub session: Mutex<Option<Arc<Session>>>,
    pub open_lock: tokio::sync::Mutex<()>,
    pub cpu_count: u32,
}

/// Application state and command logic. Cheap to clone (shared state).
#[derive(Clone)]
pub struct AppCore {
    pub(crate) inner: Arc<Inner>,
}

impl AppCore {
    /// Build the core: clean up what a crash left behind (PRD 7.8), load the settings and
    /// tell the user when FFmpeg is missing.
    pub fn new(config: CoreConfig) -> Self {
        let CoreConfig {
            paths,
            sidecars,
            sink,
            platform,
        } = config;
        // Before anything else: kill stale FFmpeg children and delete journaled .part files.
        let stale = rustcut_export::cleanup_stale_with(&paths.data, sidecars.as_ref().ok());
        if !stale.is_empty() {
            tracing::info!(
                count = stale.len(),
                "removed leftover partial exports from a previous run"
            );
        }
        let settings = load_settings(&paths.data);
        let (sidecars, sidecar_error) = match sidecars {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e)),
        };
        let media = sidecars.clone().map(|sidecars| {
            rustcut_media::MediaService::new(rustcut_media::MediaConfig {
                sidecars,
                cache_dir: paths.cache.clone(),
                data_dir: paths.data.clone(),
                cache_cap_bytes: u64::from(settings.cache_cap_gb) * (1 << 30),
            })
        });
        if let Some(detail) = &sidecar_error {
            tracing::error!(%detail, "FFmpeg sidecars not found");
            sink.send(AppEvent::Toast {
                notice: Notice::new(
                    NoticeLevel::Error,
                    "ffmpeg_missing",
                    ffmpeg_missing(detail).message,
                ),
            });
        }
        let recents = RecentStore::new(&paths.data);
        let cpu_count =
            std::thread::available_parallelism().map_or(4, |n| u32::try_from(n.get()).unwrap_or(4));
        AppCore {
            inner: Arc::new(Inner {
                paths,
                sink,
                platform,
                settings: Mutex::new(settings),
                sidecars,
                sidecar_error,
                media,
                caps: tokio::sync::OnceCell::new(),
                hw: Mutex::new(None),
                engine: Mutex::new(None),
                engine_hw_known: std::sync::atomic::AtomicBool::new(false),
                start_lock: tokio::sync::Mutex::new(()),
                recents,
                session: Mutex::new(None),
                open_lock: tokio::sync::Mutex::new(()),
                cpu_count,
            }),
        }
    }

    /// Background warm-up at launch: probe the FFmpeg build and detect hardware encoders once.
    pub async fn warm_up(&self) {
        let _ = self.caps().await;
        if self.inner.media.is_some() {
            let _ = self.detect_hw_encoders(false).await;
        }
    }

    pub(crate) fn emit(&self, event: AppEvent) {
        self.inner.sink.send(event);
    }

    pub(crate) fn media(&self) -> Result<&rustcut_media::MediaService, CommandError> {
        self.inner.media.as_ref().ok_or_else(|| {
            ffmpeg_missing(self.inner.sidecar_error.as_deref().unwrap_or("not located"))
        })
    }

    pub(crate) fn sidecars(&self) -> Result<&Sidecars, CommandError> {
        self.inner.sidecars.as_ref().ok_or_else(|| {
            ffmpeg_missing(self.inner.sidecar_error.as_deref().unwrap_or("not located"))
        })
    }

    pub(crate) async fn caps(&self) -> Option<Arc<Capabilities>> {
        let sidecars = self.inner.sidecars.as_ref()?;
        self.inner
            .caps
            .get_or_init(|| async {
                match probe_capabilities(sidecars).await {
                    Ok(caps) => Some(caps),
                    Err(e) => {
                        tracing::warn!(error = %e, "could not read the FFmpeg capabilities");
                        if let Some(notice) = capability_notice(&e) {
                            self.emit(AppEvent::Toast { notice });
                        }
                        None
                    }
                }
            })
            .await
            .clone()
    }

    pub(crate) fn session(&self) -> Result<Arc<Session>, CommandError> {
        lk(&self.inner.session).clone().ok_or_else(no_project)
    }

    pub(crate) fn store_options(&self) -> StoreOptions {
        StoreOptions {
            min_clip_duration_sec: lk(&self.inner.settings).min_clip_duration_sec,
        }
    }

    /// Current settings (normalized).
    pub fn settings(&self) -> AppSettings {
        lk(&self.inner.settings).clone()
    }

    // ───────────────────────────── app level commands ─────────────────────────────

    /// `get_app_info`.
    pub async fn app_info(&self) -> AppInfo {
        let caps = self.caps().await;
        AppInfo {
            app_version: APP_VERSION.to_string(),
            schema_version: SCHEMA_VERSION,
            ffmpeg_version: caps.map(|c| c.ffmpeg_version.clone()),
            ffmpeg_found: self.inner.sidecars.is_some(),
            ffprobe_found: self.inner.sidecars.is_some(),
            data_dir: self.inner.paths.data.to_string_lossy().into_owned(),
            log_dir: self.inner.paths.log.to_string_lossy().into_owned(),
            cache_dir: self.inner.paths.cache.to_string_lossy().into_owned(),
            platform: std::env::consts::OS.to_string(),
            cpu_count: self.inner.cpu_count,
            hw_encoders: lk(&self.inner.hw).clone().unwrap_or_default(),
        }
    }

    /// `set_settings`: normalize, persist to `<data>/settings.json` and apply the cache cap.
    ///
    /// # Errors
    /// [`ErrorCode::Io`] when the file cannot be written.
    pub async fn set_settings(&self, settings: AppSettings) -> Result<AppSettings, CommandError> {
        let settings = settings.normalized();
        let path = self.inner.paths.data.join(SETTINGS_FILE);
        let bytes = serde_json::to_vec_pretty(&settings).map_err(|e| {
            CommandError::new(
                ErrorCode::Internal,
                format!("Could not encode the settings: {e}"),
            )
        })?;
        rustcut_project::persist::write_atomic(&path, &bytes)?;
        *lk(&self.inner.settings) = settings.clone();
        if let Some(media) = &self.inner.media {
            media
                .set_cache_cap(u64::from(settings.cache_cap_gb) * (1 << 30))
                .await;
        }
        Ok(settings)
    }

    /// `list_recent_projects`.
    pub fn list_recent_projects(&self) -> Vec<RecentProject> {
        self.inner.recents.list()
    }

    /// `detect_hw_encoders` (cached unless `force`).
    ///
    /// # Errors
    /// [`ErrorCode::Ffmpeg`] when FFmpeg is missing.
    pub async fn detect_hw_encoders(
        &self,
        force: bool,
    ) -> Result<Vec<HwEncoderInfo>, CommandError> {
        let list = self.media()?.detect_hw_encoders(force).await;
        // An empty list may be a failed probe (not cached by the media service): keep it
        // out of the cache so a later call retries.
        if !list.is_empty() {
            *lk(&self.inner.hw) = Some(list.clone());
        }
        if force {
            // A changed list only matters for the next engine; keep a running one.
            let mut engine = lk(&self.inner.engine);
            if engine.as_ref().is_some_and(|e| !e.is_running()) {
                *engine = None;
            }
        }
        Ok(list)
    }

    pub(crate) async fn hw_encoders(&self) -> Vec<HwEncoderInfo> {
        if let Some(list) = lk(&self.inner.hw).clone() {
            return list;
        }
        self.detect_hw_encoders(false).await.unwrap_or_default()
    }

    /// `clear_cache`: bytes freed (the file being played stays).
    ///
    /// # Errors
    /// [`ErrorCode::Ffmpeg`] when FFmpeg is missing (the media service does not exist).
    pub async fn clear_cache(&self) -> Result<u64, CommandError> {
        Ok(self.media()?.clear_cache().await)
    }

    /// `crop_window` (FRM-04).
    pub fn crop_window(&self, display_width: u32, display_height: u32, offset: f64) -> CropWindow {
        rustcut_framing::crop_window(display_width, display_height, offset)
    }

    /// Write `text` to `path` (the save dialog's result) for `save_text_file`.
    ///
    /// # Errors
    /// [`ErrorCode::Io`] naming the file.
    pub fn write_text_file(&self, path: &Path, text: &str) -> Result<(), CommandError> {
        std::fs::write(path, text).map_err(|e| io_error("write", path, &e))
    }

    // ───────────────────────────── project state ─────────────────────────────

    /// `get_snapshot`.
    pub fn get_snapshot(&self) -> Option<ProjectSnapshot> {
        let session = lk(&self.inner.session).clone()?;
        Some(session.snapshot())
    }

    /// `save_now`: write pending changes and report the result.
    pub async fn save_now(&self) -> Result<SaveState, CommandError> {
        let session = self.session()?;
        session.flush().await;
        Ok(session.tracker.save_state())
    }

    /// `set_view_state` (saved, not undoable, no patch).
    pub fn set_view_state(&self, view: ViewState) -> Result<(), CommandError> {
        let session = self.session()?;
        let mut store = lk(&session.store);
        store.set_view_state(view);
        session.schedule_save(&mut store);
        Ok(())
    }

    /// Run a mutation that yields one patch: schedule the save and send the patch event
    /// (only when the revision moved).
    fn commit_patch(
        &self,
        f: impl FnOnce(&mut ProjectStore) -> Result<StatePatch, StoreError>,
    ) -> Result<StatePatch, CommandError> {
        let session = self.session()?;
        let mut store = lk(&session.store);
        let before = store.revision();
        let patch = f(&mut store)?;
        let changed = store.revision() != before;
        session.schedule_save(&mut store);
        if changed {
            self.emit(AppEvent::Patch {
                patch: patch.clone(),
            });
        }
        Ok(patch)
    }

    /// `add_clip` (MRK-01).
    pub fn add_clip(&self, req: &AddClipRequest) -> Result<AddClipResult, CommandError> {
        let opts = self.store_options();
        let session = self.session()?;
        let mut store = lk(&session.store);
        let result = store.add_clip(req, &opts)?;
        session.schedule_save(&mut store);
        self.emit(AppEvent::Patch {
            patch: result.patch.clone(),
        });
        Ok(result)
    }

    /// `edit_clips`.
    pub fn edit_clips(&self, req: &EditClipsRequest) -> Result<StatePatch, CommandError> {
        let opts = self.store_options();
        self.commit_patch(|s| s.edit_clips(&req.ids, &req.edit, &opts))
    }

    /// `edit_clips_batch`: validate and commit a bulk action as one undo step.
    pub fn edit_clips_batch(&self, batch: &[EditClipsRequest]) -> Result<StatePatch, CommandError> {
        let opts = self.store_options();
        self.commit_patch(|s| s.edit_batch(batch, &opts))
    }

    /// `get_platform_presets`: a user file that cannot be used falls back to the bundled
    /// list and is reported as a toast.
    pub fn platform_presets(&self) -> Vec<rustcut_model::PlatformPreset> {
        let (presets, warning) = rustcut_export::load_platform_presets(&self.inner.paths.data);
        if let Some(message) = warning {
            self.emit(AppEvent::Toast {
                notice: Notice::new(NoticeLevel::Warning, "platform_presets_invalid", message),
            });
        }
        presets
    }

    /// `delete_clips`.
    pub fn delete_clips(&self, ids: &[Uuid]) -> Result<StatePatch, CommandError> {
        self.commit_patch(|s| s.delete_clips(ids))
    }

    /// `undo`: `None` when there is nothing to undo.
    pub fn undo(&self) -> Result<Option<StatePatch>, CommandError> {
        self.history_step(ProjectStore::undo)
    }

    /// `redo`: `None` when there is nothing to redo.
    pub fn redo(&self) -> Result<Option<StatePatch>, CommandError> {
        self.history_step(ProjectStore::redo)
    }

    fn history_step(
        &self,
        f: impl FnOnce(&mut ProjectStore) -> Option<StatePatch>,
    ) -> Result<Option<StatePatch>, CommandError> {
        let session = self.session()?;
        let mut store = lk(&session.store);
        let patch = f(&mut store);
        session.schedule_save(&mut store);
        if let Some(p) = &patch {
            self.emit(AppEvent::Patch { patch: p.clone() });
        }
        Ok(patch)
    }

    /// `renumber_clips` (QUE-02).
    pub fn renumber_clips(&self) -> Result<StatePatch, CommandError> {
        self.commit_patch(ProjectStore::renumber_chronologically)
    }

    /// `set_tags` (MRK-06).
    pub fn set_tags(&self, tags: Vec<Tag>) -> Result<StatePatch, CommandError> {
        self.commit_patch(|s| s.set_tags(tags))
    }

    /// `set_export_settings`.
    pub fn set_export_settings(
        &self,
        settings: ExportSettings,
    ) -> Result<StatePatch, CommandError> {
        export::validate_output_dir(&settings)?;
        self.commit_patch(|s| s.set_export_settings(settings))
    }

    // ───────────────────────────── timestamps (EXP-16/17) ─────────────────────────────

    /// `export_timestamps`: all clips of the open project.
    pub fn export_timestamps(
        &self,
        format: TimestampFormat,
    ) -> Result<TimestampExport, CommandError> {
        let session = self.session()?;
        let project = lk(&session.store).project().clone();
        Ok(rustcut_naming::timestamps::export(&project, None, format))
    }

    /// `parse_timestamps`: preview of pasted text against the open source.
    pub fn parse_timestamps(&self, text: &str) -> Result<ImportPreview, CommandError> {
        let session = self.session()?;
        let duration = lk(&session.store).project().source.duration_sec;
        let min = self.store_options().min_duration();
        Ok(rustcut_naming::timestamps::parse_import(
            text, duration, min,
        ))
    }

    /// `import_timestamps`: valid items as one undo step; skipped ones are reported in a toast.
    pub fn import_timestamps(&self, items: &[ImportedClip]) -> Result<StatePatch, CommandError> {
        let opts = self.store_options();
        let session = self.session()?;
        let mut store = lk(&session.store);
        let result = store.import_clips(items, &opts)?;
        session.schedule_save(&mut store);
        if !result.added.is_empty() {
            self.emit(AppEvent::Patch {
                patch: result.patch.clone(),
            });
        }
        if !result.skipped.is_empty() {
            self.emit(AppEvent::Toast {
                notice: Notice::new(
                    NoticeLevel::Warning,
                    "import_skipped",
                    format!(
                        "{} of {} timestamp lines were skipped because their times are invalid.",
                        result.skipped.len(),
                        items.len()
                    ),
                ),
            });
        }
        Ok(result.patch)
    }

    // ───────────────────────────── media info ─────────────────────────────

    /// `get_media_index`: `None` until the background index is built.
    pub fn get_media_index(&self) -> Result<Option<MediaIndexInfo>, CommandError> {
        let session = self.session()?;
        Ok(lk(&session.index).as_ref().map(|e| e.info.clone()))
    }

    /// Graceful shutdown (window close / app exit): cancel exports and background jobs,
    /// flush saves, release the lock (PRD 7.8). Never fails and gives up after
    /// [`SHUTDOWN_TIMEOUT`]; see [`AppCore::try_shutdown`] to keep the window on a save error.
    pub async fn shutdown(&self) {
        let _ = self.try_shutdown().await;
    }

    /// Like [`AppCore::shutdown`], but returns the save error when the final flush fails,
    /// leaving the project open so the user can fix the disk and close again.
    ///
    /// # Errors
    /// The message of the failed final save.
    pub async fn try_shutdown(&self) -> Result<(), String> {
        let work = async {
            // An in-flight open must finish (or not start) before the session is torn down.
            let _guard = self.inner.open_lock.lock().await;
            let current = lk(&self.inner.session).clone();
            if let Some(session) = current
                && let Some(error) = session.flush().await
            {
                return Err(error);
            }
            self.close_session().await;
            Ok(())
        };
        match tokio::time::timeout(SHUTDOWN_TIMEOUT, work).await {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!("shutdown did not finish in time; exiting anyway");
                Ok(())
            }
        }
    }
}

/// Longest the final flush and teardown may take before the app exits regardless (a
/// stalled network drive must not keep the window open forever).
pub const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

const SETTINGS_FILE: &str = "settings.json";

/// An unsupported sidecar build needs a visible recovery action even when warm-up degrades.
fn capability_notice(error: &rustcut_runner::RunnerError) -> Option<Notice> {
    match error {
        rustcut_runner::RunnerError::FfmpegTooOld { .. } => Some(Notice::new(
            NoticeLevel::Warning,
            "ffmpeg_too_old",
            format!(
                "{error}. Reinstall RustCut to restore its bundled FFmpeg, or point RUSTCUT_FFMPEG_DIR to a folder with a supported ffmpeg and ffprobe pair."
            ),
        )),
        _ => None,
    }
}

fn load_settings(data_dir: &Path) -> AppSettings {
    let path: PathBuf = data_dir.join(SETTINGS_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<AppSettings>(&bytes) {
            Ok(s) => s.normalized(),
            Err(e) => {
                tracing::warn!(error = %e, "settings.json is unreadable; using the defaults");
                AppSettings::default()
            }
        },
        Err(_) => AppSettings::default(),
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    #[test]
    fn old_ffmpeg_has_a_specific_actionable_warning() {
        let notice = capability_notice(&rustcut_runner::RunnerError::FfmpegTooOld {
            found: "5.1".into(),
            required: "6.1".into(),
        })
        .expect("visible warning");
        assert_eq!(notice.code, "ffmpeg_too_old");
        assert_eq!(notice.level, NoticeLevel::Warning);
        for text in ["5.1", "6.1", "Reinstall RustCut", "RUSTCUT_FFMPEG_DIR"] {
            assert!(notice.message.contains(text), "{text}");
        }
    }
}
