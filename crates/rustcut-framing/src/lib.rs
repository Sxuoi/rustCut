//! Framing math for vertical and square output (PRD §4.5) and the FFmpeg
//! video filter chains that use it (§7.6 C–H).
//!
//! The overlay in the UI calls [`crop_window`] and [`frame_layout`] over IPC
//! (FRM-04), and the export builds its filters from the same functions, so
//! preview and output cannot drift apart. Every number placed in a filter
//! string is a literal integer computed here; chain E keeps the PRD's runtime
//! expressions, which [`frame_layout`] reproduces (checked against FFmpeg in
//! `rustcut-ffmpeg`'s real-FFmpeg tests).

use rustcut_model::{
    BlurBackground, CropWindow, ExportSettings, FrameLayout, Framing, OutputResolution, VideoInfo,
};

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

/// Largest 1:1 window inside the display frame (FRM-09), all values even. The
/// side is the shorter dimension rounded down to an even number. A landscape
/// frame slides the window on X with `offset`, mapped as in [`crop_window`];
/// a portrait frame centres it on Y and a square frame uses the whole picture
/// (offset ignored).
pub fn square_window(dw: u32, dh: u32, offset: f64) -> CropWindow {
    let off = if offset.is_finite() {
        offset.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let side = dw.min(dh) & !1;
    if dw >= dh {
        CropWindow {
            w: side,
            h: side,
            x: floor_even((dw - side) as f64 * (1.0 + off) / 2.0),
            y: 0,
        }
    } else {
        CropWindow {
            w: side,
            h: side,
            x: 0,
            y: floor_even((dh - side) as f64 / 2.0),
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
    /// Colour primaries and transfer of the frames entering the graph (BT.709
    /// after chain F). FFmpeg's stack filters drop both, so chains G and H
    /// set them back with `setparams`; `None` when unknown.
    pub color_primaries: Option<String>,
    pub color_trc: Option<String>,
}

/// `color_primaries` names `setparams` accepts (FFmpeg's own names, as FFprobe prints them).
const PRIMARIES: [&str; 12] = [
    "bt709",
    "bt470m",
    "bt470bg",
    "smpte170m",
    "smpte240m",
    "film",
    "bt2020",
    "smpte428",
    "smpte431",
    "smpte432",
    "jedec-p22",
    "ebu3213",
];

/// `color_trc` names `setparams` accepts.
const TRANSFERS: [&str; 16] = [
    "bt709",
    "bt470m",
    "bt470bg",
    "smpte170m",
    "smpte240m",
    "linear",
    "log100",
    "log316",
    "iec61966-2-4",
    "bt1361e",
    "iec61966-2-1",
    "bt2020-10",
    "bt2020-12",
    "smpte2084",
    "smpte428",
    "arib-std-b67",
];

fn known(value: Option<&str>, names: &[&str]) -> Option<String> {
    value.filter(|v| names.contains(v)).map(str::to_string)
}

impl FramingInput {
    /// Derive from the probed video and the export settings.
    pub fn new(video: &VideoInfo, settings: &ExportSettings, zscale_available: bool) -> Self {
        let sar = video.sar_ratio();
        let tonemap_hdr = video.hdr && zscale_available;
        let (color_primaries, color_trc) = if tonemap_hdr {
            (Some("bt709".to_string()), Some("bt709".to_string()))
        } else {
            (
                known(video.color_primaries.as_deref(), &PRIMARIES),
                known(video.color_transfer.as_deref(), &TRANSFERS),
            )
        };
        FramingInput {
            display_w: video.display_width,
            display_h: video.display_height,
            normalize_sar: sar.0 != sar.1,
            fps_cap: effective_fps_cap(video.fps(), settings.fps_cap),
            tonemap_hdr,
            output: settings.output_resolution,
            color_primaries,
            color_trc,
        }
    }

    /// `setparams` restoring the known colour tags after a stack filter.
    fn color_params(&self) -> Option<String> {
        let mut set = Vec::new();
        if let Some(p) = known(self.color_primaries.as_deref(), &PRIMARIES) {
            set.push(format!("color_primaries={p}"));
        }
        if let Some(t) = known(self.color_trc.as_deref(), &TRANSFERS) {
            set.push(format!("color_trc={t}"));
        }
        (!set.is_empty()).then(|| format!("setparams={}", set.join(":")))
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

/// Output label of the blur chains (E, G, H); map it with `-map "[vout]"`.
pub const BLUR_OUTPUT_LABEL: &str = "[vout]";

/// Size of an `iw`×`ih` picture fitted inside `ow`×`oh`, aspect kept and both
/// sides even. Rounds exactly like FFmpeg's
/// `scale=OW:OH:force_original_aspect_ratio=decrease:force_divisible_by=2`
/// (`ff_scale_adjust_dimensions`), which chain E relies on.
fn fit_inside(iw: u32, ih: u32, ow: u32, oh: u32) -> (u32, u32) {
    let (iw, ih) = (u64::from(iw.max(1)), u64::from(ih.max(1)));
    let (ow, oh) = (u64::from(ow), u64::from(oh));
    // av_rescale(h, iw, ih * 2) * 2 rounds half up; then min and round down to even.
    let tmp_w = (oh * iw + ih) / (2 * ih) * 2;
    let tmp_h = (ow * ih + iw) / (2 * iw) * 2;
    let w = tmp_w.min(ow) / 2 * 2;
    let h = tmp_h.min(oh) / 2 * 2;
    // Never empty: FFmpeg rejects a 0-pixel side (only absurd aspect ratios get here).
    (w.max(2) as u32, h.max(2) as u32)
}

/// The centred part of `src` with the output's aspect ratio: what a
/// cover-scale to `ow`×`oh` followed by a centre crop keeps. Every value is
/// even, so FFmpeg's `crop` takes it as is on a 4:2:0 picture (chains G, H).
fn cover(src: &CropWindow, ow: u32, oh: u32) -> CropWindow {
    let (sw, sh) = (u64::from(src.w & !1), u64::from(src.h & !1));
    let (ow, oh) = (u64::from(ow.max(1)), u64::from(oh.max(1)));
    if sw * oh >= sh * ow {
        let w = (((sh * ow + oh / 2) / oh) & !1).max(2).min(sw);
        CropWindow {
            w: w as u32,
            h: sh as u32,
            x: src.x + (((sw - w) / 2) & !1) as u32,
            y: src.y,
        }
    } else {
        let h = (((sw * oh + ow / 2) / ow) & !1).max(2).min(sh);
        CropWindow {
            w: sw as u32,
            h: h as u32,
            x: src.x,
            y: src.y + (((sh - h) / 2) & !1) as u32,
        }
    }
}

/// Size the background is blurred at: a quarter of the output, even (FRM-05).
fn quarter(v: u32) -> u32 {
    ((v / 4) & !1).max(2)
}

/// `boxblur` radii (luma, chroma) for a `w`×`h` 4:2:0 picture: the PRD's 10,
/// capped where FFmpeg requires `radius ≤ (shorter side of the plane − 1) / 2`
/// (FFmpeg 9.0.2; only tiny Native outputs, under about 176 px, get less).
fn blur_radii(w: u32, h: u32) -> (u32, u32) {
    let max = |side: u32| side.saturating_sub(1) / 2;
    let luma = max(w.min(h)).min(10);
    let chroma = max(w.div_ceil(2).min(h.div_ceil(2))).min(10);
    (luma, chroma)
}

/// The `boxblur` filter for a `w`×`h` background: `boxblur=10:2` (§7.6 E),
/// or explicit radii when the picture is too small for 10.
fn boxblur(w: u32, h: u32) -> String {
    match blur_radii(w, h) {
        (10, 10) => "boxblur=10:2".to_string(),
        (luma, chroma) => {
            format!("boxblur=luma_radius={luma}:luma_power=2:chroma_radius={chroma}:chroma_power=2")
        }
    }
}

/// What chain E's runtime `scale=BW:BH:force_original_aspect_ratio=increase,
/// crop=BW:BH` resolves to on a `dw`×`dh` frame: the cover-scaled size and the
/// centre crop's position `(sw, sh, cx, cy)`, rounded as FFmpeg 9.0.2 does
/// (`av_rescale` rounds half up; `crop` rounds its centred position with
/// `lrint`, ties to even, then aligns it to the 4:2:0 chroma grid).
pub fn vertical_blur_background_crop(dw: u32, dh: u32, bw: u32, bh: u32) -> (u32, u32, u32, u32) {
    let (dw, dh, bw, bh) = (
        u64::from(dw.max(1)),
        u64::from(dh.max(1)),
        u64::from(bw),
        u64::from(bh),
    );
    let rescale = |a: u64, b: u64, c: u64| (a * b + c / 2) / c;
    let sw = rescale(bh, dw, dh).max(bw);
    let sh = rescale(bw, dh, dw).max(bh);
    // lrint((in - out) / 2): an odd difference ties to the even neighbour.
    let centre = |d: u64| {
        let half = d / 2;
        let rounded = if d % 2 == 1 && half % 2 == 1 {
            half + 1
        } else {
            half
        };
        rounded & !1
    };
    let cx = centre(sw - bw);
    let cy = centre(sh - bh);
    (sw as u32, sh as u32, cx as u32, cy as u32)
}

/// The display-frame region chain E's background really samples (see
/// [`vertical_blur_background_crop`]), to the nearest pixel.
fn vertical_blur_source(dw: u32, dh: u32, bw: u32, bh: u32) -> CropWindow {
    let (sw, sh, cx, cy) = vertical_blur_background_crop(dw, dh, bw, bh);
    let to_display = |v: u32, full: u32, scaled: u32| {
        ((u64::from(v) * u64::from(full) + u64::from(scaled) / 2) / u64::from(scaled.max(1))) as u32
    };
    let x = to_display(cx, dw, sw).min(dw);
    let y = to_display(cy, dh, sh).min(dh);
    CropWindow {
        w: to_display(bw, dw, sw).min(dw - x),
        h: to_display(bh, dh, sh).min(dh - y),
        x,
        y,
    }
}

/// The blurred background of an `ow`×`oh` output, taken from `source`.
fn background(source: CropWindow, ow: u32, oh: u32) -> BlurBackground {
    let (w, h) = (quarter(ow), quarter(oh));
    BlurBackground {
        source,
        w,
        h,
        radius: blur_radii(w, h).0,
    }
}

/// The full display frame fitted and centred over its own blurred,
/// cover-scaled copy (chains E and H). The overlay position is rounded down
/// to even, as FFmpeg's `overlay` does on a 4:2:0 picture.
fn fit_on_blur(dw: u32, dh: u32, ow: u32, oh: u32) -> FrameLayout {
    let window = CropWindow {
        w: dw,
        h: dh,
        x: 0,
        y: 0,
    };
    let (fw, fh) = fit_inside(dw, dh, ow, oh);
    let fg = CropWindow {
        w: fw,
        h: fh,
        x: (ow.saturating_sub(fw) / 2) & !1,
        y: (oh.saturating_sub(fh) / 2) & !1,
    };
    let blur = background(cover(&window, ow, oh), ow, oh);
    FrameLayout {
        out_w: ow,
        out_h: oh,
        window,
        fg,
        blur: Some(blur),
    }
}

/// Pixel layout of a framed export for a display frame (FRM-04, FRM-09);
/// `None` for the original framing. The player preview draws exactly this,
/// and chains D, G and H are built from it.
///
/// * Vertical Crop: the 9:16 window fills the output.
/// * Vertical Blur and Square Blur: the whole frame fitted inside the 9:16 or
///   1:1 output over a blurred, cover-scaled copy of the frame.
/// * Square on Blur: the 1:1 window scaled to the output width and centred
///   vertically over a blurred, cover-scaled copy of the same window.
///   *Native* keeps the window's pixels: output width = window side, height
///   = width × 16 / 9.
pub fn frame_layout(
    framing: Framing,
    dw: u32,
    dh: u32,
    offset: f64,
    output: OutputResolution,
) -> Option<FrameLayout> {
    let layout = match framing {
        Framing::Original => return None,
        Framing::VerticalCrop => {
            let window = crop_window(dw, dh, offset);
            let (ow, oh) = output_dims(output, &window);
            FrameLayout {
                out_w: ow,
                out_h: oh,
                window,
                fg: CropWindow {
                    w: ow,
                    h: oh,
                    x: 0,
                    y: 0,
                },
                blur: None,
            }
        }
        Framing::VerticalBlur => {
            let (ow, oh) = output_dims(output, &crop_window(dw, dh, 0.0));
            let mut layout = fit_on_blur(dw, dh, ow, oh);
            // Chain E keeps its runtime cover crop: name the region it really samples.
            if let Some(b) = layout.blur.as_mut() {
                b.source = vertical_blur_source(dw, dh, b.w, b.h);
            }
            layout
        }
        Framing::SquareBlur => {
            let (ow, oh) = output.square_dims().unwrap_or_else(|| {
                let side = square_window(dw, dh, 0.0).w;
                (side, side)
            });
            fit_on_blur(dw, dh, ow, oh)
        }
        Framing::VerticalSquare => {
            let window = square_window(dw, dh, offset);
            let (ow, oh) = output
                .dims()
                .unwrap_or((window.w, even_round(window.w as f64 * 16.0 / 9.0)));
            let fg = CropWindow {
                w: ow,
                h: ow,
                x: 0,
                y: floor_even(oh.saturating_sub(ow) as f64 / 2.0),
            };
            let blur = background(cover(&window, ow, oh), ow, oh);
            FrameLayout {
                out_w: ow,
                out_h: oh,
                window,
                fg,
                blur: Some(blur),
            }
        }
    };
    Some(layout)
}

/// `-filter_complex` for Vertical Blur 9:16 (chain E): the full display frame
/// fitted inside the output over a quarter-resolution, blurred, cover-scaled
/// copy of itself.
pub fn vertical_blur_filter(input: &FramingInput) -> String {
    let win = crop_window(input.display_w, input.display_h, 0.0);
    let (ow, oh) = output_dims(input.output, &win);
    // Quarter resolution background, kept even.
    let bw = quarter(ow);
    let bh = quarter(oh);
    let mut head = input.prefix();
    if let Some(cap) = input.fps_cap {
        head.push(format!("fps={cap}"));
    }
    head.push("split=2[bg][fg]".to_string());
    format!(
        "[0:v]{head};[bg]scale={bw}:{bh}:force_original_aspect_ratio=increase,crop={bw}:{bh},{blur},scale={ow}:{oh},setsar=1[bgv];[fg]scale={ow}:{oh}:force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1[fgv];[bgv][fgv]overlay=(W-w)/2:(H-h)/2,format=yuv420p{BLUR_OUTPUT_LABEL}",
        head = head.join(","),
        blur = boxblur(bw, bh),
    )
}

/// `-filter_complex` for Square on Blur 9:16 (chain G, FRM-09): the 1:1
/// window scaled to the output width and centred on a blurred, cover-scaled
/// copy of the same window. The crop runs before the split, so both branches
/// only ever see the window.
pub fn vertical_square_filter(input: &FramingInput, crop_x_offset: f64) -> String {
    let layout = frame_layout(
        Framing::VerticalSquare,
        input.display_w,
        input.display_h,
        crop_x_offset,
        input.output,
    )
    .expect("Square on Blur always has a layout");
    let win = &layout.window;
    let mut head = input.prefix();
    head.push(format!("crop={}:{}:{}:{}", win.w, win.h, win.x, win.y));
    if let Some(cap) = input.fps_cap {
        head.push(format!("fps={cap}"));
    }
    blur_composite(head, &layout, input.color_params())
}

/// `-filter_complex` for Square Blur 1:1 (chain H, FRM-09): the full display
/// frame fitted inside the square output over a blurred, cover-scaled copy of
/// itself. The layout of chain E on a 1:1 canvas, with literal numbers.
pub fn square_blur_filter(input: &FramingInput) -> String {
    let layout = frame_layout(
        Framing::SquareBlur,
        input.display_w,
        input.display_h,
        0.0,
        input.output,
    )
    .expect("Square Blur always has a layout");
    let mut head = input.prefix();
    if let Some(cap) = input.fps_cap {
        head.push(format!("fps={cap}"));
    }
    blur_composite(head, &layout, input.color_params())
}

/// The parts of the output the sharp window leaves to the background, as
/// crops of the full-size background, before and after the window in
/// stacking order. `None` when the window spans neither the full width nor
/// the full height (no current layout does that).
struct Bands {
    vertical: bool,
    before: Option<CropWindow>,
    after: Option<CropWindow>,
}

fn bands(l: &FrameLayout) -> Option<Bands> {
    let (ow, oh, g) = (l.out_w, l.out_h, &l.fg);
    let some =
        |w: u32, h: u32, x: u32, y: u32| (w > 0 && h > 0).then_some(CropWindow { w, h, x, y });
    if g.x == 0 && g.w == ow {
        let below = oh.saturating_sub(g.y + g.h);
        Some(Bands {
            vertical: true,
            before: some(ow, g.y, 0, 0),
            after: some(ow, below, 0, g.y + g.h),
        })
    } else if g.y == 0 && g.h == oh {
        let right = ow.saturating_sub(g.x + g.w);
        Some(Bands {
            vertical: false,
            before: some(g.x, oh, 0, 0),
            after: some(right, oh, g.x + g.w, 0),
        })
    } else {
        None
    }
}

/// The video graph of chains G and H. The sharp branch is the layout's
/// window scaled to its place (Lanczos, skipped when it already has that
/// size). The background branch crops the layout's background source,
/// blurs it at a quarter of the output size and scales it back up; the
/// bands of it around the window are cut out and stacked with the window
/// (`vstack`/`hstack`), which costs far less than an `overlay` on a whole
/// frame (docs/PERFORMANCE.md). Both branches start at the window's origin:
/// chain G crops the window before the split, chain H's window is the frame.
/// The stack makes new frames without the colour primaries and transfer, so
/// `color` (a `setparams`) puts the known ones back.
fn blur_composite(mut head: Vec<String>, layout: &FrameLayout, color: Option<String>) -> String {
    let blur = layout
        .blur
        .as_ref()
        .expect("blur layouts carry a background");
    let (ow, oh, win, fg) = (layout.out_w, layout.out_h, &layout.window, &layout.fg);
    let mut front = Vec::new();
    if (fg.w, fg.h) != (win.w, win.h) {
        front.push(format!("scale={}:{}:flags=lanczos", fg.w, fg.h));
    }
    front.push("setsar=1".to_string());
    let Some(Bands {
        vertical,
        before,
        after,
    }) = bands(layout)
    else {
        // Unreachable with today's layouts: overlay the window at its place.
        return overlay_composite(head, layout, &front);
    };
    let window_after_first_band = before.is_some();
    let parts: Vec<(&str, CropWindow)> = [
        (if vertical { "[top]" } else { "[left]" }, before),
        (if vertical { "[bottom]" } else { "[right]" }, after),
    ]
    .into_iter()
    .filter_map(|(label, band)| band.map(|b| (label, b)))
    .collect();
    if parts.is_empty() {
        // The window fills the output: nothing is left to blur.
        head.extend(front);
        head.push("format=yuv420p".to_string());
        return format!("[0:v]{}{BLUR_OUTPUT_LABEL}", head.join(","));
    }
    head.push("split=2[bg][fg]".to_string());
    let s = &blur.source;
    let mut graph = vec![
        format!("[0:v]{}", head.join(",")),
        format!(
            "[bg]crop={}:{}:{}:{},scale={}:{},{},scale={ow}:{oh},setsar=1",
            s.w,
            s.h,
            s.x - win.x,
            s.y - win.y,
            blur.w,
            blur.h,
            boxblur(blur.w, blur.h)
        ),
    ];
    let crop = |b: &CropWindow| format!("crop={}:{}:{}:{}", b.w, b.h, b.x, b.y);
    if let [(label, band)] = parts.as_slice() {
        graph[1].push_str(&format!(",{}{label}", crop(band)));
    } else {
        graph[1].push_str(",split=2[bg1][bg2]");
        for (i, (label, band)) in parts.iter().enumerate() {
            graph.push(format!("[bg{}]{}{label}", i + 1, crop(band)));
        }
    }
    graph.push(format!("[fg]{}[fgv]", front.join(",")));
    // Stacking order: the band before the window, the window, the band after it.
    let inputs: String = match parts.as_slice() {
        [(first, _), (second, _)] => format!("{first}[fgv]{second}"),
        [(only, _)] if window_after_first_band => format!("{only}[fgv]"),
        [(only, _)] => format!("[fgv]{only}"),
        _ => unreachable!("one or two bands"),
    };
    let color = color.map(|c| format!(",{c}")).unwrap_or_default();
    graph.push(format!(
        "{inputs}{}={}{color},format=yuv420p{BLUR_OUTPUT_LABEL}",
        if vertical { "vstack" } else { "hstack" },
        parts.len() + 1
    ));
    graph.join(";")
}

/// Fallback of [`blur_composite`]: the whole background, the window overlaid.
fn overlay_composite(mut head: Vec<String>, layout: &FrameLayout, front: &[String]) -> String {
    let blur = layout
        .blur
        .as_ref()
        .expect("blur layouts carry a background");
    let (ow, oh, win, fg) = (layout.out_w, layout.out_h, &layout.window, &layout.fg);
    head.push("split=2[bg][fg]".to_string());
    let s = &blur.source;
    format!(
        "[0:v]{head};[bg]crop={}:{}:{}:{},scale={}:{},{},scale={ow}:{oh},setsar=1[bgv];[fg]{front}[fgv];[bgv][fgv]overlay={}:{},format=yuv420p{BLUR_OUTPUT_LABEL}",
        s.w,
        s.h,
        s.x - win.x,
        s.y - win.y,
        blur.w,
        blur.h,
        boxblur(blur.w, blur.h),
        fg.x,
        fg.y,
        head = head.join(","),
        front = front.join(","),
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
            color_primaries: None,
            color_trc: None,
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

    #[test]
    fn square_vectors() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../tests/square_vectors.json")).unwrap();
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
            let label = case["label"].as_str().unwrap();
            for (i, off) in offsets.iter().enumerate() {
                let win = square_window(dw, dh, *off);
                let want = CropWindow {
                    w: case["w"].as_u64().unwrap() as u32,
                    h: case["h"].as_u64().unwrap() as u32,
                    x: case["x"][i].as_u64().unwrap() as u32,
                    y: case["y"].as_u64().unwrap() as u32,
                };
                assert_eq!(win, want, "{label} @ {off}");
            }
        }
    }

    #[test]
    fn square_window_always_inside_square_and_even() {
        for dw in (2..4000).step_by(37) {
            for dh in (2..4000).step_by(41) {
                for off in [-1.0, -0.37, 0.0, 0.5, 1.0, f64::NAN, 7.0] {
                    let w = square_window(dw, dh, off);
                    assert!(
                        w.x + w.w <= dw && w.y + w.h <= dh,
                        "{dw}x{dh} @ {off}: {w:?}"
                    );
                    assert_eq!(w.w, w.h, "{dw}x{dh}: square");
                    assert!(w.w + 1 >= dw.min(dh), "{dw}x{dh}: largest square");
                    assert!(
                        [w.w, w.h, w.x, w.y].iter().all(|v| v.is_multiple_of(2)),
                        "{dw}x{dh} @ {off}: even {w:?}"
                    );
                }
            }
        }
    }

    fn rect(w: u32, h: u32, x: u32, y: u32) -> CropWindow {
        CropWindow { w, h, x, y }
    }

    #[test]
    fn layout_examples() {
        use OutputResolution::{Full1080x1920 as Full, Hd720x1280 as Hd, Native};
        // Square on Blur, 1080p at offset 0: the square at y = 420, its centre
        // 9:16 strip (the 9:16 crop window) feeds the background.
        let l = frame_layout(Framing::VerticalSquare, 1920, 1080, 0.0, Full).unwrap();
        assert_eq!((l.out_w, l.out_h), (1080, 1920));
        assert_eq!(l.window, rect(1080, 1080, 420, 0));
        assert_eq!(l.fg, rect(1080, 1080, 0, 420));
        let b = l.blur.unwrap();
        assert_eq!((b.source, b.w, b.h), (rect(608, 1080, 656, 0), 270, 480));
        // 720x1280: the square shrinks to the output width.
        let l = frame_layout(Framing::VerticalSquare, 1920, 1080, 1.0, Hd).unwrap();
        assert_eq!(
            (l.out_w, l.out_h, l.fg),
            (720, 1280, rect(720, 720, 0, 280))
        );
        assert_eq!(l.window, rect(1080, 1080, 840, 0));
        assert_eq!(l.blur.unwrap().source, rect(608, 1080, 1076, 0));
        // Native keeps the window's pixels: 720 wide, 16:9 tall.
        let l = frame_layout(Framing::VerticalSquare, 1280, 720, 0.0, Native).unwrap();
        assert_eq!(
            (l.out_w, l.out_h, l.fg),
            (720, 1280, rect(720, 720, 0, 280))
        );
        // Even values, so FFmpeg's crop takes the background source as is.
        assert_eq!(l.blur.unwrap().source, rect(404, 720, 438, 0));
        // Square Blur: the whole frame fitted inside 1080x1080.
        let l = frame_layout(Framing::SquareBlur, 1920, 1080, 0.7, Full).unwrap();
        assert_eq!((l.out_w, l.out_h), (1080, 1080));
        assert_eq!(l.window, rect(1920, 1080, 0, 0));
        assert_eq!(l.fg, rect(1080, 608, 0, 236));
        let b = l.blur.unwrap();
        assert_eq!((b.source, b.w, b.h), (rect(1080, 1080, 420, 0), 270, 270));
        let l = frame_layout(Framing::SquareBlur, 1920, 1080, 0.0, Hd).unwrap();
        assert_eq!((l.out_w, l.fg), (720, rect(720, 406, 0, 156)));
        let l = frame_layout(Framing::SquareBlur, 1080, 1920, 0.0, Native).unwrap();
        assert_eq!(
            (l.out_w, l.out_h, l.fg),
            (1080, 1080, rect(608, 1080, 236, 0))
        );
        assert_eq!(l.blur.unwrap().source, rect(1080, 1080, 0, 420));
        // Vertical Blur: chain E's runtime numbers, background included
        // (853x480 cover scale, crop at x = lrint(291.5) = 292, so source x = 657, not 656).
        let l = frame_layout(Framing::VerticalBlur, 1920, 1080, 0.0, Full).unwrap();
        assert_eq!(
            (l.out_w, l.out_h, l.fg),
            (1080, 1920, rect(1080, 608, 0, 656))
        );
        assert_eq!(
            vertical_blur_background_crop(1920, 1080, 270, 480),
            (853, 480, 292, 0)
        );
        assert_eq!(l.blur.as_ref().unwrap().source, rect(608, 1080, 657, 0));
        // 1280x720 Native: 320x180 scale, 100x180 crop at x = 110 = 400x720 at 440.
        let l = frame_layout(Framing::VerticalBlur, 1280, 720, 0.0, Native).unwrap();
        assert_eq!(l.blur.unwrap().source, rect(400, 720, 440, 0));
        let l = frame_layout(Framing::VerticalBlur, 1920, 1080, 0.0, Native).unwrap();
        assert_eq!(
            (l.out_w, l.out_h, l.fg),
            (608, 1080, rect(608, 342, 0, 368))
        );
        // Vertical Crop: the window fills the output, no background.
        let l = frame_layout(Framing::VerticalCrop, 1920, 1080, 0.3, Full).unwrap();
        assert_eq!(
            (l.window, l.fg),
            (rect(608, 1080, 852, 0), rect(1080, 1920, 0, 0))
        );
        assert!(l.blur.is_none());
        assert!(frame_layout(Framing::Original, 1920, 1080, 0.0, Full).is_none());
    }

    #[test]
    fn fit_inside_matches_ffmpeg_rounding() {
        assert_eq!(fit_inside(1920, 1080, 1080, 1920), (1080, 608));
        assert_eq!(fit_inside(1440, 1080, 1080, 1920), (1080, 810));
        assert_eq!(fit_inside(2560, 1080, 1080, 1920), (1080, 456));
        assert_eq!(fit_inside(1080, 1080, 1080, 1920), (1080, 1080));
        assert_eq!(fit_inside(1080, 1920, 1080, 1920), (1080, 1920));
        assert_eq!(fit_inside(1080, 2340, 1080, 1920), (886, 1920));
        assert_eq!(fit_inside(1920, 1080, 720, 720), (720, 406));
    }

    #[test]
    fn layouts_stay_inside_and_even() {
        use OutputResolution::{Full1080x1920, Hd720x1280, Native};
        let framings = [
            Framing::VerticalCrop,
            Framing::VerticalBlur,
            Framing::VerticalSquare,
            Framing::SquareBlur,
        ];
        for dw in (16..4000).step_by(97) {
            for dh in (16..4000).step_by(89) {
                for f in framings {
                    for res in [Full1080x1920, Hd720x1280, Native] {
                        for off in [-1.0, 0.0, 0.41, 1.0] {
                            let l = frame_layout(f, dw & !1, dh & !1, off, res).unwrap();
                            let (dw, dh) = (dw & !1, dh & !1);
                            let tag = format!("{f:?} {dw}x{dh} {res:?} @ {off}: {l:?}");
                            let w = &l.window;
                            assert!(w.x + w.w <= dw && w.y + w.h <= dh, "window inside {tag}");
                            let g = &l.fg;
                            assert!(
                                g.x + g.w <= l.out_w && g.y + g.h <= l.out_h,
                                "fg inside {tag}"
                            );
                            assert!(
                                [l.out_w, l.out_h, g.w, g.h, g.x, g.y]
                                    .iter()
                                    .all(|v| v.is_multiple_of(2) && *v < 10_000),
                                "even {tag}"
                            );
                            assert!(g.w >= 2 && g.h >= 2, "visible {tag}");
                            if let Some(b) = &l.blur {
                                let s = &b.source;
                                assert!(s.x + s.w <= dw && s.y + s.h <= dh, "bg inside {tag}");
                                assert!(
                                    s.x >= w.x
                                        && s.y >= w.y
                                        && s.x + s.w <= w.x + w.w
                                        && s.y + s.h <= w.y + w.h,
                                    "bg comes from the window {tag}"
                                );
                                assert!(b.w.is_multiple_of(2) && b.h.is_multiple_of(2), "{tag}");
                                // Chains G and H crop this rectangle literally: the output's
                                // aspect within the 2 px of even rounding, every value even.
                                // Chain E's is what FFmpeg's runtime cover crop samples.
                                if f != Framing::VerticalBlur {
                                    let lhs = f64::from(s.w) * f64::from(l.out_h);
                                    let rhs = f64::from(s.h) * f64::from(l.out_w);
                                    assert!(
                                        (lhs - rhs).abs() <= 2.0 * f64::from(l.out_w.max(l.out_h)),
                                        "bg aspect {tag}"
                                    );
                                    assert!(
                                        [s.w, s.h, s.x, s.y].iter().all(|v| v.is_multiple_of(2)),
                                        "bg even {tag}"
                                    );
                                }
                                let (luma, chroma) = blur_radii(b.w, b.h);
                                assert_eq!(b.radius, luma, "{tag}");
                                // FFmpeg: radius <= (shorter side of the plane - 1) / 2.
                                assert!(2 * luma < b.w.min(b.h), "luma radius {tag}");
                                assert!(
                                    2 * chroma < b.w.div_ceil(2).min(b.h.div_ceil(2)),
                                    "chroma radius {tag}"
                                );
                            }
                            match f {
                                Framing::VerticalSquare => {
                                    assert_eq!(w.w, w.h, "{tag}");
                                    assert_eq!((g.w, g.h, g.x), (l.out_w, l.out_w, 0), "{tag}");
                                    assert!(l.out_h - g.h - 2 * g.y <= 2, "centred {tag}");
                                }
                                Framing::SquareBlur => assert_eq!(l.out_w, l.out_h, "{tag}"),
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn chain_d_and_e_use_the_layout_numbers() {
        use OutputResolution::{Full1080x1920, Hd720x1280, Native};
        for (dw, dh) in [
            (1920, 1080),
            (1440, 1080),
            (2560, 1080),
            (1080, 1920),
            (768, 576),
        ] {
            for res in [Full1080x1920, Hd720x1280, Native] {
                let mut x = input(dw, dh);
                x.output = res;
                let d = frame_layout(Framing::VerticalCrop, dw, dh, 0.3, res).unwrap();
                let w = &d.window;
                assert!(
                    vertical_crop_filter(&x, 0.3)
                        .starts_with(&format!("crop={}:{}:{}:{}", w.w, w.h, w.x, w.y)),
                    "{dw}x{dh} {res:?}"
                );
                let e = frame_layout(Framing::VerticalBlur, dw, dh, 0.0, res).unwrap();
                let b = e.blur.unwrap();
                let chain = vertical_blur_filter(&x);
                assert!(
                    chain.contains(&format!(
                        "[bg]scale={0}:{1}:force_original_aspect_ratio=increase,crop={0}:{1},boxblur=10:2,scale={2}:{3},",
                        b.w, b.h, e.out_w, e.out_h
                    )),
                    "{dw}x{dh} {res:?}: {chain}"
                );
                assert!(
                    chain.contains(&format!(
                        "[fg]scale={}:{}:force_original_aspect_ratio=decrease",
                        e.out_w, e.out_h
                    )),
                    "{chain}"
                );
            }
        }
    }

    #[test]
    fn chain_g_matches_prd() {
        assert_eq!(
            vertical_square_filter(&input(1920, 1080), 0.3),
            "[0:v]crop=1080:1080:546:0,split=2[bg][fg];[bg]crop=608:1080:236:0,scale=270:480,boxblur=10:2,scale=1080:1920,setsar=1,split=2[bg1][bg2];[bg1]crop=1080:420:0:0[top];[bg2]crop=1080:420:0:1500[bottom];[fg]setsar=1[fgv];[top][fgv][bottom]vstack=3,format=yuv420p[vout]"
        );
        // A 720p window is scaled up with Lanczos.
        assert_eq!(
            vertical_square_filter(&input(1280, 720), 0.0),
            "[0:v]crop=720:720:280:0,split=2[bg][fg];[bg]crop=404:720:158:0,scale=270:480,boxblur=10:2,scale=1080:1920,setsar=1,split=2[bg1][bg2];[bg1]crop=1080:420:0:0[top];[bg2]crop=1080:420:0:1500[bottom];[fg]scale=1080:1080:flags=lanczos,setsar=1[fgv];[top][fgv][bottom]vstack=3,format=yuv420p[vout]"
        );
        // Native keeps the window size: 720x1280 around a 720x720 window.
        let mut native = input(1280, 720);
        native.output = OutputResolution::Native;
        assert_eq!(
            vertical_square_filter(&native, 0.0),
            "[0:v]crop=720:720:280:0,split=2[bg][fg];[bg]crop=404:720:158:0,scale=180:320,boxblur=10:2,scale=720:1280,setsar=1,split=2[bg1][bg2];[bg1]crop=720:280:0:0[top];[bg2]crop=720:280:0:1000[bottom];[fg]setsar=1[fgv];[top][fgv][bottom]vstack=3,format=yuv420p[vout]"
        );
        // Order: tone-map, SAR, crop, frame-rate cap, then the split (§7.6 F).
        let mut all = input(768, 576);
        all.normalize_sar = true;
        all.fps_cap = Some(30);
        all.tonemap_hdr = true;
        assert!(vertical_square_filter(&all, 0.0).starts_with(&format!(
            "[0:v]{HDR_TONEMAP_CHAIN},scale=768:576,setsar=1,crop=576:576:96:0,fps=30,split=2[bg][fg];"
        )));
    }

    #[test]
    fn chain_h_matches_prd() {
        assert_eq!(
            square_blur_filter(&input(1920, 1080)),
            "[0:v]split=2[bg][fg];[bg]crop=1080:1080:420:0,scale=270:270,boxblur=10:2,scale=1080:1080,setsar=1,split=2[bg1][bg2];[bg1]crop=1080:236:0:0[top];[bg2]crop=1080:236:0:844[bottom];[fg]scale=1080:608:flags=lanczos,setsar=1[fgv];[top][fgv][bottom]vstack=3,format=yuv420p[vout]"
        );
        let mut hd = input(1920, 1080);
        hd.output = OutputResolution::Hd720x1280;
        assert_eq!(
            square_blur_filter(&hd),
            "[0:v]split=2[bg][fg];[bg]crop=1080:1080:420:0,scale=180:180,boxblur=10:2,scale=720:720,setsar=1,split=2[bg1][bg2];[bg1]crop=720:156:0:0[top];[bg2]crop=720:158:0:562[bottom];[fg]scale=720:406:flags=lanczos,setsar=1[fgv];[top][fgv][bottom]vstack=3,format=yuv420p[vout]"
        );
        // Portrait sources are fitted by height: bands left and right, hstack.
        assert_eq!(
            square_blur_filter(&input(1080, 1920)),
            "[0:v]split=2[bg][fg];[bg]crop=1080:1080:0:420,scale=270:270,boxblur=10:2,scale=1080:1080,setsar=1,split=2[bg1][bg2];[bg1]crop=236:1080:0:0[left];[bg2]crop=236:1080:844:0[right];[fg]scale=608:1080:flags=lanczos,setsar=1[fgv];[left][fgv][right]hstack=3,format=yuv420p[vout]"
        );
        // A square source fills the square output: nothing to blur.
        let mut native = input(1080, 1080);
        native.output = OutputResolution::Native;
        assert_eq!(
            square_blur_filter(&native),
            "[0:v]setsar=1,format=yuv420p[vout]"
        );
        let mut small = input(1080, 1080);
        small.output = OutputResolution::Hd720x1280;
        assert_eq!(
            square_blur_filter(&small),
            "[0:v]scale=720:720:flags=lanczos,setsar=1,format=yuv420p[vout]"
        );
        // Two pixels short of square: one band (below), vstack of two; the frame keeps its size.
        assert_eq!(
            square_blur_filter(&input(1080, 1078)),
            "[0:v]split=2[bg][fg];[bg]crop=1078:1078:0:0,scale=270:270,boxblur=10:2,scale=1080:1080,setsar=1,crop=1080:2:0:1078[bottom];[fg]setsar=1[fgv];[fgv][bottom]vstack=2,format=yuv420p[vout]"
        );
        let mut all = input(1920, 1080);
        all.fps_cap = Some(60);
        all.tonemap_hdr = true;
        assert!(
            square_blur_filter(&all)
                .starts_with(&format!("[0:v]{HDR_TONEMAP_CHAIN},fps=60,split=2[bg][fg];"))
        );
    }

    #[test]
    fn stacked_chains_restore_the_colour_tags() {
        let mut tagged = input(1920, 1080);
        tagged.color_primaries = Some("bt709".into());
        tagged.color_trc = Some("smpte170m".into());
        assert!(vertical_square_filter(&tagged, 0.0).ends_with(
            "vstack=3,setparams=color_primaries=bt709:color_trc=smpte170m,format=yuv420p[vout]"
        ));
        assert!(square_blur_filter(&tagged).contains("vstack=3,setparams=color_primaries=bt709:"));
        // Unknown or unexpected names are left out, never passed into the graph.
        tagged.color_primaries = Some("unknown".into());
        tagged.color_trc = Some("bt709,evil=1".into());
        assert!(vertical_square_filter(&tagged, 0.0).ends_with("vstack=3,format=yuv420p[vout]"));
        // Tone-mapped sources are BT.709 after chain F.
        let video = VideoInfo {
            codec: "hevc".into(),
            coded_width: 1920,
            coded_height: 1080,
            display_width: 1920,
            display_height: 1080,
            rotation: 0,
            sar: "1:1".into(),
            frame_rate: "30/1".into(),
            vfr: false,
            hdr: true,
            stream_index: Some(0),
            pix_fmt: Some("yuv420p10le".into()),
            profile: None,
            level: None,
            color_transfer: Some("arib-std-b67".into()),
            color_primaries: Some("bt2020".into()),
            bit_depth: Some(10),
        };
        let mut settings = ExportSettings::default();
        settings.apply_preset(rustcut_model::Preset::Vsquare);
        let hdr = FramingInput::new(&video, &settings, true);
        assert!(
            vertical_square_filter(&hdr, 0.0)
                .contains("setparams=color_primaries=bt709:color_trc=bt709,")
        );
        // Without zscale the HDR tags pass through unchanged.
        let raw = FramingInput::new(&video, &settings, false);
        assert!(
            square_blur_filter(&raw)
                .contains("setparams=color_primaries=bt2020:color_trc=arib-std-b67,")
        );
        // A window that fills the output needs no stack, so nothing is lost.
        let mut whole = input(1080, 1080);
        whole.output = OutputResolution::Native;
        whole.color_trc = Some("bt709".into());
        assert_eq!(
            square_blur_filter(&whole),
            "[0:v]setsar=1,format=yuv420p[vout]"
        );
    }

    #[test]
    fn tiny_outputs_get_a_blur_ffmpeg_accepts() {
        // FFmpeg needs 2 x radius <= min(plane width, height); chroma planes are half size.
        assert_eq!(blur_radii(270, 480), (10, 10));
        assert_eq!(blur_radii(44, 44), (10, 10));
        assert_eq!(blur_radii(40, 40), (10, 9));
        assert_eq!(blur_radii(32, 56), (10, 7));
        assert_eq!(blur_radii(18, 32), (8, 4));
        assert_eq!(blur_radii(12, 22), (5, 2));
        assert_eq!(blur_radii(2, 2), (0, 0));
        let mut sq = input(128, 128);
        sq.output = OutputResolution::Native;
        assert!(
            vertical_square_filter(&sq, 0.0).contains(
                "scale=32:56,boxblur=luma_radius=10:luma_power=2:chroma_radius=7:chroma_power=2,scale=128:228,"
            ),
            "{}",
            vertical_square_filter(&sq, 0.0)
        );
        let mut e = input(160, 90);
        e.output = OutputResolution::Native;
        assert!(
            vertical_blur_filter(&e).contains(
                "crop=12:22,boxblur=luma_radius=5:luma_power=2:chroma_radius=2:chroma_power=2,scale=50:90,"
            ),
            "{}",
            vertical_blur_filter(&e)
        );
        // Every regular output keeps the PRD's boxblur=10:2.
        for (dw, dh) in [(1920, 1080), (1280, 720), (854, 480), (640, 360)] {
            for res in [
                OutputResolution::Full1080x1920,
                OutputResolution::Hd720x1280,
            ] {
                let mut x = input(dw, dh);
                x.output = res;
                for chain in [
                    vertical_blur_filter(&x),
                    vertical_square_filter(&x, 0.0),
                    square_blur_filter(&x),
                ] {
                    assert!(chain.contains(",boxblur=10:2,"), "{chain}");
                }
            }
        }
    }

    #[test]
    fn bands_and_window_tile_the_output() {
        for (dw, dh) in [
            (1920, 1080),
            (1080, 1920),
            (1080, 1078),
            (1078, 1080),
            (2560, 1080),
            (1080, 2340),
        ] {
            for f in [
                Framing::VerticalBlur,
                Framing::VerticalSquare,
                Framing::SquareBlur,
            ] {
                for res in [OutputResolution::Full1080x1920, OutputResolution::Native] {
                    let l = frame_layout(f, dw, dh, 0.25, res).unwrap();
                    let b = bands(&l).expect("the window spans a full side");
                    let (g, area) = (&l.fg, u64::from(l.out_w) * u64::from(l.out_h));
                    let mut covered = u64::from(g.w) * u64::from(g.h);
                    for band in [&b.before, &b.after].into_iter().flatten() {
                        assert!(band.x + band.w <= l.out_w && band.y + band.h <= l.out_h);
                        covered += u64::from(band.w) * u64::from(band.h);
                    }
                    assert_eq!(covered, area, "{f:?} {dw}x{dh} {res:?}: {l:?}");
                }
            }
        }
    }

    /// Golden layouts shared with the TypeScript port (`src/lib/util/crop.test.ts`).
    /// Regenerate with `RUSTCUT_UPDATE_VECTORS=1 cargo test -p rustcut-framing`
    /// after a deliberate change, and review the diff.
    #[test]
    fn layout_vectors() {
        let frames = [
            (1920, 1080),
            (3840, 2160),
            (1280, 720),
            (2560, 1080),
            (1440, 1080),
            (1080, 1080),
            (768, 576),
            (1080, 1920),
            (1080, 2340),
        ];
        let framings = [
            ("vertical_crop", Framing::VerticalCrop),
            ("vertical_blur", Framing::VerticalBlur),
            ("vertical_square", Framing::VerticalSquare),
            ("square_blur", Framing::SquareBlur),
        ];
        let resolutions = [
            ("1080x1920", OutputResolution::Full1080x1920),
            ("720x1280", OutputResolution::Hd720x1280),
            ("native", OutputResolution::Native),
        ];
        let mut cases = Vec::new();
        for (dw, dh) in frames {
            for (fname, f) in framings {
                let offsets: &[f64] = if f.uses_crop_offset() {
                    &[-1.0, 0.0, 0.3, 1.0]
                } else {
                    &[0.0]
                };
                for (rname, res) in resolutions {
                    for off in offsets {
                        let layout = frame_layout(f, dw, dh, *off, res).unwrap();
                        cases.push(serde_json::json!({
                            "framing": fname, "dw": dw, "dh": dh, "offset": off,
                            "output_resolution": rname, "layout": layout,
                        }));
                    }
                }
            }
        }
        let generated = serde_json::json!({
            "description": "Golden frame_layout results (FRM-04, FRM-09) for the §7.7 display frames. Generated by rustcut-framing's layout_vectors test (RUSTCUT_UPDATE_VECTORS=1); the TypeScript port (src/lib/util/crop.test.ts) must reproduce every case.",
            "cases": cases,
        });
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/layout_vectors.json");
        if std::env::var_os("RUSTCUT_UPDATE_VECTORS").is_some() {
            // One case per line keeps the file small and its diffs readable.
            let lines: Vec<String> = generated["cases"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| format!("    {c}"))
                .collect();
            let text = format!(
                "{{\n  \"description\": {},\n  \"cases\": [\n{}\n  ]\n}}\n",
                generated["description"],
                lines.join(",\n")
            );
            std::fs::write(&path, text).unwrap();
            return;
        }
        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("layout_vectors.json"))
                .unwrap();
        assert_eq!(
            stored, generated,
            "frame_layout changed; regenerate and review layout_vectors.json"
        );
    }
}
