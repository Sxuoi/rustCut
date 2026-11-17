//! Output verification (PRD 7.8): a clip succeeds when FFmpeg exited with code 0, reported
//! `progress=end`, and the output probes with a video stream of the expected duration.
//!
//! MP4 and MOV outputs (every re-encode, and Lossless whenever the source's codecs fit MP4)
//! are probed in-process from their `moov` box: the movie header's duration and the tracks'
//! handler types and sample entries. Reading those few boxes takes well under a millisecond;
//! an ffprobe run took about 90 ms (the static sidecar alone needs about 60 ms to start).
//!
//! The two durations are not computed the same way: the movie header holds the longest
//! track's edit list, while FFmpeg measures from the earliest first timestamp of any stream to
//! the latest end (an HEVC Lossless cut with B-frames and primed audio: 3.167 s against
//! ffprobe's 3.088 s). The reader therefore settles only clear passes, a duration at least
//! [`MOOV_MARGIN_SEC`] inside the tolerance. Everything else goes to ffprobe as before: other
//! containers, a missing or damaged `moov`, fragmented files, a duration near the edge of the
//! tolerance and any MP4 whose boxes contradict the expectation, so every failure verdict
//! still comes from ffprobe.

use rustcut_ffmpeg::probe::{VerifyReport, parse_verify_json};
use rustcut_ffmpeg::{Muxer, first_error_line, verify_args};
use rustcut_runner::run_capture;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

/// Verification tolerance of the output duration (PRD 7.8).
pub const VERIFY_TOLERANCE_SEC: f64 = 1.0;
/// How far inside the tolerance a `moov` duration must lie for the reader to pass the file on
/// its own; nearer the edge, ffprobe decides.
pub const MOOV_MARGIN_SEC: f64 = 0.5;
/// Timeout of the ffprobe run.
const VERIFY_TIMEOUT: Duration = Duration::from_secs(60);
/// ffprobe's `format_name` for everything FFmpeg's `mov` demuxer reads.
const MOV_FORMAT_NAME: &str = "mov,mp4,m4a,3gp,3g2,mj2";
/// Largest box body read into memory (`mvhd`, `hdlr`, `stsd` are a few hundred bytes).
const MAX_LEAF_BYTES: u64 = 1024 * 1024;

/// The duration a clip's output must have.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Expected {
    /// Expected duration, Lossless lead-in included.
    pub sec: f64,
    /// Allowed excess: [`VERIFY_TOLERANCE_SEC`], more when the lead-in is unknown.
    pub upper_tolerance: f64,
}

/// Verify a finished `.part` written with `muxer`; `Err` carries the message for the user.
pub(crate) async fn verify_output(
    ffprobe: &Path,
    part: &Path,
    muxer: Muxer,
    expected: Expected,
) -> Result<(), String> {
    if muxer.is_iso_bmff() {
        let path = part.to_path_buf();
        let fast = tokio::task::spawn_blocking(move || mp4_verify_report(&path))
            .await
            .ok()
            .flatten();
        match fast {
            Some(report) if check(&report, expected, MOOV_MARGIN_SEC).is_ok() => return Ok(()),
            Some(report) => {
                tracing::debug!(?report, "the moov box is not a clear pass; asking ffprobe")
            }
            None => tracing::debug!("the moov box could not be read; asking ffprobe"),
        }
    }
    let report = ffprobe_report(ffprobe, part).await?;
    check(&report, expected, 0.0)
}

async fn ffprobe_report(ffprobe: &Path, part: &Path) -> Result<VerifyReport, String> {
    let out = run_capture(ffprobe, &verify_args(part), VERIFY_TIMEOUT)
        .await
        .map_err(|e| format!("The exported file could not be checked: {e}"))?;
    if !out.success() {
        let err = out.stderr_lossy();
        return Err(format!(
            "The exported file is unreadable: {}",
            first_error_line(&err).unwrap_or_else(|| "ffprobe rejected it".to_string())
        ));
    }
    parse_verify_json(&out.stdout_lossy(), part)
        .map_err(|e| format!("The exported file could not be checked: {e}"))
}

/// A video stream and the expected duration (PRD 7.8), `margin` seconds inside the tolerance.
fn check(report: &VerifyReport, expected: Expected, margin: f64) -> Result<(), String> {
    if report.video_codec.is_none() {
        return Err("The exported file contains no video stream".into());
    }
    let actual = report
        .duration_sec
        .filter(|d| *d > 0.0)
        .ok_or("The exported file has no duration")?;
    let Expected {
        sec,
        upper_tolerance,
    } = expected;
    if actual < sec - VERIFY_TOLERANCE_SEC + margin || actual > sec + upper_tolerance - margin {
        return Err(format!(
            "The exported file is {actual:.1} s long but about {sec:.1} s were expected (tolerance 1 s)"
        ));
    }
    Ok(())
}

/// The verification report of an MP4/MOV file, read from its `moov` box instead of ffprobe:
/// the movie header's duration (a little longer than ffprobe's format duration when a stream
/// starts late, see the module docs), the codec (sample entry) and coded size of the first video
/// track, and the channel count in each audio track's sample entry.
///
/// `None` when the boxes cannot be read or the file is outside what this reader handles: no
/// `moov` or `mdat`, a box running past its parent or the end of the file, or a fragmented
/// file (`mvex`, whose duration FFmpeg takes from the fragments instead).
pub fn mp4_verify_report(path: &Path) -> Option<VerifyReport> {
    let file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    read_report(&mut BufReader::with_capacity(16 * 1024, file), len)
        .ok()
        .flatten()
}

#[derive(Debug, Clone, Copy)]
struct BoxHeader {
    kind: [u8; 4],
    body: u64,
    end: u64,
}

/// The box at `pos`, which must end by `end`; `None` when its size is impossible.
fn read_header<R: Read + Seek>(r: &mut R, pos: u64, end: u64) -> io::Result<Option<BoxHeader>> {
    r.seek(SeekFrom::Start(pos))?;
    let mut head = [0u8; 8];
    r.read_exact(&mut head)?;
    let kind = [head[4], head[5], head[6], head[7]];
    let (size, header_len) = match u32::from_be_bytes([head[0], head[1], head[2], head[3]]) {
        // Size 0: the box runs to the end of its parent (the file, for a last `mdat`).
        0 => (end - pos, 8),
        1 => {
            if end - pos < 16 {
                return Ok(None);
            }
            let mut large = [0u8; 8];
            r.read_exact(&mut large)?;
            (u64::from_be_bytes(large), 16)
        }
        size => (u64::from(size), 8),
    };
    if size < header_len || size > end - pos {
        return Ok(None);
    }
    Ok(Some(BoxHeader {
        kind,
        body: pos + header_len,
        end: pos + size,
    }))
}

/// The boxes inside `parent`'s body (fewer than 8 trailing bytes are ignored).
fn children<R: Read + Seek>(r: &mut R, parent: BoxHeader) -> io::Result<Option<Vec<BoxHeader>>> {
    let mut boxes = Vec::new();
    let mut pos = parent.body;
    while parent.end - pos >= 8 {
        let Some(child) = read_header(r, pos, parent.end)? else {
            return Ok(None);
        };
        pos = child.end;
        boxes.push(child);
    }
    Ok(Some(boxes))
}

fn find(boxes: &[BoxHeader], kind: &[u8; 4]) -> Option<BoxHeader> {
    boxes.iter().copied().find(|b| &b.kind == kind)
}

/// The body of a small leaf box.
fn body<R: Read + Seek>(r: &mut R, b: BoxHeader) -> io::Result<Option<Vec<u8>>> {
    let len = b.end - b.body;
    if len > MAX_LEAF_BYTES {
        return Ok(None);
    }
    r.seek(SeekFrom::Start(b.body))?;
    let mut bytes = vec![0u8; len as usize];
    r.read_exact(&mut bytes)?;
    Ok(Some(bytes))
}

/// The box at `path` below `parent` (`None` if one on the way is missing or damaged).
fn descend<R: Read + Seek>(
    r: &mut R,
    parent: BoxHeader,
    path: &[&[u8; 4]],
) -> io::Result<Option<BoxHeader>> {
    let mut current = parent;
    for kind in path {
        let Some(boxes) = children(r, current)? else {
            return Ok(None);
        };
        let Some(next) = find(&boxes, kind) else {
            return Ok(None);
        };
        current = next;
    }
    Ok(Some(current))
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// `mvhd` duration in seconds (`None` when unknown, i.e. all ones, or the time scale is 0).
fn movie_duration(mvhd: &[u8]) -> Option<f64> {
    let (scale, duration) = match *mvhd.first()? {
        1 => (be32(mvhd, 20)?, be64(mvhd, 24)?),
        _ => (
            be32(mvhd, 12)?,
            be32(mvhd, 16).filter(|d| *d != u32::MAX)?.into(),
        ),
    };
    (scale > 0 && duration != u64::MAX).then(|| duration as f64 / f64::from(scale))
}

/// FFmpeg's name for the codec of a sample entry, as ffprobe prints it (the four-character
/// code itself for codecs RustCut does not write).
fn codec_name(fourcc: [u8; 4]) -> String {
    match &fourcc {
        b"avc1" | b"avc3" => "h264".into(),
        b"hvc1" | b"hev1" => "hevc".into(),
        b"av01" => "av1".into(),
        b"vp09" => "vp9".into(),
        b"vp08" => "vp8".into(),
        b"mp4v" => "mpeg4".into(),
        b"apch" | b"apcn" | b"apcs" | b"apco" | b"ap4h" | b"ap4x" => "prores".into(),
        _ => String::from_utf8_lossy(&fourcc).trim().to_string(),
    }
}

/// Handler type and first sample entry (code and body) of a `trak`.
type Track = ([u8; 4], Option<([u8; 4], Vec<u8>)>);

fn read_track<R: Read + Seek>(r: &mut R, trak: BoxHeader) -> io::Result<Option<Track>> {
    let Some(mdia) = descend(r, trak, &[b"mdia"])? else {
        return Ok(None);
    };
    let Some(mdia_boxes) = children(r, mdia)? else {
        return Ok(None);
    };
    let Some(hdlr) = find(&mdia_boxes, b"hdlr") else {
        return Ok(None);
    };
    let Some(hdlr) = body(r, hdlr)? else {
        return Ok(None);
    };
    let Some(handler) = hdlr.get(8..12).and_then(|h| <[u8; 4]>::try_from(h).ok()) else {
        return Ok(None);
    };
    let entry = match find(&mdia_boxes, b"minf") {
        Some(minf) => match descend(r, minf, &[b"stbl", b"stsd"])? {
            Some(stsd) => body(r, stsd)?.and_then(|stsd| first_sample_entry(&stsd)),
            None => None,
        },
        None => None,
    };
    Ok(Some((handler, entry)))
}

/// `stsd`: version and flags, entry count, then entries (size, code, body).
fn first_sample_entry(stsd: &[u8]) -> Option<([u8; 4], Vec<u8>)> {
    if be32(stsd, 4)? == 0 {
        return None;
    }
    let size = usize::try_from(be32(stsd, 8)?).ok()?;
    let code = <[u8; 4]>::try_from(stsd.get(12..16)?).ok()?;
    let entry = stsd.get(16..8 + size)?;
    Some((code, entry.to_vec()))
}

fn read_report<R: Read + Seek>(r: &mut R, len: u64) -> io::Result<Option<VerifyReport>> {
    let file = BoxHeader {
        kind: *b"file",
        body: 0,
        end: len,
    };
    let Some(top) = children(r, file)? else {
        return Ok(None);
    };
    let (Some(moov), true) = (find(&top, b"moov"), top.iter().any(|b| &b.kind == b"mdat")) else {
        return Ok(None);
    };
    let Some(movie) = children(r, moov)? else {
        return Ok(None);
    };
    if find(&movie, b"mvex").is_some() {
        return Ok(None);
    }
    let Some(mvhd) = find(&movie, b"mvhd") else {
        return Ok(None);
    };
    let Some(mvhd) = body(r, mvhd)? else {
        return Ok(None);
    };
    let mut report = VerifyReport {
        duration_sec: movie_duration(&mvhd),
        format_name: Some(MOV_FORMAT_NAME.to_string()),
        ..VerifyReport::default()
    };
    for trak in movie.iter().filter(|b| &b.kind == b"trak") {
        let Some((handler, entry)) = read_track(r, *trak)? else {
            return Ok(None);
        };
        match (&handler, entry) {
            // VisualSampleEntry: 6 reserved, data reference index, 16 pre-defined/reserved, then
            // width and height.
            (b"vide", Some((code, entry))) if report.video_codec.is_none() => {
                report.video_codec = Some(codec_name(code));
                report.width = be16(&entry, 24).map(u32::from);
                report.height = be16(&entry, 26).map(u32::from);
            }
            // AudioSampleEntry (and QuickTime v1): channel count after 8 reserved/version bytes;
            // QuickTime v2 keeps the real count further on.
            (b"soun", Some((_, entry))) => {
                let channels = match be16(&entry, 8) {
                    Some(2) => be32(&entry, 40),
                    _ => be16(&entry, 16).map(u32::from),
                };
                report.audio_channels.push(channels.unwrap_or(0));
            }
            _ => {}
        }
    }
    Ok(Some(report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::PathBuf;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = (u32::try_from(body.len() + 8).unwrap())
            .to_be_bytes()
            .to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn full(version: u8, rest: &[u8]) -> Vec<u8> {
        let mut v = vec![version, 0, 0, 0];
        v.extend_from_slice(rest);
        v
    }

    fn mvhd_v0(scale: u32, duration: u32) -> Vec<u8> {
        let mut rest = vec![0u8; 8];
        rest.extend_from_slice(&scale.to_be_bytes());
        rest.extend_from_slice(&duration.to_be_bytes());
        rest.extend_from_slice(&[0u8; 80]);
        bx(b"mvhd", &full(0, &rest))
    }

    fn mvhd_v1(scale: u32, duration: u64) -> Vec<u8> {
        let mut rest = vec![0u8; 16];
        rest.extend_from_slice(&scale.to_be_bytes());
        rest.extend_from_slice(&duration.to_be_bytes());
        rest.extend_from_slice(&[0u8; 80]);
        bx(b"mvhd", &full(1, &rest))
    }

    fn trak(handler: &[u8; 4], code: &[u8; 4], entry: &[u8]) -> Vec<u8> {
        let mut hdlr = vec![0u8; 4];
        hdlr.extend_from_slice(handler);
        hdlr.extend_from_slice(&[0u8; 13]);
        let mut stsd = 1u32.to_be_bytes().to_vec();
        stsd.extend(bx(code, entry));
        let stbl = bx(b"stbl", &bx(b"stsd", &full(0, &stsd)));
        let minf = bx(b"minf", &stbl);
        let mut mdia = bx(b"mdhd", &full(0, &[0u8; 20]));
        mdia.extend(bx(b"hdlr", &full(0, &hdlr)));
        mdia.extend(minf);
        let mut t = bx(b"tkhd", &full(0, &[0u8; 80]));
        t.extend(bx(b"mdia", &mdia));
        bx(b"trak", &t)
    }

    fn video(code: &[u8; 4], w: u16, h: u16) -> Vec<u8> {
        let mut e = vec![0u8; 24];
        e.extend_from_slice(&w.to_be_bytes());
        e.extend_from_slice(&h.to_be_bytes());
        e.extend_from_slice(&[0u8; 50]);
        trak(b"vide", code, &e)
    }

    fn audio(channels: u16) -> Vec<u8> {
        let mut e = vec![0u8; 16];
        e.extend_from_slice(&channels.to_be_bytes());
        e.extend_from_slice(&[0u8; 10]);
        trak(b"soun", b"mp4a", &e)
    }

    fn moov(parts: &[Vec<u8>]) -> Vec<u8> {
        bx(b"moov", &parts.concat())
    }

    fn report(bytes: &[u8]) -> Option<VerifyReport> {
        read_report(&mut Cursor::new(bytes), bytes.len() as u64)
            .ok()
            .flatten()
    }

    #[test]
    fn faststart_file_reports_duration_codec_size_and_channels() {
        let file = [
            bx(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41"),
            moov(&[mvhd_v0(1000, 59_983), video(b"avc1", 1080, 1920), audio(2)]),
            bx(b"free", &[]),
            bx(b"mdat", &[0u8; 64]),
        ]
        .concat();
        let r = report(&file).expect("report");
        assert_eq!(r.duration_sec, Some(59.983));
        assert_eq!(r.video_codec.as_deref(), Some("h264"));
        assert_eq!((r.width, r.height), (Some(1080), Some(1920)));
        assert_eq!(r.audio_channels, vec![2]);
        assert_eq!(r.format_name.as_deref(), Some(MOV_FORMAT_NAME));
        assert!(r.is_playable());
    }

    #[test]
    fn index_at_the_end_64_bit_mdat_and_version_1_header() {
        let mut mdat = 1u32.to_be_bytes().to_vec();
        mdat.extend_from_slice(b"mdat");
        mdat.extend_from_slice(&(16u64 + 32).to_be_bytes());
        mdat.extend_from_slice(&[0u8; 32]);
        let file = [
            bx(b"ftyp", b"qt  \0\0\x02\0qt  "),
            mdat,
            moov(&[mvhd_v1(90_000, 450_000), video(b"hvc1", 640, 360)]),
        ]
        .concat();
        let r = report(&file).expect("report");
        assert_eq!(r.duration_sec, Some(5.0));
        assert_eq!(r.video_codec.as_deref(), Some("hevc"));
        assert!(r.audio_channels.is_empty());
    }

    #[test]
    fn a_last_box_of_size_zero_runs_to_the_end_of_the_file() {
        let mut file = [
            bx(b"ftyp", b"isom\0\0\x02\0"),
            moov(&[mvhd_v0(600, 1200), video(b"avc1", 320, 240)]),
        ]
        .concat();
        file.extend_from_slice(&0u32.to_be_bytes());
        file.extend_from_slice(b"mdat");
        file.extend_from_slice(&[7u8; 40]);
        assert_eq!(report(&file).expect("report").duration_sec, Some(2.0));
    }

    #[test]
    fn audio_only_file_has_no_video_codec() {
        let file = [moov(&[mvhd_v0(1000, 3000), audio(6)]), bx(b"mdat", &[])].concat();
        let r = report(&file).expect("report");
        assert_eq!(r.video_codec, None);
        assert_eq!(r.audio_channels, vec![6]);
        assert!(!r.is_playable());
    }

    #[test]
    fn unknown_duration_is_none() {
        let file = [
            moov(&[mvhd_v0(1000, u32::MAX), video(b"avc1", 2, 2)]),
            bx(b"mdat", &[]),
        ]
        .concat();
        assert_eq!(report(&file).expect("report").duration_sec, None);
    }

    #[test]
    fn files_the_reader_does_not_handle_are_left_to_ffprobe() {
        let good_moov = moov(&[mvhd_v0(1000, 3000), video(b"avc1", 2, 2)]);
        // No moov (truncated while writing, as FFmpeg writes it last without faststart).
        assert_eq!(
            report(&[bx(b"ftyp", b"isom"), bx(b"mdat", &[1, 2])].concat()),
            None
        );
        // No mdat.
        assert_eq!(
            report(&[bx(b"ftyp", b"isom"), good_moov.clone()].concat()),
            None
        );
        // An mdat claiming more bytes than the file has.
        let mut cut = [
            bx(b"ftyp", b"isom"),
            good_moov.clone(),
            bx(b"mdat", &[0u8; 100]),
        ]
        .concat();
        cut.truncate(cut.len() - 10);
        assert_eq!(report(&cut), None);
        // Fragmented: FFmpeg takes the duration from the fragments, not from mvhd.
        let fragmented = [
            moov(&[mvhd_v0(1000, 0), video(b"avc1", 2, 2), bx(b"mvex", &[])]),
            bx(b"moof", &[]),
            bx(b"mdat", &[]),
        ]
        .concat();
        assert_eq!(report(&fragmented), None);
        // A track without a handler.
        let broken = [
            moov(&[mvhd_v0(1000, 3000), bx(b"trak", &bx(b"mdia", &[]))]),
            bx(b"mdat", &[]),
        ]
        .concat();
        assert_eq!(report(&broken), None);
        // Not an ISO file at all.
        assert_eq!(report(b"\x1aE\xdf\xa3 matroska bytes"), None);
        assert_eq!(report(b""), None);
    }

    fn write_mp4(dir: &Path, seconds: u32) -> PathBuf {
        let path = dir.join("clip.mp4.part");
        let file = [
            bx(b"ftyp", b"isom"),
            moov(&[mvhd_v0(1, seconds), video(b"avc1", 16, 16), audio(2)]),
            bx(b"mdat", &[0u8; 16]),
        ]
        .concat();
        std::fs::write(&path, file).unwrap();
        path
    }

    #[tokio::test]
    async fn mp4_outputs_skip_ffprobe_and_every_other_verdict_comes_from_it() {
        let dir = tempfile::tempdir().unwrap();
        let part = write_mp4(dir.path(), 5);
        // No ffprobe at this path: any verdict that needs it reports the failed run.
        let ffprobe = dir.path().join("missing-ffprobe.exe");
        let ok = Expected {
            sec: 5.4,
            upper_tolerance: VERIFY_TOLERANCE_SEC,
        };
        assert_eq!(verify_output(&ffprobe, &part, Muxer::Mp4, ok).await, Ok(()));
        assert_eq!(verify_output(&ffprobe, &part, Muxer::Mov, ok).await, Ok(()));
        let needs_ffprobe = |r: Result<(), String>| {
            r.is_err_and(|e| e.starts_with("The exported file could not be checked"))
        };
        // Other containers always go to ffprobe.
        assert!(needs_ffprobe(
            verify_output(&ffprobe, &part, Muxer::Matroska, ok).await
        ));
        // Within the tolerance but less than MOOV_MARGIN_SEC inside it: ffprobe decides.
        let near_edge = Expected {
            sec: 5.8,
            upper_tolerance: VERIFY_TOLERANCE_SEC,
        };
        assert!(needs_ffprobe(
            verify_output(&ffprobe, &part, Muxer::Mp4, near_edge).await
        ));
        // An MP4 whose moov contradicts the expectation is failed by ffprobe, not by the reader.
        let long = Expected {
            sec: 9.0,
            upper_tolerance: VERIFY_TOLERANCE_SEC,
        };
        assert!(needs_ffprobe(
            verify_output(&ffprobe, &part, Muxer::Mp4, long).await
        ));
        std::fs::write(&part, b"not an mp4").unwrap();
        assert!(needs_ffprobe(
            verify_output(&ffprobe, &part, Muxer::Mp4, ok).await
        ));
    }

    #[test]
    fn check_needs_video_and_the_expected_duration() {
        let mut r = VerifyReport {
            duration_sec: Some(10.0),
            video_codec: Some("h264".into()),
            ..VerifyReport::default()
        };
        let exp = |sec, upper_tolerance| Expected {
            sec,
            upper_tolerance,
        };
        assert_eq!(check(&r, exp(10.5, 1.0), 0.0), Ok(()));
        assert_eq!(check(&r, exp(9.1, 1.0), 0.0), Ok(()));
        assert!(check(&r, exp(8.9, 1.0), 0.0).is_err());
        assert!(check(&r, exp(11.1, 1.0), 0.0).is_err());
        assert_eq!(check(&r, exp(5.0, 60.0), 0.0), Ok(()));
        // The reader's margin: 0.5 s inside the tolerance on both sides.
        assert_eq!(check(&r, exp(10.4, 1.0), MOOV_MARGIN_SEC), Ok(()));
        assert!(check(&r, exp(10.6, 1.0), MOOV_MARGIN_SEC).is_err());
        assert!(check(&r, exp(9.4, 1.0), MOOV_MARGIN_SEC).is_err());
        assert_eq!(check(&r, exp(5.0, 60.0), MOOV_MARGIN_SEC), Ok(()));
        r.duration_sec = Some(0.0);
        assert_eq!(
            check(&r, exp(0.0, 1.0), 0.0),
            Err("The exported file has no duration".into())
        );
        r.video_codec = None;
        assert_eq!(
            check(&r, exp(10.0, 1.0), 0.0),
            Err("The exported file contains no video stream".into())
        );
    }
}
