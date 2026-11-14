//! Clip + settings → [`ExportCommand`] (PRD §7.6 B–F).
//!
//! | Preset | Video | Audio (EXP-04) |
//! | :--- | :--- | :--- |
//! | Lossless (B) | stream copy, `hvc1` for HEVC in MP4/MOV | every track, copied (or AAC when EXP-03's offer was accepted) |
//! | Frame-Accurate (C) | libx264 CRF 18, `-vf` only for a frame-rate cap or chain F | every track → AAC 192k |
//! | Vertical Crop (D) | `-vf` chain D, CRF 20, 16M/32M cap | the chosen track → AAC 192k stereo, or *Mix all* |
//! | Vertical Blur (E) | `-filter_complex` chain E, CRF 20, 16M/32M cap | as D |
//!
//! Chains come from `rustcut_framing` (the same functions the preview
//! overlay uses, FRM-04). HDR sources get chain F and BT.709 tags when
//! `zscale` is available (FRM-07). Subtitle, data and attachment streams are
//! never mapped; sources without audio need no special case thanks to the
//! `?` map suffix.

use crate::args::{
    AAC_KBPS, AudioPlan, CRF_ACCURATE, CRF_VERTICAL, ExportCommand, Muxer, OUTPUT_PIX_FMT,
    VERTICAL_BUFSIZE_BITS, VERTICAL_MAXRATE_BPS, VideoCodec, VideoEncode, VideoFilter, VideoPlan,
    X264_PRESET,
};
use crate::compat::{ContainerDecision, LosslessAudio, unsupported_audio_indices};
use crate::hw::hw_video_args;
use rustcut_framing::{
    BLUR_OUTPUT_LABEL, FramingInput, original_filter, vertical_blur_filter, vertical_crop_filter,
};
use rustcut_model::{EncoderChoice, ExportSettings, Preset, SourceInfo};
use std::path::Path;

/// The video encoder a re-encode actually uses, after `Auto` was resolved
/// against the detected hardware (EXP-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedEncoder {
    Libx264,
    /// A hardware encoder; `Auto`/`Libx264` inside fall back to libx264.
    Hardware(EncoderChoice),
}

/// Everything needed to build one clip's command.
#[derive(Debug, Clone)]
pub struct JobSpec<'a> {
    /// The original source file (export never reads cache files, §7.3).
    pub input: &'a Path,
    /// `<final name>.part` (EXP-08).
    pub output_part: &'a Path,
    pub source: &'a SourceInfo,
    /// Clip In, seconds relative to the source's first PTS.
    pub start_sec: f64,
    /// Clip Out (exclusive: the last frame before Out is the last frame).
    pub end_sec: f64,
    /// FRM-03 crop position, −1…+1 (Vertical Crop only).
    pub crop_x_offset: f64,
    pub settings: &'a ExportSettings,
    /// From [`crate::compat::lossless_container`] (used by Lossless).
    pub container: &'a ContainerDecision,
    pub encoder: ResolvedEncoder,
    /// `-threads` for parallel software encodes (cores ÷ jobs, EXP-12).
    pub threads: Option<u32>,
    /// The FFmpeg build has `zscale` (chain F, FRM-07).
    pub zscale_available: bool,
    /// MRK-07 *Snap In to keyframe*: the keyframe at or before In. Lossless
    /// only; ignored by re-encodes.
    pub snap_in_to: Option<f64>,
}

/// Build the export command of one clip (PRD §7.6 B–F, EXP-02/03/04).
///
/// Out is clamped to the source duration when it lies beyond it (§9); a clip
/// whose In is past the end is left alone (the queue flags it *Out of
/// range*). Ranges are otherwise used as given: the project validates them
/// (MRK-01).
pub fn build_export_command(job: &JobSpec<'_>) -> ExportCommand {
    let preset = job.settings.preset();
    let (start_sec, end_sec) = clip_range(job, preset);
    let (muxer, video, audio, faststart) = match preset {
        Preset::Lossless => lossless_plan(job),
        Preset::Accurate | Preset::Vcrop | Preset::Vblur => reencode_plan(job, preset),
    };
    ExportCommand {
        input: job.input.as_os_str().to_os_string(),
        output_part: job.output_part.as_os_str().to_os_string(),
        start_sec,
        end_sec,
        muxer,
        video,
        audio,
        skip_audio: unsupported_audio_indices(job.source),
        faststart,
    }
}

fn clip_range(job: &JobSpec<'_>, preset: Preset) -> (f64, f64) {
    let mut start = job.start_sec;
    if preset == Preset::Lossless
        && let Some(kf) = job
            .snap_in_to
            .filter(|k| k.is_finite() && *k >= 0.0 && *k <= start + 1e-9)
    {
        start = ceil_ms(kf);
    }
    let duration = job.source.duration_sec;
    let mut end = job.end_sec;
    if duration.is_finite() && duration > 0.0 && end > duration && start < duration {
        end = duration;
    }
    (start, end)
}

/// Round a keyframe time up to the next millisecond, so `-ss` (3 decimals)
/// never lands just before the keyframe and makes FFmpeg seek one GOP early.
fn ceil_ms(t: f64) -> f64 {
    ((t * 1000.0 - 1e-6).ceil() / 1000.0).max(0.0)
}

type Plan = (Muxer, VideoPlan, AudioPlan, bool);

fn lossless_plan(job: &JobSpec<'_>) -> Plan {
    let muxer = job.container.muxer;
    let hvc1_tag = job.source.video.codec.eq_ignore_ascii_case("hevc") && muxer.is_iso_bmff();
    let audio = match job.container.audio {
        LosslessAudio::Copy => AudioPlan::CopyAll,
        LosslessAudio::AacTranscode => AudioPlan::AacAll { kbps: AAC_KBPS },
    };
    (muxer, VideoPlan::Copy { hvc1_tag }, audio, false)
}

fn reencode_plan(job: &JobSpec<'_>, preset: Preset) -> Plan {
    let framing = FramingInput::new(&job.source.video, job.settings, job.zscale_available);
    let (crf, cap, filter, pix_fmt) = match preset {
        Preset::Vcrop => (
            CRF_VERTICAL,
            Some((VERTICAL_MAXRATE_BPS, VERTICAL_BUFSIZE_BITS)),
            Some(VideoFilter::Simple(vertical_crop_filter(
                &framing,
                job.crop_x_offset,
            ))),
            Some(OUTPUT_PIX_FMT),
        ),
        Preset::Vblur => (
            CRF_VERTICAL,
            Some((VERTICAL_MAXRATE_BPS, VERTICAL_BUFSIZE_BITS)),
            Some(VideoFilter::Complex {
                graph: vertical_blur_filter(&framing),
                out_label: BLUR_OUTPUT_LABEL.to_string(),
            }),
            None,
        ),
        _ => (
            CRF_ACCURATE,
            None,
            original_filter(&framing).map(VideoFilter::Simple),
            Some(OUTPUT_PIX_FMT),
        ),
    };
    let mut enc = VideoEncode {
        codec: VideoCodec::Libx264 {
            preset: X264_PRESET.to_string(),
            crf,
        },
        filter,
        maxrate_bps: cap.map(|c| c.0),
        bufsize_bits: cap.map(|c| c.1),
        pix_fmt: pix_fmt.map(str::to_string),
        threads: job.threads.filter(|t| *t > 0),
        bt709_tags: framing.tonemap_hdr,
        hw_device_args: Vec::new(),
    };
    if let ResolvedEncoder::Hardware(choice) = job.encoder {
        apply_hardware(&mut enc, choice, crf, cap, preset);
    }
    let audio = match preset {
        Preset::Vcrop | Preset::Vblur => vertical_audio(job.source, job.settings),
        _ => AudioPlan::AacAll { kbps: AAC_KBPS },
    };
    (Muxer::Mp4, VideoPlan::Encode(enc), audio, true)
}

/// Swap libx264 for a hardware encoder (EXP-13). The rate control moves into
/// the encoder args; `-threads` is dropped (it splits CPU for software jobs).
fn apply_hardware(
    enc: &mut VideoEncode,
    choice: EncoderChoice,
    crf: u8,
    cap: Option<(u64, u64)>,
    preset: Preset,
) {
    let Some(hw) = hw_video_args(choice, crf, cap.map(|c| c.0), cap.map(|c| c.1)) else {
        return;
    };
    enc.codec = VideoCodec::Hardware {
        encoder: choice,
        args: hw.codec_args,
    };
    enc.maxrate_bps = None;
    enc.bufsize_bits = None;
    enc.threads = None;
    enc.hw_device_args = hw.device_args;
    // Chain E already ends in format=yuv420p; only a different format matters.
    enc.pix_fmt = match (preset, hw.pix_fmt) {
        (Preset::Vblur, Some(OUTPUT_PIX_FMT)) => None,
        (_, pix) => pix.map(str::to_string),
    };
    if let Some(suffix) = hw.filter_suffix {
        enc.filter = Some(match enc.filter.take() {
            None => VideoFilter::Simple(suffix.to_string()),
            Some(VideoFilter::Simple(chain)) => VideoFilter::Simple(format!("{chain},{suffix}")),
            Some(VideoFilter::Complex { graph, out_label }) => {
                let graph = match graph.strip_suffix(out_label.as_str()) {
                    Some(head) => format!("{head},{suffix}{out_label}"),
                    None => graph,
                };
                VideoFilter::Complex { graph, out_label }
            }
        });
    }
}

/// EXP-04 vertical presets: the chosen track (1-based setting, default 1)
/// downmixed to stereo, or *Mix all tracks* when there are at least two. A
/// track number beyond the source's tracks falls back to the first track
/// rather than silently exporting without sound.
fn vertical_audio(source: &SourceInfo, settings: &ExportSettings) -> AudioPlan {
    let tracks = u32::try_from(source.audio_tracks.len()).unwrap_or(u32::MAX);
    let unsupported = unsupported_audio_indices(source);
    let skip = unsupported
        .iter()
        .filter(|i| **i < 64)
        .fold(0u64, |m, i| m | (1u64 << *i));
    let usable = tracks.saturating_sub(u32::try_from(unsupported.len()).unwrap_or(0));
    if settings.mix_all_audio && usable >= 2 {
        return AudioPlan::MixAll {
            tracks,
            kbps: AAC_KBPS,
            skip,
        };
    }
    let wanted = settings.audio_track.max(1) - 1;
    // Beyond the source's tracks, or a track FFmpeg cannot decode: the first
    // usable track, rather than failing or silently exporting without sound.
    let track = if tracks > 0 && (wanted >= tracks || unsupported.contains(&wanted)) {
        (0..tracks).find(|i| !unsupported.contains(i)).unwrap_or(0)
    } else {
        wanted
    };
    AudioPlan::AacTrack {
        track,
        kbps: AAC_KBPS,
        stereo: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::lossless_container;
    use crate::testutil::source;
    use rustcut_model::{CutMode, Framing};

    fn settings(preset: Preset) -> ExportSettings {
        let mut s = ExportSettings::default();
        s.apply_preset(preset);
        s
    }

    fn run(
        src: &SourceInfo,
        s: &ExportSettings,
        start: f64,
        end: f64,
        snap: Option<f64>,
    ) -> ExportCommand {
        let container = lossless_container(src, s);
        build_export_command(&JobSpec {
            input: Path::new("in.mp4"),
            output_part: Path::new("out.mp4.part"),
            source: src,
            start_sec: start,
            end_sec: end,
            crop_x_offset: 0.0,
            settings: s,
            container: &container,
            encoder: ResolvedEncoder::Libx264,
            threads: None,
            zscale_available: true,
            snap_in_to: snap,
        })
    }

    #[test]
    fn mrk07_snap_rounds_keyframe_up_to_the_millisecond() {
        let src = source("mov,mp4", "a.mp4", "h264", &["aac"]);
        let cmd = run(&src, &settings(Preset::Lossless), 3.5, 8.0, Some(2.002002));
        assert_eq!(cmd.start_sec, 2.003);
        let cmd = run(&src, &settings(Preset::Lossless), 3.5, 8.0, Some(2.0));
        assert_eq!(cmd.start_sec, 2.0);
        // A "keyframe" after In is not a snap target; re-encodes ignore snapping.
        assert_eq!(
            run(&src, &settings(Preset::Lossless), 3.5, 8.0, Some(4.0)).start_sec,
            3.5
        );
        assert_eq!(
            run(&src, &settings(Preset::Accurate), 3.5, 8.0, Some(2.0)).start_sec,
            3.5
        );
    }

    #[test]
    fn edge_out_clamped_to_source_duration() {
        let src = source("mov,mp4", "a.mp4", "h264", &["aac"]);
        let cmd = run(&src, &settings(Preset::Accurate), 55.0, 75.0, None);
        assert_eq!((cmd.start_sec, cmd.end_sec), (55.0, 60.0));
        let out_of_range = run(&src, &settings(Preset::Accurate), 65.0, 75.0, None);
        assert_eq!((out_of_range.start_sec, out_of_range.end_sec), (65.0, 75.0));
    }

    #[test]
    fn exp04_vertical_track_selection() {
        let src = source("matroska,webm", "a.mkv", "h264", &["aac", "aac", "aac"]);
        let mut s = settings(Preset::Vcrop);
        s.audio_track = 3;
        assert_eq!(
            vertical_audio(&src, &s),
            AudioPlan::AacTrack {
                track: 2,
                kbps: 192,
                stereo: true
            }
        );
        s.audio_track = 9;
        assert_eq!(
            vertical_audio(&src, &s),
            AudioPlan::AacTrack {
                track: 0,
                kbps: 192,
                stereo: true
            }
        );
        s.mix_all_audio = true;
        assert_eq!(
            vertical_audio(&src, &s),
            AudioPlan::MixAll {
                tracks: 3,
                kbps: 192,
                skip: 0
            }
        );
        let one = source("matroska,webm", "a.mkv", "h264", &["aac"]);
        assert_eq!(
            vertical_audio(&one, &s),
            AudioPlan::AacTrack {
                track: 0,
                kbps: 192,
                stereo: true
            }
        );
        let silent = source("matroska,webm", "a.mkv", "h264", &[]);
        s.mix_all_audio = false;
        s.audio_track = 2;
        assert_eq!(
            vertical_audio(&silent, &s),
            AudioPlan::AacTrack {
                track: 1,
                kbps: 192,
                stereo: true
            }
        );
    }

    #[test]
    fn hardware_encoder_replaces_libx264_options() {
        let src = source("mov,mp4", "a.mp4", "h264", &["aac"]);
        let s = ExportSettings {
            cut_mode: CutMode::Accurate,
            framing: Framing::VerticalBlur,
            ..Default::default()
        };
        let container = lossless_container(&src, &s);
        let mut job = JobSpec {
            input: Path::new("in.mp4"),
            output_part: Path::new("out.mp4.part"),
            source: &src,
            start_sec: 1.0,
            end_sec: 2.0,
            crop_x_offset: 0.0,
            settings: &s,
            container: &container,
            encoder: ResolvedEncoder::Hardware(EncoderChoice::H264Vaapi),
            threads: Some(4),
            zscale_available: false,
            snap_in_to: None,
        };
        let args = build_export_command(&job).to_string_args().join(" ");
        assert!(
            args.starts_with(
                "-hide_banner -nostdin -y -vaapi_device /dev/dri/renderD128 -ss 0.999"
            ),
            "{args}"
        );
        assert!(
            args.contains("format=yuv420p,format=nv12,hwupload[vout]"),
            "{args}"
        );
        assert!(
            !args.contains("-threads") && !args.contains("-pix_fmt"),
            "{args}"
        );
        job.encoder = ResolvedEncoder::Hardware(EncoderChoice::H264Qsv);
        let args = build_export_command(&job).to_string_args().join(" ");
        assert!(args.contains(
            "-c:v h264_qsv -preset medium -global_quality 20 -pix_fmt nv12 -fps_mode passthrough"
        ));
        job.encoder = ResolvedEncoder::Hardware(EncoderChoice::Auto);
        let args = build_export_command(&job).to_string_args().join(" ");
        assert!(
            args.contains("-c:v libx264 -preset fast -crf 20 -maxrate 16M -bufsize 32M -threads 4"),
            "{args}"
        );
    }

    #[test]
    fn unsupported_apple_apac_track_is_never_mapped() {
        let src = source(
            "mov,mp4,m4a,3gp,3g2,mj2",
            "a.mov",
            "hevc",
            &["aac", "apple_apac"],
        );
        // Lossless: MP4 with hvc1 (apac does not block MP4), apac excluded.
        let cmd = run(&src, &settings(Preset::Lossless), 1.0, 3.0, None);
        assert_eq!(cmd.muxer, Muxer::Mp4);
        let args = cmd.to_string_args().join(" ");
        assert!(args.contains("-map 0:a? -map -0:a:1"), "{args}");
        assert!(args.contains("-tag:v hvc1"), "{args}");
        // Frame-Accurate (AacAll) excludes it too.
        let args = run(&src, &settings(Preset::Accurate), 1.0, 3.0, None)
            .to_string_args()
            .join(" ");
        assert!(args.contains("-map 0:a? -map -0:a:1"), "{args}");
        // Vertical: asking for track 2 (apac) falls back to track 1; mix needs two usable tracks.
        let mut s = settings(Preset::Vcrop);
        s.audio_track = 2;
        s.mix_all_audio = true;
        assert_eq!(
            vertical_audio(&src, &s),
            AudioPlan::AacTrack {
                track: 0,
                kbps: 192,
                stereo: true
            }
        );
        let three = source(
            "mov,mp4,m4a,3gp,3g2,mj2",
            "a.mov",
            "hevc",
            &["apple_apac", "aac", "aac"],
        );
        assert_eq!(
            vertical_audio(&three, &s),
            AudioPlan::MixAll {
                tracks: 3,
                kbps: 192,
                skip: 0b001
            }
        );
        s.mix_all_audio = false;
        s.audio_track = 1;
        assert_eq!(
            vertical_audio(&three, &s),
            AudioPlan::AacTrack {
                track: 1,
                kbps: 192,
                stereo: true
            }
        );
    }
}
