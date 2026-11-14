//! EXP-16 timestamp export: YouTube chapters, CSV, CMX3600 EDL and FCPXML 1.10.

use rustcut_model::time::{format_timecode, parse_rational};
use rustcut_model::{Clip, ExportStatus, Project, TimestampExport, TimestampFormat};
use uuid::Uuid;

/// Minimum spacing between YouTube chapters (EXP-16).
const MIN_CHAPTER_GAP_SEC: u64 = 10;
/// Minimum number of YouTube chapters (EXP-16).
const MIN_CHAPTERS: usize = 3;

/// Export clip timestamps in `format`.
///
/// `clip_ids` limits the export to those clips (`None` = all clips). YouTube
/// chapters, EDL and FCPXML are chronological; CSV follows clip numbers.
/// Rule violations are reported in `warnings`, never by dropping entries.
pub fn export(
    project: &Project,
    clip_ids: Option<&[Uuid]>,
    format: TimestampFormat,
) -> TimestampExport {
    let mut clips: Vec<&Clip> = project
        .clips
        .iter()
        .filter(|c| clip_ids.is_none_or(|ids| ids.contains(&c.id)))
        .collect();
    let mut warnings = Vec::new();
    let (text, ext) = match format {
        TimestampFormat::Csv => {
            clips.sort_by_key(|c| c.index);
            (csv(project, &clips), "csv")
        }
        TimestampFormat::YoutubeChapters => {
            clips.sort_by(|a, b| {
                a.start_sec
                    .total_cmp(&b.start_sec)
                    .then(a.index.cmp(&b.index))
            });
            (youtube_chapters(&clips, &mut warnings), "txt")
        }
        TimestampFormat::Edl => {
            clips.sort_by(|a, b| {
                a.start_sec
                    .total_cmp(&b.start_sec)
                    .then(a.index.cmp(&b.index))
            });
            (edl(project, &clips, &mut warnings), "edl")
        }
        TimestampFormat::Fcpxml => {
            clips.sort_by(|a, b| {
                a.start_sec
                    .total_cmp(&b.start_sec)
                    .then(a.index.cmp(&b.index))
            });
            (fcpxml(project, &clips, &mut warnings), "fcpxml")
        }
    };
    if clips.is_empty() {
        warnings.push("No clips to export.".to_owned());
    }
    TimestampExport {
        format,
        text,
        file_extension: ext.to_owned(),
        warnings,
    }
}

/// Single-line display title for a clip.
fn title(clip: &Clip) -> String {
    let t = clip.name.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        format!("Clip {}", clip.index)
    } else {
        t
    }
}

// ───────────────────────────── YouTube chapters ─────────────────────────────

fn chapter_stamp(sec: u64) -> String {
    let (h, m, s) = (sec / 3600, (sec / 60) % 60, sec % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else if sec == 0 {
        "00:00".to_owned()
    } else {
        format!("{m}:{s:02}")
    }
}

fn whole_sec(sec: f64) -> u64 {
    if sec.is_finite() && sec > 0.0 {
        sec.floor() as u64
    } else {
        0
    }
}

fn youtube_chapters(clips: &[&Clip], warnings: &mut Vec<String>) -> String {
    let mut entries: Vec<(u64, String)> = clips
        .iter()
        .map(|c| (whole_sec(c.start_sec), title(c)))
        .collect();
    if entries.first().is_none_or(|(s, _)| *s > 0) && !entries.is_empty() {
        entries.insert(0, (0, "Intro".to_owned()));
        warnings
            .push("YouTube requires the first chapter at 00:00: added \"00:00 Intro\".".to_owned());
    }
    if !entries.is_empty() && entries.len() < MIN_CHAPTERS {
        warnings.push(format!(
            "YouTube needs at least {MIN_CHAPTERS} chapters to show them; this list has {}.",
            entries.len()
        ));
    }
    for pair in entries.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if b.0 <= a.0 {
            warnings.push(format!(
                "Chapters \"{}\" and \"{}\" start at the same second ({}); YouTube needs ascending timestamps.",
                a.1,
                b.1,
                chapter_stamp(b.0)
            ));
        } else if b.0 - a.0 < MIN_CHAPTER_GAP_SEC {
            warnings.push(format!(
                "Chapter \"{}\" ({}) is only {} s after \"{}\"; YouTube needs at least {MIN_CHAPTER_GAP_SEC} s per chapter.",
                b.1,
                chapter_stamp(b.0),
                b.0 - a.0,
                a.1
            ));
        }
    }
    entries
        .iter()
        .map(|(s, t)| format!("{} {t}\n", chapter_stamp(*s)))
        .collect()
}

// ───────────────────────────── CSV ─────────────────────────────

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

fn status_str(s: ExportStatus) -> &'static str {
    match s {
        ExportStatus::Pending => "pending",
        ExportStatus::Exporting => "exporting",
        ExportStatus::Completed => "completed",
        ExportStatus::Failed => "failed",
        ExportStatus::Outdated => "outdated",
    }
}

fn csv(project: &Project, clips: &[&Clip]) -> String {
    let mut out = String::from("index,name,start,end,duration,tag,status\r\n");
    for c in clips {
        let tag = c
            .tag_id
            .as_deref()
            .and_then(|id| project.tag(id))
            .map(|t| t.label.as_str())
            .unwrap_or("");
        let row = [
            c.index.to_string(),
            csv_field(&c.name),
            format_timecode(c.start_sec),
            format_timecode(c.end_sec),
            format_timecode(c.duration()),
            csv_field(tag),
            status_str(c.export_status).to_owned(),
        ];
        out.push_str(&row.join(","));
        out.push_str("\r\n");
    }
    out
}

// ───────────────────────────── frame math (EDL / FCPXML) ─────────────────────────────

/// Exact rate as a rational `(num, den)`; unknown rates fall back to 30/1.
fn rational_rate(project: &Project) -> (u64, u64) {
    let s = &project.source.video.frame_rate;
    if let Some((n, d)) = s.split_once('/')
        && let (Ok(n), Ok(d)) = (n.trim().parse::<u64>(), d.trim().parse::<u64>())
        && n > 0
        && d > 0
    {
        return (n, d);
    }
    match parse_rational(s) {
        Some(f) if f.is_finite() && f > 0.0 => {
            // Common NTSC rates are written exactly; others as an integer rate.
            for (n, d) in [(24000, 1001), (30000, 1001), (60000, 1001)] {
                if (f - n as f64 / d as f64).abs() < 0.005 {
                    return (n, d);
                }
            }
            (f.round().max(1.0) as u64, 1)
        }
        _ => (30, 1),
    }
}

fn frame_of(sec: f64, rate: (u64, u64)) -> u64 {
    if !sec.is_finite() || sec <= 0.0 {
        return 0;
    }
    (sec * rate.0 as f64 / rate.1 as f64).round() as u64
}

/// Integer timebase for non-drop-frame timecode (29.97 -> 30).
fn timebase(rate: (u64, u64)) -> u64 {
    (((rate.0 as f64) / (rate.1 as f64)).round() as u64).max(1)
}

fn smpte(frames: u64, tb: u64) -> String {
    let (f, total_s) = (frames % tb, frames / tb);
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        total_s / 3600,
        (total_s / 60) % 60,
        total_s % 60,
        f
    )
}

/// One timeline event: (source in, source out, record in, duration) in frames.
fn events(clips: &[&Clip], rate: (u64, u64)) -> Vec<(u64, u64, u64, u64)> {
    let mut record = 0;
    clips
        .iter()
        .map(|c| {
            let (a, b) = (frame_of(c.start_sec, rate), frame_of(c.end_sec, rate));
            let dur = b.saturating_sub(a).max(1);
            let ev = (a, a + dur, record, dur);
            record += dur;
            ev
        })
        .collect()
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ───────────────────────────── EDL ─────────────────────────────

fn edl(project: &Project, clips: &[&Clip], warnings: &mut Vec<String>) -> String {
    let rate = rational_rate(project);
    let tb = timebase(rate);
    if project.source.video.vfr {
        warnings.push(
            "The source has a variable frame rate; EDL times use its average frame rate."
                .to_owned(),
        );
    }
    let mut out = format!(
        "TITLE: {}\nFCM: NON-DROP FRAME\n\n",
        one_line(&project.source.stem())
    );
    for (i, (c, (src_in, src_out, rec_in, dur))) in
        clips.iter().zip(events(clips, rate)).enumerate()
    {
        out.push_str(&format!(
            "{:03}  AX       AA/V  C        {} {} {} {}\n",
            i + 1,
            smpte(src_in, tb),
            smpte(src_out, tb),
            smpte(rec_in, tb),
            smpte(rec_in + dur, tb)
        ));
        out.push_str(&format!("* FROM CLIP NAME: {}\n\n", title(c)));
    }
    out
}

// ───────────────────────────── FCPXML 1.10 ─────────────────────────────

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// `file://` URL with a percent-encoded path (Windows drive and UNC paths too).
fn file_url(path: &str) -> String {
    let p = path.replace('\\', "/");
    let mut encoded = String::with_capacity(p.len() + 8);
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/' | b':') {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    let bytes = p.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        format!("file:///{encoded}")
    } else if p.starts_with("//") {
        format!("file:{encoded}")
    } else if p.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

/// `N/Ds` rational time for `frames` frames at `rate`.
fn rational_time(frames: u64, rate: (u64, u64)) -> String {
    if frames == 0 {
        "0s".to_owned()
    } else {
        format!("{}/{}s", frames * rate.1, rate.0)
    }
}

fn fcpxml(project: &Project, clips: &[&Clip], warnings: &mut Vec<String>) -> String {
    let rate = rational_rate(project);
    let src = &project.source;
    if src.video.vfr {
        warnings.push(
            "The source has a variable frame rate; FCPXML times use its average frame rate."
                .to_owned(),
        );
    }
    let stem = xml_escape(&one_line(&src.stem()));
    let fps = (rate.0 as f64 / rate.1 as f64).round() as u64;
    let (w, h) = (src.video.display_width, src.video.display_height);
    let evs = events(clips, rate);
    let total: u64 = evs.iter().map(|e| e.3).sum();
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE fcpxml>\n<fcpxml version=\"1.10\">\n  <resources>\n");
    x.push_str(&format!(
        "    <format id=\"r1\" name=\"FFVideoFormat{h}p{fps}\" frameDuration=\"{}/{}s\" width=\"{w}\" height=\"{h}\"/>\n",
        rate.1, rate.0
    ));
    x.push_str(&format!(
        "    <asset id=\"r2\" name=\"{stem}\" start=\"0s\" duration=\"{}\" hasVideo=\"1\" hasAudio=\"{}\" format=\"r1\">\n      <media-rep kind=\"original-media\" src=\"{}\"/>\n    </asset>\n  </resources>\n",
        rational_time(frame_of(src.duration_sec, rate), rate),
        u8::from(!src.audio_tracks.is_empty()),
        xml_escape(&file_url(&src.path))
    ));
    x.push_str(&format!(
        "  <library>\n    <event name=\"{stem}\">\n      <project name=\"{stem} clips\">\n        <sequence format=\"r1\" duration=\"{}\" tcStart=\"0s\" tcFormat=\"NDF\">\n          <spine>\n",
        rational_time(total, rate)
    ));
    for (c, (src_in, _, rec_in, dur)) in clips.iter().zip(&evs) {
        x.push_str(&format!(
            "            <asset-clip ref=\"r2\" name=\"{}\" offset=\"{}\" start=\"{}\" duration=\"{}\" format=\"r1\" tcFormat=\"NDF\"/>\n",
            xml_escape(&title(c)),
            rational_time(*rec_in, rate),
            rational_time(*src_in, rate),
            rational_time(*dur, rate)
        ));
    }
    x.push_str("          </spine>\n        </sequence>\n      </project>\n    </event>\n  </library>\n</fcpxml>\n");
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcut_model::{AudioTrack, Fingerprint, SourceInfo, VideoInfo};

    fn source(frame_rate: &str) -> SourceInfo {
        SourceInfo {
            // A native absolute path; on Unix `D:\...` would be one file name containing backslashes.
            path: if cfg!(windows) {
                "D:\\Videos\\live stream & co.mp4"
            } else {
                "/Videos/live stream & co.mp4"
            }
            .into(),
            relative_path: "live stream & co.mp4".into(),
            fingerprint: Fingerprint {
                size_bytes: 1,
                xxh3_head_tail: "x".into(),
            },
            duration_sec: 7200.0,
            start_time_sec: 0.0,
            video: VideoInfo {
                codec: "h264".into(),
                coded_width: 1920,
                coded_height: 1080,
                display_width: 1920,
                display_height: 1080,
                rotation: 0,
                sar: "1:1".into(),
                frame_rate: frame_rate.into(),
                vfr: false,
                hdr: false,
                stream_index: None,
                pix_fmt: None,
                profile: None,
                level: None,
                color_transfer: None,
                bit_depth: None,
            },
            audio_tracks: vec![AudioTrack {
                stream_index: 1,
                codec: "aac".into(),
                channels: 2,
                title: None,
                language: None,
                sample_rate: None,
                is_default: true,
            }],
            format_name: None,
            bit_rate: None,
        }
    }

    fn project(frame_rate: &str, clips: &[(&str, f64, f64)]) -> Project {
        let mut p = Project::new(source(frame_rate));
        for (name, s, e) in clips {
            let index = p.next_index;
            p.next_index += 1;
            p.clips.push(Clip {
                id: Uuid::from_u128(index as u128),
                index,
                name: (*name).into(),
                start_sec: *s,
                end_sec: *e,
                tag_id: (index == 1).then(|| "funny".to_owned()),
                crop_x_offset: 0.5,
                selected_for_export: true,
                export_status: ExportStatus::Pending,
                last_export: None,
            });
        }
        p
    }

    #[test]
    fn chapters_basic_with_hour_and_intro() {
        let p = project(
            "30/1",
            &[
                ("Jump scare", 70.4, 90.0),
                ("Epic win", 3725.0, 3800.0),
                ("Fail", 20.0, 40.0),
            ],
        );
        let e = export(&p, None, TimestampFormat::YoutubeChapters);
        assert_eq!(e.file_extension, "txt");
        assert_eq!(
            e.text,
            "00:00 Intro\n0:20 Fail\n1:10 Jump scare\n1:02:05 Epic win\n"
        );
        assert_eq!(e.warnings.len(), 1, "{:?}", e.warnings); // only the Intro insert
        assert!(e.warnings[0].contains("00:00 Intro"));
    }

    #[test]
    fn chapters_first_at_zero_no_intro() {
        let p = project(
            "30/1",
            &[("A", 0.4, 20.0), ("B", 30.0, 40.0), ("C", 60.0, 70.0)],
        );
        let e = export(&p, None, TimestampFormat::YoutubeChapters);
        assert_eq!(e.text, "00:00 A\n0:30 B\n1:00 C\n");
        assert!(e.warnings.is_empty());
    }

    #[test]
    fn chapters_warn_too_few_and_too_close_without_dropping() {
        let p = project("30/1", &[("A", 0.0, 8.0), ("B", 9.0, 12.0)]);
        let e = export(&p, None, TimestampFormat::YoutubeChapters);
        assert_eq!(e.text.lines().count(), 2, "nothing dropped: {}", e.text);
        let w = e.warnings.join("\n");
        assert!(w.contains("at least 3"), "{w}");
        assert!(w.contains("only 9 s after"), "{w}");
    }

    #[test]
    fn chapters_same_second_warns() {
        let p = project(
            "30/1",
            &[("A", 0.0, 8.0), ("B", 0.5, 12.0), ("C", 40.0, 50.0)],
        );
        let e = export(&p, None, TimestampFormat::YoutubeChapters);
        assert_eq!(e.text.lines().count(), 3);
        assert!(
            e.warnings.iter().any(|w| w.contains("same second")),
            "{:?}",
            e.warnings
        );
    }

    #[test]
    fn chapters_empty_name_and_selection() {
        let p = project(
            "30/1",
            &[("", 0.0, 20.0), ("B", 30.0, 40.0), ("C", 60.0, 70.0)],
        );
        let ids = [p.clips[0].id, p.clips[2].id];
        let e = export(&p, Some(&ids), TimestampFormat::YoutubeChapters);
        assert_eq!(e.text, "00:00 Clip 1\n1:00 C\n");
        let none = export(&p, Some(&[]), TimestampFormat::YoutubeChapters);
        assert!(none.text.is_empty() && none.warnings.iter().any(|w| w.contains("No clips")));
    }

    #[test]
    fn csv_rfc4180() {
        let p = project(
            "30/1",
            &[("Say \"hi\", world", 1.5, 3.25), ("two\nlines", 10.0, 20.0)],
        );
        let e = export(&p, None, TimestampFormat::Csv);
        assert_eq!(e.file_extension, "csv");
        assert_eq!(
            e.text,
            "index,name,start,end,duration,tag,status\r\n\
             1,\"Say \"\"hi\"\", world\",00:00:01.500,00:00:03.250,00:00:01.750,Funny,pending\r\n\
             2,\"two\nlines\",00:00:10.000,00:00:20.000,00:00:10.000,,pending\r\n"
        );
    }

    #[test]
    fn csv_is_in_clip_number_order() {
        let p = project("30/1", &[("late", 100.0, 110.0), ("early", 1.0, 5.0)]);
        let e = export(&p, None, TimestampFormat::Csv);
        let rows: Vec<&str> = e.text.lines().skip(1).collect();
        assert!(rows[0].starts_with("1,late") && rows[1].starts_with("2,early"));
    }

    #[test]
    fn edl_cmx3600_integer_rate() {
        let p = project("30/1", &[("Second", 100.0, 110.5), ("First", 10.0, 20.0)]);
        let e = export(&p, None, TimestampFormat::Edl);
        assert_eq!(e.file_extension, "edl");
        let expected = "TITLE: live stream & co\nFCM: NON-DROP FRAME\n\n\
            001  AX       AA/V  C        00:00:10:00 00:00:20:00 00:00:00:00 00:00:10:00\n\
            * FROM CLIP NAME: First\n\n\
            002  AX       AA/V  C        00:01:40:00 00:01:50:15 00:00:10:00 00:00:20:15\n\
            * FROM CLIP NAME: Second\n\n";
        assert_eq!(e.text, expected);
    }

    #[test]
    fn edl_ntsc_rate_uses_30_timebase() {
        let p = project("30000/1001", &[("A", 100.0, 101.0)]);
        let e = export(&p, None, TimestampFormat::Edl);
        // 100 s * 29.97 = 2997 frames = 00:01:39:27 in a 30 fps non-drop count.
        assert!(e.text.contains("00:01:39:27 00:01:40:27"), "{}", e.text);
        // Record side is 30 frames long (1.001 s rounded to 30 frames).
        assert!(e.text.contains("00:00:00:00 00:00:01:00"), "{}", e.text);
    }

    #[test]
    fn edl_23976_rounds_to_24() {
        assert_eq!(timebase(rational_rate(&project("24000/1001", &[]))), 24);
        assert_eq!(timebase(rational_rate(&project("23.976", &[]))), 24);
        assert_eq!(rational_rate(&project("garbage", &[])), (30, 1));
    }

    #[test]
    fn fcpxml_is_well_formed_with_rational_times() {
        let p = project(
            "30000/1001",
            &[("Q&A \"live\" <b>", 10.0, 20.0), ("B", 30.0, 31.0)],
        );
        let e = export(&p, None, TimestampFormat::Fcpxml);
        assert_eq!(e.file_extension, "fcpxml");
        let t = &e.text;
        assert!(t.contains("<fcpxml version=\"1.10\">"));
        assert!(t.contains("frameDuration=\"1001/30000s\""));
        assert!(
            t.contains("src=\"file:///D:/Videos/live%20stream%20%26%20co.mp4\""),
            "{t}"
        );
        // 10 s = 300 frames = 300*1001/30000s.
        assert!(t.contains("start=\"300300/30000s\""), "{t}");
        assert!(t.contains("offset=\"0s\""));
        check_xml(t);
    }

    fn check_xml(text: &str) {
        use quick_xml::Reader;
        use quick_xml::events::Event;
        let mut r = Reader::from_str(text);
        let (mut clips, mut assets, mut depth) = (0, 0, 0i32);
        loop {
            match r.read_event().expect("well-formed xml") {
                Event::Start(e) => {
                    depth += 1;
                    if e.name().as_ref() == b"asset" {
                        assets += 1;
                    }
                }
                Event::End(_) => depth -= 1,
                Event::Empty(e) => {
                    if e.name().as_ref() == b"asset-clip" {
                        clips += 1;
                        for a in e.attributes() {
                            a.expect("valid attribute")
                                .unescape_value()
                                .expect("valid escapes");
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        assert_eq!(depth, 0);
        assert_eq!(assets, 1);
        assert!(clips >= 1);
    }

    #[test]
    fn fcpxml_name_roundtrips_through_parser() {
        use quick_xml::Reader;
        use quick_xml::events::Event;
        let p = project("25/1", &[("Q&A \"live\" <b>", 10.0, 20.0)]);
        let e = export(&p, None, TimestampFormat::Fcpxml);
        let mut r = Reader::from_str(&e.text);
        let mut found = None;
        loop {
            match r.read_event().unwrap() {
                Event::Empty(s) if s.name().as_ref() == b"asset-clip" => {
                    for a in s.attributes().flatten() {
                        if a.key.as_ref() == b"name" {
                            found = Some(a.unescape_value().unwrap().into_owned());
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        assert_eq!(found.as_deref(), Some("Q&A \"live\" <b>"));
        assert!(e.text.contains("duration=\"250/25s\""));
    }

    #[test]
    fn file_urls() {
        assert_eq!(file_url("/home/me/a b.mp4"), "file:///home/me/a%20b.mp4");
        assert_eq!(file_url("C:\\x\\\u{e9}.mp4"), "file:///C:/x/%C3%A9.mp4");
        assert_eq!(file_url("\\\\nas\\share\\a.mp4"), "file://nas/share/a.mp4");
    }

    #[test]
    fn empty_selection_still_valid_xml() {
        let p = project("30/1", &[("A", 1.0, 2.0)]);
        let e = export(&p, Some(&[]), TimestampFormat::Fcpxml);
        assert!(e.warnings.iter().any(|w| w.contains("No clips")));
        assert!(e.text.contains("duration=\"0s\""));
    }
}
