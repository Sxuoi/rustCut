//! App-level settings (stored in the app data folder, not in projects).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where a project file lives (PRJ-02); also the user's preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProjectLocation {
    /// `<source name>.rustcut` next to the video.
    #[default]
    NextToVideo,
    /// `<app data>/library/<fingerprint key>.rustcut`.
    AppLibrary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct AppSettings {
    pub project_location: ProjectLocation,
    /// MRK-01 minimum clip duration (0.1–5 s, default 0.5).
    pub min_clip_duration_sec: f64,
    /// Remux/proxy cache cap in GB (§7.3, default 50).
    pub cache_cap_gb: u32,
    /// §8.4: off by default; nothing is sent when off.
    pub telemetry_opt_in: bool,
    /// Default for new projects' `spaces_to_underscores`.
    pub default_spaces_to_underscores: bool,
    /// `None` = follow the OS reduced-motion setting (§8.5).
    pub reduced_motion: Option<bool>,
    /// UI language (English at launch, §8.5).
    pub language: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            project_location: ProjectLocation::NextToVideo,
            min_clip_duration_sec: crate::DEFAULT_MIN_CLIP_SEC,
            cache_cap_gb: 50,
            telemetry_opt_in: false,
            default_spaces_to_underscores: true,
            reduced_motion: None,
            language: "en".into(),
        }
    }
}

impl AppSettings {
    pub fn normalized(mut self) -> Self {
        let (lo, hi) = crate::MIN_CLIP_SEC_RANGE;
        if !self.min_clip_duration_sec.is_finite() {
            self.min_clip_duration_sec = crate::DEFAULT_MIN_CLIP_SEC;
        }
        self.min_clip_duration_sec = self.min_clip_duration_sec.clamp(lo, hi);
        self.cache_cap_gb = self.cache_cap_gb.clamp(1, 10_000);
        if self.language.trim().is_empty() {
            self.language = "en".into();
        }
        self
    }
}

/// Start-screen entry (PRJ-07).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RecentProject {
    pub project_path: String,
    pub source_path: String,
    pub source_name: String,
    pub opened_at: DateTime<Utc>,
    pub clip_count: u32,
    pub location: ProjectLocation,
}
