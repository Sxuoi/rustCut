//! Times in tool arguments: seconds (number) or "HH:MM:SS(.mmm)" / "MM:SS" / "SS.s" / "12m30s" text.

use serde::Deserialize;
use serde_json::Value;

/// A time as an agent writes it: a number of seconds or a clock string.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "Value")]
pub enum TimeArg {
    /// Seconds from the start of the video.
    Num(f64),
    /// `HH:MM:SS(.mmm)`, `MM:SS(.mmm)`, `12m30s` or plain seconds as text.
    Str(String),
}

impl TryFrom<Value> for TimeArg {
    type Error = String;

    fn try_from(v: Value) -> Result<Self, String> {
        match v {
            Value::Number(n) => n
                .as_f64()
                .map(TimeArg::Num)
                .ok_or_else(|| "the time is not a usable number".to_string()),
            Value::String(s) => Ok(TimeArg::Str(s)),
            other => Err(format!(
                "a time must be a number of seconds (e.g. 75.5) or text like \"12:10\", but got {other}"
            )),
        }
    }
}

/// Parses a [`TimeArg`] into seconds (finite, not negative).
///
/// # Errors
/// A message that says what formats are accepted.
pub fn parse_time(arg: &TimeArg) -> Result<f64, String> {
    match arg {
        TimeArg::Num(n) => check(*n, &n.to_string()),
        TimeArg::Str(s) => parse_time_str(s),
    }
}

fn check(v: f64, shown: &str) -> Result<f64, String> {
    if v.is_finite() && v >= 0.0 {
        Ok(v)
    } else {
        Err(format!(
            "\"{shown}\" is not a valid time. Use seconds from the video start (e.g. 75.5) or \"HH:MM:SS.mmm\" / \"MM:SS\"."
        ))
    }
}

/// Parses `HH:MM:SS(.mmm)`, `MM:SS(.mmm)`, `12m30s` or seconds given as text.
///
/// # Errors
/// A message that says what formats are accepted.
pub fn parse_time_str(text: &str) -> Result<f64, String> {
    let s = text.trim();
    let bad = || {
        format!(
            "\"{text}\" is not a valid time. Use seconds from the video start (e.g. 75.5) or \"HH:MM:SS.mmm\" / \"MM:SS\"."
        )
    };
    if s.is_empty() {
        return Err(bad());
    }
    if !s.contains(':') && s.contains(['h', 'm', 's', 'H', 'M', 'S']) {
        return parse_units(s).ok_or_else(bad).and_then(|v| check(v, text));
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 {
        return Err(bad());
    }
    let mut total = 0.0;
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let p = part.trim().replace(',', ".");
        if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return Err(bad());
        }
        if !last && p.contains('.') {
            return Err(bad());
        }
        let v: f64 = p.parse().map_err(|_| bad())?;
        if parts.len() > 1 && i > 0 && v >= 60.0 {
            return Err(bad());
        }
        total = total * 60.0 + v;
    }
    check(total, text)
}

/// `1h2m3s`, `12m30s`, `45s`, `12m30` (a trailing bare number is seconds).
fn parse_units(s: &str) -> Option<f64> {
    let s = s.to_lowercase();
    let mut total = 0.0;
    let mut num = String::new();
    let mut seen = 0;
    for c in s.chars().filter(|c| !c.is_whitespace()) {
        match c {
            '0'..='9' | '.' => num.push(c),
            'h' | 'm' | 's' => {
                let v: f64 = num.parse().ok()?;
                num.clear();
                total += v * match c {
                    'h' => 3600.0,
                    'm' => 60.0,
                    _ => 1.0,
                };
                seen += 1;
            }
            _ => return None,
        }
    }
    if !num.is_empty() {
        total += num.parse::<f64>().ok()?;
    }
    (seen > 0 && total.is_finite()).then_some(total)
}

fn is_dash(c: char) -> bool {
    matches!(
        c,
        '-' | '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2212}'
    )
}

/// A time token at the start of `s` (digits, `:`, `.`, `h`, `m`, `s`) and the rest of the line.
fn lex_time(s: &str) -> Option<(&str, &str)> {
    if !s.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, ':' | '.' | 'h' | 'm' | 's')))
        .unwrap_or(s.len());
    // "5min" or "10apples" is a word, not a time.
    if s[end..].starts_with(|c: char| c.is_alphanumeric()) {
        return None;
    }
    let tok = s[..end].trim_end_matches([':', '.']);
    Some((tok, &s[tok.len()..]))
}

fn strong(tok: &str) -> bool {
    tok.contains([':', 'h', 'm', 's'])
}

enum Rewrite {
    Unchanged,
    Line(String),
    Error(String),
}

/// Recognizes `a -> b name`, `a --> b name`, `[a -> b] name`, `90-100 name` and `12m30s-13m name`
/// and rewrites them to the `HH:MM:SS.mmm-HH:MM:SS.mmm name` form the importer reads.
fn rewrite_range(raw: &str) -> Rewrite {
    let line = raw.trim().trim_start_matches(|c: char| {
        c.is_whitespace() || matches!(c, '[' | '(' | '*' | '\u{2022}' | '>')
    });
    let line = match line.strip_prefix('-') {
        Some(r) if r.starts_with(char::is_whitespace) => r.trim_start(),
        _ => line,
    };
    let Some((t1, rest)) = lex_time(line) else {
        return Rewrite::Unchanged;
    };
    let rest = rest.trim_start();
    let (arrow, after) = if let Some(r) = rest.strip_prefix("-->") {
        (true, r)
    } else if let Some(r) = rest.strip_prefix("->") {
        (true, r)
    } else if let Some(r) = rest.strip_prefix('\u{2192}') {
        (true, r)
    } else if let Some(r) = rest.strip_prefix(is_dash) {
        (false, r)
    } else {
        return Rewrite::Unchanged;
    };
    let Some((t2, rest2)) = lex_time(after.trim_start()) else {
        return Rewrite::Unchanged;
    };
    // "10:00 - 5 reasons" is a chapter line whose name starts with a number.
    if !(arrow || strong(t2) || !strong(t1)) {
        return Rewrite::Unchanged;
    }
    let (s, e) = match (parse_time_str(t1), parse_time_str(t2)) {
        (Ok(s), Ok(e)) => (s, e),
        (Err(m), _) | (_, Err(m)) => return Rewrite::Error(m),
    };
    if e <= s {
        if arrow || strong(t2) {
            return Rewrite::Error(format!(
                "end ({}) must be after start ({}).",
                fmt_ts(e),
                fmt_ts(s)
            ));
        }
        return Rewrite::Unchanged;
    }
    let name = rest2
        .trim_start_matches(|c: char| {
            c.is_whitespace() || matches!(c, ']' | ')' | ':' | '.' | ',' | '|') || is_dash(c)
        })
        .trim();
    Rewrite::Line(
        format!("{}-{} {}", fmt_ts(s), fmt_ts(e), name)
            .trim_end()
            .to_string(),
    )
}

/// Rewrites range lines of pasted timestamps text into the canonical `start-end name` form
/// (line numbers are kept). Returns the text and `(line, error)` for lines that cannot be read.
pub fn normalize_timestamps(text: &str) -> (String, Vec<(u32, String)>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        match rewrite_range(raw) {
            Rewrite::Unchanged => out.push(raw.to_string()),
            Rewrite::Line(l) => out.push(l),
            Rewrite::Error(m) => {
                errors.push((i as u32 + 1, m));
                out.push(String::new());
            }
        }
    }
    (out.join("\n"), errors)
}

/// `HH:MM:SS.mmm`.
pub fn fmt_ts(sec: f64) -> String {
    let ms = (sec.max(0.0) * 1000.0).round() as u64;
    let (h, m, s, milli) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    format!("{h:02}:{m:02}:{s:02}.{milli:03}")
}

/// `HH:MM:SS` (rounded to the second).
pub fn fmt_hms(sec: f64) -> String {
    let s = sec.max(0.0).round() as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<f64, String> {
        parse_time_str(s)
    }

    #[test]
    fn accepts_the_documented_forms() {
        assert_eq!(p("75.5").unwrap(), 75.5);
        assert_eq!(p("01:02:03").unwrap(), 3723.0);
        assert_eq!(p("01:02:03.250").unwrap(), 3723.25);
        assert_eq!(p("12:10").unwrap(), 730.0);
        assert_eq!(p(" 90:00 ").unwrap(), 5400.0);
        assert_eq!(p("0:05,5").unwrap(), 5.5);
        assert_eq!(parse_time(&TimeArg::Num(12.0)).unwrap(), 12.0);
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["", "abc", "1:2:3:4", "-5", "10:75", "1.5:20", "1::2", "NaN"] {
            assert!(p(bad).is_err(), "{bad}");
        }
        assert!(parse_time(&TimeArg::Num(-1.0)).is_err());
        assert!(parse_time(&TimeArg::Num(f64::NAN)).is_err());
    }

    #[test]
    fn deserializes_numbers_and_strings() {
        let a: Vec<TimeArg> = serde_json::from_str(r#"[5, "00:10", 2.5]"#).unwrap();
        assert_eq!(a[0], TimeArg::Num(5.0));
        assert_eq!(a[1], TimeArg::Str("00:10".into()));
    }

    #[test]
    fn accepts_units() {
        assert_eq!(p("12m30s").unwrap(), 750.0);
        assert_eq!(p("1h2m3s").unwrap(), 3723.0);
        assert_eq!(p("13m").unwrap(), 780.0);
        assert!(p("12x").is_err());
    }

    #[test]
    fn time_arg_errors_are_plain_sentences() {
        let e = serde_json::from_str::<TimeArg>("null")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("number of seconds") && !e.contains("untagged"),
            "{e}"
        );
    }

    fn norm(t: &str) -> String {
        let (s, e) = normalize_timestamps(t);
        assert!(e.is_empty(), "{e:?}");
        s
    }

    #[test]
    fn arrow_and_bracket_lines_become_ranges() {
        assert_eq!(
            norm("10:00 --> 10:20 arrow form"),
            "00:10:00.000-00:10:20.000 arrow form"
        );
        assert_eq!(
            norm("[00:10:00 -> 00:10:05] bracket line"),
            "00:10:00.000-00:10:05.000 bracket line"
        );
        assert_eq!(
            norm("90-100 plain seconds"),
            "00:01:30.000-00:01:40.000 plain seconds"
        );
        assert_eq!(norm("12m30s-13m ok"), "00:12:30.000-00:13:00.000 ok");
    }

    #[test]
    fn chapter_and_ordinary_lines_are_left_alone() {
        for l in ["12:10 Intro", "10:00 - 5 reasons to go", "name only"] {
            assert_eq!(norm(l), l);
        }
        let (_, e) = normalize_timestamps("10:20 --> 10:00 backwards");
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_ts(3723.25), "01:02:03.250");
        assert_eq!(fmt_ts(0.0), "00:00:00.000");
        assert_eq!(fmt_hms(59.6), "00:01:00");
    }
}
