//! Preview proxy decisions (pure logic).
//!
//! The preview player runs inside WebView2/Chromium, which decodes H.264,
//! VP8/VP9 and AV1 in MP4/WebM containers — but not HEVC, ProRes, MPEG-4
//! Part 2, 10/12-bit H.264, or Matroska/FLV/AVI. Those files play as a black
//! rectangle with no error, so the app builds an H.264 preview proxy once and
//! previews that instead. The source file is never modified: exports keep
//! reading the original.

use std::path::Path;

use crate::probe::MediaInfo;

/// Video codecs the embedded Chromium can decode.
const BROWSER_CODECS: [&str; 5] = ["h264", "vp8", "vp9", "av1", "theora"];

/// Containers the embedded Chromium can demux.
const BROWSER_EXTENSIONS: [&str; 4] = ["mp4", "m4v", "mov", "webm"];

/// Lowercased extension without the dot (`""` when there is none).
pub fn container_ext(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// True when the embedded player cannot decode this file as-is.
///
/// Checks the codec, the bit depth (`pix_fmt` with `10`/`12` bits is outside
/// WebView2's H.264 profile) and the container extension.
pub fn needs_proxy(path: &Path, info: &MediaInfo) -> bool {
    proxy_reason(path, info).is_some()
}

/// Human-readable reason for the proxy, or `None` when direct playback works.
pub fn proxy_reason(path: &Path, info: &MediaInfo) -> Option<String> {
    let codec = info.codec_name.to_lowercase();
    let ext = container_ext(path);

    if !BROWSER_CODECS.contains(&codec.as_str()) {
        let label = match codec.as_str() {
            "hevc" | "h265" => "HEVC/H.265".to_string(),
            "" => "unknown codec".to_string(),
            other => other.to_uppercase(),
        };
        return Some(format!("{label} is not playable in the embedded player"));
    }

    let pix_fmt = info.pix_fmt.to_lowercase();
    if pix_fmt.contains("p10") || pix_fmt.contains("p12") {
        return Some(format!(
            "{}-bit {} is not playable in the embedded player",
            if pix_fmt.contains("p12") { 12 } else { 10 },
            codec.to_uppercase()
        ));
    }

    if !BROWSER_EXTENSIONS.contains(&ext.as_str()) {
        return Some(format!(
            "the .{} container is not playable in the embedded player",
            if ext.is_empty() { "unknown" } else { ext.as_str() }
        ));
    }

    None
}

/// Encoder used to build the proxy. `Qsv` decodes in hardware on Intel iGPUs
/// (roughly 4x faster than software HEVC decode on a 4-core CPU); `Software`
/// is the portable fallback used when QSV is absent or fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyEncoder {
    Qsv,
    Software,
}

impl ProxyEncoder {
    /// Candidate order: hardware first, portable fallback last.
    pub const ALL: [ProxyEncoder; 2] = [ProxyEncoder::Qsv, ProxyEncoder::Software];

    pub fn label(&self) -> &'static str {
        match self {
            ProxyEncoder::Qsv => "qsv",
            ProxyEncoder::Software => "software",
        }
    }
}

/// Bumped when the proxy command changes, so stale caches are rebuilt.
pub const PREVIEW_FORMAT_VERSION: &str = "v3";

/// ffmpeg arguments that build the H.264 preview proxy.
///
/// `preset` trades size for time: `veryfast` for ordinary clips, `ultrafast`
/// for long sources where waiting on the first preview would be worse.
/// The proxy is capped at 1600 px wide: wide enough for framing decisions,
/// small enough to seek quickly.
pub fn build_proxy_args(
    input: &Path,
    output: &Path,
    duration_sec: f64,
    encoder: ProxyEncoder,
) -> Vec<String> {
    let preset = if duration_sec > 600.0 {
        "ultrafast"
    } else {
        "veryfast"
    };

    let mut args: Vec<String> = vec!["-hide_banner".into(), "-y".into()];
    if encoder == ProxyEncoder::Qsv {
        args.push("-hwaccel".into());
        args.push("qsv".into());
    }
    args.push("-i".into());
    args.push(input.to_string_lossy().into_owned());
    args.push("-map".into());
    args.push("0:v:0".into());
    args.push("-map".into());
    args.push("0:a:0?".into());

    match encoder {
        ProxyEncoder::Qsv => {
            args.push("-c:v".into());
            args.push("h264_qsv".into());
            args.push("-global_quality".into());
            args.push("24".into());
        }
        ProxyEncoder::Software => {
            args.push("-c:v".into());
            args.push("libx264".into());
            args.push("-preset".into());
            args.push(preset.into());
            args.push("-crf".into());
            args.push("23".into());
            args.push("-pix_fmt".into());
            args.push("yuv420p".into());
            // Software frames are plain memory, so the scale filter is cheap
            // here; it keeps the proxy small enough to seek quickly.
            args.push("-vf".into());
            args.push("scale='min(1600,iw)':-2".into());
        }
    }

    args.push("-c:a".into());
    args.push("aac".into());
    args.push("-b:a".into());
    args.push("160k".into());
    args.push("-ac".into());
    args.push("2".into());
    args.push("-movflags".into());
    args.push("+faststart".into());
    args.push(output.to_string_lossy().into_owned());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(codec: &str, pix_fmt: &str) -> MediaInfo {
        MediaInfo {
            duration_sec: 10.0,
            width: 1920,
            height: 1080,
            fps: 30.0,
            bitrate_bps: 8_000_000.0,
            codec_name: codec.to_string(),
            format_name: "mov,mp4,m4a,3gp,3g2,mj2".to_string(),
            pix_fmt: pix_fmt.to_string(),
        }
    }

    #[test]
    fn hevc_needs_a_proxy() {
        assert!(needs_proxy(Path::new("clip.mp4"), &info("hevc", "yuv420p")));
        let reason = proxy_reason(Path::new("clip.mp4"), &info("hevc", "yuv420p")).unwrap();
        assert!(reason.contains("HEVC"));
    }

    #[test]
    fn plain_h264_mp4_plays_directly() {
        assert!(!needs_proxy(Path::new("clip.mp4"), &info("h264", "yuv420p")));
    }

    #[test]
    fn ten_bit_h264_needs_a_proxy() {
        assert!(needs_proxy(
            Path::new("clip.mp4"),
            &info("h264", "yuv420p10le")
        ));
    }

    #[test]
    fn unsupported_containers_need_a_proxy_even_with_h264() {
        assert!(needs_proxy(Path::new("clip.mkv"), &info("h264", "yuv420p")));
        assert!(needs_proxy(Path::new("clip.flv"), &info("h264", "yuv420p")));
        assert!(!needs_proxy(Path::new("clip.webm"), &info("vp9", "yuv420p")));
    }

    #[test]
    fn software_proxy_args_encode_h264_and_keep_audio() {
        let args = build_proxy_args(
            Path::new("in.hevc.mp4"),
            Path::new("out.mp4"),
            40.0,
            ProxyEncoder::Software,
        );
        let joined = args.join(" ");
        assert!(joined.contains("-c:v libx264"));
        assert!(joined.contains("-pix_fmt yuv420p"));
        assert!(joined.contains("-preset veryfast"));
        assert!(joined.contains("-c:a aac"));
        assert!(joined.contains("0:a:0?"));
        assert!(!joined.contains("-hwaccel qsv"));
        assert_eq!(args.last().unwrap(), "out.mp4");
    }

    #[test]
    fn qsv_proxy_args_decode_in_hardware() {
        let args = build_proxy_args(
            Path::new("in.hevc.mp4"),
            Path::new("out.mp4"),
            40.0,
            ProxyEncoder::Qsv,
        );
        let joined = args.join(" ");
        assert!(joined.contains("-hwaccel qsv"));
        assert!(joined.contains("-c:v h264_qsv"));
        assert!(!joined.contains("libx264"));
        // QSV frames are hardware surfaces; a software scale filter after
        // them fails, so the QSV proxy keeps the source resolution.
        assert!(!joined.contains("-vf"));
    }

    #[test]
    fn long_sources_use_the_fastest_preset() {
        let args = build_proxy_args(
            Path::new("in.mp4"),
            Path::new("out.mp4"),
            3600.0,
            ProxyEncoder::Software,
        );
        assert!(args.join(" ").contains("-preset ultrafast"));
    }
}
