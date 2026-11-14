//! Growing-file watch (PRD section 9: "File still growing (recording in progress): not supported
//! in v1; a size change during the session shows a warning").

use std::path::PathBuf;
use std::time::Duration;

use rustcut_runner::CancelToken;
use tokio::task::JoinHandle;

/// A size change seen by [`watch_growth`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrowthChange {
    /// Size at the previous check.
    pub previous_bytes: u64,
    /// Size now.
    pub current_bytes: u64,
}

/// Handle of a running watch. Dropping it stops the watch.
#[derive(Debug)]
pub struct GrowthWatch {
    cancel: CancelToken,
    handle: Option<JoinHandle<()>>,
}

impl GrowthWatch {
    /// Stops the watch; no callback runs after the task has observed this (at most one interval).
    pub fn stop(&self) {
        self.cancel.cancel();
    }

    /// Stops the watch and waits for the task to end.
    pub async fn stop_and_join(mut self) {
        self.cancel.cancel();
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }

    /// Whether the task has ended.
    pub fn is_finished(&self) -> bool {
        self.handle.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for GrowthWatch {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Polls the size of `path` every `interval` and calls `on_change` whenever it differs from the
/// previous check (the size at the call is the baseline). A file that cannot be read (moved,
/// being replaced) is skipped until it can; relocation is PRJ-04's concern.
///
/// Must be called inside a tokio runtime.
pub fn watch_growth(
    path: impl Into<PathBuf>,
    interval: Duration,
    on_change: impl Fn(GrowthChange) + Send + 'static,
) -> GrowthWatch {
    let path = path.into();
    let cancel = CancelToken::new();
    let token = cancel.clone();
    let mut last = std::fs::metadata(&path).ok().map(|m| m.len());
    let interval = interval.max(Duration::from_millis(10));
    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                () = token.cancelled() => break,
                () = tokio::time::sleep(interval) => {}
            }
            let Ok(meta) = tokio::fs::metadata(&path).await else {
                continue;
            };
            let now = meta.len();
            match last {
                Some(prev) if prev != now => {
                    last = Some(now);
                    on_change(GrowthChange {
                        previous_bytes: prev,
                        current_bytes: now,
                    });
                }
                None => last = Some(now),
                _ => {}
            }
        }
    });
    GrowthWatch {
        cancel,
        handle: Some(handle),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn reports_growth_and_stops_when_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rec.mp4");
        std::fs::write(&path, [0u8; 100]).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let watch = watch_growth(&path, Duration::from_millis(20), move |c| {
            sink.lock().unwrap().push(c);
        });
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(seen.lock().unwrap().is_empty(), "no change, no report");
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(&[1u8; 50]).unwrap();
        f.flush().unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            *seen.lock().unwrap(),
            vec![GrowthChange {
                previous_bytes: 100,
                current_bytes: 150
            }]
        );
        watch.stop_and_join().await;
        f.write_all(&[1u8; 50]).unwrap();
        f.flush().unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(seen.lock().unwrap().len(), 1, "stopped watches stay silent");
    }
}
