//! FFmpeg failure classification from the stderr tail (EXP-10, EXP-12,
//! EXP-13). Messages come from FFmpeg itself (`av_strerror`, encoder
//! wrappers) and from the OS (Windows, macOS, Linux wording).

use serde::{Deserialize, Serialize};

/// What went wrong, and therefore what the batch does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// The output volume is full: pause the batch (EXP-10).
    DiskFull,
    /// The output folder or drive vanished: pause the batch (EXP-10).
    OutputMissing,
    /// "Out of encoder sessions": re-queue the clip, lower concurrency by one
    /// (EXP-12).
    EncoderSessionsExhausted,
    /// Hardware encoder failure: retry the clip with libx264 (EXP-13).
    HwEncoderFailed,
    /// Corrupt or unreadable input data.
    InvalidData,
    Other,
}

impl FailureKind {
    /// Every remaining clip would fail too: pause with a prompt (EXP-10).
    pub fn pauses_batch(self) -> bool {
        matches!(self, FailureKind::DiskFull | FailureKind::OutputMissing)
    }

    /// Retry this clip with libx264 (EXP-13).
    pub fn retry_with_software(self) -> bool {
        self == FailureKind::HwEncoderFailed
    }

    /// Put the clip back in the queue and lower concurrency (EXP-12).
    pub fn requeue_with_less_concurrency(self) -> bool {
        self == FailureKind::EncoderSessionsExhausted
    }
}

const DISK_FULL: &[&str] = &[
    "no space left on device",      // ENOSPC (Linux, macOS, MSVCRT strerror)
    "not enough space on the disk", // Windows ERROR_DISK_FULL
    "disk quota exceeded",          // EDQUOT
    "there is not enough space",    // Windows Explorer/Win32 variants
    "insufficient disk space",
];

/// Errors that mean the output location is gone when they concern the output.
const GONE: &[&str] = &[
    "no such file or directory",
    "no such device",
    "input/output error",
    "the system cannot find the path specified",
    "the device is not ready",
    "the specified network name is no longer available",
    "the network path was not found",
    "a device which does not exist was specified",
    "device not configured", // ENXIO on macOS when a USB disk is pulled
];

/// Hints that a message concerns the output side.
const OUTPUT_CONTEXT: &[&str] = &[
    "output",
    ".part",
    "error writing",
    "av_interleaved_write_frame",
    "trailer",
    "error closing",
    "muxing",
];

/// Hints that a message concerns the input (a missing source is not a
/// vanished output folder; "Input/output error" is not input context).
const INPUT_CONTEXT: &[&str] = &["opening input", "input file", "[in#"];

/// Windows messages that only ever describe a vanished location.
const GONE_ANYWHERE: &[&str] = &[
    "the system cannot find the path specified",
    "the device is not ready",
    "the specified network name is no longer available",
    "a device which does not exist was specified",
];

const SESSIONS: &[&str] = &[
    "openencodesessionex failed: out of memory", // NVENC session limit
    "nv_enc_err_out_of_memory",
    "out of encoder sessions",
    "too many concurrent sessions",
    "concurrent session",
    "kvtvideoencodernotavailablenowerr", // VideoToolbox busy
    "-12915",
    "amf_too_many",
];

const HW_TOKENS: &[&str] = &[
    "nvenc",
    "nvcuda",
    "libcuda",
    "cuda",
    "_amf",
    "amfrt",
    "amf_",
    "_qsv",
    "mfx",
    "videotoolbox",
    "vtenc",
    "vaapi",
    "hwupload",
    "d3d11",
    "dxva",
];

const ERROR_WORDS: &[&str] = &[
    "error",
    "failed",
    "fail ",
    "cannot",
    "can't",
    "could not",
    "unable",
    "not supported",
    "unsupported",
    "invalid",
    "no device",
    "not found",
    "no such",
    "denied",
    "no space",
    "no capable",
    "unknown encoder",
    "unrecognized option",
];

const INVALID_DATA: &[&str] = &[
    "invalid data found when processing input",
    "moov atom not found",
    "error while decoding",
    "corrupt",
    "invalid nal unit",
    "partial file",
    "truncated",
];

/// Generic follow-up lines that never name the cause.
const NOISE: &[&str] = &[
    "conversion failed!",
    "task finished with error code",
    "terminating thread with return code",
    "nothing was written into output file",
    "error sending frames to consumers",
    "could not open encoder before eof",
    "error while opening encoder - maybe incorrect parameters",
];

/// Classify a failed run from its stderr tail (EXP-10).
pub fn classify(tail: &str) -> FailureKind {
    let lines: Vec<String> = tail.lines().map(str::to_ascii_lowercase).collect();
    let any = |pred: &dyn Fn(&str) -> bool| lines.iter().any(|l| pred(l));
    let has = |l: &str, list: &[&str]| list.iter().any(|p| l.contains(p));
    if any(&|l| has(l, DISK_FULL)) {
        FailureKind::DiskFull
    } else if any(&|l| {
        has(l, GONE_ANYWHERE) || (has(l, GONE) && has(l, OUTPUT_CONTEXT) && !has(l, INPUT_CONTEXT))
    }) {
        FailureKind::OutputMissing
    } else if any(&|l| has(l, SESSIONS)) {
        FailureKind::EncoderSessionsExhausted
    } else if any(&|l| has(l, HW_TOKENS) && has(l, ERROR_WORDS)) {
        FailureKind::HwEncoderFailed
    } else if any(&|l| has(l, INVALID_DATA)) {
        FailureKind::InvalidData
    } else {
        FailureKind::Other
    }
}

/// The most specific error line of a stderr tail, with FFmpeg's
/// `[component @ 0x…]` prefixes reduced to `component: ` (stable across
/// runs). Generic follow-ups such as `Conversion failed!` are skipped; falls
/// back to the last non-empty line.
pub fn first_error_line(tail: &str) -> Option<String> {
    let mut last = None;
    for raw in tail.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let clean = strip_context(line);
        last = Some(clean.clone());
        let low = line.to_ascii_lowercase();
        if low.starts_with("frame=")
            || low.starts_with("size=")
            || NOISE.iter().any(|n| low.contains(n))
        {
            continue;
        }
        if ERROR_WORDS.iter().any(|w| low.contains(w)) {
            return Some(clean);
        }
    }
    last
}

/// `[vost#0:0/h264_nvenc @ 0x1] [enc:h264_nvenc @ 0x2] msg` → `enc:h264_nvenc: msg`.
fn strip_context(line: &str) -> String {
    let mut rest = line;
    let mut name = None;
    while let Some(body) = rest.strip_prefix('[') {
        let Some(end) = body.find(']') else { break };
        let inner = &body[..end];
        name = Some(inner.split(" @ ").next().unwrap_or(inner).trim());
        rest = body[end + 1..].trim_start();
    }
    match name {
        Some(n) if !n.is_empty() => format!("{n}: {rest}"),
        _ => rest.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp10_disk_full_on_every_os() {
        for msg in [
            "[out#0/mp4 @ 0x55] Error writing trailer of a.mp4.part: No space left on device",
            "av_interleaved_write_frame(): No space left on device",
            "Error closing file a.part: There is not enough space on the disk.",
            "a.part: Disk quota exceeded",
        ] {
            assert_eq!(classify(msg), FailureKind::DiskFull, "{msg}");
            assert!(classify(msg).pauses_batch());
        }
    }

    #[test]
    fn exp10_output_missing_but_not_input_missing() {
        for msg in [
            "[out#0/mp4 @ 0x1] Error opening output E:/clips/a.mp4.part: No such file or directory",
            "Error opening output files: No such file or directory",
            "[mp4 @ 0x1] Error writing trailer of E:/a.mp4.part: Input/output error",
            "av_interleaved_write_frame(): Device not configured",
            "The system cannot find the path specified.",
            "The device is not ready.",
        ] {
            assert_eq!(classify(msg), FailureKind::OutputMissing, "{msg}");
        }
        let input = "[in#0 @ 0x1] Error opening input: No such file or directory\nError opening input file a.mp4.";
        assert_eq!(classify(input), FailureKind::Other);
    }

    #[test]
    fn exp12_encoder_sessions() {
        for msg in [
            "[h264_nvenc @ 0x1] OpenEncodeSessionEx failed: out of memory (10): (no details)",
            "[h264_videotoolbox @ 0x1] Error: cannot create compression session: -12915",
        ] {
            let kind = classify(msg);
            assert_eq!(kind, FailureKind::EncoderSessionsExhausted, "{msg}");
            assert!(kind.requeue_with_less_concurrency());
        }
    }

    #[test]
    fn exp13_hardware_failures() {
        for msg in [
            "[h264_vaapi @ 0x1] Failed to initialise VAAPI connection: -1 (unknown libva error).",
            "[h264_videotoolbox @ 0x1] Error: cannot create compression session: -12902",
            "[h264_nvenc @ 0x1] No capable devices found",
        ] {
            let kind = classify(msg);
            assert_eq!(kind, FailureKind::HwEncoderFailed, "{msg}");
            assert!(kind.retry_with_software() && !kind.pauses_batch());
        }
    }

    #[test]
    fn invalid_and_other() {
        assert_eq!(
            classify("[h264 @ 0x1] Invalid NAL unit size (1234 > 99)."),
            FailureKind::InvalidData
        );
        assert_eq!(classify("Unknown encoder 'libfoo'"), FailureKind::Other);
        assert_eq!(classify(""), FailureKind::Other);
    }

    #[test]
    fn first_error_line_skips_noise_and_strips_pointers() {
        let tail = "frame=    0 fps=0.0\n[vost#0:0/h264_nvenc @ 000001ca] [enc:h264_nvenc @ 000001cb] Could not open encoder before EOF\n\
                    [h264_nvenc @ 000001cabe9be440] Cannot load nvcuda.dll\nConversion failed!\n";
        assert_eq!(
            first_error_line(tail).as_deref(),
            Some("h264_nvenc: Cannot load nvcuda.dll")
        );
        assert_eq!(
            first_error_line("just text\nmore text\n").as_deref(),
            Some("more text")
        );
        assert_eq!(first_error_line("\n  \n"), None);
        assert_eq!(strip_context("[a @ 0x1] [b] msg"), "b: msg");
        assert_eq!(strip_context("[unterminated msg"), "[unterminated msg");
    }
}
