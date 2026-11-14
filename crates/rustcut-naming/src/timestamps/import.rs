//! EXP-17 timestamp import from CSV or pasted lines.

use rustcut_model::time::{parse_timecode, round_ms};
use rustcut_model::{ImportPreview, ImportedClip};

/// A non-blocking remark about an imported line (for example an end time that
/// was clamped to the source duration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportNote {
    /// 1-based line of the item.
    pub line: u32,
    /// Human-readable note.
    pub message: String,
}

/// An item before validation: times may still be unparsable.
struct Raw {
    line: u32,
    start: Result<f64, String>,
    /// `None` = chapter style (ends at the next start).
    end: Option<Result<f64, String>>,
    name: String,
}

const DASHES: [char; 7] = [
    '-', '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2014}', '\u{2212}',
];

/// Parse pasted timestamps (EXP-17), see [`parse_import_detailed`]. Notes are
/// discarded.
pub fn parse_import(text: &str, source_duration_sec: f64, min_clip_sec: f64) -> ImportPreview {
    parse_import_detailed(text, source_duration_sec, min_clip_sec).0
}

/// Parse CSV (a header with `start`/`end`/`name` columns in any order, or
/// headerless `start,end,name`) or free lines such as `12:10-13:05 Jump scare`,
/// `12:10 - 13:05 Name` (hyphen, en or em dash), `00:12:10.500 00:13:05 Name`,
/// `[12:10] Name` and chapter-style `12:10 Name` (the end is the next item's
/// start; the last item ends at the source end).
///
/// Every input line yields an item; unusable ones carry `error`: unparsable
/// time, end not after start, shorter than `min_clip_sec`, start at or beyond
/// the source duration. An end beyond the source is clamped and reported in
/// the returned notes. A non-positive `source_duration_sec` means "unknown":
/// no bounds are applied, and a chapter-style last item is an error.
pub fn parse_import_detailed(
    text: &str,
    source_duration_sec: f64,
    min_clip_sec: f64,
) -> (ImportPreview, Vec<ImportNote>) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let raws = match detect_csv(text) {
        Some(header) => parse_csv(text, header),
        None => parse_lines(text),
    };
    finalize(raws, source_duration_sec, min_clip_sec)
}

// ───────────────────────────── validation ─────────────────────────────

fn finalize(raws: Vec<Raw>, duration: f64, min_clip_sec: f64) -> (ImportPreview, Vec<ImportNote>) {
    let known_duration = duration.is_finite() && duration > 0.0;
    let mut items = Vec::with_capacity(raws.len());
    let mut notes = Vec::new();
    for (i, raw) in raws.iter().enumerate() {
        let mk = |start: f64, end: f64, error: Option<String>| ImportedClip {
            line: raw.line,
            start_sec: start,
            end_sec: end,
            name: raw.name.clone(),
            error,
        };
        let start = match &raw.start {
            Ok(s) => *s,
            Err(e) => {
                items.push(mk(0.0, 0.0, Some(e.clone())));
                continue;
            }
        };
        let end = match &raw.end {
            Some(Ok(e)) => Ok(*e),
            Some(Err(e)) => Err(e.clone()),
            None => match raws[i + 1..]
                .iter()
                .find_map(|r| r.start.as_ref().ok().copied())
            {
                Some(next) => Ok(next),
                None if known_duration => Ok(duration),
                None => Err(
                    "The source duration is unknown, so the last chapter has no end time."
                        .to_owned(),
                ),
            },
        };
        let mut end = match end {
            Ok(e) => e,
            Err(e) => {
                items.push(mk(start, start, Some(e)));
                continue;
            }
        };
        if end <= start {
            items.push(mk(
                start,
                end,
                Some(format!(
                    "The end time must be after the start time ({} to {}).",
                    fmt(start),
                    fmt(end)
                )),
            ));
            continue;
        }
        if known_duration {
            if start >= duration {
                items.push(mk(
                    start,
                    end,
                    Some(format!(
                        "The start time {} is beyond the end of the source ({}).",
                        fmt(start),
                        fmt(duration)
                    )),
                ));
                continue;
            }
            if end > duration {
                notes.push(ImportNote {
                    line: raw.line,
                    message: format!(
                        "End time {} is beyond the source; clamped to {}.",
                        fmt(end),
                        fmt(duration)
                    ),
                });
                end = round_ms(duration);
            }
        }
        if end - start + 1e-9 < min_clip_sec {
            items.push(mk(
                start,
                end,
                Some(format!(
                    "The clip is {:.1} s long, shorter than the {:.1} s minimum.",
                    end - start,
                    min_clip_sec
                )),
            ));
            continue;
        }
        items.push(mk(start, end, None));
    }
    let valid_count = items.iter().filter(|c| c.error.is_none()).count() as u32;
    (ImportPreview { items, valid_count }, notes)
}

fn fmt(sec: f64) -> String {
    rustcut_model::time::format_timecode(sec)
}

// ───────────────────────────── CSV ─────────────────────────────

/// Column positions of a CSV import.
#[derive(Clone, Copy)]
struct Columns {
    start: usize,
    end: Option<usize>,
    name: Option<usize>,
    has_header: bool,
}

/// RFC 4180 records with the 1-based line each starts on.
fn csv_records(text: &str) -> Vec<(u32, Vec<String>)> {
    let mut out = Vec::new();
    let mut fields = Vec::new();
    let mut cur = String::new();
    let (mut in_quotes, mut line, mut rec_line, mut touched) = (false, 1u32, 1u32, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if !touched {
            rec_line = line;
        }
        match c {
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cur.push('"');
                } else {
                    in_quotes = false;
                }
            }
            '"' if cur.is_empty() => {
                in_quotes = true;
                touched = true;
            }
            ',' if !in_quotes => {
                fields.push(std::mem::take(&mut cur));
                touched = true;
            }
            '\r' if !in_quotes => {}
            '\n' if !in_quotes => {
                line += 1;
                if touched || !cur.is_empty() {
                    fields.push(std::mem::take(&mut cur));
                    out.push((rec_line, std::mem::take(&mut fields)));
                }
                touched = false;
            }
            c => {
                if c == '\n' {
                    line += 1;
                }
                cur.push(c);
                touched = true;
            }
        }
    }
    if touched || !cur.is_empty() {
        fields.push(cur);
        out.push((rec_line, fields));
    }
    out.retain(|(_, f)| f.iter().any(|x| !x.trim().is_empty()));
    out
}

fn header_index(cells: &[String], names: &[&str]) -> Option<usize> {
    cells
        .iter()
        .position(|c| names.contains(&c.trim().to_lowercase().as_str()))
}

/// `Some(columns)` when the first record looks like CSV: a header naming
/// start and end columns, or a headerless record whose first two cells are times.
fn detect_csv(text: &str) -> Option<Columns> {
    let (_, first) = csv_records(text).into_iter().next()?;
    if first.len() < 2 {
        return None;
    }
    let start = header_index(&first, &["start", "start_sec", "start_time", "in"]);
    let end = header_index(&first, &["end", "end_sec", "end_time", "out"]);
    if let (Some(start), Some(end)) = (start, end) {
        let name = header_index(&first, &["name", "title", "label", "description"]);
        return Some(Columns {
            start,
            end: Some(end),
            name,
            has_header: true,
        });
    }
    if first.iter().take(2).all(|c| parse_timecode(c).is_some()) {
        return Some(Columns {
            start: 0,
            end: Some(1),
            name: (first.len() > 2).then_some(2),
            has_header: false,
        });
    }
    None
}

fn cell_time(cell: Option<&String>, what: &str) -> Result<f64, String> {
    let raw = cell.map(|c| c.trim()).unwrap_or("");
    if raw.is_empty() {
        return Err(format!("Missing {what} time."));
    }
    parse_timecode(raw).ok_or_else(|| {
        format!("Could not read the {what} time \"{raw}\"; use a form like 12:10 or 1:02:03.5.")
    })
}

fn parse_csv(text: &str, cols: Columns) -> Vec<Raw> {
    csv_records(text)
        .into_iter()
        .skip(usize::from(cols.has_header))
        .map(|(line, cells)| Raw {
            line,
            start: cell_time(cells.get(cols.start), "start"),
            end: cols.end.map(|i| cell_time(cells.get(i), "end")),
            name: cols
                .name
                .and_then(|i| cells.get(i))
                .map(|n| n.trim().to_owned())
                .unwrap_or_default(),
        })
        .collect()
}

// ───────────────────────────── free lines ─────────────────────────────

/// Maximal run of digits, `:` and `.` at the start of `s`, minus trailing
/// `:`/`.` (those belong to the separator/name). Empty when `s` does not
/// start with a digit.
fn time_token(s: &str) -> &str {
    if !s.starts_with(|c: char| c.is_ascii_digit()) {
        return "";
    }
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || c == ':' || c == '.'))
        .unwrap_or(s.len());
    s[..end].trim_end_matches([':', '.'])
}

fn unreadable(tok: &str) -> String {
    format!("Could not read the time \"{tok}\"; use a form like 12:10 or 1:02:03.5.")
}

fn skip_closers(s: &str) -> &str {
    s.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ']' | ')'))
}

fn parse_line(line_no: u32, raw_line: &str) -> Option<Raw> {
    let line = raw_line.trim();
    if line.is_empty() {
        return None;
    }
    let line = line.trim_start_matches(|c: char| {
        c.is_whitespace() || matches!(c, '[' | '(' | '*' | '\u{2022}' | '>')
    });
    // A leading list dash is only a bullet when followed by whitespace.
    let line = match line.strip_prefix('-') {
        Some(rest) if rest.starts_with(char::is_whitespace) => rest.trim_start(),
        _ => line,
    };
    let tok1 = time_token(line);
    if tok1.is_empty() {
        let first = line.split_whitespace().next().unwrap_or(line);
        return Some(Raw {
            line: line_no,
            start: Err(format!(
                "No start time found; lines must begin with a time like 12:10 (found \"{first}\")."
            )),
            end: None,
            name: String::new(),
        });
    }
    if !tok1.contains(':') {
        return Some(Raw {
            line: line_no,
            start: Err(unreadable(tok1)),
            end: None,
            name: line[tok1.len()..].trim().to_owned(),
        });
    }
    let start = parse_timecode(tok1).ok_or_else(|| unreadable(tok1));
    let mut rest = skip_closers(&line[tok1.len()..]);
    let mut end = None;
    // Optional explicit end: "-13:05", "- 13:05", or just "13:05".
    let after_dash = rest.strip_prefix(DASHES).map(|r| r.trim_start());
    let candidate = after_dash.unwrap_or(rest);
    let tok2 = time_token(candidate);
    if tok2.contains(':') {
        end = Some(parse_timecode(tok2).ok_or_else(|| unreadable(tok2)));
        rest = &candidate[tok2.len()..];
    }
    let name = rest
        .trim_start_matches(|c: char| {
            c.is_whitespace()
                || matches!(c, ']' | ')' | ':' | '.' | ',' | '|')
                || DASHES.contains(&c)
        })
        .trim()
        .to_owned();
    Some(Raw {
        line: line_no,
        start,
        end,
        name,
    })
}

fn parse_lines(text: &str) -> Vec<Raw> {
    text.lines()
        .enumerate()
        .filter_map(|(i, l)| parse_line(i as u32 + 1, l))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUR: f64 = 3600.0;

    fn one(text: &str) -> ImportedClip {
        let p = parse_import(text, DUR, 0.5);
        assert_eq!(p.items.len(), 1, "{p:?}");
        p.items.into_iter().next().unwrap()
    }

    fn ok(c: &ImportedClip, s: f64, e: f64, name: &str) {
        assert_eq!(c.error, None, "{c:?}");
        assert_eq!((c.start_sec, c.end_sec, c.name.as_str()), (s, e, name));
    }

    #[test]
    fn range_with_hyphen_en_dash_em_dash() {
        ok(&one("12:10-13:05 Jump scare"), 730.0, 785.0, "Jump scare");
        ok(&one("12:10 - 13:05 Jump scare"), 730.0, 785.0, "Jump scare");
        ok(
            &one("12:10 \u{2013} 13:05 Jump scare"),
            730.0,
            785.0,
            "Jump scare",
        );
        ok(
            &one("12:10\u{2014}13:05 Jump scare"),
            730.0,
            785.0,
            "Jump scare",
        );
    }

    #[test]
    fn ms_and_hour_forms() {
        ok(&one("00:12:10.500 00:13:05 Name"), 730.5, 785.0, "Name");
        let p = parse_import("1:00:00 - 1:01:30 Hour mark", 7200.0, 0.5);
        ok(&p.items[0], 3600.0, 3690.0, "Hour mark");
    }

    #[test]
    fn bracketed_and_chapter_styles() {
        let p = parse_import("[12:10] Name\n[13:00] Next one\n", DUR, 0.5);
        ok(&p.items[0], 730.0, 780.0, "Name");
        ok(&p.items[1], 780.0, DUR, "Next one");
        assert_eq!(p.valid_count, 2);
    }

    #[test]
    fn chapter_style_with_separators() {
        let p = parse_import("0:00 Intro\n12:10 - Jump scare\n13:05: Outro\n", DUR, 0.5);
        ok(&p.items[0], 0.0, 730.0, "Intro");
        ok(&p.items[1], 730.0, 785.0, "Jump scare");
        ok(&p.items[2], 785.0, DUR, "Outro");
    }

    #[test]
    fn name_may_start_with_digits() {
        ok(&one("12:10 - 5 reasons why"), 730.0, DUR, "5 reasons why");
        ok(&one("12:10 2024 recap"), 730.0, DUR, "2024 recap");
    }

    #[test]
    fn empty_name_allowed() {
        ok(&one("12:10-13:05"), 730.0, 785.0, "");
    }

    #[test]
    fn bullets_and_blank_lines_and_line_numbers() {
        let p = parse_import("\n- 1:00-1:30 A\n\n* 2:00-2:30 B\n", DUR, 0.5);
        assert_eq!(p.items.len(), 2);
        assert_eq!((p.items[0].line, p.items[1].line), (2, 4));
        assert_eq!(p.items[0].name, "A");
    }

    #[test]
    fn error_unparsable_time() {
        let c = one("12:99-13:05 x");
        assert!(c.error.as_deref().unwrap().contains("12:99"), "{c:?}");
        let c = one("12:10-13:99 x");
        assert!(c.error.as_deref().unwrap().contains("13:99"), "{c:?}");
        let c = one("Jump scare 12:10");
        assert!(
            c.error.as_deref().unwrap().contains("No start time"),
            "{c:?}"
        );
        let c = one("90 seconds in");
        assert!(c.error.is_some());
    }

    #[test]
    fn error_end_not_after_start() {
        let c = one("13:05-12:10 x");
        assert!(
            c.error.as_deref().unwrap().contains("after the start"),
            "{c:?}"
        );
        assert!(one("12:10-12:10 x").error.is_some());
    }

    #[test]
    fn chapter_going_backwards_is_error() {
        let p = parse_import("20:00 A\n10:00 B\n", DUR, 0.5);
        assert!(p.items[0].error.is_some());
        assert!(p.items[1].error.is_none());
        assert_eq!(p.valid_count, 1);
    }

    #[test]
    fn error_shorter_than_minimum() {
        let c = one("12:10-12:10.300 x");
        assert!(c.error.as_deref().unwrap().contains("shorter"), "{c:?}");
        let p = parse_import("12:10-12:10.500 x", DUR, 0.5);
        assert!(p.items[0].error.is_none());
    }

    #[test]
    fn start_beyond_source_is_error_and_end_is_clamped_with_note() {
        let (p, notes) = parse_import_detailed("59:00-61:00 tail\n70:00-71:00 gone\n", DUR, 0.5);
        ok(&p.items[0], 3540.0, DUR, "tail");
        assert!(
            p.items[1].error.as_deref().unwrap().contains("beyond"),
            "{:?}",
            p.items[1]
        );
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].line, 1);
        assert!(notes[0].message.contains("clamped"));
        assert_eq!(p.valid_count, 1);
    }

    #[test]
    fn clamped_end_that_makes_clip_too_short_is_error() {
        let c = one("59:59.8-61:00 x");
        assert!(c.error.as_deref().unwrap().contains("shorter"), "{c:?}");
    }

    #[test]
    fn unknown_duration_applies_no_bounds() {
        let p = parse_import("99:00-99:30 a\n100:00 b\n", 0.0, 0.5);
        assert!(p.items[0].error.is_none());
        assert!(p.items[1].error.as_deref().unwrap().contains("unknown"));
    }

    #[test]
    fn csv_header_any_order() {
        let text =
            "name,end,start\nJump scare,13:05,12:10\n\"Say \"\"hi\"\", ok\",00:00:20.5,00:00:10\n";
        let p = parse_import(text, DUR, 0.5);
        ok(&p.items[0], 730.0, 785.0, "Jump scare");
        ok(&p.items[1], 10.0, 20.5, "Say \"hi\", ok");
        assert_eq!((p.items[0].line, p.items[1].line), (2, 3));
    }

    #[test]
    fn csv_without_header() {
        let p = parse_import("12:10,13:05,Jump scare\n1:00,1:30\n", DUR, 0.5);
        ok(&p.items[0], 730.0, 785.0, "Jump scare");
        ok(&p.items[1], 60.0, 90.0, "");
    }

    #[test]
    fn csv_numeric_seconds_and_crlf() {
        let p = parse_import("start,end,name\r\n10,20,A\r\n30.5,40,B\r\n", DUR, 0.5);
        ok(&p.items[0], 10.0, 20.0, "A");
        ok(&p.items[1], 30.5, 40.0, "B");
    }

    #[test]
    fn csv_bad_cells_are_per_row_errors() {
        let p = parse_import("start,end,name\nxx,20,A\n10,,B\n10,5,C\n", DUR, 0.5);
        assert!(p.items[0].error.as_deref().unwrap().contains("xx"));
        assert!(p.items[1].error.as_deref().unwrap().contains("Missing end"));
        assert!(
            p.items[2]
                .error
                .as_deref()
                .unwrap()
                .contains("after the start")
        );
        assert_eq!(p.valid_count, 0);
    }

    #[test]
    fn csv_roundtrips_own_export_header() {
        let text = "index,name,start,end,duration,tag,status\r\n2,Jump scare,00:12:10.000,00:13:05.000,00:00:55.000,Funny,pending\r\n";
        let p = parse_import(text, DUR, 0.5);
        ok(&p.items[0], 730.0, 785.0, "Jump scare");
    }

    #[test]
    fn csv_multiline_quoted_name_line_numbers() {
        let p = parse_import("start,end,name\n10,20,\"two\nlines\"\n30,40,B\n", DUR, 0.5);
        assert_eq!(p.items[0].name, "two\nlines");
        assert_eq!((p.items[0].line, p.items[1].line), (2, 4));
    }

    #[test]
    fn free_text_with_comma_is_not_csv() {
        ok(&one("12:10 Jump scare, wow"), 730.0, DUR, "Jump scare, wow");
    }

    #[test]
    fn empty_input() {
        let p = parse_import("  \n\n", DUR, 0.5);
        assert!(p.items.is_empty() && p.valid_count == 0);
    }

    #[test]
    fn bom_is_ignored() {
        ok(&one("\u{feff}12:10-13:05 x"), 730.0, 785.0, "x");
    }
}
