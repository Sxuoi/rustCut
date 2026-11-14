//! Clips (PRD §4.2, §4.3, §7.5) and the edits the store applies to them.

use crate::export::{ExportSettings, Preset};
use crate::time::to_ms;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;
use xxhash_rust::xxh3::xxh3_64;

/// QUE-05 statuses. `exporting` resets to `pending` on load (PRJ-03).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExportStatus {
    #[default]
    Pending,
    Exporting,
    Completed,
    Failed,
    Outdated,
}

/// Result of the most recent export attempt of a clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LastExport {
    pub output_path: Option<String>,
    pub exported_at: DateTime<Utc>,
    /// Hash of the clip's times and crop plus the export settings used (§7.5).
    pub settings_hash: String,
    pub error: Option<String>,
    // ---- additions (optional on read) ----
    /// Preset used; lets the store decide which edits make the export outdated.
    #[serde(default)]
    pub preset: Option<Preset>,
    /// [`Clip::content_hash`] of the clip values that were exported.
    #[serde(default)]
    pub content_hash: Option<String>,
    /// FFmpeg exit code of a failed export (EXP-10).
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Last 20 lines of FFmpeg output of a failed export (EXP-10).
    #[serde(default)]
    pub log_tail: Vec<String>,
    /// Full per-clip log file.
    #[serde(default)]
    pub log_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Clip {
    /// UUID v7 (time-sortable).
    pub id: Uuid,
    /// Stable clip number (QUE-02), never reused.
    pub index: u32,
    pub name: String,
    pub start_sec: f64,
    pub end_sec: f64,
    pub tag_id: Option<String>,
    /// [-1, +1]; -1 = left edge, 0 = centre, +1 = right edge (FRM-03).
    pub crop_x_offset: f64,
    pub selected_for_export: bool,
    pub export_status: ExportStatus,
    pub last_export: Option<LastExport>,
}

/// The user-editable part of a clip. Undo/redo restores exactly these fields;
/// export status is system state and is recomputed, never rolled back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ClipFields {
    pub name: String,
    pub start_sec: f64,
    pub end_sec: f64,
    pub tag_id: Option<String>,
    pub crop_x_offset: f64,
    pub selected_for_export: bool,
}

impl Clip {
    pub fn duration(&self) -> f64 {
        self.end_sec - self.start_sec
    }

    pub fn fields(&self) -> ClipFields {
        ClipFields {
            name: self.name.clone(),
            start_sec: self.start_sec,
            end_sec: self.end_sec,
            tag_id: self.tag_id.clone(),
            crop_x_offset: self.crop_x_offset,
            selected_for_export: self.selected_for_export,
        }
    }

    /// Overwrite the user-editable fields, then recompute Completed/Outdated.
    pub fn set_fields(&mut self, f: &ClipFields) {
        let before = self.fields();
        self.name = f.name.clone();
        self.start_sec = f.start_sec;
        self.end_sec = f.end_sec;
        self.tag_id = f.tag_id.clone();
        self.crop_x_offset = f.crop_x_offset;
        self.selected_for_export = f.selected_for_export;
        self.after_user_edit(&before);
    }

    /// Hash of the clip values that determine an export's content and file
    /// name under `preset` (8 hex chars). Crop only matters for Vertical Crop.
    pub fn content_hash(&self, preset: Preset) -> String {
        let crop = if preset == Preset::Vcrop {
            (self.crop_x_offset * 10_000.0).round() as i64
        } else {
            0
        };
        let canonical = format!(
            "{}|{}|{}|{}|{}",
            to_ms(self.start_sec),
            to_ms(self.end_sec),
            crop,
            self.name,
            preset.as_str()
        );
        format!("{:08x}", xxh3_64(canonical.as_bytes()) as u32)
    }

    /// Recompute Completed <-> Outdated from [`LastExport::content_hash`].
    /// Other statuses are left alone.
    pub fn refresh_status(&mut self) {
        let Some(le) = &self.last_export else { return };
        let (Some(preset), Some(hash)) = (le.preset, le.content_hash.as_deref()) else {
            return;
        };
        let current = self.content_hash(preset);
        match self.export_status {
            ExportStatus::Completed if current != hash => {
                self.export_status = ExportStatus::Outdated
            }
            ExportStatus::Outdated if current == hash => {
                self.export_status = ExportStatus::Completed
            }
            _ => {}
        }
    }

    /// Apply EXP-01 / QUE-05 after a user edit: a completed clip whose output
    /// no longer matches becomes Outdated (and back, if the edit is undone).
    /// Exporting clips stay Exporting; the engine compares hashes when the
    /// job ends.
    pub fn after_user_edit(&mut self, before: &ClipFields) {
        let has_hash = self
            .last_export
            .as_ref()
            .is_some_and(|le| le.preset.is_some() && le.content_hash.is_some());
        if has_hash {
            self.refresh_status();
        } else if self.export_status == ExportStatus::Completed {
            // Files written by older versions carry no content hash: any change
            // to times, name or crop marks the export outdated.
            let changed = to_ms(before.start_sec) != to_ms(self.start_sec)
                || to_ms(before.end_sec) != to_ms(self.end_sec)
                || before.name != self.name
                || (before.crop_x_offset - self.crop_x_offset).abs() > 1e-9;
            if changed {
                self.export_status = ExportStatus::Outdated;
            }
        }
    }
}

/// `settings_hash` (§7.5): clip content plus every output-affecting export
/// setting. Equal hashes mean a re-export would produce the same file.
pub fn settings_hash(clip: &Clip, settings: &ExportSettings) -> String {
    let preset = settings.preset();
    let canonical = format!(
        "{}#{}",
        clip.content_hash(preset),
        settings.output_signature()
    );
    format!("{:08x}", xxh3_64(canonical.as_bytes()) as u32)
}

/// An undoable edit applied to one or more clips (MRK-04, QUE-07).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum ClipEdit {
    Rename {
        name: String,
    },
    /// Set In and/or Out (MRK-05 trim, `Alt+I`/`Alt+O`). Values are clamped to
    /// the source and must keep `end - start >= min clip duration`.
    Retime {
        start_sec: Option<f64>,
        end_sec: Option<f64>,
    },
    SetTag {
        tag_id: Option<String>,
    },
    /// Clamped to [-1, 1].
    SetCrop {
        crop_x_offset: f64,
    },
    SetIncluded {
        included: bool,
    },
}

impl ClipEdit {
    /// Label for undo/redo menus ("Undo Rename clip").
    pub fn label(&self) -> &'static str {
        match self {
            ClipEdit::Rename { .. } => "Rename clip",
            ClipEdit::Retime { .. } => "Change clip times",
            ClipEdit::SetTag { .. } => "Change tag",
            ClipEdit::SetCrop { .. } => "Change crop position",
            ClipEdit::SetIncluded { .. } => "Toggle include",
        }
    }
}

/// What the export engine reports for each finished clip; the store applies
/// it to `export_status` / `last_export` (not undoable).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExportRecord {
    pub clip_id: Uuid,
    pub outcome: crate::ipc::ClipOutcome,
    pub exported_at: DateTime<Utc>,
    pub preset: Preset,
    /// [`settings_hash`] of the job (computed from the clip as it was exported).
    pub settings_hash: String,
    /// [`Clip::content_hash`] of the clip as it was exported.
    pub content_hash: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip() -> Clip {
        Clip {
            id: Uuid::now_v7(),
            index: 2,
            name: "Jump Scare Reaction".into(),
            start_sec: 920.12,
            end_sec: 955.8,
            tag_id: None,
            crop_x_offset: 0.3,
            selected_for_export: true,
            export_status: ExportStatus::Completed,
            last_export: None,
        }
    }

    #[test]
    fn outdated_roundtrip_with_hash() {
        let mut c = clip();
        let h = c.content_hash(Preset::Lossless);
        c.last_export = Some(LastExport {
            output_path: Some("x.mp4".into()),
            exported_at: crate::now_utc(),
            settings_hash: "00000000".into(),
            error: None,
            preset: Some(Preset::Lossless),
            content_hash: Some(h),
            exit_code: None,
            log_tail: vec![],
            log_path: None,
        });
        let original = c.fields();
        let mut f = c.fields();
        f.crop_x_offset = -0.5; // crop is irrelevant to a lossless export
        c.set_fields(&f);
        assert_eq!(c.export_status, ExportStatus::Completed);
        f.name = "Renamed".into();
        c.set_fields(&f);
        assert_eq!(c.export_status, ExportStatus::Outdated);
        c.set_fields(&original);
        assert_eq!(c.export_status, ExportStatus::Completed);
    }

    #[test]
    fn outdated_without_hash() {
        let mut c = clip();
        let mut f = c.fields();
        f.end_sec = 960.0;
        c.set_fields(&f);
        assert_eq!(c.export_status, ExportStatus::Outdated);
    }

    #[test]
    fn settings_hash_changes_with_settings() {
        let c = clip();
        let mut s = ExportSettings::default();
        let a = settings_hash(&c, &s);
        s.naming_template = "{name}".into();
        assert_ne!(a, settings_hash(&c, &s));
        assert_eq!(a.len(), 8);
    }
}
