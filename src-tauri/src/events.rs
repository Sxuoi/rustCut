//! Delivery of [`AppEvent`]s to the WebView (ARCHITECTURE section 4).

use std::sync::{Mutex, MutexGuard};

use rustcut_model::AppEvent;
use tauri::ipc::Channel;

/// Where the core sends events. The Tauri build uses [`ChannelSink`]; tests use
/// [`crate::testing::VecSink`].
pub trait EventSink: Send + Sync {
    /// Deliver one event. Never blocks for long and never fails the caller.
    fn send(&self, event: AppEvent);
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Most events held while no channel is subscribed (startup notices).
const BACKLOG_LIMIT: usize = 64;

#[derive(Default)]
struct ChannelState {
    channel: Option<Channel<AppEvent>>,
    backlog: Vec<AppEvent>,
}

/// An [`EventSink`] over the `subscribe_events` channel. A newer subscription
/// replaces the previous one; events sent before the first subscription are
/// kept (bounded) and delivered when it arrives.
#[derive(Default)]
pub struct ChannelSink {
    state: Mutex<ChannelState>,
}

impl ChannelSink {
    /// `subscribe_events`: replace the channel and flush the backlog.
    pub fn replace(&self, channel: Channel<AppEvent>) {
        let mut state = lock(&self.state);
        for event in std::mem::take(&mut state.backlog) {
            if let Err(e) = channel.send(event) {
                tracing::debug!(error = %e, "could not deliver a queued event");
            }
        }
        state.channel = Some(channel);
    }
}

impl EventSink for ChannelSink {
    fn send(&self, event: AppEvent) {
        let mut state = lock(&self.state);
        match state.channel.as_ref() {
            Some(channel) => {
                if let Err(e) = channel.send(event) {
                    tracing::debug!(error = %e, "could not deliver an event (the WebView may be reloading)");
                }
            }
            None => {
                if state.backlog.len() < BACKLOG_LIMIT {
                    state.backlog.push(event);
                }
            }
        }
    }
}
