//! FRM-08: platform presets (vertical framing + length/fps limits). The values
//! live in an updatable JSON file, not in code; see `rustcut-export`'s loader.

use crate::export::{Framing, OutputResolution};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One platform preset (e.g. YouTube Shorts).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlatformPreset {
    pub id: String,
    pub label: String,
    /// `vertical_crop` or `vertical_blur`.
    pub framing: Framing,
    pub output_resolution: OutputResolution,
    /// Longest clip the platform accepts; `None` = no limit.
    #[serde(default)]
    pub max_duration_sec: Option<f64>,
    /// Frame-rate cap; `None` = keep the source rate.
    #[serde(default)]
    pub fps_cap: Option<u32>,
}
