//! PRJ-01 auto-save: [`AutoSaver`] writes scheduled project snapshots on a
//! tokio task.
//!
//! * A burst of changes is written at most ~1 s after its **first** unsaved
//!   change: each write waits for a [`DEBOUNCE`] pause in the changes but
//!   never longer than [`MAX_DELAY`] after the first one. [`Urgency::Immediate`]
//!   (adds, deletes) writes as soon as the task is free.
//! * Only the newest snapshot per path is written; writes run one at a time
//!   (they never overlap), atomically via [`crate::persist::save_atomic`],
//!   followed by a throttled backup ([`BackupStore::write`]).
//! * Every attempt is reported through the `on_result` callback. A failed
//!   write keeps its snapshot and is retried on the next change for that
//!   path, on [`AutoSaver::flush`], and by itself after a backoff
//!   ([`RETRY_BASE`] doubling up to [`RETRY_MAX`]), so a transient failure
//!   (e.g. a virus scanner holding the file) never strands unsaved edits.
//! * Dropping the `AutoSaver` writes whatever is pending, if the runtime is
//!   still running; call [`AutoSaver::flush`] before shutting down.

use crate::backup::BackupStore;
use crate::persist::save_atomic;
use chrono::{DateTime, Utc};
use rustcut_model::Project;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

/// Quiet period that ends a burst of changes.
pub const DEBOUNCE: Duration = Duration::from_millis(250);
/// Longest wait after the first unsaved change of a burst.
pub const MAX_DELAY: Duration = Duration::from_millis(900);
/// First automatic retry after a failed write.
pub const RETRY_BASE: Duration = Duration::from_millis(500);
/// Longest wait between automatic retries.
pub const RETRY_MAX: Duration = Duration::from_secs(10);

/// How soon a scheduled snapshot must be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Urgency {
    /// Adds and deletes: write now (PRJ-01).
    Immediate,
    /// Everything else: debounced, at most ~1 s after the first change.
    #[default]
    Debounced,
}

/// Result of one write attempt.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveOutcome {
    /// Time of the successful write; `None` when it failed.
    pub saved_at: Option<DateTime<Utc>>,
    /// Revision passed to [`AutoSaver::schedule`] for the written snapshot.
    pub revision: u64,
    /// User-facing error message when the write failed.
    pub error: Option<String>,
    pub path: PathBuf,
}

/// Callback receiving every [`SaveOutcome`] (called on the auto-save task).
pub type SaveCallback = Arc<dyn Fn(SaveOutcome) + Send + Sync>;

enum Command {
    Schedule {
        path: PathBuf,
        project: Box<Project>,
        revision: u64,
        urgency: Urgency,
        at: Instant,
    },
    Flush {
        reply: oneshot::Sender<Option<SaveOutcome>>,
    },
}

/// Handle to the auto-save task. Cheap to call from any thread.
#[derive(Debug, Clone)]
pub struct AutoSaver {
    tx: mpsc::UnboundedSender<Command>,
}

impl AutoSaver {
    /// Start the auto-save task on `handle`. `backups` (if any) receives a
    /// throttled backup after each successful write.
    pub fn spawn(
        handle: &tokio::runtime::Handle,
        backups: Option<BackupStore>,
        on_result: SaveCallback,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        handle.spawn(Worker::new(backups, on_result).run(rx));
        AutoSaver { tx }
    }

    /// Queue `project` (state at `revision`) for writing to `path`. Never
    /// blocks; a newer snapshot for the same path replaces an unwritten one.
    pub fn schedule(&self, path: PathBuf, project: Project, revision: u64, urgency: Urgency) {
        let command = Command::Schedule {
            path,
            project: Box::new(project),
            revision,
            urgency,
            at: Instant::now(),
        };
        if self.tx.send(command).is_err() {
            tracing::warn!("auto-save task is not running; a change was not saved");
        }
    }

    /// Write everything scheduled so far now (retrying failed writes) and
    /// wait for it. Returns the outcome of the last write this needed (a
    /// failed one if any failed); when nothing was pending, the most recent
    /// outcome of any write (`None` if nothing was ever written).
    pub async fn flush(&self) -> Option<SaveOutcome> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Command::Flush { reply }).ok()?;
        rx.await.ok().flatten()
    }
}

struct Pending {
    project: Box<Project>,
    revision: u64,
    first: Instant,
    last: Instant,
    immediate: bool,
    /// Set after a failed write: when to try again by itself.
    retry_at: Option<Instant>,
    /// Consecutive failed writes of this snapshot (drives the backoff).
    failures: u32,
    seq: u64,
}

impl Pending {
    fn deadline(&self) -> Option<Instant> {
        if let Some(retry_at) = self.retry_at {
            Some(retry_at)
        } else if self.immediate {
            Some(self.first)
        } else {
            Some((self.last + DEBOUNCE).min(self.first + MAX_DELAY))
        }
    }
}

/// Wait before the automatic retry after `failures` consecutive failures.
fn retry_backoff(failures: u32) -> Duration {
    RETRY_BASE
        .saturating_mul(1 << failures.saturating_sub(1).min(8))
        .min(RETRY_MAX)
}

struct Waiter {
    needs: Vec<(PathBuf, u64)>,
    reply: oneshot::Sender<Option<SaveOutcome>>,
    outcome: Option<SaveOutcome>,
}

struct Worker {
    backups: Option<BackupStore>,
    on_result: SaveCallback,
    pending: HashMap<PathBuf, Pending>,
    /// Highest schedule sequence number attempted per path.
    attempted: HashMap<PathBuf, u64>,
    seq: u64,
    waiters: Vec<Waiter>,
    last_outcome: Option<SaveOutcome>,
}

impl Worker {
    fn new(backups: Option<BackupStore>, on_result: SaveCallback) -> Self {
        Worker {
            backups,
            on_result,
            pending: HashMap::new(),
            attempted: HashMap::new(),
            seq: 0,
            waiters: Vec::new(),
            last_outcome: None,
        }
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Command>) {
        loop {
            let deadline = self.pending.values().filter_map(Pending::deadline).min();
            tokio::select! {
                // Queued commands first: a write then uses the newest snapshot
                // and a flush is registered before it (handling one is µs).
                biased;
                command = rx.recv() => match command {
                    Some(command) => self.handle(command),
                    None => {
                        for p in self.pending.values_mut() {
                            p.immediate = true;
                            p.retry_at = None;
                        }
                        self.write_due().await;
                        self.resolve_waiters();
                        return;
                    }
                },
                () = sleep_until(deadline) => self.write_due().await,
            }
            self.resolve_waiters();
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Schedule {
                path,
                project,
                revision,
                urgency,
                at,
            } => {
                self.seq += 1;
                let immediate = urgency == Urgency::Immediate;
                match self.pending.get_mut(&path) {
                    Some(p) => {
                        if p.retry_at.take().is_some() {
                            // A new change after a failure starts a new burst.
                            p.first = at;
                        }
                        p.project = project;
                        p.revision = revision;
                        p.last = at;
                        p.immediate |= immediate;
                        p.seq = self.seq;
                    }
                    None => {
                        let p = Pending {
                            project,
                            revision,
                            first: at,
                            last: at,
                            immediate,
                            retry_at: None,
                            failures: 0,
                            seq: self.seq,
                        };
                        self.pending.insert(path, p);
                    }
                }
            }
            Command::Flush { reply } => {
                // Re-arm with fresh sequence numbers so the flush waits for a
                // new attempt even for a snapshot whose last write failed.
                for p in self.pending.values_mut() {
                    p.immediate = true;
                    p.retry_at = None;
                    self.seq += 1;
                    p.seq = self.seq;
                }
                let needs: Vec<(PathBuf, u64)> = self
                    .pending
                    .iter()
                    .map(|(path, p)| (path.clone(), p.seq))
                    .collect();
                if needs.is_empty() {
                    let _ = reply.send(self.last_outcome.clone());
                } else {
                    self.waiters.push(Waiter {
                        needs,
                        reply,
                        outcome: None,
                    });
                }
            }
        }
    }

    /// Write every pending snapshot whose deadline has passed, one at a time.
    async fn write_due(&mut self) {
        let now = Instant::now();
        let mut due: Vec<(PathBuf, Instant)> = self
            .pending
            .iter()
            .filter_map(|(path, p)| {
                p.deadline()
                    .filter(|d| *d <= now)
                    .map(|d| (path.clone(), d))
            })
            .collect();
        due.sort_by_key(|(_, d)| *d);
        for (path, _) in due {
            let Some(p) = self.pending.remove(&path) else {
                continue;
            };
            let (project, outcome) =
                write_snapshot(path.clone(), p.project, p.revision, self.backups.clone()).await;
            self.attempted.insert(path.clone(), p.seq);
            for waiter in &mut self.waiters {
                if waiter.needs.iter().any(|(needed, _)| needed == &path)
                    && !waiter.outcome.as_ref().is_some_and(|o| o.error.is_some())
                {
                    waiter.outcome = Some(outcome.clone());
                }
            }
            if outcome.error.is_some()
                && let Some(project) = project
            {
                let failures = p.failures.saturating_add(1);
                let failed = Pending {
                    project,
                    retry_at: Some(Instant::now() + retry_backoff(failures)),
                    failures,
                    immediate: false,
                    ..p
                };
                self.pending.insert(path, failed);
            }
            self.last_outcome = Some(outcome.clone());
            (self.on_result)(outcome);
        }
    }

    fn resolve_waiters(&mut self) {
        let attempted = &self.attempted;
        let (done, waiting): (Vec<Waiter>, Vec<Waiter>) = std::mem::take(&mut self.waiters)
            .into_iter()
            .partition(|w| {
                w.needs
                    .iter()
                    .all(|(path, seq)| attempted.get(path).is_some_and(|a| a >= seq))
            });
        self.waiters = waiting;
        for waiter in done {
            let _ = waiter.reply.send(waiter.outcome);
        }
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending::<()>().await,
    }
}

/// Save on the blocking pool; returns the project (kept for a retry; `None`
/// if the save task panicked) and the outcome.
async fn write_snapshot(
    path: PathBuf,
    project: Box<Project>,
    revision: u64,
    backups: Option<BackupStore>,
) -> (Option<Box<Project>>, SaveOutcome) {
    let target = path.clone();
    let joined = tokio::task::spawn_blocking(move || {
        let result = save_atomic(&target, &project).map_err(|e| e.to_string());
        if result.is_ok()
            && let Some(backups) = &backups
            && let Err(e) = backups.write(&target, &project, revision, false)
        {
            tracing::warn!("project backup failed (details in the debug log)");
            tracing::debug!("project backup failed: {e}");
        }
        (project, result)
    })
    .await;
    match joined {
        Ok((project, Ok(()))) => (
            Some(project),
            SaveOutcome {
                saved_at: Some(rustcut_model::now_utc()),
                revision,
                error: None,
                path,
            },
        ),
        Ok((project, Err(error))) => (
            Some(project),
            SaveOutcome {
                saved_at: None,
                revision,
                error: Some(error),
                path,
            },
        ),
        Err(join_error) => {
            // The save task panicked: report it; the next change schedules a fresh snapshot.
            let error = format!(
                "Saving failed unexpectedly ({join_error}); it will be retried on the next change."
            );
            (
                None,
                SaveOutcome {
                    saved_at: None,
                    revision,
                    error: Some(error),
                    path,
                },
            )
        }
    }
}
