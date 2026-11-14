//! Opening and relinking projects (PRJ-02 .. PRJ-06).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rustcut_media::ProbedSource;
use rustcut_model::{
    CommandError, ErrorCode, Notice, NoticeLevel, OpenResult, Project, ProjectLocation,
    ReadOnlyReason, RecentProject, RelinkResult, SourceInfo, SourceStatus, now_utc,
};
use rustcut_project::error::read_only_message;
use rustcut_project::location::{
    PROJECT_EXTENSION, dir_is_writable, find_existing_project, find_relink_candidate,
    relative_source_path, resolve_new_project_location, resolve_source,
};
use rustcut_project::{
    AutoSaver, BackupStore, Loaded, LockOutcome, ProjectLock, ProjectStore, Urgency,
};

use super::session::{Jobs, SaveTracker, Session};
use super::{AppCore, lk};
use crate::error::{join_error, media_error};

/// Everything decided about a project before the session is created.
struct Plan {
    /// Held from before the project is loaded until the new session takes ownership.
    lock: Option<Arc<ProjectLock>>,
    project: Project,
    project_path: PathBuf,
    location: ProjectLocation,
    notices: Vec<Notice>,
    read_only: Option<ReadOnlyReason>,
    source_path: PathBuf,
    source_status: SourceStatus,
    mime: Option<String>,
    /// Write the project right after opening (new, migrated, recovered or re-pathed).
    save_initial: bool,
    saved_at: Option<DateTime<Utc>>,
}

struct ProjectAccess {
    lock: Option<Arc<ProjectLock>>,
    read_only: Option<ReadOnlyReason>,
    notices: Vec<Notice>,
}

fn is_project_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(PROJECT_EXTENSION))
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn file_time(path: &Path) -> Option<DateTime<Utc>> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()
        .map(DateTime::<Utc>::from)
}

fn notice(level: NoticeLevel, code: &str, message: impl Into<String>) -> Notice {
    Notice::new(level, code, message)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// PRJ-04: `Some(changed)` when `probed` is the file the project was made from (the
/// project's source info is refreshed with the current path), `None` on a fingerprint mismatch.
fn reconcile(
    project: &mut Project,
    project_path: &Path,
    source_path: &Path,
    probed: &SourceInfo,
) -> Option<bool> {
    if project.source.fingerprint != probed.fingerprint {
        return None;
    }
    let mut fresh = probed.clone();
    fresh.path = source_path.to_string_lossy().into_owned();
    fresh.relative_path = relative_source_path(project_path, source_path);
    let changed = project.source != fresh;
    project.source = fresh;
    Some(changed)
}

/// Notices about the probed file (HDR, a file that is still being recorded, other findings).
fn probe_notices(source: &SourceInfo, warnings: &[String]) -> Vec<Notice> {
    let mut out = Vec::new();
    if source.video.hdr {
        out.push(notice(
            NoticeLevel::Info,
            "hdr_source",
            "This video is HDR. Previews may look washed out; re-encoded exports convert to SDR.",
        ));
    }
    for w in warnings {
        if w.contains("still being recorded") {
            out.push(notice(NoticeLevel::Warning, "source_growing", w.clone()));
        } else if !w.contains("HDR") {
            out.push(notice(NoticeLevel::Info, "source_warning", w.clone()));
        }
    }
    out
}

impl AppCore {
    /// `open_path`: a video (find or create its project) or a `.rustcut` file.
    ///
    /// # Errors
    /// [`ErrorCode::NotFound`] for a missing path, probe/FFmpeg errors for an unreadable
    /// video, [`ErrorCode::Io`] for an unreadable project or a project that cannot be saved.
    pub async fn open_path(&self, path: &str) -> Result<OpenResult, CommandError> {
        let _guard = self.inner.open_lock.lock().await;
        let path = PathBuf::from(path);
        if !path.is_file() {
            return Err(CommandError::new(
                ErrorCode::NotFound,
                format!(
                    "\"{}\" was not found. Check that the file still exists and the drive is connected.",
                    path.display()
                ),
            ));
        }
        let current = lk(&self.inner.session).clone();
        if let Some(session) = current {
            session.flush().await;
        }
        let plan = if is_project_file(&path) {
            self.plan_project_file(&path).await?
        } else {
            self.plan_video(&path).await?
        };
        self.close_session().await;
        self.finish_open(plan).await
    }

    /// Acquire single-writer access before reading any state that could later be saved.
    async fn project_access(&self, path: &Path) -> ProjectAccess {
        let current = lk(&self.inner.session).clone();
        if let Some(session) = current {
            let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| absolute(p));
            if canonical(&session.project_path) == canonical(path) {
                // Share our own lock through planning. Closing the previous session on
                // success hands ownership to the plan; failure leaves the session locked.
                if let Some(lock) = lk(&session.lock).clone() {
                    return ProjectAccess {
                        lock: Some(lock),
                        read_only: None,
                        notices: Vec::new(),
                    };
                }
            }
        }
        match ProjectLock::try_acquire(path) {
            Ok(LockOutcome::Acquired(lock)) => ProjectAccess {
                lock: Some(Arc::new(lock)),
                read_only: None,
                notices: Vec::new(),
            },
            Ok(LockOutcome::HeldElsewhere { holder }) => {
                let reason = ReadOnlyReason::OpenElsewhere { holder };
                let message = read_only_message(&reason);
                ProjectAccess {
                    lock: None,
                    read_only: Some(reason),
                    notices: vec![notice(NoticeLevel::Warning, "read_only_locked", message)],
                }
            }
            Err(error) => {
                tracing::warn!(%error, "could not create the project lock; opening read-only");
                let reason = ReadOnlyReason::Unwritable {
                    path: path_string(parent_of(path)),
                };
                let message = read_only_message(&reason);
                ProjectAccess {
                    lock: None,
                    read_only: Some(reason),
                    notices: vec![notice(
                        NoticeLevel::Warning,
                        "read_only_unwritable",
                        message,
                    )],
                }
            }
        }
    }

    async fn load_project(&self, path: &Path) -> Result<(Loaded, ProjectAccess), CommandError> {
        self.load_project_with(path, |path, backups| {
            rustcut_project::load_with_recovery(&path, &backups)
        })
        .await
    }

    /// Keep access acquisition and loading in one operation; inject only the disk loader in tests.
    async fn load_project_with(
        &self,
        path: &Path,
        load: impl FnOnce(PathBuf, BackupStore) -> Result<Loaded, rustcut_project::error::PersistError>
        + Send
        + 'static,
    ) -> Result<(Loaded, ProjectAccess), CommandError> {
        let access = self.project_access(path).await;
        let (p, backups) = (
            path.to_path_buf(),
            BackupStore::in_data_dir(&self.inner.paths.data),
        );
        let loaded = tokio::task::spawn_blocking(move || load(p, backups))
            .await
            .map_err(|e| join_error(&e))??;
        Ok((loaded, access))
    }

    async fn relink_candidate(&self, dirs: Vec<PathBuf>, source: &SourceInfo) -> Option<PathBuf> {
        let fingerprint = source.fingerprint.clone();
        tokio::task::spawn_blocking(move || {
            dirs.iter()
                .find_map(|d| find_relink_candidate(d, &fingerprint))
        })
        .await
        .ok()
        .flatten()
    }

    fn location_of(&self, project_path: &Path) -> ProjectLocation {
        if project_path.starts_with(&self.inner.paths.library) {
            ProjectLocation::AppLibrary
        } else {
            ProjectLocation::NextToVideo
        }
    }

    /// A video was opened (PRJ-02/03).
    async fn plan_video(&self, video: &Path) -> Result<Plan, CommandError> {
        let media = self.media()?;
        let abs = absolute(video);
        let probed = media.probe_source(&abs).await.map_err(media_error)?;
        let data = self.inner.paths.data.clone();
        let existing = find_existing_project(&abs, &probed.source.fingerprint, &data);
        let ProbedSource {
            source,
            mime_type,
            warnings,
        } = probed;

        let Some((project_path, location)) = existing else {
            return Ok(self
                .plan_new_project(&abs, source, mime_type, &warnings)
                .await);
        };
        let (loaded, access) = self.load_project(&project_path).await?;
        let mut project = loaded.project;
        let mut notices = loaded.notices;
        notices.extend(access.notices);
        let mut save_initial = loaded.migrated_from.is_some()
            || loaded.recovered_from.is_some()
            || notices.iter().any(|n| n.code == "partial_read");
        let saved_at = file_time(&project_path);
        let mut read_only = access.read_only.or(loaded.read_only);
        let (status, mime, source_path) =
            match reconcile(&mut project, &project_path, &abs, &source) {
                Some(changed) => {
                    save_initial |= changed;
                    if !project.clips.is_empty() {
                        notices.insert(
                            0,
                            notice(
                                NoticeLevel::Info,
                                "restored",
                                format!(
                                    "Restored your project: {} from your last session.",
                                    plural(project.clips.len(), "clip", "clips")
                                ),
                            ),
                        );
                    }
                    notices.extend(probe_notices(&project.source, &warnings));
                    (SourceStatus::Ok, Some(mime_type), abs)
                }
                None => {
                    // PRJ-04: a different file sits at the recorded path.
                    let suggested = self
                        .relink_candidate(vec![parent_of(&abs)], &project.source)
                        .await;
                    read_only.get_or_insert(ReadOnlyReason::SourceMissing);
                    notices.push(changed_source_notice(&abs));
                    (
                        SourceStatus::Missing {
                            suggested_path: suggested.map(path_string),
                        },
                        None,
                        abs,
                    )
                }
            };
        Ok(Plan {
            lock: access.lock,
            project,
            project_path,
            location,
            notices,
            read_only,
            source_path,
            source_status: status,
            mime,
            save_initial,
            saved_at,
        })
    }

    async fn plan_new_project(
        &self,
        abs: &Path,
        mut source: SourceInfo,
        mime: String,
        warnings: &[String],
    ) -> Plan {
        let settings = self.settings();
        let (project_path, location) = resolve_new_project_location(
            abs,
            &source.fingerprint,
            settings.project_location,
            &self.inner.paths.data,
            &dir_is_writable,
        );
        source.path = path_string(abs.to_path_buf());
        source.relative_path = relative_source_path(&project_path, abs);
        let mut project = Project::new(source);
        project.export_settings.spaces_to_underscores = settings.default_spaces_to_underscores;
        let mut notices = vec![notice(
            NoticeLevel::Info,
            "created",
            format!("Created a new project for {}.", project.source.file_name()),
        )];
        if location == ProjectLocation::AppLibrary {
            let why = if settings.project_location == ProjectLocation::AppLibrary {
                "as set in your preferences"
            } else {
                "because the folder of the video is not writable"
            };
            notices.push(notice(
                NoticeLevel::Info,
                "stored_in_library",
                format!(
                    "The project is stored in the RustCut library {why}: {}",
                    project_path.display()
                ),
            ));
        }
        notices.extend(probe_notices(&project.source, warnings));
        let access = self.project_access(&project_path).await;
        notices.extend(access.notices);
        Plan {
            lock: access.lock,
            project,
            project_path,
            location,
            notices,
            read_only: access.read_only,
            source_path: abs.to_path_buf(),
            source_status: SourceStatus::Ok,
            mime: Some(mime),
            save_initial: true,
            saved_at: None,
        }
    }

    /// A `.rustcut` file was opened (PRJ-03/04).
    async fn plan_project_file(&self, project_path: &Path) -> Result<Plan, CommandError> {
        let project_path = absolute(project_path);
        let (loaded, access) = self.load_project(&project_path).await?;
        let mut project = loaded.project;
        let mut notices = loaded.notices;
        notices.extend(access.notices);
        let mut save_initial = loaded.migrated_from.is_some()
            || loaded.recovered_from.is_some()
            || notices.iter().any(|n| n.code == "partial_read");
        let mut read_only = access.read_only.or(loaded.read_only);
        let location = self.location_of(&project_path);
        let saved_at = file_time(&project_path);
        let Some(found) = resolve_source(&project_path, &project.source) else {
            let recorded = PathBuf::from(&project.source.path);
            let dirs = vec![parent_of(&recorded), parent_of(&project_path)];
            let suggested = self.relink_candidate(dirs, &project.source).await;
            read_only.get_or_insert(ReadOnlyReason::SourceMissing);
            notices.push(notice(
                NoticeLevel::Warning,
                "source_missing",
                format!(
                    "The source video was not found at {}. Use Locate Missing Media to relink it; the queue is read-only until then.",
                    recorded.display()
                ),
            ));
            return Ok(Plan {
                lock: access.lock,
                project,
                project_path,
                location,
                notices,
                read_only,
                source_path: recorded,
                source_status: SourceStatus::Missing {
                    suggested_path: suggested.map(path_string),
                },
                mime: None,
                save_initial,
                saved_at,
            });
        };
        let found = absolute(&found);
        let probed = self
            .media()?
            .probe_source(&found)
            .await
            .map_err(media_error)?;
        let (status, mime) = match reconcile(&mut project, &project_path, &found, &probed.source) {
            Some(changed) => {
                save_initial |= changed;
                if !project.clips.is_empty() {
                    notices.insert(
                        0,
                        notice(
                            NoticeLevel::Info,
                            "restored",
                            format!(
                                "Restored your project: {} from your last session.",
                                plural(project.clips.len(), "clip", "clips")
                            ),
                        ),
                    );
                }
                notices.extend(probe_notices(&project.source, &probed.warnings));
                (SourceStatus::Ok, Some(probed.mime_type))
            }
            None => {
                let suggested = self
                    .relink_candidate(vec![parent_of(&found)], &project.source)
                    .await;
                read_only.get_or_insert(ReadOnlyReason::SourceMissing);
                notices.push(changed_source_notice(&found));
                (
                    SourceStatus::Missing {
                        suggested_path: suggested.map(path_string),
                    },
                    None,
                )
            }
        };
        Ok(Plan {
            lock: access.lock,
            project,
            project_path,
            location,
            notices,
            read_only,
            source_path: found,
            source_status: status,
            mime,
            save_initial,
            saved_at,
        })
    }

    /// Create the session, save when needed, remember it as recent and start the
    /// background jobs.
    async fn finish_open(&self, mut plan: Plan) -> Result<OpenResult, CommandError> {
        let read_only = plan.read_only.take();
        let mut notices = std::mem::take(&mut plan.notices);
        if let Some(reason @ ReadOnlyReason::NewerSchema { .. }) = &read_only
            && !notices.iter().any(|n| n.code.starts_with("read_only"))
        {
            notices.push(notice(
                NoticeLevel::Warning,
                "read_only_newer_schema",
                read_only_message(reason),
            ));
        }

        let mut store = ProjectStore::new(plan.project, read_only.clone());
        for n in store.normalize_on_load() {
            if !notices.iter().any(|existing| existing.code == n.code) {
                notices.push(n);
            }
        }
        let stray = store.project().out_of_range_ids().len();
        if stray > 0 && !notices.iter().any(|n| n.code == "out_of_range_clips") {
            notices.push(notice(
                NoticeLevel::Warning,
                "out_of_range_clips",
                format!(
                    "{} after the end of this video and will be skipped on export.",
                    if stray == 1 {
                        "1 clip starts".to_string()
                    } else {
                        format!("{stray} clips start")
                    }
                ),
            ));
        }

        let tracker = Arc::new(SaveTracker::new(
            plan.project_path.clone(),
            plan.location,
            plan.saved_at,
        ));
        let saver = AutoSaver::spawn(
            &tokio::runtime::Handle::current(),
            Some(BackupStore::in_data_dir(&self.inner.paths.data)),
            Self::saved_callback(self.inner.sink.clone(), tracker.clone()),
        );
        let session = Arc::new(Session {
            store: Mutex::new(store),
            project_path: plan.project_path.clone(),
            location: plan.location,
            source_path: Mutex::new(plan.source_path.clone()),
            source_status: Mutex::new(plan.source_status.clone()),
            mime: Mutex::new(plan.mime.clone().unwrap_or_default()),
            saver,
            tracker,
            lock: Mutex::new(plan.lock),
            index: Mutex::new(None),
            jobs: Mutex::new(Jobs::default()),
            playback: Mutex::new(None),
        });

        if plan.save_initial && read_only.is_none() {
            let (project, revision) = {
                let store = lk(&session.store);
                (store.project().clone(), store.revision())
            };
            session.tracker.note_scheduled(revision);
            session.saver.schedule(
                plan.project_path.clone(),
                project,
                revision,
                Urgency::Immediate,
            );
            if let Some(outcome) = session.saver.flush().await
                && let Some(error) = outcome.error
            {
                // The previous session is already closed; keep this one open (in memory)
                // and tell the user instead of leaving the app without a project.
                tracing::warn!(%error, "initial save failed");
                notices.push(notice(
                    NoticeLevel::Warning,
                    "initial_save_failed",
                    format!("Could not save the project yet: {error}. It will be retried."),
                ));
            }
        }
        self.remember_recent(&session);
        *lk(&self.inner.session) = Some(session.clone());

        let source_ok = matches!(plan.source_status, SourceStatus::Ok);
        let playback = if source_ok {
            self.inner.platform.allow_asset(&plan.source_path);
            self.start_jobs(&session);
            let source = lk(&session.store).project().source.clone();
            Some(rustcut_media::initial_playback(&source, &lk(&session.mime)))
        } else {
            None
        };
        tracing::debug!(project = %plan.project_path.display(), read_only = ?read_only, "project opened");
        Ok(OpenResult {
            snapshot: session.snapshot(),
            playback,
            notices,
        })
    }

    fn remember_recent(&self, session: &Session) {
        if !session.project_path.is_file() {
            return;
        }
        let (source, clip_count) = {
            let store = lk(&session.store);
            (store.project().source.clone(), store.project().clips.len())
        };
        let entry = RecentProject {
            project_path: path_string(session.project_path.clone()),
            source_path: source.path.clone(),
            source_name: source.file_name(),
            opened_at: now_utc(),
            clip_count: u32::try_from(clip_count).unwrap_or(u32::MAX),
            location: session.location,
        };
        if let Err(e) = self.inner.recents.add(entry) {
            tracing::warn!(error = %e, "could not update the recent projects list");
        }
    }

    /// `relink_source` (PRJ-04).
    ///
    /// # Errors
    /// [`ErrorCode::ReadOnly`] when the project is open read-only for another reason,
    /// [`ErrorCode::Busy`] while exporting, probe errors for an unreadable file.
    pub async fn relink_source(
        &self,
        path: &str,
        accept_mismatch: bool,
    ) -> Result<RelinkResult, CommandError> {
        let _guard = self.inner.open_lock.lock().await;
        let session = self.session()?;
        {
            let store = lk(&session.store);
            if let Some(
                reason @ (ReadOnlyReason::OpenElsewhere { .. }
                | ReadOnlyReason::NewerSchema { .. }
                | ReadOnlyReason::Unwritable { .. }),
            ) = store.read_only()
            {
                return Err(CommandError::new(
                    ErrorCode::ReadOnly,
                    read_only_message(reason),
                ));
            }
        }
        if lk(&self.inner.engine)
            .as_ref()
            .is_some_and(rustcut_export::ExportEngine::is_running)
        {
            return Err(CommandError::new(
                ErrorCode::Busy,
                "An export is running. Wait for it to finish or cancel it before locating the video again.",
            ));
        }
        let new_path = absolute(Path::new(path));
        if !new_path.is_file() {
            return Err(CommandError::new(
                ErrorCode::NotFound,
                format!(
                    "\"{}\" was not found. Pick the video file itself.",
                    new_path.display()
                ),
            ));
        }
        let probed = self
            .media()?
            .probe_source(&new_path)
            .await
            .map_err(media_error)?;
        let (expected, same, out_of_range) = {
            let store = lk(&session.store);
            let current = &store.project().source;
            (
                current.fingerprint.clone(),
                current.fingerprint == probed.source.fingerprint,
                store
                    .clips_out_of_range_for(probed.source.duration_sec)
                    .len(),
            )
        };
        if !same && !accept_mismatch {
            return Ok(RelinkResult::Mismatch {
                expected,
                found: probed.source.fingerprint,
                found_duration_sec: probed.source.duration_sec,
                clips_out_of_range: u32::try_from(out_of_range).unwrap_or(u32::MAX),
            });
        }

        let ProbedSource {
            mut source,
            mime_type,
            warnings,
        } = probed;
        source.path = path_string(new_path.clone());
        source.relative_path = relative_source_path(&session.project_path, &new_path);
        lk(&session.jobs).cancel_all();
        let released = lk(&session.playback).take();
        if let Some(p) = released {
            self.media()?.release_playback(&p);
        }
        let stray = {
            let mut store = lk(&session.store);
            let stray = store.relink(source.clone());
            session.schedule_save(&mut store);
            stray
        };
        *lk(&session.index) = None;
        *lk(&session.source_path) = new_path.clone();
        *lk(&session.source_status) = SourceStatus::Ok;
        *lk(&session.mime) = mime_type.clone();
        // Never forbid the old path: Tauri's forbidden patterns are permanent and always win,
        // so relinking back (or to another spelling of the path) would block the new source.
        self.inner.platform.allow_asset(&new_path);
        self.start_jobs(&session);
        self.remember_recent(&session);

        let mut notices = vec![notice(
            NoticeLevel::Success,
            "relinked",
            format!("The project now uses {}.", source.file_name()),
        )];
        if !same {
            notices.push(notice(
                NoticeLevel::Warning,
                "source_changed",
                "This video differs from the one the project was made from. Check your clips before exporting.",
            ));
        }
        if !stray.is_empty() {
            notices.push(notice(
                NoticeLevel::Warning,
                "out_of_range_clips",
                format!(
                    "{} after the end of this video and will be skipped on export.",
                    if stray.len() == 1 {
                        "1 clip starts".to_string()
                    } else {
                        format!("{} clips start", stray.len())
                    }
                ),
            ));
        }
        notices.extend(probe_notices(&source, &warnings));
        let snapshot = session.snapshot();
        self.emit(rustcut_model::AppEvent::Snapshot {
            snapshot: snapshot.clone(),
        });
        let playback = Some(rustcut_media::initial_playback(&source, &mime_type));
        Ok(RelinkResult::Relinked {
            result: OpenResult {
                snapshot,
                playback,
                notices,
            },
        })
    }
}

fn parent_of(path: &Path) -> PathBuf {
    path.parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn path_string(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

fn changed_source_notice(found: &Path) -> Notice {
    notice(
        NoticeLevel::Warning,
        "source_changed",
        format!(
            "The video at {} is not the file this project was made from (its size or content changed, for example after a re-record). The project is read-only until you pick the right video with Locate Missing Media.",
            found.display()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::CoreConfig;
    use crate::paths::AppPaths;
    use crate::testing::{NullPlatform, VecSink};

    #[tokio::test]
    async fn single_writer_lock_precedes_loading_and_lives_until_session_handoff() {
        let root = rustcut_testkit::temp_dir();
        let paths = AppPaths::under(&root);
        paths.ensure().expect("app folders");
        let core = AppCore::new(CoreConfig {
            paths,
            sidecars: Err("unused in project loader test".into()),
            sink: Arc::new(VecSink::default()),
            platform: Arc::new(NullPlatform::default()),
        });
        let path = root.join("locked-before-load.rustcut");
        std::fs::write(
            &path,
            include_bytes!("../../../crates/rustcut-project/tests/fixtures/v1_project.rustcut"),
        )
        .expect("project fixture");
        let (_, access) = core
            .load_project_with(&path, |path, backups| {
                assert!(matches!(
                    ProjectLock::try_acquire(&path).expect("try lock before reading"),
                    LockOutcome::HeldElsewhere { .. }
                ));
                rustcut_project::load_with_recovery(&path, &backups)
            })
            .await
            .expect("load");
        assert!(access.read_only.is_none());
        assert!(matches!(
            ProjectLock::try_acquire(&path).expect("lock during handoff"),
            LockOutcome::HeldElsewhere { .. }
        ));
        drop(access);
        assert!(matches!(
            ProjectLock::try_acquire(&path).expect("released"),
            LockOutcome::Acquired(_)
        ));
        std::fs::write(&path, b"invalid project").expect("corrupt");
        assert!(core.load_project(&path).await.is_err());
        assert!(matches!(
            ProjectLock::try_acquire(&path).expect("released on failed load"),
            LockOutcome::Acquired(_)
        ));
    }
}
