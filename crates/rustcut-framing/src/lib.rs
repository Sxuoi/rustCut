//! Framing math for vertical output (PRD §4.5) and the FFmpeg video filter
//! chains that use it (§7.6 C/D/E/F).
//!
//! The overlay in the UI calls [`crop_window`] over IPC (FRM-04), and the
//! export builds its `crop=` from the same function, so preview and output
//! cannot drift apart. Every number placed in a filter string is a literal
//! integer computed here.

use rustcut_model::{CropWindow, ExportSettings, OutputResolution, VideoInfo};

/// HDR → SDR BT.709 tone-map chain F (FRM-07, needs `zscale`).
pub const HDR_TONEMAP_CHAIN: &str = "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p";

/// Output colour tags added after chain F.
pub const BT709_TAG_ARGS: [&str; 6] = [
    "-color_primaries",
    "bt709",
    "-color_trc",
    "bt709",
    "-colorspace",
    "bt709",
];

fn even_round(v: f64) -> u32 {
    ((v / 2.0).round().max(0.0) as u32) * 2
}

fn floor_even(v: f64) -> u32 {
    ((v / 2.0).floor().max(0.0) as u32) * 2
}

/// Display frame (rotation applied, square pixels) from coded size, SAR and
/// clockwise rotation (FRM-02). Width is SAR-corrected to the nearest even
/// number; 90°/270° swap the axes.
pub fn display_dims(coded_w: u32, coded_h: u32, sar: (u32, u32), rotation: u32) -> (u32, u32) {
    let (n, d) = if sar.0 == 0 || sar.1 == 0 {
        (1, 1)
    } else {
        sar
    };
    let w = if n == d {
        coded_w
    } else {
        even_round(coded_w as f64 * n as f64 / d as f64).max(2)
    };
    let h = coded_h;
    match rotation % 360 {
        90 | 270 => (h, w),
        _ => (w, h),
    }
}

/// Largest 9:16 window inside the display frame (§7.7). All values even.
/// `offset` ∈ [-1, 1] slides the window horizontally; it is ignored when the
/// frame is taller than 9:16 or exactly 9:16 (window spans the full width).
pub fn crop_window(dw: u32, dh: u32, offset: f64) -> CropWindow {
    let off = if offset.is_finite() {
        offset.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    if dw < 2 || dh < 2 {
        return CropWindow {
            w: dw & !1,
            h: dh & !1,
            x: 0,
            y: 0,
        };
    }
    if dw as u64 * 16 >= dh as u64 * 9 {
        // At least as wide as 9:16: full height, slide on X.
        let h = dh & !1;
        let w = even_round(dh as f64 * 9.0 / 16.0).min(dw & !1);
        let x = floor_even((dw - w) as f64 * (1.0 + off) / 2.0);
        CropWindow { w, h, x, y: 0 }
    } else {
        // Taller than 9:16: full width, centred on Y, offset ignored.
        let w = dw & !1;
        let h = even_round(dw as f64 * 16.0 / 9.0).min(dh & !1);
        CropWindow {
            w,
            h,
            x: 0,
            y: floor_even((dh - h) as f64 / 2.0),
        }
    }
}

/// True when the window already spans the full width, so the crop position
/// has no effect (FRM-03: 9:16 and taller sources).
pub fn offset_has_effect(dw: u32, dh: u32) -> bool {
    let w = crop_window(dw, dh, 0.0);
    w.w < (dw & !1)
}

/// Inverse of the x mapping: offset that places the window's left edge at
/// `x` (used when the user drags the overlay). Clamped to [-1, 1].
pub fn offset_for_x(dw: u32, dh: u32, x: f64) -> f64 {
    let w = crop_window(dw, dh, 0.0);
    let range = dw.saturating_sub(w.w) as f64;
    if range <= 0.0 {
        return 0.0;
    }
    (x / range * 2.0 - 1.0).clamp(-1.0, 1.0)
}

/// Inputs shared by every video chain.
#[derive(Debug, Clone, PartialEq)]
pub struct FramingInput {
    pub display_w: u32,
    pub display_h: u32,
    /// Normalise non-square pixels first (`scale=DW:DH,setsar=1`).
    pub normalize_sar: bool,
    /// Insert `fps=N` (source exceeds the cap; FRM-06).
    pub fps_cap: Option<u32>,
    /// Prepend chain F (HDR source and `zscale` available; FRM-07).
    pub tonemap_hdr: bool,
    pub output: OutputResolution,
}

impl FramingInput {
    /// Derive from the probed video and the export settings.
    pub fn new(video: &VideoInfo, settings: &ExportSettings, zscale_available: bool) -> Self {
        let sar = video.sar_ratio();
        FramingInput {
            display_w: video.display_width,
            display_h: video.display_height,
            normalize_sar: sar.0 != sar.1,
            fps_cap: effective_fps_cap(video.fps(), settings.fps_cap),
            tonemap_hdr: video.hdr && zscale_available,
            output: settings.output_resolution,
        }
    }

    fn prefix(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if self.tonemap_hdr {
            parts.push(HDR_TONEMAP_CHAIN.to_string());
        }
        if self.normalize_sar {
            parts.push(format!("scale={}:{}", self.display_w, self.display_h));
            parts.push("setsar=1".to_string());
        }
        parts
    }
}

/// `Some(cap)` only when the source rate is known to exceed the cap.
pub fn effective_fps_cap(source_fps: Option<f64>, cap: Option<u32>) -> Option<u32> {
    match (source_fps, cap) {
        (Some(fps), Some(cap)) if cap > 0 && fps > cap as f64 + 0.01 => Some(cap),
        _ => None,
    }
}

/// Output size of a vertical export (FRM-06): fixed size, or the window size
/// for Native (never upscaled).
pub fn output_dims(output: OutputResolution, window: &CropWindow) -> (u32, u32) {
    output.dims().unwrap_or((window.w, window.h))
}

/// `-vf` for Frame-Accurate *Original* (chain C): only tone-map and frame-rate
/// cap apply; `None` when nothing is needed.
pub fn original_filter(input: &FramingInput) -> Option<String> {
    let mut parts = Vec::new();
    if input.tonemap_hdr {
        parts.push(HDR_TONEMAP_CHAIN.to_string());
    }
    if let Some(cap) = input.fps_cap {
        parts.push(format!("fps={cap}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(","))
    }
}

/// `-vf` for Vertical Crop 9:16 (chain D):
/// `[F,][scale=DW:DH,setsar=1,]crop=W:H:X:Y[,fps=N][,scale=OW:OH:flags=lanczos],setsar=1`.
pub fn vertical_crop_filter(input: &FramingInput, crop_x_offset: f64) -> String {
    let win = crop_window(input.display_w, input.display_h, crop_x_offset);
    let mut parts = input.prefix();
    parts.push(format!("crop={}:{}:{}:{}", win.w, win.h, win.x, win.y));
    if let Some(cap) = input.fps_cap {
        parts.push(format!("fps={cap}"));
    }
    if let Some((ow, oh)) = input.output.dims() {
        parts.push(format!("scale={ow}:{oh}:flags=lanczos"));
    }
    parts.push("setsar=1".to_string());
    parts.join(",")
}

/// Output label of [`vertical_blur_filter`]; map it with `-map "[vout]"`.
pub const BLUR_OUTPUT_LABEL: &str = "[vout]";

/// `-filter_complex` for Vertical Blur 9:16 (chain E): the full display frame
/// fitted inside the output over a quarter-resolution, blurred, cover-scaled
/// copy of itself.
pub fn vertical_blur_filter(input: &FramingInput) -> String {
    let win = crop_window(input.display_w, input.display_h, 0.0);
    let (ow, oh) = output_dims(input.output, &win);
    // Quarter resolution background, kept even.
    let bw = ((ow / 4) & !1).max(2);
    let bh = ((oh / 4) & !1).max(2);
    let mut head = input.prefix();
    if let Some(cap) = input.fps_cap {
        head.push(format!("fps={cap}"));
    }
    head.push("split=2[bg][fg]".to_string());
    format!(
        "[0:v]{head};[bg]scale={bw}:{bh}:force_original_aspect_ratio=increase,crop={bw}:{bh},boxblur=10:2,scale={ow}:{oh},setsar=1[bgv];[fg]scale={ow}:{oh}:force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1[fgv];[bgv][fgv]overlay=(W-w)/2:(H-h)/2,format=yuv420p{BLUR_OUTPUT_LABEL}",
        head = head.join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> serde_json::Value {
        serde_json::from_str(include_str!("../tests/crop_vectors.json")).unwrap()
    }

    #[test]
    fn prd_vectors() {
        let v = vectors();
        let offsets: Vec<f64> = v["offsets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o.as_f64().unwrap())
            .collect();
        for case in v["cases"].as_array().unwrap() {
            let (dw, dh) = (
                case["dw"].as_u64().unwrap() as u32,
                case["dh"].as_u64().unwrap() as u32,
            );
            for (i, off) in offsets.iter().enumerate() {
                let win = crop_window(dw, dh, *off);
                let label = case["label"].as_str().unwrap();
                assert_eq!(win.w as u64, case["w"].as_u64().unwrap(), "{label} w");
                assert_eq!(win.h as u64, case["h"].as_u64().unwrap(), "{label} h");
                assert_eq!(
                    win.x as u64,
                    case["x"][i].as_u64().unwrap(),
                    "{label} x @ {off}"
                );
                assert_eq!(win.y as u64, case["y"].as_u64().unwrap(), "{label} y");
                // Never leaves the picture.
                assert!(win.x + win.w <= dw && win.y + win.h <= dh, "{label} inside");
                assert!(
                    [win.w, win.h, win.x, win.y]
                        .iter()
                        .all(|v| v.is_multiple_of(2)),
                    "{label} even"
                );
            }
        }
    }

    #[test]
    fn window_always_inside_for_any_frame() {
        for dw in (2..4000).step_by(37) {
            for dh in (2..4000).step_by(41) {
                for off in [-1.0, -0.37, 0.0, 0.5, 1.0, f64::NAN, 7.0] {
                    let w = crop_window(dw, dh, off);
                    assert!(
                        w.x + w.w <= dw && w.y + w.h <= dh,
                        "{dw}x{dh} @ {off}: {w:?}"
                    );
                    // 9:16 within one even step.
                    let ratio = w.w as f64 / w.h as f64;
                    assert!(
                        (ratio - 9.0 / 16.0).abs() < 2.5 / w.h.max(1) as f64 + 0.01,
                        "{dw}x{dh}: {w:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn display_dims_rules() {
        assert_eq!(display_dims(720, 576, (16, 15), 0), (768, 576));
        assert_eq!(display_dims(1920, 1080, (1, 1), 90), (1080, 1920));
        assert_eq!(display_dims(1920, 1080, (1, 1), 270), (1080, 1920));
        assert_eq!(display_dims(1920, 1080, (0, 0), 180), (1920, 1080));
    }

    fn input(dw: u32, dh: u32) -> FramingInput {
        FramingInput {
            display_w: dw,
            display_h: dh,
            normalize_sar: false,
            fps_cap: None,
            tonemap_hdr: false,
            output: OutputResolution::Full1080x1920,
        }
    }

    #[test]
    fn chain_d_matches_prd() {
        assert_eq!(
            vertical_crop_filter(&input(1920, 1080), 0.3),
            "crop=608:1080:852:0,scale=1080:1920:flags=lanczos,setsar=1"
        );
        let mut sar = input(768, 576);
        sar.normalize_sar = true;
        assert_eq!(
            vertical_crop_filter(&sar, 0.0),
            "scale=768:576,setsar=1,crop=324:576:222:0,scale=1080:1920:flags=lanczos,setsar=1"
        );
        let mut native = input(1920, 1080);
        native.output = OutputResolution::Native;
        assert_eq!(
            vertical_crop_filter(&native, 0.3),
            "crop=608:1080:852:0,setsar=1"
        );
        let mut capped = input(1920, 1080);
        capped.fps_cap = Some(60);
        assert_eq!(
            vertical_crop_filter(&capped, 0.3),
            "crop=608:1080:852:0,fps=60,scale=1080:1920:flags=lanczos,setsar=1"
        );
        let mut hdr = input(1920, 1080);
        hdr.tonemap_hdr = true;
        assert!(vertical_crop_filter(&hdr, 0.3).starts_with("zscale=t=linear:npl=100,"));
    }

    #[test]
    fn chain_e_matches_prd() {
        assert_eq!(
            vertical_blur_filter(&input(1920, 1080)),
            "[0:v]split=2[bg][fg];[bg]scale=270:480:force_original_aspect_ratio=increase,crop=270:480,boxblur=10:2,scale=1080:1920,setsar=1[bgv];[fg]scale=1080:1920:force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1[fgv];[bgv][fgv]overlay=(W-w)/2:(H-h)/2,format=yuv420p[vout]"
        );
    }

    #[test]
    fn fps_cap_only_when_exceeded() {
        assert_eq!(effective_fps_cap(Some(144.0), Some(60)), Some(60));
        assert_eq!(effective_fps_cap(Some(60.0), Some(60)), None);
        assert_eq!(effective_fps_cap(Some(59.94), Some(60)), None);
        assert_eq!(effective_fps_cap(Some(144.0), None), None);
        assert_eq!(effective_fps_cap(None, Some(60)), None);
    }

    #[test]
    fn drag_inverse() {
        let win = crop_window(1920, 1080, 0.3);
        let off = offset_for_x(1920, 1080, win.x as f64);
        assert!((off - 0.3).abs() < 0.01);
        assert!(!offset_has_effect(1080, 1920));
        assert!(offset_has_effect(1920, 1080));
    }
}
