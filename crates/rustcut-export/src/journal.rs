//! Export journal (PRD 7.8, EXP-08): `.part` files and FFmpeg PIDs of running
//! clips, so the next launch can remove stale `.part` files and (macOS/Linux)
//! kill FFmpeg processes that outlived a crashed app.
//!
//! The file is `<data_dir>/export-journal.json`, rewritten atomically (temp file
//! in the same folder, `sync_all`, rename) on every change. Every mutation
//! re-reads the file, so [`cleanup_stale`] and a running engine never clobber
//! each other. A corrupt journal reads as empty.

use crate::error::ExportError;
use chrono::{DateTime, Utc};
use rustcut_runner::Sidecars;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// File name of the journal inside the data directory.
pub const JOURNAL_FILE: &str = "export-journal.json";

/// One clip being written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// The `.part` file FFmpeg writes.
    pub part_path: PathBuf,
    /// FFmpeg's PID once spawned.
    pub pid: Option<u32>,
    /// When the clip started.
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct JournalFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: Vec<JournalEntry>,
}

/// Handle on the journal file of one data directory.
#[derive(Debug)]
pub struct Journal {
    path: PathBuf,
    lock: Mutex<()>,
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl Journal {
    /// Journal in `data_dir` (created on first write).
    pub fn new(data_dir: &Path) -> Self {
        Journal {
            path: data_dir.join(JOURNAL_FILE),
            lock: Mutex::new(()),
        }
    }

    /// Path of the journal file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Current entries (empty when the file is missing or corrupt).
    pub fn entries(&self) -> Vec<JournalEntry> {
        read_entries(&self.path)
    }

    /// Journal `part_path` BEFORE FFmpeg is spawned (no PID yet).
    ///
    /// # Errors
    /// [`ExportError::Journal`] when the file cannot be written.
    pub fn record_start(&self, part_path: &Path) -> Result<(), ExportError> {
        self.mutate(|entries| {
            entries.retain(|e| e.part_path != part_path);
            entries.push(JournalEntry {
                part_path: part_path.to_path_buf(),
                pid: None,
                started_at: Utc::now(),
            });
        })
    }

    /// Record the PID of the process writing `part_path`.
    ///
    /// # Errors
    /// [`ExportError::Journal`] when the file cannot be written.
    pub fn set_pid(&self, part_path: &Path, pid: u32) -> Result<(), ExportError> {
        self.mutate(|entries| {
            if let Some(e) = entries.iter_mut().find(|e| e.part_path == part_path) {
                e.pid = Some(pid);
            }
        })
    }

    /// Drop the entry of `part_path` (finished, failed or cancelled).
    ///
    /// # Errors
    /// [`ExportError::Journal`] when the file cannot be written.
    pub fn remove(&self, part_path: &Path) -> Result<(), ExportError> {
        self.mutate(|entries| entries.retain(|e| e.part_path != part_path))
    }

    fn mutate(&self, f: impl FnOnce(&mut Vec<JournalEntry>)) -> Result<(), ExportError> {
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut entries = read_entries(&self.path);
        f(&mut entries);
        write_entries(&self.path, entries)
    }
}

fn read_entries(path: &Path) -> Vec<JournalEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match serde_json::from_str::<JournalFile>(&text) {
        Ok(f) => f.entries,
        Err(e) => {
            tracing::warn!(error = %e, "export journal is corrupt; treating it as empty");
            Vec::new()
        }
    }
}

/// `rename` with a few retries: on Windows a virus scanner or indexer can briefly
/// hold the file and make the replace fail with a sharing violation.
fn rename_retrying(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut last = None;
    for attempt in 0..6u64 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(20 * (attempt + 1)));
    }
    Err(last.unwrap())
}

fn write_entries(path: &Path, entries: Vec<JournalEntry>) -> Result<(), ExportError> {
    let wrap = |source| ExportError::Journal {
        path: path.display().to_string(),
        source,
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(wrap)?;
    let json = serde_json::to_vec_pretty(&JournalFile {
        version: 1,
        entries,
    })
    .map_err(|e| wrap(std::io::Error::other(e)))?;
    let tmp = dir.join(format!(
        ".{JOURNAL_FILE}.{}.{}.tmp",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
        drop(f);
        rename_retrying(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(wrap)
}

/// Startup cleanup (PRD 7.8): delete every journaled `.part` file, kill journaled
/// PIDs that are still FFmpeg (macOS/Linux; Windows children die with the Job
/// Object), empty the journal and return the deleted `.part` paths.
pub fn cleanup_stale(data_dir: &Path) -> Vec<PathBuf> {
    cleanup_stale_with(data_dir, None)
}

/// [`cleanup_stale`] that identifies FFmpeg by the located sidecars (exact) instead
/// of by process name.
pub fn cleanup_stale_with(data_dir: &Path, sidecars: Option<&Sidecars>) -> Vec<PathBuf> {
    let journal = Journal::new(data_dir);
    let entries = journal.entries();
    let mut removed = Vec::new();
    let mut kept: Vec<PathBuf> = Vec::new();
    for e in &entries {
        if let Some(pid) = e.pid {
            kill_stale(pid, sidecars);
        }
        match std::fs::remove_file(&e.part_path) {
            Ok(()) => removed.push(e.part_path.clone()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                tracing::warn!(error = %err, "could not remove a stale .part file");
                kept.push(e.part_path.clone());
            }
        }
    }
    // Entries whose .part could not be removed stay journaled for the next start.
    if !entries.is_empty()
        && let Err(e) = journal.mutate(|v| v.retain(|x| kept.contains(&x.part_path)))
    {
        tracing::warn!(error = %e, "could not clear the export journal");
    }
    removed
}

#[cfg(unix)]
fn kill_stale(pid: u32, sidecars: Option<&Sidecars>) {
    use std::process::{Command, Stdio};
    if let Some(sc) = sidecars {
        let _ = rustcut_runner::kill_stale_sidecar(pid, sc);
        return;
    }
    let comm = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .stderr(Stdio::null())
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let name = comm.rsplit('/').next().unwrap_or("");
    if name.starts_with("ffmpeg") {
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }
}

#[cfg(not(unix))]
fn kill_stale(pid: u32, sidecars: Option<&Sidecars>) {
    // Windows children normally die with the Job Object, but assignment can fail
    // (the runner only logs it): kill a surviving sidecar so it releases its .part.
    if let Some(sc) = sidecars {
        let _ = rustcut_runner::kill_stale_sidecar(pid, sc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_roundtrip_and_cleanup_deletes_parts() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("a.mp4.part");
        std::fs::write(&part, b"x").unwrap();
        let j = Journal::new(dir.path());
        j.record_start(&part).unwrap();
        assert_eq!(j.entries().len(), 1);
        assert_eq!(j.entries()[0].pid, None);
        j.set_pid(&part, 4242).unwrap();
        assert_eq!(j.entries()[0].pid, Some(4242));
        let removed = cleanup_stale(dir.path());
        assert_eq!(removed, vec![part.clone()]);
        assert!(!part.exists());
        assert!(j.entries().is_empty());
    }

    #[test]
    fn cleanup_keeps_entries_whose_part_cannot_be_removed() {
        let dir = tempfile::tempdir().unwrap();
        // A directory cannot be removed with remove_file.
        let stuck = dir.path().join("stuck.mp4.part");
        std::fs::create_dir(&stuck).unwrap();
        let j = Journal::new(dir.path());
        j.record_start(&stuck).unwrap();
        assert!(cleanup_stale(dir.path()).is_empty());
        assert_eq!(j.entries().len(), 1);
    }

    #[test]
    fn remove_drops_entry_and_corrupt_journal_reads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let j = Journal::new(dir.path());
        let p = dir.path().join("b.part");
        j.record_start(&p).unwrap();
        j.remove(&p).unwrap();
        assert!(j.entries().is_empty());
        std::fs::write(j.path(), b"{not json").unwrap();
        assert!(j.entries().is_empty());
        assert!(cleanup_stale(dir.path()).is_empty());
        j.record_start(&p).unwrap();
        assert_eq!(j.entries().len(), 1);
    }

    #[test]
    fn missing_part_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let j = Journal::new(dir.path());
        j.record_start(&dir.path().join("gone.part")).unwrap();
        assert!(cleanup_stale(dir.path()).is_empty());
        assert!(j.entries().is_empty());
    }
}
