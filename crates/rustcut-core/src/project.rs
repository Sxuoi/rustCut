//! `.rustcut` project files: schema, atomic save, crash recovery (PRD 4.5, 7.3).
//!
//! The project file lives next to the source video (`video.mp4` ->
//! `video.rustcut`) and is rewritten atomically on every change.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::clip::Clip;

pub const PROJECT_VERSION: &str = "1.0";
pub const PROJECT_EXT: &str = "rustcut";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceVideo {
    pub path: String,
    pub duration_sec: f64,
    pub resolution: String,
    pub fps: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExportMode {
    LosslessCopy,
    SmartReencode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportSettings {
    pub output_dir: String,
    pub naming_pattern: String,
    pub mode: ExportMode,
    pub aspect_ratio_override: Option<String>,
    pub crop_style: crate::ffmpeg::CropStyle,
    pub crop_x_offset: f64,
    pub parallel_jobs: u32,
}

impl ExportSettings {
    /// Sensible defaults per the PRD: lossless copy, `{index}_{name}`
    /// templates, center crop, sequential export.
    pub fn default_for(output_dir: impl Into<String>) -> Self {
        Self {
            output_dir: output_dir.into(),
            naming_pattern: "{index}_{name}".to_string(),
            mode: ExportMode::LosslessCopy,
            aspect_ratio_override: None,
            crop_style: crate::ffmpeg::CropStyle::CenterCrop,
            crop_x_offset: 0.0,
            parallel_jobs: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectState {
    pub project_version: String,
    pub source_video: SourceVideo,
    pub export_settings: ExportSettings,
    pub clips: Vec<Clip>,
}

#[derive(Debug)]
pub enum ProjectError {
    Io(io::Error),
    Json(serde_json::Error),
    UnsupportedVersion(String),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectError::Io(e) => write!(f, "project file I/O error: {e}"),
            ProjectError::Json(e) => write!(f, "project file parse error: {e}"),
            ProjectError::UnsupportedVersion(v) => {
                write!(f, "unsupported project version {v:?} (expected {PROJECT_VERSION})")
            }
        }
    }
}

impl std::error::Error for ProjectError {}

impl From<io::Error> for ProjectError {
    fn from(e: io::Error) -> Self {
        ProjectError::Io(e)
    }
}

impl From<serde_json::Error> for ProjectError {
    fn from(e: serde_json::Error) -> Self {
        ProjectError::Json(e)
    }
}

impl ProjectState {
    pub fn new(source_video: SourceVideo, export_settings: ExportSettings) -> Self {
        Self {
            project_version: PROJECT_VERSION.to_string(),
            source_video,
            export_settings,
            clips: Vec::new(),
        }
    }

    /// `video.mp4` -> `video.rustcut` in the same directory (PRD 4.5).
    pub fn project_path_for_video(video: &Path) -> PathBuf {
        let mut p = video.to_path_buf();
        p.set_extension(PROJECT_EXT);
        p
    }

    /// True when the source video moved or was deleted - the app shows the
    /// "Locate Missing Media" dialog in that case (PRD 9).
    pub fn source_missing(&self) -> bool {
        !Path::new(&self.source_video.path).exists()
    }

    /// Atomic save: write to a temp sibling, then rename over the target.
    /// A crash mid-save can never corrupt the previous state.
    pub fn save_atomic(&self, path: &Path) -> Result<(), ProjectError> {
        let json = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension(format!("{PROJECT_EXT}.tmp"));
        fs::write(&tmp, &json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, ProjectError> {
        let raw = fs::read_to_string(path)?;
        let state: ProjectState = serde_json::from_str(&raw)?;
        if state.project_version != PROJECT_VERSION {
            return Err(ProjectError::UnsupportedVersion(state.project_version));
        }
        Ok(state)
    }

    /// Indices are positional and must stay contiguous on screen.
    pub fn renumber(&mut self) {
        for (i, clip) in self.clips.iter_mut().enumerate() {
            clip.index = i as u32 + 1;
        }
    }
}

/// Estimated output size of an export batch (PRD 9): bitrate x duration.
/// `bitrate_bps` is the source video bitrate in bits per second.
pub fn estimated_export_bytes(bitrate_bps: f64, total_duration_sec: f64) -> u64 {
    if !bitrate_bps.is_finite() || !total_duration_sec.is_finite() {
        return 0;
    }
    let bytes = bitrate_bps.max(0.0) * total_duration_sec.max(0.0) / 8.0;
    bytes.round().clamp(0.0, u64::MAX as f64) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_source() -> SourceVideo {
        SourceVideo {
            path: "D:/Recordings/livestream_2026_10_21.mp4".to_string(),
            duration_sec: 9000.0,
            resolution: "1920x1080".to_string(),
            fps: 60.0,
        }
    }

    #[test]
    fn parses_the_prd_schema_sample_verbatim() {
        // The JSON schema sample from PRD 7.3 - conformance test.
        let raw = r##"{
  "project_version": "1.0",
  "source_video": {
    "path": "D:/Recordings/livestream_2026_10_21.mp4",
    "duration_sec": 9000.0,
    "resolution": "1920x1080",
    "fps": 60.0
  },
  "export_settings": {
    "output_dir": "D:/Recordings/Clips_Output",
    "naming_pattern": "{index}_{name}",
    "mode": "lossless_copy",
    "aspect_ratio_override": null,
    "crop_style": "center_crop",
    "crop_x_offset": 0.0,
    "parallel_jobs": 2
  },
  "clips": [
    {
      "id": "a1b2c3d4-e5f6-7890-a9b8-c7d6e5f4a3b2",
      "index": 1,
      "name": "Opening Moment",
      "start_sec": 130.500,
      "end_sec": 180.000,
      "color": "#3B82F6",
      "crop_x_offset": -0.4,
      "selected_for_export": true,
      "export_status": "completed"
    },
    {
      "id": "f9e8d7c6-b5a4-3210-c9d8-e7f6a5b4c3d2",
      "index": 2,
      "name": "Jump Scare React",
      "start_sec": 920.120,
      "end_sec": 955.800,
      "color": "#EF4444",
      "crop_x_offset": 0.3,
      "selected_for_export": true,
      "export_status": "pending"
    }
  ]
}"##;
        let state: ProjectState = serde_json::from_str(raw).expect("PRD sample must parse");
        assert_eq!(state.project_version, "1.0");
        assert_eq!(state.export_settings.mode, ExportMode::LosslessCopy);
        assert_eq!(state.export_settings.parallel_jobs, 2);
        assert_eq!(state.clips.len(), 2);
        assert_eq!(state.clips[0].name, "Opening Moment");
        assert_eq!(state.clips[0].crop_x_offset, -0.4);
        assert_eq!(state.clips[1].export_status, crate::clip::ExportStatus::Pending);
    }

    #[test]
    fn round_trips_through_disk_atomically() {
        let dir = std::env::temp_dir().join(format!("rustcut-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stream.rustcut");

        let mut state = ProjectState::new(sample_source(), ExportSettings::default_for("out"));
        state
            .clips
            .push(crate::clip::Clip::new(1, "Opening Moment", 130.5, 180.0).unwrap());

        state.save_atomic(&path).unwrap();
        // Saving again must replace the file, not fail on "already exists".
        state.save_atomic(&path).unwrap();

        let loaded = ProjectState::load(&path).unwrap();
        assert_eq!(loaded.clips.len(), 1);
        assert_eq!(loaded.clips[0].name, "Opening Moment");
        assert_eq!(loaded.export_settings.naming_pattern, "{index}_{name}");
        assert!(!path.with_extension("rustcut.tmp").exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn project_path_sits_next_to_the_source_video() {
        let p = ProjectState::project_path_for_video(Path::new(
            "D:/Recordings/livestream_full.mp4",
        ));
        assert_eq!(p, PathBuf::from("D:/Recordings/livestream_full.rustcut"));
    }

    #[test]
    fn detects_missing_source_video() {
        let mut state = ProjectState::new(sample_source(), ExportSettings::default_for("out"));
        assert!(state.source_missing());
        state.source_video.path = std::env::temp_dir().to_string_lossy().into_owned();
        assert!(!state.source_missing());
    }

    #[test]
    fn rejects_unknown_project_versions() {
        let raw = r#"{
          "project_version": "99.0",
          "source_video": {"path": "x.mp4", "duration_sec": 1.0, "resolution": "1x1", "fps": 1.0},
          "export_settings": {
            "output_dir": "o", "naming_pattern": "{index}", "mode": "lossless_copy",
            "aspect_ratio_override": null, "crop_style": "center_crop",
            "crop_x_offset": 0.0, "parallel_jobs": 1
          },
          "clips": []
        }"#;
        let path = std::env::temp_dir().join(format!("rustcut-ver-{}.rustcut", uuid::Uuid::new_v4()));
        fs::write(&path, raw).unwrap();
        let err = ProjectState::load(&path).unwrap_err();
        assert!(matches!(err, ProjectError::UnsupportedVersion(_)));
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn estimates_batch_size_from_bitrate() {
        // 8 Mbit/s for 60 s = 60 MB exactly.
        assert_eq!(estimated_export_bytes(8_000_000.0, 60.0), 60_000_000);
        assert_eq!(estimated_export_bytes(f64::NAN, 60.0), 0);
    }
}
