//! Project files on disk (PRJ-01, PRJ-06): atomic saves, and loading with
//! migration (`.v<N>.bak` kept), newer-schema read-only mode and recovery
//! from the newest valid backup.

use crate::error::PersistError;
use crate::migrate;
use rustcut_model::{Notice, NoticeLevel, Project, ReadOnlyReason, SCHEMA_VERSION};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub use crate::backup::BackupStore;
pub use crate::lock::{LockOutcome, ProjectLock};

/// Files larger than this are not project files (guards against opening a video as a project).
pub const MAX_PROJECT_BYTES: u64 = 64 * 1024 * 1024;
/// Rename attempts while another process briefly holds the target (Windows;
/// about 1.5 s in total).
const RENAME_RETRIES: u32 = 20;
/// Temp files of killed writers older than this are removed on load.
const STALE_TEMP_AGE: Duration = Duration::from_secs(60);

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A project read from disk.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub project: Project,
    /// Banners for the UI (`migrated_schema`, `read_only_newer_schema`,
    /// `recovered_from_backup`, `partial_read`, …).
    pub notices: Vec<Notice>,
    /// `NewerSchema` when the file was written by a newer version (PRJ-06).
    pub read_only: Option<ReadOnlyReason>,
    /// Schema version the file was migrated from (e.g. `Some(1)`).
    pub migrated_from: Option<u32>,
    /// Backup file the project was restored from because the file was damaged.
    pub recovered_from: Option<PathBuf>,
}

/// Result of parsing project bytes (no file-system side effects).
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub project: Project,
    pub notices: Vec<Notice>,
    pub read_only: Option<ReadOnlyReason>,
    pub migrated_from: Option<u32>,
}

/// PRJ-01: write `project` as pretty JSON atomically (see [`write_atomic`]).
pub fn save_atomic(path: &Path, project: &Project) -> Result<(), PersistError> {
    let bytes =
        serde_json::to_vec_pretty(project).map_err(|e| PersistError::Encode(e.to_string()))?;
    write_atomic(path, &bytes)
}

/// Atomic replace: write a temp file in the same folder, `write_all`,
/// `sync_all`, then rename it over `path` (retrying briefly on Windows
/// sharing violations, e.g. a virus scanner holding the target). Readers see
/// either the old or the new file, never a partial one. Creates the folder
/// if needed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), PersistError> {
    let dir = parent_dir(path);
    fs::create_dir_all(&dir).map_err(|e| PersistError::io("create the folder", &dir, e))?;
    let tmp = create_temp(path, bytes).map_err(|e| PersistError::io("write", path, e))?;
    if let Err(e) = rename_replacing(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(PersistError::io("replace", path, e));
    }
    sync_dir(&dir);
    Ok(())
}

/// Read a project file. Old schemas are migrated (the original is kept as
/// `<file>.v<N>.bak`, written once), newer schemas load leniently and
/// read-only, anything unreadable is [`PersistError::Corrupt`] (callers fall
/// back to backups, see [`load_with_recovery`]). The file itself is not
/// rewritten; the caller saves the migrated project.
pub fn load(path: &Path) -> Result<Loaded, PersistError> {
    let bytes = read_project_bytes(path)?;
    cleanup_stale_temps(path);
    let parsed = parse_project_bytes(&bytes).map_err(|reason| PersistError::Corrupt {
        path: path.to_path_buf(),
        reason,
    })?;
    let mut notices = parsed.notices;
    if let Some(from) = parsed.migrated_from {
        let bak = versioned_backup_path(path, from);
        let kept = bak.exists() || write_atomic(&bak, &bytes).is_ok();
        let bak_name = bak
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let message = if kept {
            format!(
                "This project was made with an older RustCut version and has been upgraded. The original file is kept as {bak_name}."
            )
        } else {
            "This project was made with an older RustCut version and has been upgraded. The original could not be \
             copied (the folder may be read-only)."
                .to_string()
        };
        notices.insert(
            0,
            Notice::new(NoticeLevel::Info, "migrated_schema", message),
        );
    }
    Ok(Loaded {
        project: parsed.project,
        notices,
        read_only: parsed.read_only,
        migrated_from: parsed.migrated_from,
        recovered_from: None,
    })
}

/// [`load`], falling back for a damaged file (PRJ-06) to the newest valid
/// backup (found through the backup index or the fingerprint still readable
/// in the damaged text), then to a partial read of the file itself. The
/// damaged file is copied to `<file>.damaged` before anything overwrites it.
pub fn load_with_recovery(path: &Path, backups: &BackupStore) -> Result<Loaded, PersistError> {
    let (path_buf, reason) = match load(path) {
        Err(PersistError::Corrupt { path, reason }) => (path, reason),
        other => return other,
    };
    if let Some((backup_path, parsed)) = backups.newest_valid_for_project(path) {
        let kept = keep_damaged_copy(path);
        let saved = parsed.project.updated_at.format("%Y-%m-%d %H:%M UTC");
        let mut notices = vec![Notice::new(
            NoticeLevel::Warning,
            "recovered_from_backup",
            format!(
                "The project file was damaged ({reason}), so the newest valid backup (saved {saved}) was restored.{kept}"
            ),
        )];
        notices.extend(parsed.notices);
        return Ok(Loaded {
            project: parsed.project,
            notices,
            read_only: parsed.read_only,
            migrated_from: parsed.migrated_from,
            recovered_from: Some(backup_path),
        });
    }
    if let Some((project, mut notices)) = partial_read(path) {
        let kept = keep_damaged_copy(path);
        notices.insert(
            0,
            Notice::new(
                NoticeLevel::Warning,
                "partial_read",
                format!(
                    "The project file was damaged ({reason}) and no backup was found; everything readable was restored.{kept}"
                ),
            ),
        );
        return Ok(Loaded {
            project,
            notices,
            read_only: None,
            migrated_from: None,
            recovered_from: None,
        });
    }
    Err(PersistError::Corrupt {
        path: path_buf,
        reason,
    })
}

/// Parse project bytes of any known schema without touching the disk.
/// Errors are short reasons for [`PersistError::Corrupt`].
pub fn parse_project_bytes(bytes: &[u8]) -> Result<Parsed, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| format!("not valid JSON: {e}"))?;
    match detect_version(&value)? {
        1 => {
            let migrated = migrate::v1_to_v2(value)?;
            let (project, warnings) = migrate::lenient_project(&migrated, SCHEMA_VERSION)?;
            Ok(Parsed {
                project,
                notices: warning_notices(warnings),
                read_only: None,
                migrated_from: Some(1),
            })
        }
        SCHEMA_VERSION => {
            let project: Project =
                serde_json::from_value(value).map_err(|e| format!("unexpected content: {e}"))?;
            Ok(Parsed {
                project,
                notices: Vec::new(),
                read_only: None,
                migrated_from: None,
            })
        }
        found if found > SCHEMA_VERSION => {
            let (project, warnings) = match serde_json::from_value::<Project>(value.clone()) {
                Ok(project) => (project, Vec::new()),
                Err(_) => migrate::lenient_project(&value, found)?,
            };
            let reason = ReadOnlyReason::NewerSchema {
                found,
                supported: SCHEMA_VERSION,
            };
            let mut notices = vec![Notice::new(
                NoticeLevel::Warning,
                "read_only_newer_schema",
                crate::error::read_only_message(&reason),
            )];
            notices.extend(warning_notices(warnings));
            Ok(Parsed {
                project,
                notices,
                read_only: Some(reason),
                migrated_from: None,
            })
        }
        other => Err(format!("unsupported schema_version {other}")),
    }
}

/// `<file>.v<N>.bak` (PRJ-06).
pub fn versioned_backup_path(path: &Path, version: u32) -> PathBuf {
    with_suffix(path, &format!(".v{version}.bak"))
}

/// `path` with `suffix` appended to its file name.
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

/// Folder containing `path` (`.` for a bare file name).
pub(crate) fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn detect_version(value: &Value) -> Result<u32, String> {
    let obj = value.as_object().ok_or_else(|| {
        "the file does not contain a project (expected a JSON object)".to_string()
    })?;
    match obj.get("schema_version") {
        Some(v) => v
            .as_u64()
            .filter(|n| *n >= 1)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| "schema_version is not a positive whole number".to_string()),
        None if obj.contains_key("project_version") => Ok(1),
        None => Err("this is not a RustCut project file (no schema_version)".into()),
    }
}

fn warning_notices(warnings: Vec<String>) -> Vec<Notice> {
    warnings
        .into_iter()
        .map(|w| Notice::new(NoticeLevel::Warning, "partial_read", w))
        .collect()
}

fn read_project_bytes(path: &Path) -> Result<Vec<u8>, PersistError> {
    let len = fs::metadata(path)
        .map_err(|e| PersistError::io("read", path, e))?
        .len();
    if len > MAX_PROJECT_BYTES {
        return Err(PersistError::Corrupt {
            path: path.to_path_buf(),
            reason: format!("{len} bytes is far too large for a project file"),
        });
    }
    fs::read(path).map_err(|e| PersistError::io("read", path, e))
}

/// Last resort for a damaged file without backups: salvage what parses.
fn partial_read(path: &Path) -> Option<(Project, Vec<Notice>)> {
    let bytes = read_project_bytes(path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let (project, warnings) = migrate::lenient_project(&value, SCHEMA_VERSION).ok()?;
    Some((project, warning_notices(warnings)))
}

/// Copy the damaged file to `<file>.damaged`; returns a sentence for the notice.
fn keep_damaged_copy(path: &Path) -> String {
    let target = with_suffix(path, ".damaged");
    match fs::copy(path, &target) {
        Ok(_) => format!(
            " The damaged file was kept as {}.",
            target
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        ),
        Err(_) => String::new(),
    }
}

/// `.<file>.<pid>-<time>-<n>.tmp` next to `path`. The time stamp keeps a
/// new process from colliding with temp files left by a killed process
/// that had the same (reused) pid.
fn temp_path_for(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    parent_dir(path).join(format!(".{name}.{}-{stamp:x}-{n}.tmp", std::process::id()))
}

/// Write `bytes` to a new temp file next to `path` and return its path.
/// A name that already exists (never ours) is skipped, not overwritten.
fn create_temp(path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let mut attempt = 0;
    loop {
        let tmp = temp_path_for(path);
        match write_temp(&tmp, bytes) {
            Ok(()) => return Ok(tmp),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && attempt < 3 => attempt += 1,
            Err(e) => {
                if e.kind() != io::ErrorKind::AlreadyExists {
                    let _ = fs::remove_file(&tmp);
                }
                return Err(e);
            }
        }
    }
}

fn write_temp(tmp: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(tmp)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn rename_replacing(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 0u32;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < RENAME_RETRIES && is_transient_sharing_error(&e) => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(10 * u64::from(attempt.min(10))));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Windows: access denied (5), sharing violation (32), lock violation (33)
/// while a scanner or indexer briefly opens the target.
pub(crate) fn is_transient_sharing_error(e: &io::Error) -> bool {
    cfg!(windows)
        && (e.kind() == io::ErrorKind::PermissionDenied
            || matches!(e.raw_os_error(), Some(5 | 32 | 33)))
}

/// Make the rename durable (POSIX); Windows has no directory handles for this.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Remove `.<file>.<pid>-<n>.tmp` files left by writers that were killed.
fn cleanup_stale_temps(path: &Path) {
    let Some(name) = path.file_name() else { return };
    let prefix = format!(".{}.", name.to_string_lossy());
    let Ok(entries) = fs::read_dir(parent_dir(path)) else {
        return;
    };
    for entry in entries.flatten().take(20_000) {
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !(file_name.starts_with(&prefix) && file_name.ends_with(".tmp")) {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE_TEMP_AGE);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}
