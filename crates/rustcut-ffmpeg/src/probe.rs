//! FFprobe JSON (§7.6 A) → [`SourceInfo`] (PLY-01), plus actionable messages
//! when probing fails (§9 truncated/corrupt sources).
//!
//! Rules (FFmpeg 9.0.2 output, see `tests/data/probe_*.json`):
//! * Video = the first video stream that is not an attached picture (cover
//!   art in MP4/MKV is a `png`/`mjpeg` stream with `disposition.attached_pic`).
//! * Rotation is clockwise: the display matrix's `rotation` is
//!   counter-clockwise (`-90` = 90° clockwise); the legacy `rotate` tag is
//!   already clockwise. Normalised to 0/90/180/270.
//! * SAR `0:1`/`N/A` → `1:1`; display size via `rustcut_framing::display_dims`.
//! * Frame rate: `avg_frame_rate` when valid, else `r_frame_rate`; VFR when
//!   the two differ by more than 1 % (refined later by the media index).
//! * HDR: transfer `smpte2084` (PQ) or `arib-std-b67` (HLG) (FRM-07).
//! * Duration: `format.duration`, else the longest stream (including
//!   Matroska `DURATION` tags); start time: `format.start_time` (default 0).
//! * Tags are matched case-insensitively (Matroska writes `DURATION`,
//!   `title`, `language`; some muxers write `TITLE`).

use rustcut_model::time::parse_rational;
use rustcut_model::{AudioTrack, Fingerprint, SourceInfo, VideoInfo};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

/// A parsed probe.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    /// `fingerprint.xxh3_head_tail` and `relative_path` are empty: the caller
    /// hashes the file and knows the project folder.
    pub source: SourceInfo,
    pub streams: StreamCounts,
    pub warnings: Vec<ProbeWarning>,
}

/// Streams by type. `video` excludes attached pictures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct StreamCounts {
    pub video: u32,
    pub audio: u32,
    pub subtitle: u32,
    pub data: u32,
    pub attachment: u32,
    pub attached_pictures: u32,
}

/// Non-fatal findings worth showing in the source panel.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProbeWarning {
    /// More video streams than the first; they are ignored.
    ExtraVideoStreams {
        ignored: u32,
    },
    /// Subtitle/data/attachment streams are never exported (EXP-04).
    DroppedStreams {
        subtitle: u32,
        data: u32,
        attachment: u32,
    },
    /// Audio tracks FFmpeg cannot decode (Apple Positional Audio); never exported.
    UnsupportedAudio {
        count: u32,
    },
    VariableFrameRate,
    /// PQ/HLG source (FRM-07).
    Hdr {
        transfer: String,
    },
    UnknownDuration,
    UnknownFrameRate,
}

impl ProbeWarning {
    /// User-facing sentence.
    pub fn message(&self) -> String {
        match self {
            ProbeWarning::ExtraVideoStreams { ignored } => {
                format!("The file has {ignored} more video stream(s); only the first one is used.")
            }
            ProbeWarning::DroppedStreams { subtitle, data, attachment } => {
                let parts: Vec<String> = [(subtitle, "subtitle"), (data, "data"), (attachment, "attachment")]
                    .iter()
                    .filter(|(n, _)| **n > 0)
                    .map(|(n, label)| format!("{n} {label}"))
                    .collect();
                format!("Subtitle, data and attachment streams are not exported ({}).", parts.join(", "))
            }
            ProbeWarning::UnsupportedAudio { count } => format!(
                "{count} audio track(s) use a codec FFmpeg cannot read (Apple Positional Audio); they are not exported. \n                 The other audio tracks are kept."
            ),
            ProbeWarning::VariableFrameRate => "Variable frame rate: frame stepping follows the real frame times and \
                 re-encodes keep the original timing."
                .to_string(),
            ProbeWarning::Hdr { transfer } => format!(
                "HDR video ({}): Frame-Accurate and vertical exports convert it to SDR; without tone-mapping support \
                 the colours look washed out. Lossless exports are unaffected.",
                if transfer == "arib-std-b67" { "HLG" } else { "PQ" }
            ),
            ProbeWarning::UnknownDuration => "FFprobe reported no duration; the file may be truncated or still \
                 being recorded."
                .to_string(),
            ProbeWarning::UnknownFrameRate => {
                "The frame rate is unknown; frame stepping assumes 30 fps until the media index is ready.".to_string()
            }
        }
    }
}

/// Why a probe result cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error(
        "FFprobe output for {file} is not valid JSON ({source}); the file may be damaged or FFprobe crashed"
    )]
    InvalidJson {
        file: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("{file} has no video stream: audio-only sources are not supported")]
    NoVideo { file: String },
    #[error("{file}: the video stream reports no picture size, so the file is probably damaged")]
    NoDimensions { file: String },
}

/// Parse `ffprobe -show_format -show_streams -of json` output for `path`.
pub fn parse_probe_json(json: &str, path: impl AsRef<Path>) -> Result<Probe, ProbeError> {
    let path = path.as_ref();
    let file = display_name(path);
    let root: Value = serde_json::from_str(json).map_err(|source| ProbeError::InvalidJson {
        file: file.clone(),
        source,
    })?;
    let streams: &[Value] = root
        .get("streams")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    let format = root.get("format").unwrap_or(&Value::Null);
    let counts = count_streams(streams);

    let stream = streams
        .iter()
        .find(|s| codec_type(s) == "video" && !flag(s, "attached_pic"))
        .ok_or_else(|| ProbeError::NoVideo { file: file.clone() })?;
    let video = video_info(stream).ok_or(ProbeError::NoDimensions { file })?;
    let audio_tracks: Vec<AudioTrack> = streams
        .iter()
        .filter(|s| codec_type(s) == "audio")
        .map(audio_track)
        .collect();

    let duration = number(format, "duration")
        .filter(|d| *d > 0.0)
        .or_else(|| longest_stream(streams));
    let mut warnings = warnings(&counts, &video, duration.is_none());
    let unsupported = audio_tracks
        .iter()
        .filter(|t| crate::compat::is_unsupported_audio(&t.codec))
        .count();
    if unsupported > 0 {
        warnings.insert(
            0,
            ProbeWarning::UnsupportedAudio {
                count: u32::try_from(unsupported).unwrap_or(u32::MAX),
            },
        );
    }
    let source = SourceInfo {
        path: path.to_string_lossy().into_owned(),
        relative_path: String::new(),
        fingerprint: Fingerprint {
            size_bytes: integer(format, "size")
                .and_then(|s| u64::try_from(s).ok())
                .unwrap_or(0),
            xxh3_head_tail: String::new(),
        },
        duration_sec: duration.unwrap_or(0.0),
        start_time_sec: number(format, "start_time").unwrap_or(0.0),
        video,
        audio_tracks,
        format_name: text(format, "format_name").map(str::to_string),
        bit_rate: integer(format, "bit_rate")
            .and_then(|b| u64::try_from(b).ok())
            .filter(|b| *b > 0),
    };
    Ok(Probe {
        source,
        streams: counts,
        warnings,
    })
}

fn video_info(s: &Value) -> Option<VideoInfo> {
    let width = integer(s, "width")
        .and_then(|w| u32::try_from(w).ok())
        .filter(|w| *w > 0)?;
    let height = integer(s, "height")
        .and_then(|h| u32::try_from(h).ok())
        .filter(|h| *h > 0)?;
    let sar = sample_aspect_ratio(s);
    let rotation = rotation_cw(s);
    let (display_width, display_height) =
        rustcut_framing::display_dims(width, height, sar, rotation);
    let (frame_rate, vfr) = frame_rate(s);
    let color_transfer = text(s, "color_transfer").map(str::to_string);
    let pix_fmt = text(s, "pix_fmt").map(str::to_string);
    Some(VideoInfo {
        codec: text(s, "codec_name").unwrap_or("unknown").to_string(),
        coded_width: width,
        coded_height: height,
        display_width,
        display_height,
        rotation,
        sar: format!("{}:{}", sar.0, sar.1),
        frame_rate,
        vfr,
        hdr: matches!(
            color_transfer.as_deref(),
            Some("smpte2084" | "arib-std-b67")
        ),
        stream_index: integer(s, "index").and_then(|i| u32::try_from(i).ok()),
        bit_depth: bit_depth(s, pix_fmt.as_deref()),
        pix_fmt,
        profile: text(s, "profile").map(str::to_string),
        level: integer(s, "level")
            .filter(|l| *l > 0)
            .and_then(|l| i32::try_from(l).ok()),
        color_transfer,
    })
}

fn audio_track(s: &Value) -> AudioTrack {
    AudioTrack {
        stream_index: integer(s, "index")
            .and_then(|i| u32::try_from(i).ok())
            .unwrap_or(0),
        codec: text(s, "codec_name").unwrap_or("unknown").to_string(),
        channels: integer(s, "channels")
            .and_then(|c| u32::try_from(c).ok())
            .unwrap_or(0),
        title: tag(s, "title").map(str::to_string),
        language: tag(s, "language")
            .filter(|l| !l.eq_ignore_ascii_case("und"))
            .map(str::to_string),
        sample_rate: integer(s, "sample_rate")
            .and_then(|r| u32::try_from(r).ok())
            .filter(|r| *r > 0),
        is_default: flag(s, "default"),
    }
}

fn count_streams(streams: &[Value]) -> StreamCounts {
    let mut c = StreamCounts::default();
    for s in streams {
        match codec_type(s) {
            "video" if flag(s, "attached_pic") => c.attached_pictures += 1,
            "video" => c.video += 1,
            "audio" => c.audio += 1,
            "subtitle" => c.subtitle += 1,
            "data" => c.data += 1,
            "attachment" => c.attachment += 1,
            _ => {}
        }
    }
    c
}

fn warnings(c: &StreamCounts, video: &VideoInfo, unknown_duration: bool) -> Vec<ProbeWarning> {
    let mut w = Vec::new();
    if c.video > 1 {
        w.push(ProbeWarning::ExtraVideoStreams {
            ignored: c.video - 1,
        });
    }
    if c.subtitle + c.data + c.attachment > 0 {
        w.push(ProbeWarning::DroppedStreams {
            subtitle: c.subtitle,
            data: c.data,
            attachment: c.attachment,
        });
    }
    if video.vfr {
        w.push(ProbeWarning::VariableFrameRate);
    }
    if video.hdr {
        w.push(ProbeWarning::Hdr {
            transfer: video.color_transfer.clone().unwrap_or_default(),
        });
    }
    if unknown_duration {
        w.push(ProbeWarning::UnknownDuration);
    }
    if video.fps().is_none() {
        w.push(ProbeWarning::UnknownFrameRate);
    }
    w
}

fn sample_aspect_ratio(s: &Value) -> (u32, u32) {
    text(s, "sample_aspect_ratio")
        .and_then(|r| {
            let (n, d) = r.split_once(':')?;
            let (n, d): (u32, u32) = (n.trim().parse().ok()?, d.trim().parse().ok()?);
            (n > 0 && d > 0).then_some((n, d))
        })
        .unwrap_or((1, 1))
}

/// Clockwise display rotation, normalised to 0/90/180/270.
fn rotation_cw(s: &Value) -> u32 {
    let matrix_ccw = s
        .get("side_data_list")
        .and_then(Value::as_array)
        .and_then(|list| {
            list.iter().find_map(|sd| {
                let is_matrix = text(sd, "side_data_type")
                    .is_none_or(|t| t.eq_ignore_ascii_case("Display Matrix"));
                if is_matrix {
                    number(sd, "rotation")
                } else {
                    None
                }
            })
        });
    let cw = match matrix_ccw {
        Some(ccw) => -ccw,
        None => tag(s, "rotate")
            .and_then(|r| r.trim().parse::<f64>().ok())
            .unwrap_or(0.0),
    };
    normalize_rotation(cw)
}

fn normalize_rotation(degrees: f64) -> u32 {
    if !degrees.is_finite() {
        return 0;
    }
    let quarters = (degrees / 90.0).round() as i64;
    (quarters.rem_euclid(4) * 90) as u32
}

fn frame_rate(s: &Value) -> (String, bool) {
    let valid = |key: &str| text(s, key).filter(|r| parse_rational(r).is_some());
    let (avg, real) = (valid("avg_frame_rate"), valid("r_frame_rate"));
    let vfr = match (avg.and_then(parse_rational), real.and_then(parse_rational)) {
        (Some(a), Some(r)) => (r - a).abs() / a > 0.01,
        _ => false,
    };
    (avg.or(real).unwrap_or("0/0").to_string(), vfr)
}

fn bit_depth(s: &Value, pix_fmt: Option<&str>) -> Option<u32> {
    if let Some(bits) = integer(s, "bits_per_raw_sample")
        .and_then(|b| u32::try_from(b).ok())
        .filter(|b| *b > 0)
    {
        return Some(bits);
    }
    let pix = pix_fmt?;
    let has = |needles: &[&str]| needles.iter().any(|n| pix.contains(n));
    Some(if has(&["10le", "10be", "p010", "x2rgb10", "x2bgr10"]) {
        10
    } else if has(&["12le", "12be", "p012"]) {
        12
    } else if has(&["16le", "16be", "p016", "48le", "48be", "64le", "64be"]) {
        16
    } else {
        8
    })
}

fn longest_stream(streams: &[Value]) -> Option<f64> {
    streams
        .iter()
        .filter_map(|s| {
            number(s, "duration")
                .filter(|d| *d > 0.0)
                .or_else(|| tag(s, "DURATION").and_then(parse_clock))
        })
        .fold(None, |best: Option<f64>, d| {
            Some(best.map_or(d, |b| b.max(d)))
        })
}

/// `HH:MM:SS.fraction` (Matroska `DURATION` tags).
fn parse_clock(v: &str) -> Option<f64> {
    let mut total = 0.0;
    for part in v.trim().split(':') {
        total = total * 60.0 + part.parse::<f64>().ok()?;
    }
    (total.is_finite() && total > 0.0).then_some(total)
}

fn codec_type(s: &Value) -> &str {
    text(s, "codec_type").unwrap_or("")
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)?
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "N/A")
}

fn number(v: &Value, key: &str) -> Option<f64> {
    let n = match v.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }?;
    n.is_finite().then_some(n)
}

fn integer(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.round() as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn flag(s: &Value, key: &str) -> bool {
    s.get("disposition").and_then(|d| integer(d, key)) == Some(1)
}

fn tag<'a>(s: &'a Value, name: &str) -> Option<&'a str> {
    let tags = s.get("tags")?.as_object()?;
    tags.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// A specific, actionable message for a failed probe (§9), from FFprobe's
/// stderr. Recognises truncated MP4s (`moov atom not found`, typical after a
/// recorder crash), missing files, permission errors, truncated data and
/// invalid data.
pub fn probe_failure_message(stderr: &str, path: impl AsRef<Path>) -> String {
    let name = display_name(path.as_ref());
    let low = stderr.to_ascii_lowercase();
    if low.contains("moov atom not found") {
        format!(
            "{name} is incomplete: its MP4 index (moov atom) is missing, which happens when the recorder crashed or \
             was stopped before it finished writing. Repair it with a tool such as untrunc and a healthy recording \
             from the same device; to avoid this, record to MKV and convert to MP4 afterwards."
        )
    } else if low.contains("no such file or directory") || low.contains("cannot find the file") {
        format!(
            "{name} was not found. It may have been moved, renamed or deleted, or its drive is disconnected; \
             reconnect the drive or use Locate Missing Media."
        )
    } else if low.contains("permission denied") || low.contains("access is denied") {
        format!(
            "RustCut is not allowed to read {name} (permission denied). Check the file's permissions or close the \
             program that has it locked, then try again."
        )
    } else if low.contains("end of file")
        || low.contains("partial file")
        || low.contains("truncated")
    {
        format!(
            "{name} ends unexpectedly: the file is truncated. Wait until a copy or download has finished, or export \
             the recording again."
        )
    } else if low.contains("invalid data found when processing input") {
        format!(
            "{name} could not be read as a video: the data is invalid or damaged, or it is not a media file. Check \
             that the copy or download finished; if it plays elsewhere, remux it to MP4 or MKV and open the copy."
        )
    } else {
        match crate::failure::first_error_line(stderr) {
            Some(line) => format!("FFprobe could not read {name}: {line}"),
            None => format!(
                "FFprobe could not read {name} and gave no reason. Check that the file is a video and that its \
                 drive is accessible."
            ),
        }
    }
}

/// Result of [`crate::args::verify_args`] on a finished export.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VerifyReport {
    pub duration_sec: Option<f64>,
    pub format_name: Option<String>,
    pub video_codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Channel count of each audio stream, in order.
    pub audio_channels: Vec<u32>,
}

impl VerifyReport {
    /// The output has a video stream and a positive duration.
    pub fn is_playable(&self) -> bool {
        self.video_codec.is_some() && self.duration_sec.is_some_and(|d| d > 0.0)
    }
}

/// Parse the JSON printed for [`crate::args::verify_args`].
pub fn parse_verify_json(json: &str, path: impl AsRef<Path>) -> Result<VerifyReport, ProbeError> {
    let root: Value = serde_json::from_str(json).map_err(|source| ProbeError::InvalidJson {
        file: display_name(path.as_ref()),
        source,
    })?;
    let streams: &[Value] = root
        .get("streams")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    let format = root.get("format").unwrap_or(&Value::Null);
    let video = streams.iter().find(|s| codec_type(s) == "video");
    let dim = |key: &str| {
        video
            .and_then(|v| integer(v, key))
            .and_then(|d| u32::try_from(d).ok())
    };
    Ok(VerifyReport {
        duration_sec: number(format, "duration"),
        format_name: text(format, "format_name").map(str::to_string),
        video_codec: video
            .and_then(|v| text(v, "codec_name"))
            .map(str::to_string),
        width: dim("width"),
        height: dim("height"),
        audio_channels: streams
            .iter()
            .filter(|s| codec_type(s) == "audio")
            .map(|s| {
                integer(s, "channels")
                    .and_then(|c| u32::try_from(c).ok())
                    .unwrap_or(0)
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(json: &str) -> Probe {
        parse_probe_json(json, "D:/media/x.mp4").unwrap()
    }

    #[test]
    fn rotation_conventions() {
        assert_eq!(normalize_rotation(90.0), 90);
        assert_eq!(normalize_rotation(-90.0), 270);
        assert_eq!(normalize_rotation(-180.0), 180);
        assert_eq!(normalize_rotation(450.0), 90);
        assert_eq!(normalize_rotation(89.6), 90);
        assert_eq!(normalize_rotation(f64::NAN), 0);
        let legacy = r#"{"streams":[{"index":0,"codec_type":"video","codec_name":"h264","width":1920,"height":1080,
            "avg_frame_rate":"30/1","r_frame_rate":"30/1","tags":{"rotate":"90"}}],"format":{"duration":"1"}}"#;
        let p = probe(legacy);
        assert_eq!(
            (
                p.source.video.rotation,
                p.source.video.display_width,
                p.source.video.display_height
            ),
            (90, 1080, 1920)
        );
        let matrix = r#"{"streams":[{"index":0,"codec_type":"video","width":1920,"height":1080,
            "side_data_list":[{"side_data_type":"Display Matrix","rotation":90}]}],"format":{}}"#;
        assert_eq!(probe(matrix).source.video.rotation, 270);
    }

    #[test]
    fn invalid_sar_and_rates_and_unknown_duration() {
        let json = r#"{"streams":[{"index":0,"codec_type":"video","codec_name":"h264","width":640,"height":480,
            "sample_aspect_ratio":"0:1","avg_frame_rate":"0/0","r_frame_rate":"0/0"}],"format":{"format_name":"avi"}}"#;
        let p = probe(json);
        assert_eq!(p.source.video.sar, "1:1");
        assert_eq!(p.source.video.frame_rate, "0/0");
        assert_eq!(p.source.duration_sec, 0.0);
        assert!(p.warnings.contains(&ProbeWarning::UnknownDuration));
        assert!(p.warnings.contains(&ProbeWarning::UnknownFrameRate));
        for w in &p.warnings {
            assert!(!w.message().is_empty());
        }
    }

    #[test]
    fn vfr_heuristic_uses_one_percent() {
        let json = |avg: &str, r: &str| {
            format!(
                r#"{{"streams":[{{"index":0,"codec_type":"video","width":2,"height":2,"avg_frame_rate":"{avg}","r_frame_rate":"{r}"}}],"format":{{"duration":"1"}}}}"#
            )
        };
        assert!(!probe(&json("60000/1001", "60000/1001")).source.video.vfr);
        assert!(!probe(&json("2997/100", "30/1")).source.video.vfr);
        let p = probe(&json("27/1", "30/1"));
        assert!(p.source.video.vfr && p.warnings.contains(&ProbeWarning::VariableFrameRate));
        assert_eq!(p.source.video.frame_rate, "27/1");
        assert_eq!(probe(&json("0/0", "25/1")).source.video.frame_rate, "25/1");
    }

    #[test]
    fn errors_are_specific() {
        let audio_only =
            r#"{"streams":[{"index":0,"codec_type":"audio","codec_name":"aac"}],"format":{}}"#;
        let err = parse_probe_json(audio_only, "D:/a/song.m4a").unwrap_err();
        assert!(matches!(err, ProbeError::NoVideo { .. }));
        assert_eq!(
            err.to_string(),
            "song.m4a has no video stream: audio-only sources are not supported"
        );
        let cover_only = r#"{"streams":[{"index":0,"codec_type":"video","width":64,"height":64,"disposition":{"attached_pic":1}}]}"#;
        assert!(matches!(
            parse_probe_json(cover_only, "a.mp3"),
            Err(ProbeError::NoVideo { .. })
        ));
        let no_size = r#"{"streams":[{"index":0,"codec_type":"video","width":0,"height":0}]}"#;
        assert!(matches!(
            parse_probe_json(no_size, "a.mp4"),
            Err(ProbeError::NoDimensions { .. })
        ));
        assert!(matches!(
            parse_probe_json("not json", "a.mp4"),
            Err(ProbeError::InvalidJson { .. })
        ));
    }

    #[test]
    fn bit_depth_from_pix_fmt() {
        let none = Value::Null;
        assert_eq!(bit_depth(&none, Some("yuv420p10le")), Some(10));
        assert_eq!(bit_depth(&none, Some("p010le")), Some(10));
        assert_eq!(bit_depth(&none, Some("nv12")), Some(8));
        assert_eq!(bit_depth(&none, Some("yuv444p12le")), Some(12));
        assert_eq!(bit_depth(&none, Some("yuv420p")), Some(8));
        assert_eq!(bit_depth(&none, None), None);
    }

    #[test]
    fn edge_probe_failure_messages_are_actionable() {
        let msg = probe_failure_message(
            "[mov @ 0x1] moov atom not found\nx.mp4: Invalid data",
            "D:/rec/x.mp4",
        );
        assert!(
            msg.starts_with("x.mp4 is incomplete")
                && msg.contains("recorder crashed")
                && msg.contains("MKV"),
            "{msg}"
        );
        assert!(
            probe_failure_message("a.mp4: No such file or directory", "a.mp4")
                .contains("Locate Missing Media")
        );
        assert!(
            probe_failure_message("a.mp4: Permission denied", "a.mp4")
                .contains("permission denied")
        );
        assert!(probe_failure_message("a.mp4: End of file", "a.mp4").contains("truncated"));
        assert!(
            probe_failure_message("a.mp4: Invalid data found when processing input", "a.mp4")
                .contains("remux")
        );
        assert_eq!(
            probe_failure_message("[x @ 0x1] Unsupported codec id 123", "a.mp4"),
            "FFprobe could not read a.mp4: x: Unsupported codec id 123"
        );
        assert!(probe_failure_message("", "a.mp4").contains("gave no reason"));
    }

    #[test]
    fn verify_report() {
        let json = r#"{"streams":[{"index":0,"codec_name":"h264","codec_type":"video","width":1080,"height":1920},
            {"index":1,"codec_name":"aac","codec_type":"audio","channels":2}],"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"1.500000"}}"#;
        let r = parse_verify_json(json, "o.mp4").unwrap();
        assert_eq!(
            (r.width, r.height, r.audio_channels.clone(), r.duration_sec),
            (Some(1080), Some(1920), vec![2], Some(1.5))
        );
        assert!(r.is_playable());
        assert!(
            !parse_verify_json(r#"{"format":{}}"#, "o.mp4")
                .unwrap()
                .is_playable()
        );
    }
}
