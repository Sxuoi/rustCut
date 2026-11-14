//! MIME types with RFC 6381 codec strings for the WebView playback-tier
//! check (§7.3: `canPlayType` with the exact codec string from FFprobe).
//!
//! `video/mp4; codecs="avc1.640028, mp4a.40.2"`. Codecs without a registered
//! string are passed by their FFprobe name so `canPlayType` answers "no"
//! instead of guessing. Unknown levels (`level: -99`) are derived from the
//! frame size.

use rustcut_model::{AudioTrack, SourceInfo, VideoInfo};

/// MIME type of the source, with every distinct codec string (video first).
pub fn mime_type(source: &SourceInfo) -> String {
    let mut codecs: Vec<String> = vec![video_codec_string(&source.video)];
    // Only the track the preview plays: the default (else first) audio track that
    // is supported and has an RFC 6381 mapping. Unknown tokens make canPlayType
    // answer "" and force a needless remux.
    let playable = |t: &&AudioTrack| {
        !crate::compat::is_unsupported_audio(&t.codec) && audio_codec_token(&t.codec).is_some()
    };
    let track = source
        .audio_tracks
        .iter()
        .filter(playable)
        .find(|t| t.is_default)
        .or_else(|| source.audio_tracks.iter().find(playable));
    if let Some(track) = track {
        let c = audio_codec_string(track);
        if !codecs.contains(&c) {
            codecs.push(c);
        }
    }
    format!(
        "{}; codecs=\"{}\"",
        container_mime(source),
        codecs.join(", ")
    )
}

/// Container MIME type from `format_name` (and the extension when the format
/// is unknown, or to tell WebM from Matroska).
///
/// QuickTime `.mov` is reported as `video/mp4`: Chromium (WebView2) answers
/// `canPlayType("video/quicktime…")` with "" although its MP4 demuxer plays
/// QuickTime files, so every iPhone/camera `.mov` would be remuxed (a full
/// copy into the cache) before the first frame. WKWebView accepts both. When
/// the WebView cannot play a file after all, the first-frame check falls back
/// to Remux (section 7.3).
pub fn container_mime(source: &SourceInfo) -> &'static str {
    let format = source
        .format_name
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let names: Vec<&str> = format.split(',').collect();
    let ext = source.extension();
    let has = |n: &str| names.contains(&n);
    if has("mov")
        || has("mp4")
        || (format.is_empty() && matches!(ext.as_str(), "mp4" | "m4v" | "mov"))
    {
        "video/mp4"
    } else if has("matroska")
        || has("webm")
        || (format.is_empty() && matches!(ext.as_str(), "mkv" | "webm"))
    {
        if ext == "webm" || is_webm_content(source) {
            "video/webm"
        } else {
            "video/x-matroska"
        }
    } else if has("flv") {
        "video/x-flv"
    } else if has("mpegts") {
        "video/mp2t"
    } else if has("avi") {
        "video/x-msvideo"
    } else if has("asf") {
        "video/x-ms-asf"
    } else if has("ogg") {
        "video/ogg"
    } else if has("mpeg") {
        "video/mpeg"
    } else {
        "application/octet-stream"
    }
}

fn is_webm_content(source: &SourceInfo) -> bool {
    crate::compat::WEBM_VIDEO_CODECS.contains(&source.video.codec.as_str())
        && source
            .audio_tracks
            .iter()
            .all(|a| crate::compat::WEBM_AUDIO_CODECS.contains(&a.codec.as_str()))
}

/// Size class used when the stream does not report its level:
/// 0 ≤ 720p, 1 ≤ 1080p, 2 ≤ 4K, 3 above.
fn size_class(v: &VideoInfo) -> usize {
    let pixels = u64::from(v.coded_width) * u64::from(v.coded_height);
    match pixels {
        0..=921_600 => 0,
        921_601..=2_228_224 => 1,
        2_228_225..=8_912_896 => 2,
        _ => 3,
    }
}

fn known_level(v: &VideoInfo) -> Option<u32> {
    v.level
        .and_then(|l| u32::try_from(l).ok())
        .filter(|l| *l > 0)
}

fn bit_depth(v: &VideoInfo) -> u32 {
    v.bit_depth.filter(|b| *b > 0).unwrap_or(8)
}

/// RFC 6381 string of the video stream (`avc1.640028`, `hvc1.2.4.L120.B0`,
/// `vp09.00.41.08`, `av01.0.08M.10`).
pub fn video_codec_string(v: &VideoInfo) -> String {
    let profile = v.profile.as_deref().unwrap_or("").to_ascii_lowercase();
    match v.codec.as_str() {
        "h264" => {
            let (idc, constraints) = match profile.as_str() {
                "baseline" => (0x42, 0x00),
                "constrained baseline" => (0x42, 0xE0),
                "main" => (0x4D, 0x40),
                "extended" => (0x58, 0x00),
                "high 10" => (0x6E, 0x00),
                "high 10 intra" => (0x6E, 0x10),
                "high 4:2:2" => (0x7A, 0x00),
                "high 4:2:2 intra" => (0x7A, 0x10),
                "high 4:4:4 predictive" => (0xF4, 0x00),
                "high 4:4:4 intra" => (0xF4, 0x10),
                _ => (0x64, 0x00), // High and unknown
            };
            let level = known_level(v).unwrap_or([31, 42, 51, 52][size_class(v)]);
            format!("avc1.{idc:02X}{constraints:02X}{:02X}", level.min(255))
        }
        "hevc" => {
            let (idc, compat) = match profile.as_str() {
                "main 10" => (2, "4"),
                "rext" | "format range extensions" => (4, "10"),
                _ => (1, "6"),
            };
            let level = known_level(v).unwrap_or([93, 123, 153, 183][size_class(v)]);
            format!("hvc1.{idc}.{compat}.L{level}.B0")
        }
        "vp9" => {
            let idc: u32 = profile
                .trim_start_matches("profile")
                .trim()
                .parse()
                .unwrap_or(0);
            let level = known_level(v).unwrap_or([31, 41, 51, 61][size_class(v)]);
            format!("vp09.{idc:02}.{level:02}.{:02}", bit_depth(v))
        }
        "av1" => {
            let idc = match profile.as_str() {
                "high" => 1,
                "professional" => 2,
                _ => 0,
            };
            let level = known_level(v).unwrap_or([5, 9, 13, 16][size_class(v)]);
            format!("av01.{idc}.{level:02}M.{:02}", bit_depth(v))
        }
        "vp8" => "vp8".to_string(),
        "mpeg4" => "mp4v.20.9".to_string(),
        other => other.to_string(),
    }
}

/// RFC 6381 string of an audio track (AAC is assumed LC).
pub fn audio_codec_string(a: &AudioTrack) -> String {
    audio_codec_token(&a.codec).unwrap_or(&a.codec).to_string()
}

fn audio_codec_token(codec: &str) -> Option<&'static str> {
    Some(match codec {
        "aac" => "mp4a.40.2",
        "mp3" => "mp4a.6B",
        "opus" => "opus",
        "vorbis" => "vorbis",
        "flac" => "flac",
        "ac3" => "ac-3",
        "eac3" => "ec-3",
        "alac" => "alac",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::source;

    #[test]
    fn s73_h264_aac_mp4() {
        let mut s = source("mov,mp4,m4a,3gp,3g2,mj2", "a.mp4", "h264", &["aac", "aac"]);
        s.video.profile = Some("High".into());
        s.video.level = Some(40);
        assert_eq!(
            mime_type(&s),
            "video/mp4; codecs=\"avc1.640028, mp4a.40.2\""
        );
        s.video.profile = Some("Constrained Baseline".into());
        s.video.level = Some(30);
        assert_eq!(video_codec_string(&s.video), "avc1.42E01E");
        s.video.profile = Some("Main".into());
        s.video.level = None; // unknown → from 1080p size
        assert_eq!(video_codec_string(&s.video), "avc1.4D402A");
    }

    #[test]
    fn s73_skips_unsupported_and_unmapped_audio() {
        let s = source(
            "mov,mp4,m4a,3gp,3g2,mj2",
            "a.MOV",
            "hevc",
            &["aac", "apple_apac"],
        );
        assert_eq!(
            mime_type(&s),
            "video/mp4; codecs=\"hvc1.1.6.L123.B0, mp4a.40.2\""
        );
        let s = source(
            "mov,mp4,m4a,3gp,3g2,mj2",
            "a.MOV",
            "hevc",
            &["apple_apac", "aac"],
        );
        assert!(mime_type(&s).ends_with("mp4a.40.2\""));
        let s = source("matroska,webm", "a.mkv", "h264", &["dts"]);
        assert!(!mime_type(&s).contains("dts"));
    }

    #[test]
    fn s73_container_variants() {
        let mov = source("mov,mp4,m4a,3gp,3g2,mj2", "a.MOV", "hevc", &["pcm_s16le"]);
        assert_eq!(
            mime_type(&mov),
            // MOV is probed as video/mp4 (Chromium has no video/quicktime).
            "video/mp4; codecs=\"hvc1.1.6.L123.B0\""
        );
        let mkv = source("matroska,webm", "a.mkv", "h264", &["aac"]);
        assert!(mime_type(&mkv).starts_with("video/x-matroska; "));
        let webm = source("matroska,webm", "a.mkv", "vp9", &["opus"]);
        assert!(
            mime_type(&webm).starts_with("video/webm; codecs=\"vp09.00.41.08, opus\""),
            "{}",
            mime_type(&webm)
        );
        assert!(mime_type(&source("flv", "a.flv", "h264", &[])).starts_with("video/x-flv; "));
        assert!(mime_type(&source("mpegts", "a.ts", "h264", &[])).starts_with("video/mp2t; "));
        assert!(mime_type(&source("", "a.mp4", "h264", &[])).starts_with("video/mp4; "));
        assert!(
            mime_type(&source("weird", "a.xyz", "h264", &[]))
                .starts_with("application/octet-stream; ")
        );
    }

    #[test]
    fn s73_hevc_av1_vp9_and_audio_strings() {
        let mut s = source("mov,mp4", "a.mp4", "hevc", &[]);
        s.video.profile = Some("Main 10".into());
        s.video.level = Some(120);
        assert_eq!(video_codec_string(&s.video), "hvc1.2.4.L120.B0");
        s.video.codec = "av1".into();
        s.video.profile = Some("Main".into());
        s.video.level = Some(8);
        s.video.bit_depth = Some(10);
        assert_eq!(video_codec_string(&s.video), "av01.0.08M.10");
        s.video.codec = "vp9".into();
        s.video.profile = Some("Profile 2".into());
        s.video.level = None;
        assert_eq!(video_codec_string(&s.video), "vp09.02.41.10");
        let a = |c: &str| audio_codec_string(&source("", "a.mp4", "h264", &[c]).audio_tracks[0]);
        assert_eq!(
            [a("mp3"), a("opus"), a("ac3"), a("eac3"), a("flac")],
            ["mp4a.6B", "opus", "ac-3", "ec-3", "flac"].map(String::from)
        );
    }
}
