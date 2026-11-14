//! PRJ-07 recent projects for the start screen: `<data dir>/recent.json`,
//! most recent first, at most [`RECENT_LIMIT`] entries, one per project
//! file; entries whose project file no longer exists are dropped.

use crate::error::PersistError;
use crate::persist::write_atomic;
use rustcut_model::RecentProject;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Maximum number of recent projects kept.
pub const RECENT_LIMIT: usize = 20;

/// The recent-projects list stored in the app data folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentStore {
    path: PathBuf,
}

impl RecentStore {
    /// List stored at `<data_dir>/recent.json`.
    pub fn new(data_dir: &Path) -> Self {
        RecentStore {
            path: data_dir.join("recent.json"),
        }
    }

    /// The JSON file backing the list.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Entries, most recent first, without duplicates or missing projects.
    /// A missing or unreadable file yields an empty list.
    pub fn list(&self) -> Vec<RecentProject> {
        let items: Vec<Value> = fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut entries: Vec<RecentProject> = items
            .into_iter()
            .filter_map(|v| serde_json::from_value::<RecentProject>(v).ok())
            .filter(|e| Path::new(&e.project_path).is_file())
            .collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.opened_at));
        let mut unique: Vec<RecentProject> = Vec::with_capacity(entries.len());
        for entry in entries {
            if !unique
                .iter()
                .any(|u| same_path(&u.project_path, &entry.project_path))
            {
                unique.push(entry);
            }
        }
        unique.truncate(RECENT_LIMIT);
        unique
    }

    /// Put `entry` first (replacing an entry for the same project) and save.
    /// Returns the new list.
    pub fn add(&self, entry: RecentProject) -> Result<Vec<RecentProject>, PersistError> {
        let mut items = self.list();
        items.retain(|e| !same_path(&e.project_path, &entry.project_path));
        items.insert(0, entry);
        items.truncate(RECENT_LIMIT);
        self.save(&items)?;
        Ok(items)
    }

    /// Remove the entry of `project_path` and save. Returns the new list.
    pub fn remove(&self, project_path: &str) -> Result<Vec<RecentProject>, PersistError> {
        let mut items = self.list();
        items.retain(|e| !same_path(&e.project_path, project_path));
        self.save(&items)?;
        Ok(items)
    }

    fn save(&self, items: &[RecentProject]) -> Result<(), PersistError> {
        let bytes =
            serde_json::to_vec_pretty(items).map_err(|e| PersistError::Encode(e.to_string()))?;
        write_atomic(&self.path, &bytes)
    }
}

/// Same project file (case- and separator-insensitive on Windows).
fn same_path(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.replace('/', "\\").to_lowercase() == b.replace('/', "\\").to_lowercase()
    } else {
        a == b
    }
}
