//! Project state and persistence for RustCut (PRD §4.2–4.3 store rules,
//! §4.6 PRJ-01..07, §7.5 field notes, §8.2 reliability).
//!
//! * [`store::ProjectStore`]: commands, validation (MRK-01), stable clip
//!   numbers (QUE-02), duplicate hint (MRK-03), undo/redo (MRK-04, AC-15),
//!   export-status rules (QUE-04/05), read-only mode.
//! * [`persist`]: atomic saves, loading with v1→v2 migration ([`migrate`]),
//!   newer-schema read-only mode and corrupt-file recovery (PRJ-06).
//! * [`backup::BackupStore`]: 10 rolling backups per source (PRJ-01).
//! * [`lock::ProjectLock`]: single writer per project file (PRJ-05).
//! * [`location`]: next to the video or in the app library (PRJ-02/03/04).
//! * [`recent::RecentStore`]: recent projects (PRJ-07).
//! * [`autosave::AutoSaver`]: debounced, never-overlapping writes (PRJ-01,
//!   AC-04 kill test in `tests/kill.rs`).
//!
//! Typical app flow: `location::find_existing_project` (or
//! `resolve_new_project_location`) → `ProjectLock::try_acquire` →
//! `persist::load_with_recovery` → `ProjectStore::new` + `normalize_on_load`
//! → commands → `take_save_request` → `AutoSaver::schedule`.

pub mod autosave;
pub mod backup;
pub mod error;
pub mod location;
pub mod lock;
pub mod migrate;
pub mod persist;
pub mod recent;
pub mod store;

pub use autosave::{AutoSaver, SaveCallback, SaveOutcome, Urgency};
pub use backup::BackupStore;
pub use error::{PersistError, StoreError};
pub use lock::{LockOutcome, ProjectLock};
pub use persist::{Loaded, Parsed, load, load_with_recovery, save_atomic};
pub use recent::RecentStore;
pub use store::{CROP_COALESCE_WINDOW, ImportResult, ProjectStore, SaveRequest, StoreOptions};
