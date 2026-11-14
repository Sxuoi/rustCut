//! MP4/MOV sample-table reader (PLY-06 fast path).
//!
//! Reads only the boxes needed for the frame index: `mvhd`, and of the first video `trak` the
//! `mdhd`, `hdlr`, `stts`, `ctts`, `stss` and `elst`. Every other box, `mdat` above all, is
//! skipped with a seek, so the cost is the size of the `moov` leaf tables, not of the file. The
//! `moov` may be anywhere (start or end), and 64-bit box sizes are supported.
//!
//! Presentation times are modelled on what FFmpeg's `mov` demuxer reports: `pts = dts + ctts`,
//! shifted by the edit list (an empty edit delays the stream, a leading media time drops the
//! frames before it). Anything the model does not cover (fragmented files, several edits, a
//! non-1.0 edit rate, an edit that trims the end, ...) returns [`Mp4Error::Unsupported`] and the
//! caller falls back to the packet scan.

use std::io::{self, Read, Seek, SeekFrom};

/// Largest leaf table read into memory (a 10 h 60 fps `ctts` is a few MiB).
const MAX_LEAF_BYTES: u64 = 256 * 1024 * 1024;

/// Why the sample tables could not be used.
#[derive(Debug, thiserror::Error)]
pub enum Mp4Error {
    /// Reading the file failed.
    #[error("reading the MP4 boxes failed: {0}")]
    Io(#[from] io::Error),
    /// The file is valid or damaged in a way the fast path does not handle.
    #[error("{0}")]
    Unsupported(String),
}

fn unsupported<T>(reason: impl Into<String>) -> Result<T, Mp4Error> {
    Err(Mp4Error::Unsupported(reason.into()))
}

/// Absolute presentation times (seconds, edit list applied) of the first video track.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleTimes {
    /// Presentation time of every presented frame (decode order, unsorted).
    pub frame_times: Vec<f64>,
    /// Presentation time of every sync sample that is presented.
    pub keyframes: Vec<f64>,
}

#[derive(Debug, Clone, Copy)]
struct BoxHeader {
    kind: [u8; 4],
    body_start: u64,
    end: u64,
}

fn read_header<R: Read + Seek>(
    r: &mut R,
    pos: u64,
    end: u64,
) -> Result<Option<BoxHeader>, Mp4Error> {
    if end.saturating_sub(pos) < 8 {
        return Ok(None);
    }
    r.seek(SeekFrom::Start(pos))?;
    let mut head = [0u8; 8];
    r.read_exact(&mut head)?;
    let size32 = u64::from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]));
    let kind = [head[4], head[5], head[6], head[7]];
    let (header_len, box_end) = match size32 {
        0 => (8, end),
        1 => {
            let mut big = [0u8; 8];
            r.read_exact(&mut big)?;
            (16, pos.saturating_add(u64::from_be_bytes(big)))
        }
        n => (8, pos.saturating_add(n)),
    };
    if box_end < pos + header_len || box_end > end {
        return unsupported("a box has an impossible size (the file is damaged or truncated)");
    }
    Ok(Some(BoxHeader {
        kind,
        body_start: pos + header_len,
        end: box_end,
    }))
}

fn children<R: Read + Seek>(r: &mut R, start: u64, end: u64) -> Result<Vec<BoxHeader>, Mp4Error> {
    let mut out = Vec::new();
    let mut pos = start;
    while let Some(h) = read_header(r, pos, end)? {
        pos = h.end;
        out.push(h);
    }
    Ok(out)
}

fn read_body<R: Read + Seek>(r: &mut R, h: &BoxHeader) -> Result<Vec<u8>, Mp4Error> {
    let len = h.end - h.body_start;
    if len > MAX_LEAF_BYTES {
        return unsupported("a sample table is implausibly large");
    }
    r.seek(SeekFrom::Start(h.body_start))?;
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    b.get(at..at + 8).map(|s| {
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        u64::from_be_bytes(a)
    })
}

fn truncated<T>(what: &str) -> Result<T, Mp4Error> {
    unsupported(format!("the {what} box is truncated"))
}

/// `mvhd` / `mdhd` time scale.
fn timescale(body: &[u8], what: &str) -> Result<u32, Mp4Error> {
    let at = if body.first() == Some(&1) { 20 } else { 12 };
    match be32(body, at) {
        Some(ts) if ts > 0 => Ok(ts),
        Some(_) => unsupported(format!("the {what} time scale is zero")),
        None => truncated(what),
    }
}

/// `(count, value)` runs of `stts`/`ctts`; `ctts` offsets are signed (version 1, and version 0
/// as FFmpeg reads it).
fn runs(body: &[u8], what: &str) -> Result<Vec<(u32, i64)>, Mp4Error> {
    let Some(count) = be32(body, 4) else {
        return truncated(what);
    };
    let signed = what == "ctts";
    let mut out = Vec::with_capacity((count as usize).min(body.len() / 8));
    for i in 0..count as usize {
        let at = 8 + i * 8;
        let (Some(n), Some(v)) = (be32(body, at), be32(body, at + 4)) else {
            return truncated(what);
        };
        let v = if signed {
            i64::from(v as i32)
        } else {
            i64::from(v)
        };
        out.push((n, v));
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Edit {
    /// Segment duration in movie time scale units.
    duration: u64,
    /// Media time in media time scale units, -1 for an empty edit.
    media_time: i64,
    rate_int: i16,
    rate_frac: i16,
}

fn edits(body: &[u8]) -> Result<Vec<Edit>, Mp4Error> {
    let version = body.first().copied().unwrap_or(0);
    let Some(count) = be32(body, 4) else {
        return truncated("elst");
    };
    let entry = if version == 1 { 20 } else { 12 };
    let mut out = Vec::new();
    for i in 0..count as usize {
        let at = 8 + i * entry;
        let parsed = if version == 1 {
            be64(body, at)
                .zip(be64(body, at + 8))
                .map(|(d, t)| (d, t as i64))
        } else {
            be32(body, at)
                .zip(be32(body, at + 4))
                .map(|(d, t)| (u64::from(d), i64::from(t as i32)))
        };
        let (Some((duration, media_time)), Some(rate)) = (parsed, be32(body, at + entry - 4))
        else {
            return truncated("elst");
        };
        out.push(Edit {
            duration,
            media_time,
            rate_int: (rate >> 16) as i16,
            rate_frac: (rate & 0xffff) as i16,
        });
    }
    Ok(out)
}

struct VideoTrack {
    media_scale: u32,
    stts: Vec<(u32, i64)>,
    ctts: Vec<(u32, i64)>,
    /// `None` when the track has no `stss` (every sample is a sync sample).
    stss: Option<Vec<u32>>,
    edits: Vec<Edit>,
}

fn parse_trak<R: Read + Seek>(r: &mut R, trak: &BoxHeader) -> Result<Option<VideoTrack>, Mp4Error> {
    let mut edit_list = Vec::new();
    let mut media_scale = None;
    let mut handler = None;
    let mut stbl = None;
    for child in children(r, trak.body_start, trak.end)? {
        match &child.kind {
            b"edts" => {
                for e in children(r, child.body_start, child.end)? {
                    if &e.kind == b"elst" {
                        edit_list = edits(&read_body(r, &e)?)?;
                    }
                }
            }
            b"mdia" => {
                for m in children(r, child.body_start, child.end)? {
                    match &m.kind {
                        b"mdhd" => media_scale = Some(timescale(&read_body(r, &m)?, "mdhd")?),
                        b"hdlr" => {
                            let body = read_body(r, &m)?;
                            handler = body.get(8..12).map(<[u8]>::to_vec);
                        }
                        b"minf" => {
                            for s in children(r, m.body_start, m.end)? {
                                if &s.kind == b"stbl" {
                                    stbl = Some(s);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if handler.as_deref() != Some(b"vide") {
        return Ok(None);
    }
    let (Some(media_scale), Some(stbl)) = (media_scale, stbl) else {
        return unsupported("the video track has no mdhd or stbl box");
    };
    let (mut stts, mut ctts, mut stss) = (None, Vec::new(), None);
    for t in children(r, stbl.body_start, stbl.end)? {
        match &t.kind {
            b"stts" => stts = Some(runs(&read_body(r, &t)?, "stts")?),
            b"ctts" => ctts = runs(&read_body(r, &t)?, "ctts")?,
            b"stss" => stss = Some(sync_samples(&read_body(r, &t)?)?),
            _ => {}
        }
    }
    let Some(stts) = stts else {
        return unsupported("the video track has no stts box");
    };
    Ok(Some(VideoTrack {
        media_scale,
        stts,
        ctts,
        stss,
        edits: edit_list,
    }))
}

fn sync_samples(body: &[u8]) -> Result<Vec<u32>, Mp4Error> {
    let Some(count) = be32(body, 4) else {
        return truncated("stss");
    };
    (0..count as usize)
        .map(|i| be32(body, 8 + i * 4).map_or_else(|| truncated("stss"), Ok))
        .collect()
}

/// The edit-list model: `(delay in movie units, media start in media units, end limit)`.
type EditModel = (u64, i64, Option<(u64, i64)>);

fn edit_model(track: &VideoTrack) -> Result<EditModel, Mp4Error> {
    let mut list = track.edits.as_slice();
    let mut empty = 0u64;
    if let Some(first) = list.first()
        && first.media_time < 0
    {
        empty = first.duration;
        list = &list[1..];
    }
    match list {
        [] => Ok((empty, 0, None)),
        [e] if e.media_time >= 0 && e.rate_int == 1 && e.rate_frac == 0 => {
            Ok((empty, e.media_time, Some((e.duration, e.media_time))))
        }
        _ => unsupported("the edit list has several edits or a non-unit rate"),
    }
}

fn sample_times(track: &VideoTrack, movie_scale: u32) -> Result<SampleTimes, Mp4Error> {
    let total: u64 = track.stts.iter().map(|(n, _)| u64::from(*n)).sum();
    if total == 0 {
        return unsupported("the video track has no samples");
    }
    let ctts_total: u64 = track.ctts.iter().map(|(n, _)| u64::from(*n)).sum();
    if !track.ctts.is_empty() && ctts_total < total {
        return unsupported("the ctts table covers fewer samples than stts");
    }
    let (empty, media_start, trim) = edit_model(track)?;
    let media_scale = f64::from(track.media_scale);
    let delay = empty as f64 / f64::from(movie_scale);
    let edit_end =
        trim.map(|(dur, start)| start as f64 + dur as f64 * media_scale / f64::from(movie_scale));

    let mut ctts_iter = track
        .ctts
        .iter()
        .flat_map(|(n, v)| std::iter::repeat_n(*v, *n as usize));
    let mut sync = track.stss.as_ref().map(|s| s.iter().copied().peekable());
    let mut frame_times = Vec::with_capacity(total as usize);
    let mut keyframes = Vec::new();
    let mut dts: i64 = 0;
    let mut sample_no: u32 = 0;
    for (count, delta) in &track.stts {
        for _ in 0..*count {
            sample_no += 1;
            let offset = ctts_iter.next().unwrap_or(0);
            let pts = dts + offset;
            dts += delta;
            let is_sync = match sync.as_mut() {
                None => true,
                Some(it) => {
                    while it.next_if(|n| *n < sample_no).is_some() {}
                    it.next_if(|n| *n == sample_no).is_some()
                }
            };
            if trim.is_some() && pts < media_start {
                continue; // before the edit: FFmpeg discards these packets
            }
            if let Some(end) = edit_end
                && pts as f64 >= end
            {
                return unsupported("the edit list trims the end of the video");
            }
            let t = (pts - media_start) as f64 / media_scale + delay;
            frame_times.push(t);
            if is_sync {
                keyframes.push(t);
            }
        }
    }
    Ok(SampleTimes {
        frame_times,
        keyframes,
    })
}

/// Reads the frame and keyframe times of the first video track from `r`.
///
/// # Errors
/// [`Mp4Error::Unsupported`] for anything outside the model (the caller scans packets instead),
/// [`Mp4Error::Io`] when reading fails.
pub fn read_sample_times<R: Read + Seek>(r: &mut R) -> Result<SampleTimes, Mp4Error> {
    let len = r.seek(SeekFrom::End(0))?;
    let mut pos = 0;
    let mut first = true;
    while let Some(h) = read_header(r, pos, len)? {
        if first
            && !matches!(
                &h.kind,
                b"ftyp" | b"moov" | b"mdat" | b"free" | b"wide" | b"skip" | b"pnot"
            )
        {
            return unsupported("the file does not start like an MP4/MOV file");
        }
        first = false;
        if &h.kind == b"moov" {
            return parse_moov(r, &h);
        }
        pos = h.end;
    }
    unsupported("no moov box found")
}

fn parse_moov<R: Read + Seek>(r: &mut R, moov: &BoxHeader) -> Result<SampleTimes, Mp4Error> {
    let mut movie_scale = None;
    let mut video = None;
    for child in children(r, moov.body_start, moov.end)? {
        match &child.kind {
            b"mvex" => return unsupported("fragmented MP4 (mvex)"),
            b"mvhd" => movie_scale = Some(timescale(&read_body(r, &child)?, "mvhd")?),
            b"trak" if video.is_none() => video = parse_trak(r, &child)?,
            _ => {}
        }
    }
    let (Some(video), Some(movie_scale)) = (video, movie_scale) else {
        return unsupported("no video track or mvhd in the moov box");
    };
    sample_times(&video, movie_scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn bx64(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = 1u32.to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(&((body.len() + 16) as u64).to_be_bytes());
        v.extend_from_slice(body);
        v
    }

    fn table(entries: &[(u32, u32)], version: u8) -> Vec<u8> {
        let mut v = vec![version, 0, 0, 0];
        v.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (a, b) in entries {
            v.extend_from_slice(&a.to_be_bytes());
            v.extend_from_slice(&b.to_be_bytes());
        }
        v
    }

    fn stss(samples: &[u32]) -> Vec<u8> {
        let mut v = vec![0, 0, 0, 0];
        v.extend_from_slice(&(samples.len() as u32).to_be_bytes());
        for s in samples {
            v.extend_from_slice(&s.to_be_bytes());
        }
        v
    }

    fn mvhd(scale: u32) -> Vec<u8> {
        let mut v = vec![0u8; 100];
        v[12..16].copy_from_slice(&scale.to_be_bytes());
        bx(b"mvhd", &v)
    }

    fn elst(entries: &[(u32, i32)]) -> Vec<u8> {
        let mut v = vec![0, 0, 0, 0];
        v.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (d, t) in entries {
            v.extend_from_slice(&d.to_be_bytes());
            v.extend_from_slice(&t.to_be_bytes());
            v.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        }
        bx(b"edts", &bx(b"elst", &v))
    }

    struct Spec<'a> {
        handler: &'a [u8; 4],
        stts: &'a [(u32, u32)],
        ctts: Option<(&'a [(u32, u32)], u8)>,
        stss: Option<&'a [u32]>,
        edits: Option<&'a [(u32, i32)]>,
    }

    fn spec<'a>(handler: &'a [u8; 4], stts: &'a [(u32, u32)]) -> Spec<'a> {
        Spec {
            handler,
            stts,
            ctts: None,
            stss: None,
            edits: None,
        }
    }

    fn trak(s: &Spec) -> Vec<u8> {
        let mut mdhd = vec![0u8; 24];
        mdhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        let mut hdlr = vec![0u8; 12];
        hdlr[8..12].copy_from_slice(s.handler);
        let mut stbl = bx(b"stts", &table(s.stts, 0));
        if let Some((c, v)) = s.ctts {
            stbl.extend(bx(b"ctts", &table(c, v)));
        }
        if let Some(ss) = s.stss {
            stbl.extend(bx(b"stss", &stss(ss)));
        }
        let mut mdia = bx(b"mdhd", &mdhd);
        mdia.extend(bx(b"hdlr", &hdlr));
        mdia.extend(bx(b"minf", &bx(b"stbl", &stbl)));
        let mut trak = Vec::new();
        if let Some(e) = s.edits {
            trak.extend(elst(e));
        }
        trak.extend(bx(b"mdia", &mdia));
        bx(b"trak", &trak)
    }

    fn file(moov_children: &[Vec<u8>], moov_first: bool, big: bool) -> Vec<u8> {
        let ftyp = bx(b"ftyp", b"isom\0\0\0\0isom");
        let mk = if big { bx64 } else { bx };
        let mdat = mk(b"mdat", &[7u8; 1000]);
        let moov = mk(b"moov", &moov_children.concat());
        if moov_first {
            [ftyp, moov, mdat].concat()
        } else {
            [ftyp, mdat, moov].concat()
        }
    }

    fn times(file: Vec<u8>) -> Result<SampleTimes, Mp4Error> {
        read_sample_times(&mut Cursor::new(file))
    }

    #[test]
    fn plain_cfr_with_sync_samples_any_box_layout() {
        let mut s = spec(b"vide", &[(6, 100)]);
        s.stss = Some(&[1, 4]);
        let video = trak(&s);
        for (first, big) in [(true, false), (false, false), (false, true)] {
            let t = times(file(&[mvhd(1000), video.clone()], first, big)).unwrap();
            assert_eq!(t.frame_times, vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5]);
            assert_eq!(t.keyframes, vec![0.0, 0.3]);
        }
    }

    #[test]
    fn b_frames_with_edit_list_start_at_zero() {
        let mut s = spec(b"vide", &[(4, 100)]);
        s.ctts = Some((&[(1, 200), (1, 400), (1, 100), (1, 0)], 0));
        s.stss = Some(&[1]);
        s.edits = Some(&[(600, 200)]);
        // pts = 200, 500, 300, 300 minus the media time 200
        let t = times(file(&[mvhd(1000), trak(&s)], false, false)).unwrap();
        let mut got = t.frame_times.clone();
        got.sort_by(f64::total_cmp);
        assert_eq!(got, vec![0.0, 0.1, 0.1, 0.3]);
        assert_eq!(t.keyframes, vec![0.0]);
    }

    #[test]
    fn empty_edit_delays_and_preroll_is_dropped() {
        let mut s = spec(b"vide", &[(5, 100)]);
        s.stss = Some(&[1, 3]);
        s.edits = Some(&[(250, -1), (400, 200)]);
        let t = times(file(&[mvhd(1000), trak(&s)], true, false)).unwrap();
        // frames 0 and 100 are before the media time; the rest start at 0.25 s
        assert_eq!(t.frame_times, vec![0.25, 0.35, 0.45]);
        assert_eq!(t.keyframes, vec![0.25]);
    }

    #[test]
    fn negative_ctts_version_one() {
        let mut s = spec(b"vide", &[(3, 100)]);
        s.ctts = Some((&[(1, (-100i32) as u32), (1, 100), (1, 0)], 1));
        let t = times(file(&[mvhd(1000), trak(&s)], true, false)).unwrap();
        let mut got = t.frame_times;
        got.sort_by(f64::total_cmp);
        assert_eq!(got, vec![-0.1, 0.2, 0.2]);
    }

    #[test]
    fn picks_the_video_track_not_the_audio_track() {
        let audio = trak(&spec(b"soun", &[(50, 20)]));
        let video = trak(&spec(b"vide", &[(2, 500)]));
        let t = times(file(&[mvhd(1000), audio, video], true, false)).unwrap();
        assert_eq!(t.frame_times, vec![0.0, 0.5]);
        assert_eq!(t.keyframes, vec![0.0, 0.5]);
    }

    #[test]
    fn unexpected_layouts_are_unsupported() {
        let video = trak(&spec(b"vide", &[(2, 100)]));
        let fragmented = file(&[mvhd(1000), video.clone(), bx(b"mvex", &[])], true, false);
        assert!(
            matches!(times(fragmented), Err(Mp4Error::Unsupported(m)) if m.contains("fragmented"))
        );
        let mut s = spec(b"vide", &[(2, 100)]);
        s.edits = Some(&[(100, 0), (100, 100)]);
        let two = file(&[mvhd(1000), trak(&s)], true, false);
        assert!(matches!(times(two), Err(Mp4Error::Unsupported(_))));
        let mut s = spec(b"vide", &[(4, 100)]);
        s.edits = Some(&[(200, 0)]);
        let trimmed = file(&[mvhd(1000), trak(&s)], true, false);
        assert!(matches!(times(trimmed), Err(Mp4Error::Unsupported(_))));
        assert!(matches!(
            times(vec![1, 2, 3]),
            Err(Mp4Error::Unsupported(_))
        ));
        let no_moov = [bx(b"ftyp", b"isom\0\0\0\0"), bx(b"mdat", &[0; 10])].concat();
        assert!(matches!(times(no_moov), Err(Mp4Error::Unsupported(_))));
        let mut cut = file(&[mvhd(1000), video], true, false);
        cut.truncate(cut.len() - 1200);
        assert!(times(cut).is_err());
    }
}
