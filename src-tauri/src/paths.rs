//! Folders the app owns.

use std::path::{Path, PathBuf};

/// Per-user app folders (Tauri's `app.path()` in the app, a temp dir in tests).
#[derive(Debug, Clone)]
pub struct AppPaths {
    /// Settings, recent list, export journal, library and backups.
    pub data: PathBuf,
    /// Playback remux/proxy files, frame indexes, waveforms.
    pub cache: PathBuf,
    /// Daily log files and per-clip export logs.
    pub log: PathBuf,
    /// `<data>/library`: projects whose video folder is read-only (PRJ-02).
    pub library: PathBuf,
    /// `<data>/backups`: rolling project backups (PRJ-06).
    pub backups: PathBuf,
}

impl AppPaths {
    /// Derive the folders from the three roots Tauri provides.
    pub fn new(data: PathBuf, cache: PathBuf, log: PathBuf) -> Self {
        AppPaths {
            library: data.join("library"),
            backups: data.join("backups"),
            data,
            cache,
            log,
        }
    }

    /// All folders under one root (tests).
    pub fn under(root: &Path) -> Self {
        Self::new(root.join("data"), root.join("cache"), root.join("logs"))
    }

    /// Create every folder.
    ///
    /// # Errors
    /// The first folder that cannot be created.
    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [
            &self.data,
            &self.cache,
            &self.log,
            &self.library,
            &self.backups,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}
