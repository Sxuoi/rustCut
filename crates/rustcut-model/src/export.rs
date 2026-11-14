//! `export_settings` block (PRD §7.5) and the presets of the Export dialog (§6.2).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// EXP-02 cut modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CutMode {
    /// Stream copy; starts on the keyframe at or before In.
    #[default]
    Lossless,
    /// Re-encode; starts exactly on In.
    Accurate,
}

/// FRM-01 framing options (Frame-Accurate only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Framing {
    #[default]
    Original,
    VerticalCrop,
    VerticalBlur,
}

/// The four presets of the Export dialog; also the `{preset}` naming variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Preset {
    Lossless,
    Accurate,
    Vcrop,
    Vblur,
}

impl Preset {
    pub const ALL: [Preset; 4] = [
        Preset::Lossless,
        Preset::Accurate,
        Preset::Vcrop,
        Preset::Vblur,
    ];

    /// `{preset}` value (EXP-05).
    pub fn as_str(self) -> &'static str {
        match self {
            Preset::Lossless => "lossless",
            Preset::Accurate => "accurate",
            Preset::Vcrop => "vcrop",
            Preset::Vblur => "vblur",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Preset::Lossless => "Lossless (Original)",
            Preset::Accurate => "Frame-Accurate (Original)",
            Preset::Vcrop => "Vertical Crop 9:16",
            Preset::Vblur => "Vertical Blur 9:16",
        }
    }

    pub fn cut_mode(self) -> CutMode {
        if self == Preset::Lossless {
            CutMode::Lossless
        } else {
            CutMode::Accurate
        }
    }

    pub fn framing(self) -> Framing {
        match self {
            Preset::Lossless | Preset::Accurate => Framing::Original,
            Preset::Vcrop => Framing::VerticalCrop,
            Preset::Vblur => Framing::VerticalBlur,
        }
    }

    pub fn is_vertical(self) -> bool {
        matches!(self, Preset::Vcrop | Preset::Vblur)
    }

    pub fn is_reencode(self) -> bool {
        self != Preset::Lossless
    }
}

/// FRM-06 output resolution for vertical presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum OutputResolution {
    #[default]
    #[serde(rename = "1080x1920")]
    Full1080x1920,
    #[serde(rename = "720x1280")]
    Hd720x1280,
    /// Window size, no upscaling (e.g. 608×1080 from a 1080p source).
    #[serde(rename = "native")]
    Native,
}

impl OutputResolution {
    /// Fixed output size, or `None` for Native.
    pub fn dims(self) -> Option<(u32, u32)> {
        match self {
            OutputResolution::Full1080x1920 => Some((1080, 1920)),
            OutputResolution::Hd720x1280 => Some((720, 1280)),
            OutputResolution::Native => None,
        }
    }
}

/// EXP-07 policy for files that already exist in the output folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ConflictPolicy {
    #[default]
    Rename,
    Overwrite,
    Skip,
}

/// Video encoder for re-encode presets (EXP-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum EncoderChoice {
    /// Best detected hardware encoder, else libx264.
    #[default]
    Auto,
    Libx264,
    H264Nvenc,
    H264Amf,
    H264Qsv,
    H264Videotoolbox,
    H264Vaapi,
}

impl EncoderChoice {
    /// FFmpeg encoder name, or `None` for Auto.
    pub fn ffmpeg_name(self) -> Option<&'static str> {
        match self {
            EncoderChoice::Auto => None,
            EncoderChoice::Libx264 => Some("libx264"),
            EncoderChoice::H264Nvenc => Some("h264_nvenc"),
            EncoderChoice::H264Amf => Some("h264_amf"),
            EncoderChoice::H264Qsv => Some("h264_qsv"),
            EncoderChoice::H264Videotoolbox => Some("h264_videotoolbox"),
            EncoderChoice::H264Vaapi => Some("h264_vaapi"),
        }
    }

    pub fn is_hardware(self) -> bool {
        !matches!(self, EncoderChoice::Auto | EncoderChoice::Libx264)
    }

    pub const HARDWARE: [EncoderChoice; 5] = [
        EncoderChoice::H264Nvenc,
        EncoderChoice::H264Amf,
        EncoderChoice::H264Qsv,
        EncoderChoice::H264Videotoolbox,
        EncoderChoice::H264Vaapi,
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExportSettings {
    /// `None` = `<source folder>/<source name>_clips` (§6.2).
    pub output_dir: Option<String>,
    /// File-name template without extension (EXP-05). Default `{index}_{name}`.
    pub naming_template: String,
    /// EXP-06 step 3 setting (default on).
    pub spaces_to_underscores: bool,
    pub on_conflict: ConflictPolicy,
    pub cut_mode: CutMode,
    pub framing: Framing,
    pub output_resolution: OutputResolution,
    /// Frame-rate cap for re-encodes; `None` = source rate (FRM-06). Default 60.
    pub fps_cap: Option<u32>,
    /// 1-based audio track for vertical presets (EXP-04). Default 1.
    pub audio_track: u32,
    pub encoder: EncoderChoice,
    /// Concurrent FFmpeg processes, 1–4 (EXP-12).
    pub parallel_jobs: u32,
    // ---- additions (optional on read) ----
    /// EXP-03: "MP4 with AAC audio, video stays lossless".
    #[serde(default)]
    pub lossless_audio_to_aac: bool,
    /// MRK-07: move In to the keyframe at or before it for lossless exports.
    #[serde(default)]
    pub snap_in_to_keyframe: bool,
    /// EXP-04 (P1): vertical presets mix all audio tracks instead of one.
    #[serde(default)]
    pub mix_all_audio: bool,
    /// FRM-08: maximum clip duration of the chosen platform preset.
    #[serde(default)]
    pub max_duration_sec: Option<f64>,
    /// FRM-08: id of the platform preset from the updatable preset file.
    #[serde(default)]
    pub platform_preset: Option<String>,
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings {
            output_dir: None,
            naming_template: "{index}_{name}".into(),
            spaces_to_underscores: true,
            on_conflict: ConflictPolicy::Rename,
            cut_mode: CutMode::Lossless,
            framing: Framing::Original,
            output_resolution: OutputResolution::Full1080x1920,
            fps_cap: Some(60),
            audio_track: 1,
            encoder: EncoderChoice::Auto,
            parallel_jobs: 1,
            lossless_audio_to_aac: false,
            snap_in_to_keyframe: false,
            mix_all_audio: false,
            max_duration_sec: None,
            platform_preset: None,
        }
    }
}

pub const DEFAULT_NAMING_TEMPLATE: &str = "{index}_{name}";

impl ExportSettings {
    /// Lossless ignores framing; re-encodes map framing to a preset.
    pub fn preset(&self) -> Preset {
        match (self.cut_mode, self.framing) {
            (CutMode::Lossless, _) => Preset::Lossless,
            (CutMode::Accurate, Framing::Original) => Preset::Accurate,
            (CutMode::Accurate, Framing::VerticalCrop) => Preset::Vcrop,
            (CutMode::Accurate, Framing::VerticalBlur) => Preset::Vblur,
        }
    }

    /// Choosing a vertical preset switches the cut mode automatically (§4.5).
    pub fn apply_preset(&mut self, preset: Preset) {
        self.cut_mode = preset.cut_mode();
        self.framing = preset.framing();
    }

    /// Clamp every field into its valid range.
    pub fn normalized(mut self) -> Self {
        self.parallel_jobs = self.parallel_jobs.clamp(1, 4);
        self.audio_track = self.audio_track.max(1);
        if let Some(cap) = self.fps_cap {
            self.fps_cap = if cap == 0 { None } else { Some(cap.min(240)) };
        }
        if self.naming_template.trim().is_empty() {
            self.naming_template = DEFAULT_NAMING_TEMPLATE.into();
        }
        if self.cut_mode == CutMode::Lossless {
            self.framing = Framing::Original;
        }
        if let Some(d) = self.max_duration_sec {
            self.max_duration_sec = if d.is_finite() && d > 0.0 {
                Some(d)
            } else {
                None
            };
        }
        if self
            .output_dir
            .as_deref()
            .is_some_and(|d| d.trim().is_empty())
        {
            self.output_dir = None;
        }
        self
    }

    /// Canonical string of every setting that changes an output file
    /// (content or path). Used by [`crate::clip::settings_hash`].
    pub fn output_signature(&self) -> String {
        let p = self.preset();
        let mut s = format!(
            "p={};dir={};tpl={};sp={}",
            p.as_str(),
            self.output_dir.as_deref().unwrap_or(""),
            self.naming_template,
            self.spaces_to_underscores
        );
        match p {
            Preset::Lossless => {
                s.push_str(&format!(
                    ";aac={};snap={}",
                    self.lossless_audio_to_aac, self.snap_in_to_keyframe
                ));
            }
            Preset::Accurate => {
                s.push_str(&format!(";fps={:?};enc={:?}", self.fps_cap, self.encoder));
            }
            Preset::Vcrop | Preset::Vblur => {
                s.push_str(&format!(
                    ";res={:?};fps={:?};enc={:?};a={};mix={}",
                    self.output_resolution,
                    self.fps_cap,
                    self.encoder,
                    self.audio_track,
                    self.mix_all_audio
                ));
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_spellings_match_prd() {
        let s = ExportSettings::default();
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["naming_template"], "{index}_{name}");
        assert_eq!(v["on_conflict"], "rename");
        assert_eq!(v["cut_mode"], "lossless");
        assert_eq!(v["framing"], "original");
        assert_eq!(v["output_resolution"], "1080x1920");
        assert_eq!(v["fps_cap"], 60);
        assert_eq!(v["encoder"], "auto");
        assert_eq!(
            serde_json::to_value(EncoderChoice::H264Nvenc).unwrap(),
            "h264_nvenc"
        );
        assert_eq!(
            serde_json::to_value(Framing::VerticalCrop).unwrap(),
            "vertical_crop"
        );
        assert_eq!(serde_json::to_value(Preset::Vblur).unwrap(), "vblur");
    }

    #[test]
    fn presets() {
        let mut s = ExportSettings::default();
        s.apply_preset(Preset::Vcrop);
        assert_eq!(s.cut_mode, CutMode::Accurate);
        assert_eq!(s.preset(), Preset::Vcrop);
        s.cut_mode = CutMode::Lossless;
        assert_eq!(s.preset(), Preset::Lossless);
        assert_eq!(s.normalized().framing, Framing::Original);
    }
}
