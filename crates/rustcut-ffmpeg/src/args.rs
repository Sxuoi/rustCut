//! Export command model and argument vectors (PRD §7.6).
//!
//! Every command is an argument vector, never a shell string; paths pass
//! through unmodified (Unicode, spaces). [`ExportCommand::to_args`] emits the
//! options in the exact order of the PRD examples B–E so golden tests can
//! compare whole vectors:
//!
//! ```text
//! -hide_banner -nostdin -y [hw device] -ss S -to E -i IN
//! [-filter_complex G] -map V -map A [-vf F]
//! (-c copy | -c:v copy | -c:v ENC <enc opts> [-maxrate M -bufsize B] [-threads N] [-pix_fmt P] -fps_mode passthrough)
//! [-tag:v hvc1] [-c:a ... [-ac 2] | -an] [-avoid_negative_ts make_zero]
//! [-color_primaries bt709 -color_trc bt709 -colorspace bt709] [-movflags +faststart]
//! -progress pipe:1 -nostats -f MUXER OUT.part
//! ```

use rustcut_model::EncoderChoice;
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};

/// x264 preset of every re-encode preset (§7.6 C–E).
pub const X264_PRESET: &str = "fast";
/// CRF of Frame-Accurate *Original* (EXP-02: visually lossless).
pub const CRF_ACCURATE: u8 = 18;
/// CRF of the vertical presets (EXP-02).
pub const CRF_VERTICAL: u8 = 20;
/// `-maxrate` of the vertical presets (§7.6 D/E, EXP-11).
pub const VERTICAL_MAXRATE_BPS: u64 = 16_000_000;
/// `-bufsize` of the vertical presets (§7.6 D/E, EXP-11).
pub const VERTICAL_BUFSIZE_BITS: u64 = 32_000_000;
/// AAC bit rate of every re-encoded audio track (§7.6 C–E).
pub const AAC_KBPS: u32 = 192;
/// Output pixel format of software re-encodes (§7.6 C/D).
pub const OUTPUT_PIX_FMT: &str = "yuv420p";
/// Output label of the *Mix all tracks* audio graph (EXP-04, P1).
pub const MIX_OUTPUT_LABEL: &str = "[aout]";

/// Output muxer, always passed explicitly with `-f` because the output path
/// ends in `.part` (EXP-08).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Muxer {
    Mp4,
    Mov,
    Matroska,
    Webm,
    Flv,
    Mpegts,
}

impl Muxer {
    /// Name for `-f`.
    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            Muxer::Mp4 => "mp4",
            Muxer::Mov => "mov",
            Muxer::Matroska => "matroska",
            Muxer::Webm => "webm",
            Muxer::Flv => "flv",
            Muxer::Mpegts => "mpegts",
        }
    }

    /// File extension without the dot (EXP-05: appended automatically).
    pub fn extension(self) -> &'static str {
        match self {
            Muxer::Mp4 => "mp4",
            Muxer::Mov => "mov",
            Muxer::Matroska => "mkv",
            Muxer::Webm => "webm",
            Muxer::Flv => "flv",
            Muxer::Mpegts => "ts",
        }
    }

    /// Short user-facing container name.
    pub fn label(self) -> &'static str {
        match self {
            Muxer::Mp4 => "MP4",
            Muxer::Mov => "MOV",
            Muxer::Matroska => "MKV",
            Muxer::Webm => "WebM",
            Muxer::Flv => "FLV",
            Muxer::Mpegts => "MPEG-TS",
        }
    }

    /// ISO-BMFF muxers: HEVC needs the `hvc1` tag (EXP-03) and
    /// `-movflags +faststart` applies.
    pub fn is_iso_bmff(self) -> bool {
        matches!(self, Muxer::Mp4 | Muxer::Mov)
    }
}

/// Video handling of an export.
#[derive(Debug, Clone, PartialEq)]
pub enum VideoPlan {
    /// Lossless stream copy (§7.6 B). `hvc1_tag` adds `-tag:v hvc1`
    /// (HEVC into MP4/MOV, EXP-03).
    Copy { hvc1_tag: bool },
    /// Frame-Accurate re-encode (§7.6 C–F).
    Encode(VideoEncode),
}

/// Video encoder selection.
#[derive(Debug, Clone, PartialEq)]
pub enum VideoCodec {
    /// `-c:v libx264 -preset P -crf N`.
    Libx264 { preset: String, crf: u8 },
    /// `-c:v <encoder> <args>`; `args` come from [`crate::hw::hw_video_args`]
    /// and already contain the rate control (EXP-13).
    Hardware {
        encoder: EncoderChoice,
        args: Vec<String>,
    },
}

impl VideoCodec {
    /// FFmpeg encoder name.
    pub fn ffmpeg_name(&self) -> &'static str {
        match self {
            VideoCodec::Libx264 { .. } => "libx264",
            VideoCodec::Hardware { encoder, .. } => encoder.ffmpeg_name().unwrap_or("libx264"),
        }
    }
}

/// Video filter of a re-encode.
#[derive(Debug, Clone, PartialEq)]
pub enum VideoFilter {
    /// `-vf CHAIN` (chains C/D, optionally prefixed by chain F).
    Simple(String),
    /// `-filter_complex GRAPH` + `-map OUT_LABEL` (chain E).
    Complex { graph: String, out_label: String },
}

/// Re-encode parameters (§7.6 C–F, EXP-12, EXP-13).
#[derive(Debug, Clone, PartialEq)]
pub struct VideoEncode {
    pub codec: VideoCodec,
    pub filter: Option<VideoFilter>,
    /// `-maxrate` (software encoder; hardware encoders carry it in their args).
    pub maxrate_bps: Option<u64>,
    /// `-bufsize` (software encoder).
    pub bufsize_bits: Option<u64>,
    /// `-pix_fmt`; `None` when the filter graph fixes the format (chain E)
    /// or frames are hardware surfaces (VAAPI).
    pub pix_fmt: Option<String>,
    /// `-threads N` for parallel software jobs (EXP-12).
    pub threads: Option<u32>,
    /// Tag the output BT.709 after chain F (FRM-07).
    pub bt709_tags: bool,
    /// Global options placed before the input (`-vaapi_device …`).
    pub hw_device_args: Vec<String>,
}

/// Audio handling of an export (EXP-04). Track numbers are 0-based audio
/// indices (`0:a:N`). Subtitle, data and attachment streams are never mapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPlan {
    /// Every audio track, stream copy (`-map 0:a?`).
    CopyAll,
    /// Every audio track re-encoded to AAC (`-map 0:a?`).
    AacAll { kbps: u32 },
    /// One track re-encoded to AAC (`-map 0:a:N?`), optionally downmixed
    /// to stereo (`-ac 2`).
    AacTrack { track: u32, kbps: u32, stereo: bool },
    /// *Mix all tracks* (P1): `tracks` audio streams mixed by `amix` into one
    /// stereo AAC track. With fewer than two tracks it degrades to
    /// `AacTrack { track: 0, stereo: true }`. `skip` is a bit mask of audio
    /// indices FFmpeg cannot decode (see `compat::unsupported_audio_indices`);
    /// they are left out of the mix.
    MixAll { tracks: u32, kbps: u32, skip: u64 },
    /// No audio (`-an`).
    None,
}

impl AudioPlan {
    /// The `-filter_complex` fragment of [`AudioPlan::MixAll`], ending in
    /// [`MIX_OUTPUT_LABEL`]; `None` for every other plan. Its labels never
    /// collide with chain E's (`[bg]`, `[fg]`, `[bgv]`, `[fgv]`, `[vout]`),
    /// so both join into one graph with `;`.
    pub fn mix_graph(&self) -> Option<String> {
        match *self {
            AudioPlan::MixAll { tracks, skip, .. } if tracks >= 2 => {
                let kept: Vec<u32> = (0..tracks)
                    .filter(|i| *i >= 64 || skip & (1u64 << *i) == 0)
                    .collect();
                if kept.len() < 2 {
                    return None;
                }
                let inputs: String = kept.iter().map(|i| format!("[0:a:{i}]")).collect();
                let n = kept.len();
                Some(format!(
                    "{inputs}amix=inputs={n}:duration=longest:normalize=0{MIX_OUTPUT_LABEL}"
                ))
            }
            _ => None,
        }
    }

    fn map_spec(&self) -> Option<String> {
        match *self {
            AudioPlan::CopyAll | AudioPlan::AacAll { .. } => Some("0:a?".to_string()),
            AudioPlan::AacTrack { track, .. } => Some(format!("0:a:{track}?")),
            AudioPlan::MixAll { .. } if self.mix_graph().is_some() => {
                Some(MIX_OUTPUT_LABEL.to_string())
            }
            AudioPlan::MixAll { skip, .. } => {
                let first = (0..64).find(|i| skip & (1u64 << i) == 0).unwrap_or(0);
                Some(format!("0:a:{first}?"))
            }
            AudioPlan::None => None,
        }
    }
}

/// One FFmpeg export invocation (EXP-08: written to `output_part`, renamed by
/// the caller on success).
#[derive(Debug, Clone, PartialEq)]
pub struct ExportCommand {
    pub input: OsString,
    pub output_part: OsString,
    /// `-ss`, seconds relative to the source's first PTS.
    pub start_sec: f64,
    /// `-to`, an absolute position on the same timeline.
    pub end_sec: f64,
    pub muxer: Muxer,
    pub video: VideoPlan,
    pub audio: AudioPlan,
    /// 0-based audio indices never exported because FFmpeg cannot decode them
    /// (`-map -0:a:N` after `-map 0:a?`; only for the plans that map every track).
    pub skip_audio: Vec<u32>,
    /// `-movflags +faststart` (re-encodes; ignored for non-ISO-BMFF muxers).
    pub faststart: bool,
}

impl ExportCommand {
    /// Nominal output duration (`end - start`, never negative). Lossless
    /// outputs are longer by the keyframe lead-in (EXP-02).
    pub fn duration_sec(&self) -> f64 {
        let d = self.end_sec - self.start_sec;
        if d.is_finite() && d > 0.0 { d } else { 0.0 }
    }

    /// The `-filter_complex` value, if any: chain E and/or the audio mix.
    pub fn filter_complex(&self) -> Option<String> {
        let mut graphs = Vec::new();
        if let VideoPlan::Encode(VideoEncode {
            filter: Some(VideoFilter::Complex { graph, .. }),
            ..
        }) = &self.video
        {
            graphs.push(graph.clone());
        }
        if let Some(mix) = self.audio.mix_graph() {
            graphs.push(mix);
        }
        if graphs.is_empty() {
            None
        } else {
            Some(graphs.join(";"))
        }
    }

    /// The full argument vector (program name excluded).
    pub fn to_args(&self) -> Vec<OsString> {
        let mut a = Args::default();
        a.push_all(["-hide_banner", "-nostdin", "-y"]);
        if let VideoPlan::Encode(enc) = &self.video {
            a.push_all(enc.hw_device_args.iter());
        }
        let reencode = matches!(self.video, VideoPlan::Encode(_));
        let t = |sec: f64| {
            if reencode {
                fmt_time_reencode(sec)
            } else {
                fmt_time(sec)
            }
        };
        a.push_all([
            "-ss".to_string(),
            t(self.start_sec),
            "-to".to_string(),
            t(self.end_sec),
        ]);
        a.push("-i");
        a.push(safe_path_arg(&self.input));
        if let Some(graph) = self.filter_complex() {
            a.push("-filter_complex");
            a.push(graph);
        }
        self.push_maps(&mut a);
        self.push_video_codec(&mut a);
        self.push_audio_codec(&mut a);
        match &self.video {
            VideoPlan::Copy { .. } => a.push_all(["-avoid_negative_ts", "make_zero"]),
            VideoPlan::Encode(enc) if enc.bt709_tags => a.push_all(rustcut_framing::BT709_TAG_ARGS),
            VideoPlan::Encode(_) => {}
        }
        if self.faststart && self.muxer.is_iso_bmff() {
            a.push_all(["-movflags", "+faststart"]);
        }
        a.push_all([
            "-progress",
            "pipe:1",
            "-nostats",
            "-f",
            self.muxer.ffmpeg_name(),
        ]);
        a.push(safe_path_arg(&self.output_part));
        a.0
    }

    /// [`Self::to_args`] as UTF-8 strings (lossy), for logs and tests.
    pub fn to_string_args(&self) -> Vec<String> {
        self.to_args()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    fn push_maps(&self, a: &mut Args) {
        a.push("-map");
        match &self.video {
            VideoPlan::Encode(VideoEncode {
                filter: Some(VideoFilter::Complex { out_label, .. }),
                ..
            }) => a.push(out_label),
            _ => a.push("0:v:0"),
        }
        if let Some(spec) = self.audio.map_spec() {
            a.push("-map");
            a.push(spec);
            if matches!(self.audio, AudioPlan::CopyAll | AudioPlan::AacAll { .. }) {
                for idx in &self.skip_audio {
                    a.push("-map");
                    a.push(format!("-0:a:{idx}"));
                }
            }
        }
        if let VideoPlan::Encode(VideoEncode {
            filter: Some(VideoFilter::Simple(chain)),
            ..
        }) = &self.video
        {
            a.push("-vf");
            a.push(chain);
        }
    }

    fn push_video_codec(&self, a: &mut Args) {
        match &self.video {
            VideoPlan::Copy { hvc1_tag } => {
                if self.audio == AudioPlan::CopyAll {
                    a.push_all(["-c", "copy"]);
                } else {
                    a.push_all(["-c:v", "copy"]);
                }
                if *hvc1_tag {
                    a.push_all(["-tag:v", "hvc1"]);
                }
            }
            VideoPlan::Encode(enc) => {
                a.push("-c:v");
                a.push(enc.codec.ffmpeg_name());
                match &enc.codec {
                    VideoCodec::Libx264 { preset, crf } => {
                        a.push_all([
                            "-preset".to_string(),
                            preset.clone(),
                            "-crf".to_string(),
                            crf.to_string(),
                        ]);
                    }
                    VideoCodec::Hardware { args, .. } => a.push_all(args.iter()),
                }
                if let Some(rate) = enc.maxrate_bps {
                    a.push_all(["-maxrate".to_string(), fmt_bitrate(rate)]);
                }
                if let Some(size) = enc.bufsize_bits {
                    a.push_all(["-bufsize".to_string(), fmt_bitrate(size)]);
                }
                if let Some(threads) = enc.threads.filter(|t| *t > 0) {
                    a.push_all(["-threads".to_string(), threads.to_string()]);
                }
                if let Some(pix) = &enc.pix_fmt {
                    a.push("-pix_fmt");
                    a.push(pix);
                }
                a.push_all(["-fps_mode", "passthrough"]);
            }
        }
    }

    fn push_audio_codec(&self, a: &mut Args) {
        let aac = |a: &mut Args, kbps: u32| {
            a.push_all([
                "-c:a".to_string(),
                "aac".into(),
                "-b:a".into(),
                format!("{kbps}k"),
            ])
        };
        match self.audio {
            AudioPlan::CopyAll => {
                if matches!(self.video, VideoPlan::Encode(_)) {
                    a.push_all(["-c:a", "copy"]);
                }
            }
            AudioPlan::AacAll { kbps } => aac(a, kbps),
            AudioPlan::AacTrack { kbps, stereo, .. } => {
                aac(a, kbps);
                if stereo {
                    a.push_all(["-ac", "2"]);
                }
            }
            AudioPlan::MixAll { kbps, .. } => {
                aac(a, kbps);
                a.push_all(["-ac", "2"]);
            }
            AudioPlan::None => a.push("-an"),
        }
    }
}

/// Small argv builder.
#[derive(Default)]
struct Args(Vec<OsString>);

impl Args {
    fn push(&mut self, s: impl AsRef<OsStr>) {
        self.0.push(s.as_ref().to_os_string());
    }

    fn push_all<I, S>(&mut self, items: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.0
            .extend(items.into_iter().map(|s| s.as_ref().to_os_string()));
    }
}

/// FFmpeg time argument: seconds with exactly three decimals (`920.120`),
/// never exponent notation. Negative, NaN and infinite values become
/// `0.000`.
pub fn fmt_time(sec: f64) -> String {
    let s = if sec.is_finite() && sec > 0.0 {
        sec
    } else {
        0.0
    };
    format!("{s:.3}")
}

/// Time argument for re-encodes (`-ss`/`-to`). Marks are stored rounded to the
/// millisecond, which for non-integral-ms frame times (30/60 fps) can land just
/// after the real frame. Biasing by half a millisecond and flooring keeps the
/// In frame (inclusive) and excludes the Out frame (exclusive).
pub fn fmt_time_reencode(sec: f64) -> String {
    let s = if sec.is_finite() && sec > 0.0 {
        sec
    } else {
        0.0
    };
    let ms = (s * 1000.0 - 0.5).floor().max(0.0);
    format!("{:.3}", ms / 1000.0)
}

/// FFmpeg bit-rate argument with SI suffixes, as in the PRD (`16M`, `192k`);
/// other values are printed in full.
pub fn fmt_bitrate(bps: u64) -> String {
    if bps > 0 && bps.is_multiple_of(1_000_000) {
        format!("{}M", bps / 1_000_000)
    } else if bps > 0 && bps.is_multiple_of(1_000) {
        format!("{}k", bps / 1_000)
    } else {
        bps.to_string()
    }
}

/// Defence in depth: a relative path is prefixed with `./` so a file name that
/// starts with `-` can never be read as an FFmpeg/FFprobe option. Absolute
/// (rooted or drive-prefixed) paths and `./`-prefixed ones are unchanged.
pub fn safe_path_arg(path: &OsStr) -> OsString {
    use std::path::{Component, Path};
    let p = Path::new(path);
    let rooted = matches!(
        p.components().next(),
        Some(Component::Prefix(_) | Component::RootDir | Component::CurDir)
    );
    if rooted || path.is_empty() {
        return path.to_os_string();
    }
    let mut out = OsString::from("./");
    out.push(path);
    out
}

/// FFprobe stream/format probe (§7.6 A): `-v error -show_format
/// -show_streams -of json PATH`. Parse with [`crate::probe::parse_probe_json`].
pub fn probe_args(path: impl AsRef<OsStr>) -> Vec<OsString> {
    let mut a = Args::default();
    a.push_all([
        "-v",
        "error",
        "-show_format",
        "-show_streams",
        "-of",
        "json",
    ]);
    a.push(safe_path_arg(path.as_ref()));
    a.0
}

/// FFprobe media-index scan (§7.6 A, PLY-06): one `pts_time,flags` line per
/// video packet. Parse with [`crate::index::parse_packet_csv`].
pub fn packet_index_args(path: impl AsRef<OsStr>) -> Vec<OsString> {
    let mut a = Args::default();
    a.push_all([
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "packet=pts_time,flags",
        "-of",
        "csv=p=0",
    ]);
    a.push(safe_path_arg(path.as_ref()));
    a.0
}

/// FFprobe check of a finished export (container readable, streams, duration).
/// Parse with [`crate::probe::parse_verify_json`].
pub fn verify_args(path: impl AsRef<OsStr>) -> Vec<OsString> {
    let mut a = Args::default();
    a.push_all([
        "-v",
        "error",
        "-show_entries",
        "format=duration,format_name:stream=index,codec_type,codec_name,width,height,channels",
        "-of",
        "json",
    ]);
    a.push(safe_path_arg(path.as_ref()));
    a.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_get_dot_slash_prefix() {
        assert_eq!(
            safe_path_arg(OsStr::new("-evil.mp4")),
            OsString::from("./-evil.mp4")
        );
        assert_eq!(
            safe_path_arg(OsStr::new("sub/a.mp4")),
            OsString::from("./sub/a.mp4")
        );
        assert_eq!(
            safe_path_arg(OsStr::new("./a.mp4")),
            OsString::from("./a.mp4")
        );
        #[cfg(windows)]
        {
            assert_eq!(
                safe_path_arg(OsStr::new(r"C:\v\-a.mp4")),
                OsString::from(r"C:\v\-a.mp4")
            );
            assert_eq!(
                safe_path_arg(OsStr::new("C:-a.mp4")),
                OsString::from("C:-a.mp4")
            );
        }
        #[cfg(not(windows))]
        assert_eq!(
            safe_path_arg(OsStr::new("/v/-a.mp4")),
            OsString::from("/v/-a.mp4")
        );
    }

    #[test]
    fn export_and_probe_argv_never_expose_a_dash_name_as_an_option() {
        let cmd = ExportCommand {
            input: "-in.mp4".into(),
            output_part: "-out.mp4.part".into(),
            start_sec: 0.0,
            end_sec: 1.0,
            muxer: Muxer::Mp4,
            video: VideoPlan::Copy { hvc1_tag: false },
            audio: AudioPlan::CopyAll,
            skip_audio: Vec::new(),
            faststart: false,
        };
        let args = cmd.to_string_args();
        let i = args.iter().position(|a| a == "-i").unwrap();
        assert_eq!(args[i + 1], "./-in.mp4");
        assert_eq!(args.last().unwrap(), "./-out.mp4.part");
        for f in [probe_args, packet_index_args, verify_args] {
            assert_eq!(f("-x.mp4").last().unwrap(), "./-x.mp4");
        }
    }

    #[test]
    fn fmt_time_reencode_biases_half_millisecond() {
        // 60 fps frame 481 = 8.01667 s is stored as mark 8.017
        assert_eq!(fmt_time_reencode(8.017), "8.016");
        assert_eq!(fmt_time_reencode(8.083), "8.082");
        assert_eq!(fmt_time_reencode(2.0), "1.999");
        assert_eq!(fmt_time_reencode(0.0), "0.000");
        assert_eq!(fmt_time_reencode(f64::NAN), "0.000");
    }

    #[test]
    fn fmt_time_three_decimals_no_exponent_clamps_negatives() {
        assert_eq!(fmt_time(920.12), "920.120");
        assert_eq!(fmt_time(955.8), "955.800");
        assert_eq!(fmt_time(0.0), "0.000");
        assert_eq!(fmt_time(-0.0), "0.000");
        assert_eq!(fmt_time(-3.2), "0.000");
        assert_eq!(fmt_time(f64::NAN), "0.000");
        assert_eq!(fmt_time(f64::INFINITY), "0.000");
        assert_eq!(fmt_time(1e-7), "0.000");
        assert_eq!(fmt_time(10_800.0), "10800.000");
        let big = fmt_time(1e21);
        assert!(!big.contains('e') && big.ends_with(".000"), "{big}");
    }

    #[test]
    fn fmt_bitrate_uses_si_suffixes() {
        assert_eq!(fmt_bitrate(16_000_000), "16M");
        assert_eq!(fmt_bitrate(32_000_000), "32M");
        assert_eq!(fmt_bitrate(192_000), "192k");
        assert_eq!(fmt_bitrate(1_500_500), "1500500");
        assert_eq!(fmt_bitrate(0), "0");
    }

    #[test]
    fn muxer_names_and_extensions() {
        let all = [
            Muxer::Mp4,
            Muxer::Mov,
            Muxer::Matroska,
            Muxer::Webm,
            Muxer::Flv,
            Muxer::Mpegts,
        ];
        let names: Vec<_> = all.iter().map(|m| m.ffmpeg_name()).collect();
        assert_eq!(names, ["mp4", "mov", "matroska", "webm", "flv", "mpegts"]);
        let exts: Vec<_> = all.iter().map(|m| m.extension()).collect();
        assert_eq!(exts, ["mp4", "mov", "mkv", "webm", "flv", "ts"]);
        assert!(
            Muxer::Mp4.is_iso_bmff() && Muxer::Mov.is_iso_bmff() && !Muxer::Matroska.is_iso_bmff()
        );
    }

    #[test]
    fn mix_graph_labels_every_track() {
        let plan = AudioPlan::MixAll {
            tracks: 3,
            kbps: 192,
            skip: 0,
        };
        assert_eq!(
            plan.mix_graph().as_deref(),
            Some("[0:a:0][0:a:1][0:a:2]amix=inputs=3:duration=longest:normalize=0[aout]")
        );
        // A track FFmpeg cannot decode is left out of the mix.
        let skip1 = AudioPlan::MixAll {
            tracks: 3,
            kbps: 192,
            skip: 0b010,
        };
        assert_eq!(
            skip1.mix_graph().as_deref(),
            Some("[0:a:0][0:a:2]amix=inputs=2:duration=longest:normalize=0[aout]")
        );
        let skip2 = AudioPlan::MixAll {
            tracks: 2,
            kbps: 192,
            skip: 0b01,
        };
        assert_eq!(skip2.mix_graph(), None);
        assert_eq!(skip2.map_spec().as_deref(), Some("0:a:1?"));
        assert_eq!(
            AudioPlan::MixAll {
                tracks: 1,
                kbps: 192,
                skip: 0
            }
            .mix_graph(),
            None
        );
        assert_eq!(
            AudioPlan::MixAll {
                tracks: 1,
                kbps: 192,
                skip: 0
            }
            .map_spec()
            .as_deref(),
            Some("0:a:0?")
        );
    }

    #[test]
    fn duration_never_negative() {
        let cmd = ExportCommand {
            input: "in".into(),
            output_part: "out.part".into(),
            start_sec: 5.0,
            end_sec: 3.0,
            muxer: Muxer::Mp4,
            video: VideoPlan::Copy { hvc1_tag: false },
            audio: AudioPlan::CopyAll,
            skip_audio: Vec::new(),
            faststart: false,
        };
        assert_eq!(cmd.duration_sec(), 0.0);
        let mut cmd = cmd;
        cmd.end_sec = 8.0;
        cmd.skip_audio = vec![1];
        let args = cmd.to_string_args().join(" ");
        assert!(args.contains("-map 0:a? -map -0:a:1"), "{args}");
    }
}
