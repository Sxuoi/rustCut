//! Subtitle text handling: SRT parsing, ASS/markup stripping, line formatting and paging.

use crate::timeparse::fmt_ts;

/// One subtitle cue.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    /// Start in seconds.
    pub start: f64,
    /// End in seconds.
    pub end: f64,
    /// Plain text (one line).
    pub text: String,
}

/// Subtitle codecs FFmpeg can convert to text.
pub const TEXT_CODECS: &[&str] = &[
    "subrip",
    "srt",
    "ass",
    "ssa",
    "webvtt",
    "mov_text",
    "text",
    "subviewer",
    "subviewer1",
    "microdvd",
    "jacosub",
    "sami",
    "realtext",
    "stl",
    "vplayer",
    "mpl2",
    "pjs",
    "ttml",
];

/// Whether `codec` is a text subtitle format.
pub fn is_text_codec(codec: &str) -> bool {
    TEXT_CODECS.contains(&codec.to_lowercase().as_str())
}

/// Removes ASS override blocks (`{\an8}`), `\N` / `\n` line breaks (-> space), `\h`, HTML-like
/// tags (`<i>`) and collapses whitespace.
pub fn strip_markup(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut depth = 0usize;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            '\\' => match chars.peek() {
                Some('N') | Some('n') => {
                    chars.next();
                    out.push(' ');
                }
                Some('h') => {
                    chars.next();
                    out.push(' ');
                }
                _ => out.push(c),
            },
            '<' if !chars
                .peek()
                .is_some_and(|n| n.is_ascii_alphabetic() || *n == '/') =>
            {
                out.push(c);
            }
            '<' => {
                // Skip a tag only when it closes within a short distance (`<font face=... color=...>`).
                let mut look = chars.clone();
                let mut len = 0;
                let mut closed = false;
                for n in look.by_ref() {
                    len += 1;
                    if n == '>' {
                        closed = true;
                        break;
                    }
                    if len > 300 || n == '<' {
                        break;
                    }
                }
                if closed {
                    chars = look;
                } else {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `HH:MM:SS,mmm` / `HH:MM:SS.mmm` / `MM:SS.mmm` -> seconds.
pub fn parse_cue_time(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    let mut total = 0.0;
    for p in &parts {
        total = total * 60.0 + p.trim().parse::<f64>().ok()?;
    }
    (total.is_finite() && total >= 0.0).then_some(total)
}

/// Parses SRT text (also tolerant of WebVTT-style dots and a header). Markup is stripped; cues
/// without text are dropped. `shift` seconds are added to every time.
pub fn parse_srt(text: &str, shift: f64) -> Vec<Cue> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut cues = Vec::new();
    for block in normalized.split("\n\n") {
        let mut lines = block.lines().filter(|l| !l.trim().is_empty());
        let mut timing = None;
        let mut body: Vec<&str> = Vec::new();
        for line in lines.by_ref() {
            if timing.is_none() {
                if let Some((a, b)) = line.split_once("-->") {
                    // Cue settings may follow the end time (WebVTT).
                    let b = b.split_whitespace().next().unwrap_or("");
                    if let (Some(s), Some(e)) = (parse_cue_time(a), parse_cue_time(b)) {
                        timing = Some((s, e));
                    }
                }
                continue;
            }
            body.push(line);
        }
        let Some((s, e)) = timing else { continue };
        let text = strip_markup(&body.join(" "));
        if !text.is_empty() {
            cues.push(Cue {
                start: s + shift,
                end: e + shift,
                text,
            });
        }
    }
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    cues
}

/// ASS vector drawings (`m -960 -180 l 960 -180 ...`) arrive as text lines.
fn is_drawing(text: &str) -> bool {
    let toks: Vec<&str> = text.split_whitespace().collect();
    let nums = toks.iter().filter(|t| t.parse::<f64>().is_ok()).count();
    toks.len() >= 5
        && nums >= 4
        && matches!(toks[0], "m" | "n")
        && toks.iter().all(|t| {
            t.parse::<f64>().is_ok() || matches!(*t, "m" | "n" | "l" | "b" | "s" | "p" | "c")
        })
}

/// A lone web address (release-group watermark).
fn is_watermark(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    if t.contains(char::is_whitespace) {
        return false;
    }
    t.starts_with("www.")
        || t.starts_with("http://")
        || t.starts_with("https://")
        || [
            ".com", ".net", ".org", ".online", ".xyz", ".info", ".tv", ".io", ".me", ".co",
        ]
        .iter()
        .any(|tld| t.ends_with(tld) && t.len() > tld.len() + 2)
}

/// Removes what is noise for a reader of subtitles: drawing commands, watermark web addresses,
/// empty lines and repeats of the same text that overlap or touch the earlier cue.
pub fn clean_cues(cues: Vec<Cue>) -> Vec<Cue> {
    let mut last_end: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    let mut out = Vec::with_capacity(cues.len());
    for c in cues {
        if c.text.is_empty() || is_drawing(&c.text) || is_watermark(&c.text) {
            continue;
        }
        if last_end.get(&c.text).is_some_and(|e| c.start <= *e + 0.001) {
            let e = last_end.entry(c.text.clone()).or_insert(c.end);
            *e = e.max(c.end);
            continue;
        }
        last_end.insert(c.text.clone(), c.end);
        out.push(c);
    }
    out
}

/// `[HH:MM:SS.mmm -> HH:MM:SS.mmm] text` for the cues overlapping `[from, to]`.
pub fn format_lines(cues: &[Cue], from: Option<f64>, to: Option<f64>) -> Vec<String> {
    cues.iter()
        .filter(|c| from.is_none_or(|f| c.end > f) && to.is_none_or(|t| c.start < t))
        .map(|c| format!("[{} -> {}] {}", fmt_ts(c.start), fmt_ts(c.end), c.text))
        .collect()
}

/// One page of lines plus the paging facts.
#[derive(Debug, PartialEq)]
pub struct Page {
    /// Text of the page (one cue per line).
    pub body: String,
    /// Lines in the whole range.
    pub total: usize,
    /// First line shown (0-based), after clamping.
    pub offset: usize,
    /// Lines shown.
    pub shown: usize,
    /// Offset of the next page, if any.
    pub next_offset: Option<usize>,
}

/// Pages `lines` (`offset` 0-based, `limit` lines).
pub fn page(lines: &[String], offset: usize, limit: usize) -> Page {
    let limit = limit.max(1);
    let offset = offset.min(lines.len());
    let end = (offset + limit).min(lines.len());
    Page {
        body: lines[offset..end].join("\n"),
        total: lines.len(),
        offset,
        shown: end - offset,
        next_offset: (end < lines.len()).then_some(end),
    }
}

/// The full text answer of `get_transcript` / `transcribe`.
pub fn render_page(source: &str, p: &Page) -> String {
    let next = p
        .next_offset
        .map_or_else(|| "none (end of transcript)".to_string(), |n| n.to_string());
    format!(
        "Transcript source: {source}\ntotal_lines: {} | showing: {} from offset {} | next_offset: {next}\n{}",
        p.total, p.shown, p.offset, p.body
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ass_overrides_and_breaks() {
        assert_eq!(strip_markup(r"{\an8}Hello\Nworld"), "Hello world");
        assert_eq!(
            strip_markup(r"{\i1}Wow{\i0} that\nwas {\b1}loud{\b0}!"),
            "Wow that was loud!"
        );
        assert_eq!(
            strip_markup("<i>Italic</i> and <b>bold</b>"),
            "Italic and bold"
        );
        assert_eq!(strip_markup("1 < 2 and 3 > 2"), "1 < 2 and 3 > 2");
        assert_eq!(strip_markup(r"a\hb"), "a b");
    }

    const SRT: &str = "1\r\n00:00:01,000 --> 00:00:03,500\r\nHello there\r\n\r\n2\r\n00:01:00,250 --> 00:01:02,000\r\n{\\an8}Second\\Nline\r\nmore\r\n\r\n3\r\n00:02:00,000 --> 00:02:01,000\r\n{\\an8}\r\n";

    #[test]
    fn parses_srt_with_crlf_and_empty_cues() {
        let cues = parse_srt(SRT, 0.0);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].start, 1.0);
        assert_eq!(cues[1].text, "Second line more");
        assert_eq!(cues[1].end, 62.0);
        assert_eq!(parse_srt(SRT, 10.0)[0].start, 11.0);
    }

    #[test]
    fn parses_vtt_style_times() {
        let cues = parse_srt("WEBVTT\n\n00:01.500 --> 00:03.000 align:start\nHi\n", 0.0);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].start, 1.5);
        assert_eq!(cues[0].end, 3.0);
    }

    #[test]
    fn formats_and_filters_lines() {
        let cues = parse_srt(SRT, 0.0);
        let all = format_lines(&cues, None, None);
        assert_eq!(all[0], "[00:00:01.000 -> 00:00:03.500] Hello there");
        let late = format_lines(&cues, Some(30.0), None);
        assert_eq!(late.len(), 1);
        let early = format_lines(&cues, None, Some(10.0));
        assert_eq!(early.len(), 1);
        assert!(format_lines(&cues, Some(3.5), Some(60.25)).is_empty());
    }

    #[test]
    fn cleans_ass_noise() {
        assert_eq!(
            strip_markup(
                "<font face=\"Commie Sans\" size=\"66\" color=\"#ffffff\" outline=\"2\">Hello</font>"
            ),
            "Hello"
        );
        let cue = |s: f64, e: f64, t: &str| Cue {
            start: s,
            end: e,
            text: t.into(),
        };
        let cues = vec![
            cue(0.0, 30.0, "m -960 -180 l 960 -180 960 165.125 -960 165.125"),
            cue(0.0, 30.0, "www.huntersekai.online"),
            cue(1.0, 3.0, "Hello there"),
            cue(2.0, 4.0, "Hello there"),
            cue(10.0, 12.0, "Hello there"),
            cue(5.0, 6.0, "The m key is nice"),
        ];
        let t: Vec<String> = clean_cues(cues)
            .into_iter()
            .map(|c| format!("{}-{}", c.start, c.text))
            .collect();
        assert_eq!(
            t,
            ["1-Hello there", "10-Hello there", "5-The m key is nice"]
        );
    }

    #[test]
    fn pages_lines() {
        let lines: Vec<String> = (0..5).map(|i| format!("l{i}")).collect();
        let p = page(&lines, 0, 2);
        assert_eq!((p.total, p.shown, p.next_offset), (5, 2, Some(2)));
        let p = page(&lines, 4, 2);
        assert_eq!((p.shown, p.next_offset, p.body.as_str()), (1, None, "l4"));
        let p = page(&lines, 99, 2);
        assert_eq!((p.shown, p.offset, p.next_offset), (0, 5, None));
        assert!(render_page("x", &page(&lines, 0, 2)).contains("next_offset: 2"));
    }

    #[test]
    fn text_codecs() {
        assert!(is_text_codec("subrip"));
        assert!(is_text_codec("ASS"));
        assert!(!is_text_codec("hdmv_pgs_subtitle"));
        assert!(!is_text_codec("dvd_subtitle"));
    }
}
