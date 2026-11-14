//! The open project: store, auto-saver, lock and background jobs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rustcut_ffmpeg::MediaIndex;
use rustcut_media::{CancelToken, GrowthWatch};
use rustcut_model::{
    AppEvent, MediaIndexInfo, ProjectLocation, ProjectSnapshot, SaveState, SourceStatus,
};
use rustcut_project::{AutoSaver, ProjectLock, ProjectStore};

use super::{AppCore, lk};
use crate::events::EventSink;

/// Save progress shared with the auto-saver callback (SaveState for the UI).
pub(crate) struct SaveTracker {
    project_path: PathBuf,
    location: ProjectLocation,
    saved_at: Mutex<Option<DateTime<Utc>>>,
    error: Mutex<Option<String>>,
    /// Highest revision handed to the saver.
    scheduled: AtomicU64,
    /// Highest revision written successfully.
    saved: AtomicU64,
}

impl SaveTracker {
    pub(crate) fn new(
        project_path: PathBuf,
        location: ProjectLocation,
        saved_at: Option<DateTime<Utc>>,
    ) -> Self {
        SaveTracker {
            project_path,
            location,
            saved_at: Mutex::new(saved_at),
            error: Mutex::new(None),
            scheduled: AtomicU64::new(0),
            saved: AtomicU64::new(0),
        }
    }

    /// Current state for snapshots and `Saved` events.
    pub(crate) fn save_state(&self) -> SaveState {
        SaveState {
            saved_at: *lk(&self.saved_at),
            pending: self.scheduled.load(Ordering::SeqCst) > self.saved.load(Ordering::SeqCst),
            error: lk(&self.error).clone(),
            project_path: Some(self.project_path.to_string_lossy().into_owned()),
            location: self.location,
        }
    }

    pub(crate) fn note_scheduled(&self, revision: u64) {
        self.scheduled.fetch_max(revision, Ordering::SeqCst);
    }

    /// Apply an auto-saver outcome.
    pub(crate) fn note_outcome(
        &self,
        revision: u64,
        saved_at: Option<DateTime<Utc>>,
        error: Option<String>,
    ) {
        if error.is_none() {
            self.saved.fetch_max(revision, Ordering::SeqCst);
            if let Some(at) = saved_at {
                *lk(&self.saved_at) = Some(at);
            }
        }
        *lk(&self.error) = error;
    }
}

/// The built frame index and its UI summary.
pub(crate) struct IndexEntry {
    pub index: MediaIndex,
    pub info: MediaIndexInfo,
}

/// Cancellation handles of the background jobs of a session.
#[derive(Default)]
pub(crate) struct Jobs {
    pub index: CancelToken,
    pub playback: CancelToken,
    pub waveforms: HashMap<u32, CancelToken>,
    pub growth: Option<GrowthWatch>,
}

impl Jobs {
    /// Cancel everything that runs for the source (also used when the source is relinked).
    pub(crate) fn cancel_all(&mut self) {
        self.index.cancel();
        self.playback.cancel();
        for token in self.waveforms.values() {
            token.cancel();
        }
        self.waveforms.clear();
        if let Some(watch) = self.growth.take() {
            watch.stop();
        }
    }
}

/// One open project (PRJ-02..06).
pub struct Session {
    pub(crate) store: Mutex<ProjectStore>,
    pub(crate) project_path: PathBuf,
    pub(crate) location: ProjectLocation,
    pub(crate) source_path: Mutex<PathBuf>,
    pub(crate) source_status: Mutex<SourceStatus>,
    pub(crate) mime: Mutex<String>,
    pub(crate) saver: AutoSaver,
    pub(crate) tracker: Arc<SaveTracker>,
    pub(crate) lock: Mutex<Option<Arc<ProjectLock>>>,
    pub(crate) index: Mutex<Option<IndexEntry>>,
    pub(crate) jobs: Mutex<Jobs>,
    /// Cache file the player has open (pinned against eviction).
    pub(crate) playback: Mutex<Option<PathBuf>>,
}

impl Session {
    /// Hand the unsaved state (if any) to the auto-saver.
    pub(crate) fn schedule_save(&self, store: &mut ProjectStore) {
        if let Some(req) = store.take_save_request() {
            self.tracker.note_scheduled(req.revision);
            self.saver.schedule(
                self.project_path.clone(),
                req.project,
                req.revision,
                req.urgency,
            );
        }
    }

    /// Queue pending changes and wait until they are on disk. Returns the error of a failed
    /// save.
    pub(crate) async fn flush(&self) -> Option<String> {
        {
            let mut store = lk(&self.store);
            self.schedule_save(&mut store);
        }
        self.saver.flush().await.and_then(|o| o.error)
    }

    /// Full state for the UI.
    pub(crate) fn snapshot(&self) -> ProjectSnapshot {
        let status = lk(&self.source_status).clone();
        lk(&self.store).snapshot(self.tracker.save_state(), status)
    }
}

impl AppCore {
    /// Cancel jobs, stop exports, flush saves, release the lock and drop the session.
    pub(crate) async fn close_session(&self) {
        let Some(session) = lk(&self.inner.session).take() else {
            return;
        };
        lk(&session.jobs).cancel_all();
        let engine = lk(&self.inner.engine).clone();
        if let Some(engine) = engine
            && engine.is_running()
        {
            engine.cancel_all();
            // The cancelled clips report back and are applied to the store before the flush.
            for _ in 0..200 {
                if !engine.is_running() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            if engine.is_running() {
                tracing::warn!("an export did not stop within 10 s of closing the project");
            }
        }
        session.flush().await;
        let playback = lk(&session.playback).take();
        if let (Some(path), Some(media)) = (playback, self.inner.media.as_ref()) {
            media.release_playback(&path);
        }
        // Releases the project lock (PRJ-05).
        *lk(&session.lock) = None;
        tracing::debug!(project = %session.project_path.display(), "project closed");
    }

    /// `close_project`.
    pub async fn close_project(&self) {
        let _guard = self.inner.open_lock.lock().await;
        self.close_session().await;
    }

    /// Send a `Saved` event for an auto-saver outcome.
    pub(crate) fn saved_callback(
        sink: Arc<dyn EventSink>,
        tracker: Arc<SaveTracker>,
    ) -> rustcut_project::SaveCallback {
        Arc::new(move |outcome| {
            if let Some(error) = &outcome.error {
                tracing::warn!(%error, "auto-save failed; it will be retried");
            }
            tracker.note_outcome(outcome.revision, outcome.saved_at, outcome.error);
            sink.send(AppEvent::Saved {
                save: tracker.save_state(),
            });
        })
    }
}
