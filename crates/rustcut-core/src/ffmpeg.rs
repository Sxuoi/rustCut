//! FFmpeg command generation (PRD 7.4).
//!
//! Two export modes:
//!   * lossless stream copy (`-c copy`, keyframe-aligned, no re-encode)
//!   * re-encode: 16:9 working canvas + 9:16 crop window at the clip's
//!     `crop_x_offset`, H.264 via libx264 or a hardware encoder.
//!
//! All dimensions are computed in Rust and passed as literal values so the
//! output is always even (yuv420p requirement) and fully testable.

use std::path::Path;

use crate::timecode;

/// Aspect of the vertical clip format as width/height.
const NINE_SIXTEEN: f64 = 9.0 / 16.0;
const SIXTEEN_NINE: f64 = 16.0 / 9.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CropStyle {
    CenterCrop,
    /// Full frame centered over a blurred 9:16 fill (PRD 4.4).
    BlurredBackground,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwEncoder {
    Nvenc,
    Amf,
    Qsv,
    VideoToolbox,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VideoEncoder {
    Libx264 { crf: u8, preset: String },
    Hw(HwEncoder),
}

impl VideoEncoder {
    /// The default software encoder used when no GPU encoder is available.
    pub fn software_default() -> Self {
        VideoEncoder::Libx264 {
            crf: 20,
            preset: "fast".to_string(),
        }
    }

    /// `-c:v ...` arguments for this encoder.
    pub fn codec_args(&self) -> Vec<String> {
        match self {
            VideoEncoder::Libx264 { crf, preset } => vec![
                "-c:v".into(),
                "libx264".into(),
                "-preset".into(),
                preset.clone(),
                "-crf".into(),
                crf.to_string(),
            ],
            VideoEncoder::Hw(HwEncoder::Nvenc) => vec![
                "-c:v".into(),
                "h264_nvenc".into(),
                "-preset".into(),
                "p5".into(),
                "-rc".into(),
                "vbr".into(),
                "-cq".into(),
                "23".into(),
            ],
            VideoEncoder::Hw(HwEncoder::Amf) => vec![
                "-c:v".into(),
                "h264_amf".into(),
                "-quality".into(),
                "quality".into(),
                "-rc".into(),
                "cqp".into(),
                "-qp_i".into(),
                "23".into(),
            ],
            VideoEncoder::Hw(HwEncoder::Qsv) => vec![
                "-c:v".into(),
                "h264_qsv".into(),
                "-preset".into(),
                "medium".into(),
                "-global_quality".into(),
                "23".into(),
            ],
            VideoEncoder::Hw(HwEncoder::VideoToolbox) => vec![
                "-c:v".into(),
                "h264_videotoolbox".into(),
                "-b:v".into(),
                "8M".into(),
            ],
        }
    }
}

/// Rounds to the nearest even integer >= 2 (yuv420p safe).
fn even_round(x: f64) -> u32 {
    let rounded = x.round().max(2.0) as i64;
    (if rounded % 2 == 0 { rounded } else { rounded + 1 }) as u32
}

/// 16:9 working canvas for a source of the given size (PRD 4.4).
///
/// `W = max(src_w, src_h * 16/9)`, `H = W * 9/16`, both rounded to even
/// pixels. A 1920x1080 source maps to itself (identity); narrower sources
/// gain side fill, taller sources gain top/bottom fill. The source is never
/// cropped - it is scaled to fit (contain) and centered on this canvas.
pub fn canvas_dims(src_w: u32, src_h: u32) -> (u32, u32) {
    let w = even_round((src_w as f64).max(src_h as f64 * SIXTEEN_NINE));
    let h = even_round(w as f64 * NINE_SIXTEEN);
    (w, h)
}

/// Width of the 9:16 crop window on a canvas of the given height.
/// 1080-high canvas -> 608 px, matching the PRD example.
pub fn crop_width_px(canvas_h: u32) -> u32 {
    even_round(canvas_h as f64 * NINE_SIXTEEN)
}

/// X position of the 9:16 crop window for a normalized offset in
/// `[-1.0, +1.0]` across the canvas width. Result is clamped to
/// `[0, canvas_w - crop_w]`, so it is valid for every source aspect ratio.
pub fn crop_x_px(offset: f64, canvas_w: u32, canvas_h: u32) -> u32 {
    let crop_w = crop_width_px(canvas_h) as f64;
    let max_x = (canvas_w as f64 - crop_w).max(0.0);
    let off = crate::clip::clamp_offset(offset);
    let x = max_x * (1.0 + off) / 2.0;
    x.round().clamp(0.0, max_x) as u32
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Mode 1 - lossless stream copy. Seeks on input (fast keyframe seek),
/// copies both streams unchanged, resets timestamps to zero.
pub fn build_lossless_args(
    input: &Path,
    start_sec: f64,
    end_sec: f64,
    output: &Path,
) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-y".into(),
        "-ss".into(),
        timecode::format_tc(start_sec),
        "-to".into(),
        timecode::format_tc(end_sec),
        "-i".into(),
        path_arg(input),
        "-c".into(),
        "copy".into(),
        "-avoid_negative_ts".into(),
        "make_zero".into(),
        path_arg(output),
    ]
}

/// Mode 2 - frame-accurate re-encode with the 16:9 canvas and a 9:16
/// center-crop window at `crop_x_offset`.
///
/// Filter chain: `scale` (contain fit) -> `pad` (canvas) -> `crop`
/// (9:16 window). Dimensions are literal values computed by `canvas_dims`
/// and `crop_x_px`, so every dimension is even and the X range is valid.
#[allow(clippy::too_many_arguments)] // mirrors FFmpeg's own flat arg style
pub fn build_reencode_args(
    input: &Path,
    start_sec: f64,
    end_sec: f64,
    output: &Path,
    canvas: (u32, u32),
    crop_style: CropStyle,
    crop_x_offset: f64,
    encoder: &VideoEncoder,
) -> Vec<String> {
    let (w, h) = canvas;
    let out_w = crop_width_px(h);
    let out_h = h;

    let mut args = vec![
        "-hide_banner".into(),
        "-y".into(),
        "-ss".into(),
        timecode::format_tc(start_sec),
        "-to".into(),
        timecode::format_tc(end_sec),
        "-i".into(),
        path_arg(input),
    ];

    match crop_style {
        CropStyle::CenterCrop => {
            let x = crop_x_px(crop_x_offset, w, h);
            let filter = format!(
                "scale={w}:{h}:force_original_aspect_ratio=decrease,\
                 pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,\
                 crop={out_w}:{out_h}:{x}:0"
            );
            args.push("-vf".into());
            args.push(filter);
        }
        CropStyle::BlurredBackground => {
            // The full frame stays visible over a blurred fill of the same
            // source; no canvas pad and no X offset (PRD 4.4). Output keeps
            // the 9:16 window dimensions so both styles share one size.
            let filter = format!(
                "[0:v]split=2[bg][fg];\
                 [bg]scale={out_w}:{out_h}:force_original_aspect_ratio=increase,\
                 crop={out_w}:{out_h},boxblur=20:2,setsar=1[bgv];\
                 [fg]scale={out_w}:{out_h}:force_original_aspect_ratio=decrease,\
                 setsar=1[fgv];\
                 [bgv][fgv]overlay=(W-w)/2:(H-h)/2,format=yuv420p[vout]"
            );
            args.push("-filter_complex".into());
            args.push(filter);
            args.push("-map".into());
            args.push("[vout]".into());
            args.push("-map".into());
            args.push("0:a?".into());
        }
    }

    args.extend(encoder.codec_args());
    args.push("-c:a".into());
    args.push("aac".into());
    args.push("-b:a".into());
    args.push("192k".into());
    args.push(path_arg(output));
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_is_identity_for_1080p_and_grows_for_narrower_sources() {
        assert_eq!(canvas_dims(1920, 1080), (1920, 1080));
        assert_eq!(canvas_dims(1440, 1080), (1920, 1080)); // 4:3
        assert_eq!(canvas_dims(1080, 1920), (3414, 1920)); // 9:16 phone
        assert_eq!(canvas_dims(640, 480), (854, 480)); // small 4:3
    }

    #[test]
    fn crop_window_matches_the_prd_dimensions() {
        assert_eq!(crop_width_px(1080), 608); // PRD: 608x1080
    }

    #[test]
    fn crop_x_maps_offset_end_to_end() {
        // 1920x1080 canvas, 608-wide window -> X range [0, 1312].
        assert_eq!(crop_x_px(-1.0, 1920, 1080), 0);
        assert_eq!(crop_x_px(0.0, 1920, 1080), 656);
        assert_eq!(crop_x_px(1.0, 1920, 1080), 1312);
        // Out-of-range offsets clamp instead of producing negative X.
        assert_eq!(crop_x_px(-9.0, 1920, 1080), 0);
        assert_eq!(crop_x_px(9.0, 1920, 1080), 1312);
    }

    #[test]
    fn lossless_args_match_the_prd_command() {
        let args = build_lossless_args(
            Path::new("input_video.mp4"),
            920.120,
            955.800,
            Path::new("02_Jump_Scare_React.mp4"),
        );
        assert_eq!(
            args,
            vec![
                "-hide_banner",
                "-y",
                "-ss",
                "00:15:20.120",
                "-to",
                "00:15:55.800",
                "-i",
                "input_video.mp4",
                "-c",
                "copy",
                "-avoid_negative_ts",
                "make_zero",
                "02_Jump_Scare_React.mp4",
            ]
        );
    }

    #[test]
    fn reencode_args_place_canvas_then_crop() {
        let args = build_reencode_args(
            Path::new("input_video.mp4"),
            920.120,
            955.800,
            Path::new("02_Jump_Scare_React_Vertical.mp4"),
            (1920, 1080),
            CropStyle::CenterCrop,
            0.0,
            &VideoEncoder::software_default(),
        );
        let vf = args[args.iter().position(|a| a == "-vf").unwrap() + 1].clone();
        assert_eq!(
            vf,
            "scale=1920:1080:force_original_aspect_ratio=decrease,\
             pad=1920:1080:(ow-iw)/2:(oh-ih)/2,\
             crop=608:1080:656:0"
        );
        assert!(args
            .windows(4)
            .any(|w| w.iter().map(String::as_str).eq(["-c:v", "libx264", "-preset", "fast"])));
        assert!(args
            .windows(4)
            .any(|w| w.iter().map(String::as_str).eq(["-c:a", "aac", "-b:a", "192k"])));
        assert_eq!(args.last().unwrap(), "02_Jump_Scare_React_Vertical.mp4");
    }

    #[test]
    fn hardware_encoders_emit_their_codec_args() {
        let cases = [
            (HwEncoder::Nvenc, "h264_nvenc"),
            (HwEncoder::Amf, "h264_amf"),
            (HwEncoder::Qsv, "h264_qsv"),
            (HwEncoder::VideoToolbox, "h264_videotoolbox"),
        ];
        for (hw, codec) in cases {
            let args = VideoEncoder::Hw(hw).codec_args();
            assert_eq!(args[0], "-c:v");
            assert_eq!(args[1], codec);
            assert!(
                args.len().is_multiple_of(2),
                "{codec}: flags and values must pair up"
            );
        }
    }

    #[test]
    fn blurred_background_keeps_the_full_frame_visible() {
        let args = build_reencode_args(
            Path::new("input_video.mp4"),
            0.0,
            2.0,
            Path::new("out.mp4"),
            (1920, 1080),
            CropStyle::BlurredBackground,
            0.5, // offset is ignored by this style
            &VideoEncoder::software_default(),
        );
        let fc = args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1].clone();
        assert!(fc.starts_with("[0:v]split=2"), "{fc}");
        assert!(fc.contains("boxblur"), "background fill must be blurred");
        assert!(fc.contains("overlay=(W-w)/2:(H-h)/2"), "frame must be centered");
        assert!(fc.contains("scale=608:1080:force_original_aspect_ratio=decrease"), "full frame contained");
        // The background crop fills the window; the foreground has no X
        // offset at all (unlike CenterCrop's `crop=608:1080:X:0`).
        assert!(fc.contains("crop=608:1080,boxblur"));
        assert!(!fc.contains("crop=608:1080:"));
        // Filtered video and optional audio are mapped explicitly.
        assert!(args.iter().any(|a| a == "[vout]"));
        assert!(args.iter().any(|a| a == "0:a?"));
    }

    #[test]
    fn every_generated_dimension_is_even() {
        for (w, h) in [(1920, 1080), (1440, 1080), (1080, 1920), (640, 480), (3840, 2160)] {
            let (cw, ch) = canvas_dims(w, h);
            assert_eq!(cw % 2, 0, "canvas width odd for {w}x{h}");
            assert_eq!(ch % 2, 0, "canvas height odd for {w}x{h}");
            assert_eq!(crop_width_px(ch) % 2, 0);
            let x = crop_x_px(0.5, cw, ch);
            assert!(x as u64 + crop_width_px(ch) as u64 <= cw as u64);
        }
    }
}
