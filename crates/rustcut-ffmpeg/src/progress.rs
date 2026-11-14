//! `-progress pipe:1` parser (EXP-09).
//!
//! FFmpeg writes `key=value` blocks terminated by `progress=continue` or
//! `progress=end`. One [`ProgressUpdate`] is emitted per `progress=` line
//! with the values of that block. FFmpeg 9 starts with `out_time_us=N/A` and
//! `speed=N/A`, may report negative times while B-frames prime, and pads the
//! speed (`speed=  10x`).

/// Values of one `-progress` block.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProgressUpdate {
    /// Output position in seconds (`None` for `N/A`; negative values → 0).
    pub out_time_sec: Option<f64>,
    /// Encoding speed relative to real time (`2.5` for `2.5x`).
    pub speed: Option<f64>,
    pub frame: Option<u64>,
    /// Bytes written so far.
    pub total_size: Option<u64>,
    /// `progress=end`: FFmpeg finished writing the output.
    pub end: bool,
}

/// Incremental parser; feed it stdout lines in order.
#[derive(Debug, Clone, Default)]
pub struct ProgressParser {
    out_time_us: Option<f64>,
    out_time_ms: Option<f64>,
    out_time: Option<f64>,
    speed: Option<f64>,
    frame: Option<u64>,
    total_size: Option<u64>,
}

impl ProgressParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume one line (trailing `\r\n`/`\n` allowed). Returns an update on
    /// each `progress=` line, `None` otherwise.
    pub fn feed(&mut self, line: &str) -> Option<ProgressUpdate> {
        let (key, value) = line.trim().split_once('=')?;
        let value = value.trim();
        match key.trim() {
            "out_time_us" => self.out_time_us = parse_micros(value),
            // Despite its name FFmpeg prints microseconds here too.
            "out_time_ms" => self.out_time_ms = parse_micros(value),
            "out_time" => self.out_time = parse_clock(value),
            "speed" => self.speed = parse_speed(value),
            "frame" => self.frame = value.parse().ok(),
            "total_size" => self.total_size = value.parse().ok(),
            "progress" => {
                let block = std::mem::take(self);
                return Some(ProgressUpdate {
                    out_time_sec: block.out_time_us.or(block.out_time_ms).or(block.out_time),
                    speed: block.speed,
                    frame: block.frame,
                    total_size: block.total_size,
                    end: value == "end",
                });
            }
            _ => {}
        }
        None
    }

    /// Feed a chunk of text that may hold several lines.
    pub fn feed_text(&mut self, text: &str) -> Vec<ProgressUpdate> {
        text.lines().filter_map(|l| self.feed(l)).collect()
    }
}

/// Fraction of a clip done, clamped to `[0, 1]`. `clip_duration_sec` is the
/// expected output duration (for Lossless including the lead-in).
pub fn fraction(out_time_sec: f64, clip_duration_sec: f64) -> f64 {
    if !(clip_duration_sec.is_finite() && clip_duration_sec > 0.0) || !out_time_sec.is_finite() {
        return 0.0;
    }
    (out_time_sec / clip_duration_sec).clamp(0.0, 1.0)
}

fn parse_micros(value: &str) -> Option<f64> {
    let us: i64 = value.parse().ok()?;
    Some((us.max(0) as f64) / 1_000_000.0)
}

/// `HH:MM:SS.micro`, possibly negative (`-00:00:00.023220`).
fn parse_clock(value: &str) -> Option<f64> {
    let negative = value.starts_with('-');
    let mut total = 0.0;
    for part in value.trim_start_matches('-').split(':') {
        let v: f64 = part.parse().ok()?;
        total = total * 60.0 + v;
    }
    if !total.is_finite() {
        return None;
    }
    Some(if negative { 0.0 } else { total })
}

fn parse_speed(value: &str) -> Option<f64> {
    let v: f64 = value.trim().strip_suffix('x')?.trim().parse().ok()?;
    (v.is_finite() && v >= 0.0).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp09_block_emits_on_progress_line() {
        let mut p = ProgressParser::new();
        let text = "frame=90\nfps=0.00\nstream_0_0_q=-1.0\nbitrate=N/A\ntotal_size=262192\nout_time_us=1033333\n\
                    out_time_ms=1033333\nout_time=00:00:01.033333\ndup_frames=0\ndrop_frames=0\nspeed=2.44x\nprogress=continue\n";
        let ups = p.feed_text(text);
        assert_eq!(
            ups,
            vec![ProgressUpdate {
                out_time_sec: Some(1.033333),
                speed: Some(2.44),
                frame: Some(90),
                total_size: Some(262_192),
                end: false
            }]
        );
    }

    #[test]
    fn exp09_na_negative_padded_speed_and_crlf() {
        let mut p = ProgressParser::new();
        assert_eq!(p.feed("out_time_us=N/A\r\n"), None);
        assert_eq!(p.feed("speed=N/A\r\n"), None);
        let first = p.feed("progress=continue\r\n").unwrap();
        assert_eq!(
            (first.out_time_sec, first.speed, first.end),
            (None, None, false)
        );
        p.feed("out_time_us=-23220\r\n");
        p.feed("speed=  10x\r\n");
        let second = p.feed("progress=end\r\n").unwrap();
        assert_eq!(
            (second.out_time_sec, second.speed, second.end),
            (Some(0.0), Some(10.0), true)
        );
    }

    #[test]
    fn out_time_fallbacks() {
        let mut p = ProgressParser::new();
        p.feed("out_time=00:01:02.500000");
        assert_eq!(
            p.feed("progress=continue").unwrap().out_time_sec,
            Some(62.5)
        );
        p.feed("out_time=-00:00:00.023220");
        assert_eq!(p.feed("progress=continue").unwrap().out_time_sec, Some(0.0));
        p.feed("out_time_ms=2000000");
        assert_eq!(p.feed("progress=continue").unwrap().out_time_sec, Some(2.0));
        // Values never leak into the next block.
        assert_eq!(
            p.feed("progress=continue").unwrap(),
            ProgressUpdate::default()
        );
        assert_eq!(p.feed("garbage without equals"), None);
    }

    #[test]
    fn fraction_is_clamped() {
        assert_eq!(fraction(5.0, 10.0), 0.5);
        assert_eq!(fraction(12.0, 10.0), 1.0);
        assert_eq!(fraction(-1.0, 10.0), 0.0);
        assert_eq!(fraction(1.0, 0.0), 0.0);
        assert_eq!(fraction(f64::NAN, 10.0), 0.0);
        assert_eq!(fraction(1.0, f64::INFINITY), 0.0);
    }
}
