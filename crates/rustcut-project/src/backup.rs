//! Rolling project backups in the app data folder (PRJ-01, PRJ-06).
//!
//! Layout under `root` (normally `<app data>/backups`):
//! * `<fingerprint key>/<UTC yyyymmddThhmmss.mmmZ>_r<revision>.rustcut`:
//!   the newest [`BACKUPS_KEPT`] versions per source; a non-forced write is
//!   skipped while the newest backup is younger than [`BACKUP_MIN_INTERVAL`].
//! * `index.json`: project file path → fingerprint key, so a damaged project
//!   opened directly still finds its backups.

use crate::error::PersistError;
use crate::persist::{Parsed, parse_project_bytes, save_atomic, write_atomic};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use rustcut_model::Project;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Backups kept per source (PRJ-01: "the last 10 versions").
pub const BACKUPS_KEPT: usize = 10;
/// Minimum age of the newest backup before a non-forced write adds another.
pub const BACKUP_MIN_INTERVAL: Duration = Duration::from_secs(30);

const INDEX_FILE: &str = "index.json";
const INDEX_PRUNE_THRESHOLD: usize = 1000;
const NAME_TIME_FORMAT: &str = "%Y%m%dT%H%M%S%.3f";
const BACKUP_EXTENSION: &str = ".rustcut";

/// Rolling backups rooted at `root`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupStore {
    root: PathBuf,
}

impl BackupStore {
    /// Backups stored under `root` (created on first write).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        BackupStore { root: root.into() }
    }

    /// `<data_dir>/backups`.
    pub fn in_data_dir(data_dir: &Path) -> Self {
        BackupStore::new(data_dir.join("backups"))
    }

    /// The backup root folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Back up `project` (saved at `project_path`, store revision `revision`).
    /// Returns the new file, or `None` when throttled (newest backup < 30 s
    /// old and `force` is false). Keeps the newest [`BACKUPS_KEPT`] files and
    /// records `project_path` in the index.
    pub fn write(
        &self,
        project_path: &Path,
        project: &Project,
        revision: u64,
        force: bool,
    ) -> Result<Option<PathBuf>, PersistError> {
        let key = sanitize_key(&project.source.fingerprint.key());
        let dir = self.root.join(&key);
        let now = Utc::now();
        if !force && let Some(newest) = list_dir(&dir).first() {
            let age_ms = backup_time(newest).map(|t| (now - t).num_milliseconds());
            let min_ms = i64::try_from(BACKUP_MIN_INTERVAL.as_millis()).unwrap_or(i64::MAX);
            if age_ms.is_some_and(|age| (0..min_ms).contains(&age)) {
                return Ok(None);
            }
        }
        let file = dir.join(format!(
            "{}Z_r{revision:010}{BACKUP_EXTENSION}",
            now.format(NAME_TIME_FORMAT)
        ));
        save_atomic(&file, project)?;
        for old in list_dir(&dir).iter().skip(BACKUPS_KEPT) {
            let _ = fs::remove_file(old);
        }
        if let Err(e) = self.remember(project_path, &key) {
            tracing::debug!("backup index not updated: {e}");
        }
        Ok(Some(file))
    }

    /// Backups of the source with this fingerprint key, newest first.
    pub fn list(&self, fingerprint_key: &str) -> Vec<PathBuf> {
        list_dir(&self.root.join(sanitize_key(fingerprint_key)))
    }

    /// Newest backup of this source that parses (skipping damaged ones).
    pub fn newest_valid(&self, fingerprint_key: &str) -> Option<(PathBuf, Parsed)> {
        self.list(fingerprint_key).into_iter().find_map(|path| {
            let bytes = fs::read(&path).ok()?;
            let parsed = parse_project_bytes(&bytes).ok()?;
            Some((path, parsed))
        })
    }

    /// Fingerprint key recorded for a project file by an earlier backup.
    pub fn key_for_project(&self, project_path: &Path) -> Option<String> {
        self.read_index().remove(&path_key(project_path))
    }

    /// Newest valid backup of the project at `project_path`: looked up
    /// through the index, else through the fingerprint still readable in the
    /// (damaged) file's text.
    pub fn newest_valid_for_project(&self, project_path: &Path) -> Option<(PathBuf, Parsed)> {
        if let Some(found) = self
            .key_for_project(project_path)
            .and_then(|key| self.newest_valid(&key))
        {
            return Some(found);
        }
        let bytes = fs::read(project_path).ok()?;
        let key = fingerprint_key_hint(&String::from_utf8_lossy(&bytes))?;
        self.newest_valid(&key)
    }

    fn index_path(&self) -> PathBuf {
        self.root.join(INDEX_FILE)
    }

    fn read_index(&self) -> BTreeMap<String, String> {
        fs::read(self.index_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn remember(&self, project_path: &Path, key: &str) -> Result<(), PersistError> {
        let mut index = self.read_index();
        let path = path_key(project_path);
        if index.get(&path).map(String::as_str) == Some(key) {
            return Ok(());
        }
        index.insert(path, key.to_string());
        if index.len() > INDEX_PRUNE_THRESHOLD {
            index.retain(|p, _| Path::new(p).exists());
        }
        let bytes =
            serde_json::to_vec_pretty(&index).map_err(|e| PersistError::Encode(e.to_string()))?;
        write_atomic(&self.index_path(), &bytes)
    }
}

/// Backup files in `dir`, newest first (names start with a UTC timestamp).
fn list_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(BACKUP_EXTENSION))
        })
        .collect();
    files.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    files
}

/// Time a backup was written: from its name, else its modification time.
fn backup_time(path: &Path) -> Option<DateTime<Utc>> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let from_name = name
        .get(..19)
        .and_then(|stamp| NaiveDateTime::parse_from_str(stamp, NAME_TIME_FORMAT).ok())
        .map(|naive| Utc.from_utc_datetime(&naive));
    from_name.or_else(|| {
        fs::metadata(path)
            .ok()?
            .modified()
            .ok()
            .map(DateTime::<Utc>::from)
    })
}

/// Keep fingerprint keys safe as folder names.
fn sanitize_key(key: &str) -> String {
    let cleaned: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    if cleaned.is_empty() {
        "unknown".into()
    } else {
        cleaned
    }
}

/// Index key of a project path (absolute; case-insensitive on Windows).
pub(crate) fn path_key(path: &Path) -> String {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let s = abs.to_string_lossy().into_owned();
    if cfg!(windows) {
        s.replace('/', "\\").to_lowercase()
    } else {
        s
    }
}

/// Recover `<xxh3>-<size>` from the text of a damaged project.
fn fingerprint_key_hint(text: &str) -> Option<String> {
    let block = &text[text.find("\"fingerprint\"")?..];
    let hash = value_after(block, "\"xxh3_head_tail\"")?;
    let hash = hash.strip_prefix('"')?.split('"').next()?;
    if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let size: String = value_after(block, "\"size_bytes\"")?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if size.is_empty() {
        return None;
    }
    Some(format!("{hash}-{size}"))
}

/// Text after `"key"` and its colon, with leading whitespace removed.
fn value_after<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let rest = &text[text.find(key)? + key.len()..];
    Some(rest.trim_start().strip_prefix(':')?.trim_start())
}
