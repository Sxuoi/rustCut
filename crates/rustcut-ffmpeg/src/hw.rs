//! Hardware encoders (EXP-13): quality presets mapped to each encoder's
//! rate-control mode, and the 1-second detection encode.
//!
//! | Encoder | Options (CRF = preset quality) |
//! | :--- | :--- |
//! | NVENC | `-preset p5 -rc vbr -cq CRF -b:v 0` (+ `-maxrate/-bufsize`) |
//! | AMF | `-quality quality -rc qvbr -qvbr_quality_level CRF` (+ `-maxrate/-bufsize`) |
//! | Quick Sync | `-preset medium -global_quality CRF` (ICQ; no cap), `-pix_fmt nv12` |
//! | VideoToolbox | `-q:v 100−2·CRF` (+ `-maxrate`) |
//! | VAAPI | `-rc_mode QVBR -global_quality CRF -b:v/-maxrate/-bufsize` when capped, `-rc_mode ICQ` otherwise; frames uploaded with `format=nv12,hwupload` on `-vaapi_device` |

use crate::args::{OUTPUT_PIX_FMT, fmt_bitrate};
use rustcut_model::EncoderChoice;
use std::ffi::OsString;

/// Default DRM render node for VAAPI (Linux).
pub const VAAPI_DEVICE: &str = "/dev/dri/renderD128";
/// Filter appended to the video chain so VAAPI receives hardware frames.
pub const VAAPI_UPLOAD_FILTER: &str = "format=nv12,hwupload";
/// Frame size of the detection encode. FFmpeg 9.0.2 + AMF fails to
/// initialise QVBR at 256×256 (`encoder->Init() failed with error 4`) but
/// works from 320×240 up, so the test uses a small real-world size.
pub const HW_TEST_SIZE: &str = "640x360";
/// Detection encode quality/cap: the vertical presets' values, so the test
/// exercises the same rate-control mode as real exports.
const HW_TEST_CRF: u8 = crate::args::CRF_VERTICAL;

/// Encoder-specific parts of a hardware encode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HwVideoArgs {
    pub encoder: EncoderChoice,
    /// Options after `-c:v <encoder>` (rate control included).
    pub codec_args: Vec<String>,
    /// `-pix_fmt` the encoder accepts (`None` for hardware frames).
    pub pix_fmt: Option<&'static str>,
    /// Appended to the video filter chain (VAAPI upload).
    pub filter_suffix: Option<&'static str>,
    /// Global options placed before the input (`-vaapi_device`).
    pub device_args: Vec<String>,
}

/// Hardware encoder options for a preset quality (`crf`, the libx264 CRF of
/// the preset) and optional cap (EXP-13). `None` for `Auto` and `Libx264`.
pub fn hw_video_args(
    encoder: EncoderChoice,
    crf: u8,
    maxrate_bps: Option<u64>,
    bufsize_bits: Option<u64>,
) -> Option<HwVideoArgs> {
    let q = crf.to_string();
    let cap = |with_bufsize: bool| {
        let mut v = Vec::new();
        if let Some(rate) = maxrate_bps {
            v.extend(["-maxrate".to_string(), fmt_bitrate(rate)]);
            if let (true, Some(size)) = (with_bufsize, bufsize_bits) {
                v.extend(["-bufsize".to_string(), fmt_bitrate(size)]);
            }
        }
        v
    };
    let base = |codec_args: Vec<String>, pix_fmt: Option<&'static str>| HwVideoArgs {
        encoder,
        codec_args,
        pix_fmt,
        filter_suffix: None,
        device_args: Vec::new(),
    };
    let args = match encoder {
        EncoderChoice::Auto | EncoderChoice::Libx264 => return None,
        EncoderChoice::H264Nvenc => {
            let mut v = strings(&["-preset", "p5", "-rc", "vbr", "-cq", &q, "-b:v", "0"]);
            v.extend(cap(true));
            base(v, Some(OUTPUT_PIX_FMT))
        }
        EncoderChoice::H264Amf => {
            let mut v = strings(&[
                "-quality",
                "quality",
                "-rc",
                "qvbr",
                "-qvbr_quality_level",
                &q,
            ]);
            v.extend(cap(true));
            base(v, Some(OUTPUT_PIX_FMT))
        }
        EncoderChoice::H264Qsv => base(
            strings(&["-preset", "medium", "-global_quality", &q]),
            Some("nv12"),
        ),
        EncoderChoice::H264Videotoolbox => {
            let mut v = strings(&["-q:v", &videotoolbox_quality(crf).to_string()]);
            v.extend(cap(false));
            base(v, Some(OUTPUT_PIX_FMT))
        }
        EncoderChoice::H264Vaapi => {
            // QVBR needs a bit rate in VAAPI; without a cap use ICQ (quality only).
            let v = match maxrate_bps {
                Some(rate) => {
                    let mut v = strings(&[
                        "-rc_mode",
                        "QVBR",
                        "-global_quality",
                        &q,
                        "-b:v",
                        &fmt_bitrate(rate),
                    ]);
                    v.extend(cap(true));
                    v
                }
                None => strings(&["-rc_mode", "ICQ", "-global_quality", &q]),
            };
            HwVideoArgs {
                encoder,
                codec_args: v,
                pix_fmt: None,
                filter_suffix: Some(VAAPI_UPLOAD_FILTER),
                device_args: strings(&["-vaapi_device", VAAPI_DEVICE]),
            }
        }
    };
    Some(args)
}

/// VideoToolbox `-q:v` (1–100, higher is better) for an x264-style CRF:
/// `100 − 2·CRF` (CRF 18 → 64, CRF 20 → 60), clamped to 1–100.
pub fn videotoolbox_quality(crf: u8) -> u8 {
    (100i32 - 2 * i32::from(crf)).clamp(1, 100) as u8
}

/// Detection encode (EXP-13): one second of `testsrc2` at
/// [`HW_TEST_SIZE`] encoded with the same rate-control options as a
/// vertical export, discarded with `-f null -`. Exit code 0 means usable.
/// `None` for `Auto` and `Libx264`.
pub fn hw_test_args(encoder: EncoderChoice) -> Option<Vec<OsString>> {
    let name = encoder.ffmpeg_name()?;
    let hw = hw_video_args(
        encoder,
        HW_TEST_CRF,
        Some(crate::args::VERTICAL_MAXRATE_BPS),
        Some(crate::args::VERTICAL_BUFSIZE_BITS),
    )?;
    let mut v: Vec<String> = strings(&["-hide_banner", "-nostdin", "-v", "error"]);
    v.extend(hw.device_args);
    v.extend(strings(&[
        "-f",
        "lavfi",
        "-i",
        &format!("testsrc2=size={HW_TEST_SIZE}:rate=30:duration=1"),
    ]));
    if let Some(suffix) = hw.filter_suffix {
        v.extend(strings(&["-vf", suffix]));
    }
    v.extend(strings(&["-c:v", name]));
    v.extend(hw.codec_args);
    if let Some(pix) = hw.pix_fmt {
        v.extend(strings(&["-pix_fmt", pix]));
    }
    v.extend(strings(&["-f", "null", "-"]));
    Some(v.into_iter().map(OsString::from).collect())
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(a: &HwVideoArgs) -> String {
        a.codec_args.join(" ")
    }

    #[test]
    fn exp13_rate_control_per_encoder() {
        let (m, b) = (Some(16_000_000), Some(32_000_000));
        let nv = hw_video_args(EncoderChoice::H264Nvenc, 20, m, b).unwrap();
        assert_eq!(
            joined(&nv),
            "-preset p5 -rc vbr -cq 20 -b:v 0 -maxrate 16M -bufsize 32M"
        );
        assert_eq!(nv.pix_fmt, Some("yuv420p"));
        let amf = hw_video_args(EncoderChoice::H264Amf, 20, m, b).unwrap();
        assert_eq!(
            joined(&amf),
            "-quality quality -rc qvbr -qvbr_quality_level 20 -maxrate 16M -bufsize 32M"
        );
        let qsv = hw_video_args(EncoderChoice::H264Qsv, 20, m, b).unwrap();
        assert_eq!(joined(&qsv), "-preset medium -global_quality 20");
        assert_eq!(
            qsv.pix_fmt,
            Some("nv12"),
            "h264_qsv accepts only nv12 in FFmpeg 9"
        );
        let vt = hw_video_args(EncoderChoice::H264Videotoolbox, 20, m, b).unwrap();
        assert_eq!(joined(&vt), "-q:v 60 -maxrate 16M");
        let va = hw_video_args(EncoderChoice::H264Vaapi, 20, m, b).unwrap();
        assert_eq!(
            joined(&va),
            "-rc_mode QVBR -global_quality 20 -b:v 16M -maxrate 16M -bufsize 32M"
        );
        assert_eq!(va.filter_suffix, Some("format=nv12,hwupload"));
        assert_eq!(va.device_args, ["-vaapi_device", "/dev/dri/renderD128"]);
        assert_eq!(va.pix_fmt, None);
    }

    #[test]
    fn exp13_uncapped_accurate_preset() {
        let nv = hw_video_args(EncoderChoice::H264Nvenc, 18, None, None).unwrap();
        assert_eq!(joined(&nv), "-preset p5 -rc vbr -cq 18 -b:v 0");
        let amf = hw_video_args(EncoderChoice::H264Amf, 18, None, None).unwrap();
        assert_eq!(
            joined(&amf),
            "-quality quality -rc qvbr -qvbr_quality_level 18"
        );
        let va = hw_video_args(EncoderChoice::H264Vaapi, 18, None, None).unwrap();
        assert_eq!(joined(&va), "-rc_mode ICQ -global_quality 18");
        let vt = hw_video_args(EncoderChoice::H264Videotoolbox, 18, None, None).unwrap();
        assert_eq!(joined(&vt), "-q:v 64");
    }

    #[test]
    fn software_choices_have_no_hw_args() {
        assert!(hw_video_args(EncoderChoice::Auto, 20, None, None).is_none());
        assert!(hw_video_args(EncoderChoice::Libx264, 20, None, None).is_none());
        assert!(hw_test_args(EncoderChoice::Libx264).is_none());
        assert!(hw_test_args(EncoderChoice::Auto).is_none());
    }

    #[test]
    fn videotoolbox_quality_mapping_is_clamped() {
        assert_eq!(videotoolbox_quality(18), 64);
        assert_eq!(videotoolbox_quality(0), 100);
        assert_eq!(videotoolbox_quality(51), 1);
        assert_eq!(videotoolbox_quality(255), 1);
    }

    #[test]
    fn exp13_detection_encode_shape() {
        let args: Vec<String> = hw_test_args(EncoderChoice::H264Amf)
            .unwrap()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args.join(" "),
            "-hide_banner -nostdin -v error -f lavfi -i testsrc2=size=640x360:rate=30:duration=1 -c:v h264_amf \
             -quality quality -rc qvbr -qvbr_quality_level 20 -maxrate 16M -bufsize 32M -pix_fmt yuv420p -f null -"
        );
        let va: Vec<String> = hw_test_args(EncoderChoice::H264Vaapi)
            .unwrap()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let va = va.join(" ");
        assert!(
            va.starts_with(
                "-hide_banner -nostdin -v error -vaapi_device /dev/dri/renderD128 -f lavfi"
            ),
            "{va}"
        );
        assert!(
            va.contains("-vf format=nv12,hwupload -c:v h264_vaapi"),
            "{va}"
        );
    }
}
