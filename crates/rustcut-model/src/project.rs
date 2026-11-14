//! Top-level `.rustcut` document (PRD §7.5).

use crate::clip::Clip;
use crate::export::ExportSettings;
use crate::source::SourceInfo;
use crate::time::to_ms;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// Colour tag (MRK-06). Tags always show their label, never colour alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Tag {
    pub id: String,
    pub label: String,
    /// `#RRGGBB`.
    pub color: String,
}

/// Default palette: Red = Funny, Blue = Educational, Green = Highlight.
pub fn default_tags() -> Vec<Tag> {
    vec![
        Tag {
            id: "funny".into(),
            label: "Funny".into(),
            color: "#EF4444".into(),
        },
        Tag {
            id: "edu".into(),
            label: "Educational".into(),
            color: "#3B82F6".into(),
        },
        Tag {
            id: "highlight".into(),
            label: "Highlight".into(),
            color: "#22C55E".into(),
        },
    ]
}

/// Restored on reopen (PRJ-03). Not undoable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ViewState {
    pub playhead_sec: f64,
    /// Zoom timeline window in seconds (2 s – 10 min, PLY-06).
    pub zoom_window_sec: f64,
    pub selected_clip_id: Option<Uuid>,
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState {
            playhead_sec: 0.0,
            zoom_window_sec: 20.0,
            selected_clip_id: None,
        }
    }
}

pub const ZOOM_WINDOW_RANGE_SEC: (f64, f64) = (2.0, 600.0);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Project {
    pub schema_version: u32,
    pub app_version: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub source: SourceInfo,
    pub tags: Vec<Tag>,
    pub export_settings: ExportSettings,
    /// Next stable clip number (QUE-02). Monotonic: never decremented, not
    /// even by undo, so numbers are never reused.
    pub next_index: u32,
    pub clips: Vec<Clip>,
    pub view_state: ViewState,
}

impl Project {
    /// Fresh project for a newly opened source.
    pub fn new(source: SourceInfo) -> Self {
        let now = crate::now_utc();
        Project {
            schema_version: crate::SCHEMA_VERSION,
            app_version: crate::APP_VERSION.into(),
            created_at: now,
            updated_at: now,
            source,
            tags: default_tags(),
            export_settings: ExportSettings::default(),
            next_index: 1,
            clips: Vec::new(),
            view_state: ViewState::default(),
        }
    }

    pub fn clip(&self, id: Uuid) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }

    pub fn clip_mut(&mut self, id: Uuid) -> Option<&mut Clip> {
        self.clips.iter_mut().find(|c| c.id == id)
    }

    /// `{index}` zero-padding width: max(2, digits of the highest number).
    pub fn index_width(&self) -> usize {
        let max = self.clips.iter().map(|c| c.index).max().unwrap_or(1);
        max.to_string().len().max(2)
    }

    /// Effective output folder (setting or `<source folder>/<source name>_clips`).
    pub fn output_dir(&self) -> String {
        self.export_settings
            .output_dir
            .clone()
            .unwrap_or_else(|| self.source.default_output_dir())
    }

    /// Clips whose In lies at or beyond the source end (PRJ-04 "Out of range").
    pub fn out_of_range_ids(&self) -> Vec<Uuid> {
        let end = to_ms(self.source.duration_sec);
        self.clips
            .iter()
            .filter(|c| to_ms(c.start_sec) >= end)
            .map(|c| c.id)
            .collect()
    }

    /// MRK-03: index of another clip with an identical range, if any.
    pub fn duplicate_of(&self, start_sec: f64, end_sec: f64, exclude: Option<Uuid>) -> Option<u32> {
        let (s, e) = (to_ms(start_sec), to_ms(end_sec));
        self.clips
            .iter()
            .filter(|c| Some(c.id) != exclude)
            .find(|c| to_ms(c.start_sec) == s && to_ms(c.end_sec) == e)
            .map(|c| c.index)
    }

    pub fn tag(&self, id: &str) -> Option<&crate::project::Tag> {
        self.tags.iter().find(|t| t.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The PRD §7.5 sample must parse unchanged (fields beyond it are optional).
    #[test]
    fn prd_sample_parses() {
        let json = include_str!("../tests/prd_sample.rustcut");
        let p: Project = serde_json::from_str(json).expect("PRD sample parses");
        assert_eq!(p.schema_version, 2);
        assert_eq!(p.clips.len(), 3);
        assert_eq!(p.clips[1].name, "Jump Scare Reaction");
        assert_eq!(p.index_width(), 2);
        assert_eq!(p.source.fingerprint.size_bytes, 9_123_456_789);
        assert_eq!(p.export_settings.preset(), crate::Preset::Lossless);
        // Round-trips.
        let again: Project = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(again, p);
    }
}
