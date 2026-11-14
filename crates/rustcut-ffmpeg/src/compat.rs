//! Lossless output container (EXP-03).
//!
//! MP4 when every mapped stream (first video + every audio track, EXP-04) is
//! MP4-compatible, otherwise the source container. If only the audio is
//! incompatible the dialog offers *MP4 with AAC audio, video stays lossless*
//! ([`ContainerDecision::offer_aac_audio`]); accepting it sets
//! `ExportSettings::lossless_audio_to_aac`.
//!
//! "Compatible" means: FFmpeg 9.0.2's mp4 muxer accepts it by stream copy
//! **and** mainstream players (browsers, QuickTime, Windows) play it. FFmpeg
//! 9 also muxes PCM (`ipcm`), MP2, Vorbis, DTS, MPEG-2, MJPEG and FFV1 into
//! MP4, but those files do not play in common players, so they stay out of
//! the allow-lists (verified in `tests/real_ffmpeg.rs`).

use crate::args::{AAC_KBPS, Muxer};
use rustcut_model::{ExportSettings, SourceInfo};
use serde::{Deserialize, Serialize};

/// Video codecs (ffprobe `codec_name`) copied into MP4. HEVC is tagged
/// `hvc1` (FFmpeg's default `hev1` is rejected by Apple players).
pub const MP4_VIDEO_CODECS: &[&str] = &["h264", "hevc", "av1", "vp9", "mpeg4"];
/// Audio codecs copied into MP4.
pub const MP4_AUDIO_CODECS: &[&str] = &["aac", "mp3", "ac3", "eac3", "opus", "flac", "alac"];
/// Audio codecs FFmpeg 9 can neither decode nor cut reliably (Apple Positional
/// Audio, `apple_apac`, found next to the normal AAC track of recent iPhone
/// clips). They are never exported: not even by stream copy, because a copy
/// gets timestamps that disagree with the other streams and shifts the whole
/// output; a re-encode would fail with "no decoder found".
pub const UNSUPPORTED_AUDIO_CODECS: &[&str] = &["apple_apac"];
/// Video codecs legal in WebM.
pub const WEBM_VIDEO_CODECS: &[&str] = &["vp8", "vp9", "av1"];
/// Audio codecs legal in WebM.
pub const WEBM_AUDIO_CODECS: &[&str] = &["opus", "vorbis"];

/// Audio handling of a lossless export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LosslessAudio {
    /// Every track stream-copied.
    Copy,
    /// Every track re-encoded to AAC; video stays a stream copy.
    AacTranscode,
}

/// Output container of an export (EXP-03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerDecision {
    pub muxer: Muxer,
    /// Extension without the dot (`mp4`, `mkv`, …).
    pub ext: String,
    pub audio: LosslessAudio,
    /// Only the audio blocks MP4: offer *MP4 with AAC audio*.
    pub offer_aac_audio: bool,
    /// Why this container was chosen (pre-flight, EXP-11).
    pub reason: String,
}

impl ContainerDecision {
    fn new(muxer: Muxer, audio: LosslessAudio, offer_aac_audio: bool, reason: String) -> Self {
        ContainerDecision {
            muxer,
            ext: muxer.extension().to_string(),
            audio,
            offer_aac_audio,
            reason,
        }
    }
}

/// True when a video codec may be stream-copied into MP4.
pub fn is_mp4_video(codec: &str) -> bool {
    MP4_VIDEO_CODECS.contains(&codec.to_ascii_lowercase().as_str())
}

/// True when FFmpeg cannot decode (or reliably cut) this audio codec; such a
/// track is never mapped into an export.
pub fn is_unsupported_audio(codec: &str) -> bool {
    UNSUPPORTED_AUDIO_CODECS.contains(&codec.to_ascii_lowercase().as_str())
}

/// 0-based audio indices (`0:a:N`) of `source` that are never exported.
pub fn unsupported_audio_indices(source: &SourceInfo) -> Vec<u32> {
    source
        .audio_tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| is_unsupported_audio(&t.codec))
        .filter_map(|(i, _)| u32::try_from(i).ok())
        .collect()
}

/// True when an audio codec may be stream-copied into MP4 (`pcm_*` is not).
pub fn is_mp4_audio(codec: &str) -> bool {
    MP4_AUDIO_CODECS.contains(&codec.to_ascii_lowercase().as_str())
}

/// Container for a lossless export of `source` (EXP-03). Re-encode presets
/// always produce MP4 (H.264 + AAC, EXP-02); for them this returns MP4 with
/// [`LosslessAudio::AacTranscode`].
pub fn lossless_container(source: &SourceInfo, settings: &ExportSettings) -> ContainerDecision {
    let mut decision = container_decision(source, settings);
    let dropped = unsupported_audio_indices(source).len();
    if dropped > 0 {
        decision.reason.push_str(&format!(
            "; {dropped} audio track(s) FFmpeg cannot read (Apple Positional Audio) are not exported"
        ));
    }
    decision
}

fn container_decision(source: &SourceInfo, settings: &ExportSettings) -> ContainerDecision {
    if settings.preset().is_reencode() {
        return ContainerDecision::new(
            Muxer::Mp4,
            LosslessAudio::AacTranscode,
            false,
            "Frame-Accurate exports are always MP4 (H.264 + AAC)".to_string(),
        );
    }
    let video = source.video.codec.to_ascii_lowercase();
    let bad_audio = incompatible_audio(source);
    let hevc_note = if video == "hevc" {
        " (HEVC tagged hvc1 for Apple players)"
    } else {
        ""
    };
    if !is_mp4_video(&video) {
        let muxer = source_container(source);
        return ContainerDecision::new(
            muxer,
            LosslessAudio::Copy,
            false,
            format!(
                "{} video cannot be stream-copied into MP4; keeping the source container ({})",
                display_codec(&video),
                muxer.label()
            ),
        );
    }
    if bad_audio.is_empty() {
        return ContainerDecision::new(
            Muxer::Mp4,
            LosslessAudio::Copy,
            false,
            format!("Every stream is MP4-compatible{hevc_note}"),
        );
    }
    let list = bad_audio
        .iter()
        .map(|c| display_codec(c))
        .collect::<Vec<_>>()
        .join(", ");
    if settings.lossless_audio_to_aac {
        return ContainerDecision::new(
            Muxer::Mp4,
            LosslessAudio::AacTranscode,
            true,
            format!(
                "{list} audio is converted to AAC {AAC_KBPS} kbps; video stays lossless{hevc_note}"
            ),
        );
    }
    let muxer = source_container(source);
    ContainerDecision::new(
        muxer,
        LosslessAudio::Copy,
        true,
        format!(
            "{list} audio is not MP4-compatible; keeping the source container ({}). \
             Choose \"MP4 with AAC audio\" to get MP4 with lossless video",
            muxer.label()
        ),
    )
}

/// Distinct audio codecs of `source` that block MP4, in track order.
fn incompatible_audio(source: &SourceInfo) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for track in &source.audio_tracks {
        let codec = track.codec.to_ascii_lowercase();
        if is_unsupported_audio(&codec) {
            continue;
        }
        if !is_mp4_audio(&codec) && !out.contains(&codec) {
            out.push(codec);
        }
    }
    out
}

fn display_codec(codec: &str) -> String {
    match codec {
        "h264" => "H.264".into(),
        "hevc" => "HEVC".into(),
        "mpeg2video" => "MPEG-2".into(),
        "prores" => "ProRes".into(),
        c if c.starts_with("pcm_") => format!("PCM ({c})"),
        c => c.to_ascii_uppercase(),
    }
}

/// The source's own container family, used when MP4 is not possible.
/// Matroska sources become WebM when every mapped stream is WebM-legal;
/// unknown containers (AVI, MPEG-PS, ASF, Ogg, …) fall back to Matroska,
/// which accepts nearly every codec.
pub fn source_container(source: &SourceInfo) -> Muxer {
    let format = source
        .format_name
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let names: Vec<&str> = format.split(',').map(str::trim).collect();
    let family = if format.is_empty() {
        family_from_extension(&source.extension())
    } else {
        family_from_names(&names)
    };
    match family {
        Family::Matroska if webm_legal(source) => Muxer::Webm,
        Family::Matroska => Muxer::Matroska,
        Family::IsoBmff => Muxer::Mov,
        Family::Flv => Muxer::Flv,
        Family::Mpegts => Muxer::Mpegts,
    }
}

enum Family {
    Matroska,
    IsoBmff,
    Flv,
    Mpegts,
}

fn family_from_names(names: &[&str]) -> Family {
    let has = |n: &str| names.contains(&n);
    if has("mov") || has("mp4") {
        Family::IsoBmff
    } else if has("flv") {
        Family::Flv
    } else if has("mpegts") {
        Family::Mpegts
    } else {
        Family::Matroska
    }
}

fn family_from_extension(ext: &str) -> Family {
    match ext {
        "mp4" | "m4v" | "mov" | "3gp" => Family::IsoBmff,
        "flv" => Family::Flv,
        "ts" | "m2ts" | "mts" => Family::Mpegts,
        _ => Family::Matroska,
    }
}

/// Every mapped stream may go into WebM.
fn webm_legal(source: &SourceInfo) -> bool {
    WEBM_VIDEO_CODECS.contains(&source.video.codec.to_ascii_lowercase().as_str())
        && source
            .audio_tracks
            .iter()
            .all(|a| WEBM_AUDIO_CODECS.contains(&a.codec.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::source;
    use rustcut_model::{CutMode, Framing};

    fn lossless() -> ExportSettings {
        ExportSettings {
            cut_mode: CutMode::Lossless,
            ..ExportSettings::default()
        }
    }

    #[test]
    fn apple_apac_audio_never_blocks_mp4_and_is_reported() {
        let src = source(
            "mov,mp4,m4a,3gp,3g2,mj2",
            "IMG_1.MOV",
            "hevc",
            &["aac", "apple_apac"],
        );
        let d = lossless_container(&src, &lossless());
        assert_eq!(d.ext, "mp4");
        assert!(!d.offer_aac_audio, "{d:?}");
        assert!(d.reason.contains("Apple Positional Audio"), "{}", d.reason);
        assert_eq!(unsupported_audio_indices(&src), vec![1]);
    }

    #[test]
    fn exp03_obs_mkv_h264_aac_goes_to_mp4() {
        let s = source("matroska,webm", "rec.mkv", "h264", &["aac", "aac", "aac"]);
        let d = lossless_container(&s, &lossless());
        assert_eq!(
            (d.muxer, d.ext.as_str(), d.audio, d.offer_aac_audio),
            (Muxer::Mp4, "mp4", LosslessAudio::Copy, false)
        );
    }

    #[test]
    fn exp03_pcm_audio_offers_aac_and_keeps_source_container() {
        let s = source("mov,mp4,m4a,3gp,3g2,mj2", "cam.mov", "h264", &["pcm_s16le"]);
        let d = lossless_container(&s, &lossless());
        assert_eq!(
            (d.muxer, d.ext.as_str(), d.audio, d.offer_aac_audio),
            (Muxer::Mov, "mov", LosslessAudio::Copy, true)
        );
        assert!(d.reason.contains("PCM (pcm_s16le)"), "{}", d.reason);
        let mkv = source("matroska,webm", "cam.mkv", "h264", &["aac", "pcm_s24le"]);
        assert_eq!(lossless_container(&mkv, &lossless()).muxer, Muxer::Matroska);
    }

    #[test]
    fn exp03_accepted_aac_offer_gives_mp4_with_transcode() {
        let s = source("mov,mp4,m4a,3gp,3g2,mj2", "cam.mov", "hevc", &["pcm_s16le"]);
        let settings = ExportSettings {
            lossless_audio_to_aac: true,
            ..lossless()
        };
        let d = lossless_container(&s, &settings);
        assert_eq!(
            (d.muxer, d.audio, d.offer_aac_audio),
            (Muxer::Mp4, LosslessAudio::AacTranscode, true)
        );
        assert!(d.reason.contains("hvc1"), "{}", d.reason);
    }

    #[test]
    fn exp03_incompatible_video_never_offers_aac() {
        let s = source("matroska,webm", "a.mkv", "vp8", &["vorbis"]);
        let d = lossless_container(&s, &lossless());
        assert_eq!(
            (d.muxer, d.ext.as_str(), d.offer_aac_audio),
            (Muxer::Webm, "webm", false)
        );
        let s = source("matroska,webm", "a.mkv", "ffv1", &["flac"]);
        assert_eq!(lossless_container(&s, &lossless()).muxer, Muxer::Matroska);
        let s = source("mov,mp4,m4a,3gp,3g2,mj2", "a.mov", "prores", &["pcm_s24le"]);
        assert_eq!(lossless_container(&s, &lossless()).muxer, Muxer::Mov);
        let s = source("mpegts", "a.ts", "mpeg2video", &["mp2"]);
        assert_eq!(lossless_container(&s, &lossless()).muxer, Muxer::Mpegts);
        let s = source("avi", "a.avi", "msmpeg4v3", &["mp3"]);
        assert_eq!(lossless_container(&s, &lossless()).muxer, Muxer::Matroska);
    }

    #[test]
    fn exp03_compatible_sources_from_any_container_become_mp4() {
        for (fmt, path) in [
            ("flv", "a.flv"),
            ("mpegts", "a.ts"),
            ("matroska,webm", "a.webm"),
        ] {
            let s = source(fmt, path, "h264", &["aac"]);
            assert_eq!(
                lossless_container(&s, &lossless()).muxer,
                Muxer::Mp4,
                "{fmt}"
            );
        }
        let vp9 = source("matroska,webm", "a.webm", "vp9", &["opus"]);
        assert_eq!(lossless_container(&vp9, &lossless()).muxer, Muxer::Mp4);
        let silent = source("flv", "a.flv", "h264", &[]);
        assert_eq!(lossless_container(&silent, &lossless()).muxer, Muxer::Mp4);
    }

    #[test]
    fn exp03_mp2_in_ts_keeps_mpegts_and_offers_aac() {
        let s = source("mpegts", "a.ts", "h264", &["mp2"]);
        let d = lossless_container(&s, &lossless());
        assert_eq!(
            (d.muxer, d.ext.as_str(), d.offer_aac_audio),
            (Muxer::Mpegts, "ts", true)
        );
        let flv = source("flv", "a.flv", "h264", &["nellymoser"]);
        assert_eq!(lossless_container(&flv, &lossless()).muxer, Muxer::Flv);
    }

    #[test]
    fn missing_format_name_falls_back_to_extension() {
        assert_eq!(
            source_container(&source("", "a.MOV", "prores", &[])),
            Muxer::Mov
        );
        assert_eq!(source_container(&source("", "a.flv", "x", &[])), Muxer::Flv);
        assert_eq!(
            source_container(&source("", "a.m2ts", "x", &[])),
            Muxer::Mpegts
        );
        assert_eq!(
            source_container(&source("", "a.avi", "x", &[])),
            Muxer::Matroska
        );
    }

    #[test]
    fn reencode_presets_are_mp4() {
        let s = source("matroska,webm", "a.mkv", "ffv1", &["pcm_s16le"]);
        let settings = ExportSettings {
            cut_mode: CutMode::Accurate,
            framing: Framing::VerticalCrop,
            ..lossless()
        };
        let d = lossless_container(&s, &settings);
        assert_eq!(
            (d.muxer, d.audio, d.offer_aac_audio),
            (Muxer::Mp4, LosslessAudio::AacTranscode, false)
        );
    }

    #[test]
    fn codec_checks_are_case_insensitive_and_pcm_is_never_mp4() {
        assert!(is_mp4_video("H264") && is_mp4_audio("AAC"));
        for pcm in ["pcm_s16le", "pcm_s24le", "pcm_f32le", "pcm_s16be"] {
            assert!(!is_mp4_audio(pcm), "{pcm}");
        }
        for no in ["vorbis", "mp2", "dts", "truehd"] {
            assert!(!is_mp4_audio(no), "{no}");
        }
    }
}
