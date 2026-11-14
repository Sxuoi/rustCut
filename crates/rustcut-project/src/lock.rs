//! PRJ-05 single writer: `<project file>.lock`, held with the standard
//! library's OS file lock (`File::try_lock`; Windows `LockFileEx`, Unix
//! `flock`). The OS releases the lock when the holder exits or crashes, so a
//! stale lock file never blocks anyone. The file contains `pid@hostname`.
//!
//! Windows refuses reads of a locked file from other handles, so there the
//! holder text is also written to an unlocked `<project file>.lock.owner`.
//! Both files are hidden on Windows (best-effort) and deleted on release.

use crate::error::PersistError;
use crate::persist::with_suffix;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Attempts (20 ms apart) while the lock file is briefly unavailable: being
/// deleted by the previous holder, or opened by a virus scanner (Windows).
const ACQUIRE_ATTEMPTS: u32 = 50;
const HOLDER_MAX_CHARS: usize = 200;

/// An acquired project lock; released (and its files deleted) on drop.
#[derive(Debug)]
pub struct ProjectLock {
    lock_path: PathBuf,
    owner_path: Option<PathBuf>,
    holder: String,
    // Declared last: closing the handle (after `Drop::drop` deleted the
    // files) is what releases the OS lock.
    _file: File,
}

/// Result of [`ProjectLock::try_acquire`].
#[derive(Debug)]
pub enum LockOutcome {
    /// This process is the single writer.
    Acquired(ProjectLock),
    /// Another window or instance holds the lock: open read-only (PRJ-05).
    /// `holder` is its `pid@hostname`, when readable.
    HeldElsewhere { holder: Option<String> },
}

impl ProjectLock {
    /// Try to become the single writer of `project_path` without blocking.
    /// Errors only when the lock file cannot be created at all (e.g. a
    /// read-only folder, where the project could not be saved either).
    pub fn try_acquire(project_path: &Path) -> Result<LockOutcome, PersistError> {
        let lock_path = Self::lock_path_for(project_path);
        if let Some(dir) = lock_path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(|e| PersistError::io("create the folder", dir, e))?;
        }
        for attempt in 1..=ACQUIRE_ATTEMPTS {
            let file = match open_hidden(&lock_path) {
                Ok(file) => file,
                // Windows: the file is being deleted by the previous holder or
                // scanned by antivirus software.
                Err(e)
                    if crate::persist::is_transient_sharing_error(&e)
                        && attempt < ACQUIRE_ATTEMPTS =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                Err(e) => return Err(PersistError::io("create the lock file", &lock_path, e)),
            };
            match file.try_lock() {
                Ok(()) => {
                    if !still_linked(&file, &lock_path) {
                        // The previous holder deleted the file after we opened it.
                        continue;
                    }
                    let holder = current_holder();
                    let _ = write_text(&file, &holder);
                    let owner_path = write_owner_file(&lock_path, &holder);
                    return Ok(LockOutcome::Acquired(ProjectLock {
                        lock_path,
                        owner_path,
                        holder,
                        _file: file,
                    }));
                }
                Err(TryLockError::WouldBlock) => {
                    return Ok(LockOutcome::HeldElsewhere {
                        holder: read_holder(&lock_path),
                    });
                }
                Err(TryLockError::Error(e)) => return Err(PersistError::io("lock", &lock_path, e)),
            }
        }
        Ok(LockOutcome::HeldElsewhere {
            holder: read_holder(&lock_path),
        })
    }

    /// `<project file>.lock`.
    pub fn lock_path_for(project_path: &Path) -> PathBuf {
        with_suffix(project_path, ".lock")
    }

    /// Path of the held lock file.
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// `pid@hostname` written into the lock file.
    pub fn holder(&self) -> &str {
        &self.holder
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        // Delete while still holding the lock; the handle closes afterwards.
        if let Some(owner) = &self.owner_path {
            let _ = fs::remove_file(owner);
        }
        let _ = fs::remove_file(&self.lock_path);
    }
}

/// `pid@hostname` of this process.
pub fn current_holder() -> String {
    format!("{}@{}", std::process::id(), hostname())
}

fn hostname() -> String {
    for var in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim();
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    #[cfg(unix)]
    if let Ok(v) = fs::read_to_string("/etc/hostname") {
        let v = v.trim();
        if !v.is_empty() {
            return v.to_string();
        }
    }
    "unknown-host".into()
}

/// Open (creating if needed, never truncating: Windows refuses to truncate
/// hidden files) with the hidden attribute on creation.
fn open_hidden(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        options.attributes(FILE_ATTRIBUTE_HIDDEN);
    }
    options.open(path)
}

fn write_text(mut file: &File, text: &str) -> std::io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

/// Windows only: readable copy of the holder next to the locked file.
fn write_owner_file(lock_path: &Path, holder: &str) -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let owner = with_suffix(lock_path, ".owner");
    let file = open_hidden(&owner).ok()?;
    write_text(&file, holder).ok()?;
    Some(owner)
}

fn read_holder(lock_path: &Path) -> Option<String> {
    let clean = |text: String| {
        let t: String = text
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(HOLDER_MAX_CHARS)
            .collect();
        (!t.is_empty()).then_some(t)
    };
    fs::read_to_string(lock_path)
        .ok()
        .and_then(clean)
        .or_else(|| {
            fs::read_to_string(with_suffix(lock_path, ".owner"))
                .ok()
                .and_then(clean)
        })
}

/// True when `path` still names the file we locked (it may have been
/// deleted by the previous holder between our open and lock).
fn still_linked(file: &File, path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (file.metadata(), fs::metadata(path)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        fs::metadata(path).is_ok()
    }
}
