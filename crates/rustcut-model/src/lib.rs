//! Shared data model for RustCut.
//!
//! * [`project`], [`source`], [`clip`], [`export`]: the `.rustcut` project file
//!   (schema v2, PRD §7.5). Field names and enum spellings are the on-disk
//!   contract; additions beyond the PRD sample are optional on read
//!   (`#[serde(default)]`) so files written from the PRD sample parse.
//! * [`ipc`]: commands' request/response types and the [`ipc::AppEvent`] stream
//!   sent to the WebView. Every type derives `ts_rs::TS`; `cargo test -p
//!   rustcut-model` regenerates `src/lib/bindings/*.ts` (see `.cargo/config.toml`).
//! * [`time`]: formatting helpers shared by naming, UI-facing DTOs and tests.
//! * [`fingerprint`]: source identity (size + xxh3 of the first/last MiB).
//!
//! This crate is pure data + small helpers. It never spawns processes.

pub mod clip;
pub mod export;
pub mod fingerprint;
pub mod ipc;
pub mod platform;
pub mod project;
pub mod settings;
pub mod source;
pub mod time;

pub use clip::*;
pub use export::*;
pub use fingerprint::compute_fingerprint;
pub use ipc::*;
pub use platform::*;
pub use project::*;
pub use settings::*;
pub use source::*;

/// Current `.rustcut` schema version (PRJ-06). v1 = PRD v1.2 files
/// (`"project_version": "1.0"`).
pub const SCHEMA_VERSION: u32 = 2;

/// Application version written to `app_version`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Minimum clip duration default (MRK-01) and its allowed setting range.
pub const DEFAULT_MIN_CLIP_SEC: f64 = 0.5;
pub const MIN_CLIP_SEC_RANGE: (f64, f64) = (0.1, 5.0);

/// Undo history depth (MRK-04 requires at least 200).
pub const UNDO_LIMIT: usize = 500;

/// Current UTC time truncated to whole seconds, so timestamps serialise like
/// the PRD sample (`2026-10-21T19:05:11Z`).
pub fn now_utc() -> chrono::DateTime<chrono::Utc> {
    use chrono::{SubsecRound, Utc};
    Utc::now().trunc_subsecs(0)
}
