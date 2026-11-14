//! Background media work of the open project: frame index (PLY-06), growth watch (PRD 9),
//! waveform (PLY-05) and playback preparation (PLY-02..04, section 7.3).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rustcut_media::{CancelToken, MediaError, proxy_mime, remux_mime, watch_growth};
use rustcut_model::{
    AppEvent, CommandError, ErrorCode, Notice, NoticeLevel, PlaybackInfo, PlaybackTier,
    SourceStatus,
};

use super::session::{IndexEntry, Session};
use super::{AppCore, lk};
use crate::error::media_error;

/// How often the source file's size is checked for growth.
const GROWTH_POLL: Duration = Duration::from_secs(2);

fn toast(level: NoticeLevel, code: &str, message: impl Into<String>) -> AppEvent {
    AppEvent::Toast {
        notice: Notice::new(level, code, message),
    }
}

/// How much of the index build [`AppCore::spawn_index`] may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexBuild {
    /// The cache or the MP4/MOV sample tables only (at most the `moov` box), safe to run while
    /// the first frame loads.
    Quick,
    /// Also the FFprobe packet scan, which reads the whole file (AC-14: after the first frame).
    Full,
}

impl AppCore {
    /// Start the background jobs of a session whose source is present. The index starts with
    /// [`IndexBuild::Quick`]; a file that needs a packet scan waits for `start_media_index`.
    pub(crate) fn start_jobs(&self, session: &Arc<Session>) {
        self.spawn_index(session, IndexBuild::Quick);
        self.spawn_growth_watch(session);
    }

    /// `start_media_index`: build the index with a packet scan if the open could not build it
    /// quickly (no sample tables, e.g. Matroska). The UI calls it once the first frame is on
    /// screen, so the scan does not read the file before it (AC-14). Does nothing while the
    /// index exists or a full build runs.
    ///
    /// # Errors
    /// No project, or [`ErrorCode::SourceMissing`].
    pub fn start_media_index(&self) -> Result<(), CommandError> {
        let session = self.session()?;
        Self::require_source(&session)?;
        // One lock at a time (the guards of a single condition would overlap).
        let built = lk(&session.index).is_some();
        let full_build_running = lk(&session.jobs).index_full;
        if !built && !full_build_running {
            self.spawn_index(&session, IndexBuild::Full);
        }
        Ok(())
    }

    /// PLY-06: build the keyframe/frame index and announce it with `AppEvent::MediaIndex`.
    fn spawn_index(&self, session: &Arc<Session>, build: IndexBuild) {
        let Some(media) = self.inner.media.clone() else {
            return;
        };
        let path = lk(&session.source_path).clone();
        let source = lk(&session.store).project().source.clone();
        let cancel = CancelToken::new();
        {
            let mut jobs = lk(&session.jobs);
            jobs.index.cancel();
            jobs.index = cancel.clone();
            jobs.index_full = build == IndexBuild::Full;
        }
        let (session, sink) = (session.clone(), self.inner.sink.clone());
        tokio::spawn(async move {
            let result = match build {
                IndexBuild::Quick => Ok(media.quick_index(&path, &source).await),
                IndexBuild::Full => media
                    .build_index_cancellable(&path, &source, |_| {}, cancel.clone())
                    .await
                    .map(Some),
            };
            match result {
                Ok(Some(built)) => {
                    if cancel.is_cancelled() {
                        return;
                    }
                    let info = built.index.to_info(built.source, true);
                    *lk(&session.index) = Some(IndexEntry {
                        index: built.index,
                        info: info.clone(),
                    });
                    sink.send(AppEvent::MediaIndex { info });
                }
                // Only the packet scan can index this file; `start_media_index` runs it.
                Ok(None) => {}
                Err(MediaError::Cancelled) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "frame index failed");
                    sink.send(toast(
                        NoticeLevel::Warning,
                        "index_failed",
                        format!("Frame-accurate stepping may be less precise: {e}"),
                    ));
                }
            }
        });
    }

    /// PRD 9: warn once when the source file changes size during the session.
    fn spawn_growth_watch(&self, session: &Arc<Session>) {
        let path = lk(&session.source_path).clone();
        let sink = self.inner.sink.clone();
        let warned = Arc::new(AtomicBool::new(false));
        let watch = watch_growth(path, GROWTH_POLL, move |_change| {
            if !warned.swap(true, Ordering::SeqCst) {
                sink.send(toast(
                    NoticeLevel::Warning,
                    "source_growing",
                    "The video file is changing size, so it may still be recording. Growing files are not supported in this version: the preview and exports may end early. Wait until the recording has finished and reopen the video.",
                ));
            }
        });
        lk(&session.jobs).growth = Some(watch);
    }

    fn require_source(session: &Session) -> Result<(), CommandError> {
        if matches!(*lk(&session.source_status), SourceStatus::Ok) {
            Ok(())
        } else {
            Err(CommandError::new(
                ErrorCode::SourceMissing,
                "The source video is missing. Use Locate Missing Media to find it first.",
            ))
        }
    }

    /// `start_waveform`: peaks stream as `AppEvent::Waveform` chunks (track is 1-based).
    ///
    /// # Errors
    /// [`ErrorCode::SourceMissing`], [`ErrorCode::InvalidInput`] for a track the video lacks.
    pub fn start_waveform(&self, track: u32) -> Result<(), CommandError> {
        let session = self.session()?;
        Self::require_source(&session)?;
        let media = self.media()?.clone();
        let path = lk(&session.source_path).clone();
        let source = lk(&session.store).project().source.clone();
        let available = source.audio_tracks.len();
        if available > 0 && (track == 0 || track as usize > available) {
            return Err(CommandError::new(
                ErrorCode::InvalidInput,
                format!(
                    "This video has {available} audio track(s); track {track} does not exist. Choose a track between 1 and {available}."
                ),
            ));
        }
        let cancel = CancelToken::new();
        if let Some(previous) = lk(&session.jobs).waveforms.insert(track, cancel.clone()) {
            previous.cancel();
        }
        let sink = self.inner.sink.clone();
        tokio::spawn(async move {
            let chunk_sink = sink.clone();
            let result = media
                .generate_waveform(
                    &path,
                    &source,
                    track,
                    move |chunk| chunk_sink.send(AppEvent::Waveform { chunk }),
                    cancel,
                )
                .await;
            match result {
                Ok(()) | Err(MediaError::Cancelled) => {}
                Err(e) => {
                    tracing::warn!(error = %e, track, "waveform failed");
                    sink.send(toast(
                        NoticeLevel::Warning,
                        "waveform_failed",
                        e.to_string(),
                    ));
                }
            }
        });
        Ok(())
    }

    /// `request_playback`: Direct returns the source; Remux/Proxy prepare a cache file with
    /// progress events. A tier that cannot be prepared returns `unsupported_reason` and the
    /// next tier to try instead of an error.
    ///
    /// # Errors
    /// [`ErrorCode::SourceMissing`] while the source is missing.
    pub async fn request_playback(&self, tier: PlaybackTier) -> Result<PlaybackInfo, CommandError> {
        let session = self.session()?;
        Self::require_source(&session)?;
        let media = self.media()?.clone();
        let path = lk(&session.source_path).clone();
        let source = lk(&session.store).project().source.clone();
        let (mime, next_tier) = match tier {
            PlaybackTier::Direct => (lk(&session.mime).clone(), Some(PlaybackTier::Remux)),
            PlaybackTier::Remux => (remux_mime(&source), Some(PlaybackTier::Proxy)),
            PlaybackTier::Proxy => (proxy_mime(&source), None),
        };
        let info = |media_path: Option<String>,
                    preparing: bool,
                    progress: Option<f64>,
                    eta: Option<f64>,
                    why: Option<String>| {
            PlaybackInfo {
                tier,
                media_path,
                mime_type: mime.clone(),
                preparing,
                progress,
                eta_sec: eta,
                next_tier,
                unsupported_reason: why,
            }
        };
        if tier == PlaybackTier::Direct {
            self.release_playback();
            let done = info(
                Some(path.to_string_lossy().into_owned()),
                false,
                None,
                None,
                None,
            );
            self.emit(AppEvent::Playback { info: done.clone() });
            return Ok(done);
        }

        let cancel = CancelToken::new();
        {
            let mut jobs = lk(&session.jobs);
            jobs.playback.cancel();
            jobs.playback = cancel.clone();
        }
        self.emit(AppEvent::Playback {
            info: info(None, true, None, None, None),
        });
        let (sink, progress_mime) = (self.inner.sink.clone(), mime.clone());
        let on_progress = move |fraction: f64, eta: Option<f64>| {
            sink.send(AppEvent::Playback {
                info: PlaybackInfo {
                    tier,
                    media_path: None,
                    mime_type: progress_mime.clone(),
                    preparing: true,
                    progress: Some(fraction.clamp(0.0, 1.0)),
                    eta_sec: eta,
                    next_tier,
                    unsupported_reason: None,
                },
            });
        };
        let result = media
            .prepare(tier, &path, &source, on_progress, cancel)
            .await;
        let done = match result {
            Ok(out) => {
                let previous = lk(&session.playback).replace(out.clone());
                if let Some(old) = previous
                    && old != out
                {
                    media.release_playback(&old);
                }
                info(
                    Some(out.to_string_lossy().into_owned()),
                    false,
                    Some(1.0),
                    None,
                    None,
                )
            }
            Err(MediaError::Cancelled) => info(
                None,
                false,
                None,
                None,
                Some("Preparing the preview was cancelled.".into()),
            ),
            Err(e) => {
                tracing::warn!(error = %e, ?tier, "playback preparation failed");
                info(None, false, None, None, Some(media_error(e).message))
            }
        };
        self.emit(AppEvent::Playback { info: done.clone() });
        Ok(done)
    }

    /// `release_playback`: the player no longer uses the prepared cache file (it may be evicted).
    pub fn release_playback(&self) {
        let Ok(session) = self.session() else {
            return;
        };
        lk(&session.jobs).playback.cancel();
        let released = lk(&session.playback).take();
        if let (Some(path), Some(media)) = (released, self.inner.media.as_ref()) {
            media.release_playback(&path);
        }
    }
}
