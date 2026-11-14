//! Time helpers. All times are seconds (`f64`) relative to the source's first
//! presentation timestamp (PRD §7.3 "Time origin") and are stored with
//! millisecond precision (§7.5).

/// Round to millisecond precision (the precision stored in project files).
pub fn round_ms(sec: f64) -> f64 {
    if !sec.is_finite() {
        return 0.0;
    }
    (sec * 1000.0).round() / 1000.0
}

/// Whole milliseconds, for hashing and exact comparisons.
pub fn to_ms(sec: f64) -> i64 {
    if !sec.is_finite() {
        return 0;
    }
    (sec * 1000.0).round() as i64
}

/// `HH:MM:SS.mmm` (PLY-04). Negative or non-finite values render as zero.
pub fn format_timecode(sec: f64) -> String {
    let ms_total = if sec.is_finite() && sec > 0.0 {
        (sec * 1000.0).round() as u64
    } else {
        0
    };
    let ms = ms_total % 1000;
    let s_total = ms_total / 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        s_total / 3600,
        (s_total / 60) % 60,
        s_total % 60,
        ms
    )
}

/// Queue duration `m:ss.s` (QUE-01): 49.5 → `0:49.5`, 90 → `1:30.0`.
pub fn format_duration(sec: f64) -> String {
    let tenths = if sec.is_finite() && sec > 0.0 {
        (sec * 10.0).round() as u64
    } else {
        0
    };
    let m = tenths / 600;
    let rem = tenths % 600;
    format!("{}:{:02}.{}", m, rem / 10, rem % 10)
}

/// `HH-MM-SS` for the `{start}` / `{end}` naming variables (EXP-05); whole
/// seconds, truncated: 920.12 → `00-15-20`.
pub fn format_hms_dashed(sec: f64) -> String {
    let s = if sec.is_finite() && sec > 0.0 {
        sec.floor() as u64
    } else {
        0
    };
    format!("{:02}-{:02}-{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// `{duration}` naming variable: whole seconds, rounded (35.68 → `36`).
pub fn format_whole_seconds(sec: f64) -> String {
    let s = if sec.is_finite() && sec > 0.0 {
        sec.round() as u64
    } else {
        0
    };
    s.to_string()
}

/// Parse `SS`, `MM:SS`, `H:MM:SS` (each optionally with `.fff`) into seconds.
/// Used by timestamp import (EXP-17). Returns `None` for anything else.
pub fn parse_timecode(input: &str) -> Option<f64> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total = 0.0f64;
    for (i, part) in parts.iter().enumerate() {
        let last = i == parts.len() - 1;
        if part.is_empty() {
            return None;
        }
        if last {
            if !part.chars().all(|c| c.is_ascii_digit() || c == '.')
                || part.matches('.').count() > 1
            {
                return None;
            }
            let v: f64 = part.parse().ok()?;
            if parts.len() > 1 && v >= 60.0 {
                return None;
            }
            total = total * 60.0 + v;
        } else {
            if !part.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let v: f64 = part.parse().ok()?;
            if i > 0 && v >= 60.0 {
                return None;
            }
            total = total * 60.0 + v;
        }
    }
    Some(round_ms(total))
}

/// Parse an FFmpeg/FFprobe rational such as `60000/1001` or `60/1`.
pub fn parse_rational(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/').unwrap_or((s, "1"));
    let n: f64 = n.trim().parse().ok()?;
    let d: f64 = d.trim().parse().ok()?;
    if d == 0.0 || !n.is_finite() || n <= 0.0 {
        return None;
    }
    Some(n / d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timecode() {
        assert_eq!(format_timecode(920.12), "00:15:20.120");
        assert_eq!(format_timecode(9000.0), "02:30:00.000");
        assert_eq!(format_timecode(-3.0), "00:00:00.000");
        assert_eq!(format_timecode(59.9996), "00:01:00.000");
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(49.5), "0:49.5");
        assert_eq!(format_duration(955.8 - 920.12), "0:35.7");
        assert_eq!(format_duration(90.0), "1:30.0");
        assert_eq!(format_whole_seconds(955.8 - 920.12), "36");
        assert_eq!(format_hms_dashed(920.12), "00-15-20");
    }

    #[test]
    fn parse() {
        assert_eq!(parse_timecode("12:10"), Some(730.0));
        assert_eq!(parse_timecode("1:02:03.5"), Some(3723.5));
        assert_eq!(parse_timecode("45"), Some(45.0));
        assert_eq!(parse_timecode("1:75"), None);
        assert_eq!(parse_timecode("abc"), None);
        assert_eq!(
            parse_rational("60000/1001").map(|v| (v * 1000.0).round()),
            Some(59940.0)
        );
        assert_eq!(parse_rational("0/0"), None);
    }
}
