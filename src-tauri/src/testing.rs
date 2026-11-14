//! In-memory [`EventSink`] and [`Platform`] used by the integration tests (and handy for
//! driving the core without a WebView).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rustcut_model::AppEvent;

use crate::events::EventSink;
use crate::platform::Platform;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Collects every event.
#[derive(Default)]
pub struct VecSink {
    events: Mutex<Vec<AppEvent>>,
}

impl VecSink {
    /// All events so far.
    pub fn events(&self) -> Vec<AppEvent> {
        lock(&self.events).clone()
    }

    /// Wait up to `timeout` until an event satisfies `pred`; returns it.
    pub async fn wait_for(
        &self,
        timeout: Duration,
        pred: impl Fn(&AppEvent) -> bool,
    ) -> Option<AppEvent> {
        let end = Instant::now() + timeout;
        loop {
            if let Some(e) = lock(&self.events).iter().find(|e| pred(e)) {
                return Some(e.clone());
            }
            if Instant::now() >= end {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

impl EventSink for VecSink {
    fn send(&self, event: AppEvent) {
        lock(&self.events).push(event);
    }
}

/// One call made on a [`NullPlatform`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformCall {
    /// `allow_asset`.
    Allow(PathBuf),
    /// `forbid_asset`.
    Forbid(PathBuf),
    /// `notify(title, body)`.
    Notify(String, String),
    /// `reveal`.
    Reveal(PathBuf),
    /// `open_dir`.
    OpenDir(PathBuf),
}

/// Records calls and succeeds.
#[derive(Default)]
pub struct NullPlatform {
    calls: Mutex<Vec<PlatformCall>>,
}

impl NullPlatform {
    /// Calls so far.
    pub fn calls(&self) -> Vec<PlatformCall> {
        lock(&self.calls).clone()
    }
    fn push(&self, call: PlatformCall) {
        lock(&self.calls).push(call);
    }
}

impl Platform for NullPlatform {
    fn allow_asset(&self, path: &Path) {
        self.push(PlatformCall::Allow(path.to_path_buf()));
    }
    fn forbid_asset(&self, path: &Path) {
        self.push(PlatformCall::Forbid(path.to_path_buf()));
    }
    fn notify(&self, title: &str, body: &str) {
        self.push(PlatformCall::Notify(title.into(), body.into()));
    }
    fn reveal(&self, path: &Path) -> Result<(), String> {
        self.push(PlatformCall::Reveal(path.to_path_buf()));
        Ok(())
    }
    fn open_dir(&self, path: &Path) -> Result<(), String> {
        self.push(PlatformCall::OpenDir(path.to_path_buf()));
        Ok(())
    }
}
