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
    /// FRM-09: a 1:1 window of the frame, centred on a blurred 9:16 background.
    VerticalSquare,
    /// FRM-09: square 1:1 output, the whole frame over a blurred background.
    SquareBlur,
}

impl Framing {
    /// A window the user positions per clip with `crop_x_offset` (FRM-03).
    pub fn uses_crop_offset(self) -> bool {
        matches!(self, Framing::VerticalCrop | Framing::VerticalSquare)
    }

    /// The output has a blurred background (chains E, G and H).
    pub fn has_blur(self) -> bool {
        matches!(
            self,
            Framing::VerticalBlur | Framing::VerticalSquare | Framing::SquareBlur
        )
    }
}

/// The presets of the Export dialog; also the `{preset}` naming variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Preset {
    Lossless,
    Accurate,
    Vcrop,
    Vblur,
    /// Square on Blur 9:16 (FRM-09).
    Vsquare,
    /// Square Blur 1:1 (FRM-09).
    Square,
}

impl Preset {
    pub const ALL: [Preset; 6] = [
        Preset::Lossless,
        Preset::Accurate,
        Preset::Vcrop,
        Preset::Vblur,
        Preset::Vsquare,
        Preset::Square,
    ];

    /// `{preset}` value (EXP-05).
    pub fn as_str(self) -> &'static str {
        match self {
            Preset::Lossless => "lossless",
            Preset::Accurate => "accurate",
            Preset::Vcrop => "vcrop",
            Preset::Vblur => "vblur",
            Preset::Vsquare => "vsquare",
            Preset::Square => "square",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Preset::Lossless => "Lossless (Original)",
            Preset::Accurate => "Frame-Accurate (Original)",
            Preset::Vcrop => "Vertical Crop 9:16",
            Preset::Vblur => "Vertical Blur 9:16",
            Preset::Vsquare => "Square on Blur 9:16",
            Preset::Square => "Square Blur 1:1",
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
            Preset::Vsquare => Framing::VerticalSquare,
            Preset::Square => Framing::SquareBlur,
        }
    }

    /// 9:16 output (Vertical Crop, Vertical Blur, Square on Blur).
    pub fn is_vertical(self) -> bool {
        matches!(self, Preset::Vcrop | Preset::Vblur | Preset::Vsquare)
    }

    /// Any framing besides the original: the vertical presets and Square Blur
    /// 1:1. They share the output size (FRM-06), one stereo audio track
    /// (EXP-04) and the platform limits (FRM-08).
    pub fn is_framed(self) -> bool {
        self.framing() != Framing::Original
    }

    /// The clip's `crop_x_offset` changes the output (FRM-03).
    pub fn uses_crop_offset(self) -> bool {
        self.framing().uses_crop_offset()
    }

    /// The video chain ends in a blurred-background composite (E, G, H).
    pub fn has_blur(self) -> bool {
        self.framing().has_blur()
    }

    pub fn is_reencode(self) -> bool {
        self != Preset::Lossless
    }
}

/// FRM-06 output resolution of the framed presets. The values name the 9:16
/// sizes; Square Blur 1:1 uses their width for both sides (1080×1080,
/// 720×720), so one setting serves every framed preset.
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
    /// Fixed 9:16 output size, or `None` for Native.
    pub fn dims(self) -> Option<(u32, u32)> {
        match self {
            OutputResolution::Full1080x1920 => Some((1080, 1920)),
            OutputResolution::Hd720x1280 => Some((720, 1280)),
            OutputResolution::Native => None,
        }
    }

    /// Fixed 1:1 output size (Square Blur 1:1), or `None` for Native.
    pub fn square_dims(self) -> Option<(u32, u32)> {
        self.dims().map(|(w, _)| (w, w))
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
            (CutMode::Accurate, Framing::VerticalSquare) => Preset::Vsquare,
            (CutMode::Accurate, Framing::SquareBlur) => Preset::Square,
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
            Preset::Vcrop | Preset::Vblur | Preset::Vsquare | Preset::Square => {
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
        assert_eq!(
            serde_json::to_value(Framing::VerticalSquare).unwrap(),
            "vertical_square"
        );
        assert_eq!(
            serde_json::to_value(Framing::SquareBlur).unwrap(),
            "square_blur"
        );
        assert_eq!(serde_json::to_value(Preset::Vsquare).unwrap(), "vsquare");
        assert_eq!(serde_json::to_value(Preset::Square).unwrap(), "square");
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

    #[test]
    fn every_preset_round_trips_through_its_settings() {
        for p in Preset::ALL {
            let mut s = ExportSettings::default();
            s.apply_preset(p);
            assert_eq!(s.preset(), p, "{p:?}");
            assert_eq!(s.normalized().preset(), p, "{p:?} survives normalisation");
        }
    }

    #[test]
    fn square_presets_classification() {
        assert!(Preset::Vsquare.is_vertical() && Preset::Vsquare.is_framed());
        assert!(!Preset::Square.is_vertical() && Preset::Square.is_framed());
        assert!(!Preset::Accurate.is_framed() && !Preset::Lossless.is_framed());
        let crop: Vec<Preset> = Preset::ALL
            .into_iter()
            .filter(|p| p.uses_crop_offset())
            .collect();
        assert_eq!(crop, [Preset::Vcrop, Preset::Vsquare]);
        let blur: Vec<Preset> = Preset::ALL.into_iter().filter(|p| p.has_blur()).collect();
        assert_eq!(blur, [Preset::Vblur, Preset::Vsquare, Preset::Square]);
        assert_eq!(OutputResolution::Hd720x1280.square_dims(), Some((720, 720)));
        assert_eq!(OutputResolution::Native.square_dims(), None);
    }

    #[test]
    fn existing_signatures_are_unchanged_and_new_presets_have_their_own() {
        let mut s = ExportSettings::default();
        s.apply_preset(Preset::Vblur);
        assert_eq!(
            s.output_signature(),
            "p=vblur;dir=;tpl={index}_{name};sp=true;res=Full1080x1920;fps=Some(60);enc=Auto;a=1;mix=false"
        );
        s.apply_preset(Preset::Vsquare);
        assert!(s.output_signature().starts_with("p=vsquare;"));
        s.apply_preset(Preset::Square);
        assert!(s.output_signature().starts_with("p=square;"));
        assert!(s.output_signature().contains(";res=Full1080x1920;"));
    }

    #[test]
    fn project_settings_with_the_new_framings_parse() {
        let mut v = serde_json::to_value(ExportSettings::default()).unwrap();
        v["cut_mode"] = "accurate".into();
        v["framing"] = "vertical_square".into();
        let s: ExportSettings = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(s.preset(), Preset::Vsquare);
        v["framing"] = "square_blur".into();
        let s: ExportSettings = serde_json::from_value(v).unwrap();
        assert_eq!(s.preset(), Preset::Square);
    }
}
