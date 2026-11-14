//! Finding, locking, loading and saving projects exactly like the desktop app does.

use std::path::{Path, PathBuf};

use rustcut_model::{
    Clip, Project, ProjectLocation, RecentProject, SourceInfo, compute_fingerprint, now_utc,
};
use rustcut_project::error::read_only_message;
use rustcut_project::location::{
    dir_is_writable, find_existing_project, relative_source_path, resolve_new_project_location,
};
use rustcut_project::{
    BackupStore, LockOutcome, ProjectLock, ProjectStore, RecentStore, StoreOptions,
    load_with_recovery, save_atomic,
};

use crate::env::Env;

/// The error returned when another process holds the project.
pub const LOCKED_MSG: &str =
    "This project is open in RustCut or another agent. Close it there, then retry.";

/// A project opened for writing: lock held, store loaded.
pub struct Opened {
    /// The command layer (validation, stable numbers, export records).
    pub store: ProjectStore,
    /// The `.rustcut` file.
    pub project_path: PathBuf,
    /// Next to the video or in the app library.
    pub location: ProjectLocation,
    /// The video.
    pub source_path: PathBuf,
    /// The project did not exist before this call.
    pub created: bool,
    rev0: u64,
    force_save: bool,
    lock: Option<ProjectLock>,
    backups: BackupStore,
    recents: RecentStore,
}

impl Opened {
    /// Whether a save is needed (commands ran, or the file needed repairs).
    pub fn changed(&self) -> bool {
        self.store.revision() != self.rev0 || self.force_save
    }

    /// Saves atomically, writes a backup and updates the recent list.
    ///
    /// # Errors
    /// The save failure message.
    pub fn save(&mut self, force_backup: bool) -> Result<(), String> {
        if let Some(dir) = self.project_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        save_atomic(&self.project_path, self.store.project()).map_err(|e| e.to_string())?;
        let rev = self.store.revision();
        if let Err(e) =
            self.backups
                .write(&self.project_path, self.store.project(), rev, force_backup)
        {
            tracing::warn!(error = %e, "could not write a project backup");
        }
        self.remember_recent();
        self.rev0 = rev;
        self.force_save = false;
        Ok(())
    }

    fn remember_recent(&self) {
        if !self.project_path.is_file() {
            return;
        }
        let project = self.store.project();
        let entry = RecentProject {
            project_path: self.project_path.to_string_lossy().into_owned(),
            source_path: project.source.path.clone(),
            source_name: project.source.file_name(),
            opened_at: now_utc(),
            clip_count: u32::try_from(project.clips.len()).unwrap_or(u32::MAX),
            location: self.location,
        };
        if let Err(e) = self.recents.add(entry) {
            tracing::warn!(error = %e, "could not update the recent projects list");
        }
    }

    /// Releases the project lock (also happens on drop).
    pub fn release(&mut self) {
        self.lock = None;
    }
}

/// Validation options from the app settings.
pub fn store_options(env: &Env) -> StoreOptions {
    StoreOptions {
        min_clip_duration_sec: env.settings().min_clip_duration_sec,
    }
}

/// Finds the project of `video` (or decides where a new one goes), locks it, loads it.
///
/// # Errors
/// Probe failures, [`LOCKED_MSG`], a read-only or mismatching project, I/O errors.
pub async fn open_for_write(env: &Env, video: &Path) -> Result<Opened, String> {
    let probed = env.probe(video).await?;
    let settings = env.settings();
    let fingerprint = probed.source.fingerprint.clone();
    let existing = find_existing_project(video, &fingerprint, &env.paths.data);
    let (project_path, location, is_new) = match existing {
        Some((p, l)) => (p, l, false),
        None => {
            let (p, l) = resolve_new_project_location(
                video,
                &fingerprint,
                settings.project_location,
                &env.paths.data,
                &dir_is_writable,
            );
            (p, l, true)
        }
    };
    let lock = match ProjectLock::try_acquire(&project_path) {
        Ok(LockOutcome::Acquired(lock)) => lock,
        Ok(LockOutcome::HeldElsewhere { .. }) => return Err(LOCKED_MSG.to_string()),
        Err(e) => return Err(e.to_string()),
    };
    let backups = BackupStore::in_data_dir(&env.paths.data);
    let mut source: SourceInfo = probed.source;
    source.path = video.to_string_lossy().into_owned();
    source.relative_path = relative_source_path(&project_path, video);
    let mut force_save = false;
    let project = if is_new {
        let mut p = Project::new(source);
        p.export_settings.spaces_to_underscores = settings.default_spaces_to_underscores;
        p
    } else {
        let loaded = load_with_recovery(&project_path, &backups).map_err(|e| e.to_string())?;
        if let Some(reason) = &loaded.read_only {
            return Err(format!(
                "{} Open it in RustCut to resolve this.",
                read_only_message(reason)
            ));
        }
        force_save = loaded.migrated_from.is_some() || loaded.recovered_from.is_some();
        let mut p = loaded.project;
        if p.source.fingerprint != source.fingerprint {
            return Err(format!(
                "A RustCut project exists for \"{}\" but it was made from a different version of the file (re-recorded or replaced). Open it in RustCut and use Locate Missing Media, or move the old .rustcut file away.",
                video.display()
            ));
        }
        if p.source != source {
            p.source = source;
            force_save = true;
        }
        p
    };
    let mut store = ProjectStore::new(project, None);
    let _ = store.normalize_on_load();
    let rev0 = store.revision();
    Ok(Opened {
        store,
        project_path,
        location,
        source_path: video.to_path_buf(),
        created: is_new,
        rev0,
        force_save,
        lock: Some(lock),
        backups,
        recents: RecentStore::new(&env.paths.data),
    })
}

/// Loads the existing project of `video` without locking; `None` when it has none.
///
/// # Errors
/// I/O or corrupt-project messages.
pub fn find_readonly(env: &Env, video: &Path) -> Result<Option<(Project, PathBuf)>, String> {
    let fingerprint = compute_fingerprint(video)
        .map_err(|e| format!("Could not read \"{}\": {e}", video.display()))?;
    let Some((project_path, _)) = find_existing_project(video, &fingerprint, &env.paths.data)
    else {
        return Ok(None);
    };
    let backups = BackupStore::in_data_dir(&env.paths.data);
    let loaded = load_with_recovery(&project_path, &backups).map_err(|e| e.to_string())?;
    let mut store = ProjectStore::new(loaded.project, None);
    let _ = store.normalize_on_load();
    Ok(Some((store.into_project(), project_path)))
}

/// The clip with this stable number.
///
/// # Errors
/// A message listing the numbers that exist.
pub fn clip_by_index(project: &Project, index: u32) -> Result<&Clip, String> {
    project
        .clips
        .iter()
        .find(|c| c.index == index)
        .ok_or_else(|| {
            let mut have: Vec<u32> = project.clips.iter().map(|c| c.index).collect();
            have.sort_unstable();
            format!(
                "There is no clip number {index}. Existing clip numbers: {}.",
                if have.is_empty() {
                    "none (use add_clips first)".to_string()
                } else {
                    have.iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            )
        })
}

/// Resolves the tag by id or label (case-insensitive).
///
/// # Errors
/// A message listing the tags of the project.
pub fn resolve_tag(project: &Project, wanted: &str) -> Result<String, String> {
    let w = wanted.trim();
    project
        .tags
        .iter()
        .find(|t| t.id.eq_ignore_ascii_case(w) || t.label.eq_ignore_ascii_case(w))
        .map(|t| t.id.clone())
        .ok_or_else(|| {
            format!(
                "Unknown tag \"{wanted}\". Available tags: {}.",
                project
                    .tags
                    .iter()
                    .map(|t| t.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}
