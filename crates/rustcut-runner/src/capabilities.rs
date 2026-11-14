//! What the located FFmpeg build supports (version, filters, encoders), cached per binary.
//!
//! Used to pick a framing chain (`zscale` present or not) and to offer only compiled-in
//! encoders; whether a hardware encoder actually works is a separate test encode (EXP-13).

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::{RunnerError, Sidecars, run_capture};

/// Generous: the very first FFmpeg start can be slow while antivirus software scans it.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

type Cache = HashMap<PathBuf, Arc<Capabilities>>;

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Mutex::default);

/// Features of one FFmpeg build, from `ffmpeg -version`, `-filters` and `-encoders`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    /// The version as printed by `ffmpeg -version`, e.g. `9.0.2-full_build-www.gyan.dev` or
    /// `N-118000-g1234abcd`.
    pub ffmpeg_version: String,
    /// Names of all compiled-in filters, e.g. `zscale`.
    pub filters: HashSet<String>,
    /// Names of all compiled-in encoders, e.g. `libx264`, `h264_amf`.
    pub encoders: HashSet<String>,
}

impl Capabilities {
    /// Whether the filter is compiled in (e.g. `has_filter("zscale")`).
    pub fn has_filter(&self, name: &str) -> bool {
        self.filters.contains(name)
    }

    /// Whether the encoder is compiled in (e.g. `has_encoder("h264_amf")`). It may still be
    /// unusable on this machine (no GPU/driver), which only a test encode reveals (EXP-13).
    pub fn has_encoder(&self, name: &str) -> bool {
        self.encoders.contains(name)
    }
}

/// Minimum supported FFmpeg `(major, minor)` (PRD §7.1).
pub const MIN_FFMPEG: (u32, u32) = (6, 1);

/// Whether a `ffmpeg -version` token is at least `min`. Release versions are
/// compared by `major.minor`; non-release builds (`N-118000-g1234abcd`, git
/// snapshots) cannot be judged and are accepted.
pub fn version_meets_minimum(version: &str, min: (u32, u32)) -> bool {
    let mut parts = version.split(|c: char| !c.is_ascii_digit());
    let (Some(a), Some(b)) = (parts.next(), parts.next()) else {
        return true;
    };
    if !version.starts_with(|c: char| c.is_ascii_digit()) {
        return true;
    }
    match (a.parse::<u32>(), b.parse::<u32>()) {
        (Ok(major), Ok(minor)) => (major, minor) >= min,
        (Ok(major), Err(_)) => major > min.0,
        _ => true,
    }
}

/// Reports what `sidecars.ffmpeg` supports. Results are cached per FFmpeg path for the life of
/// the process, so only the first call runs FFmpeg (three quick runs, in parallel).
///
/// # Errors
/// Any error of [`run_capture`]; [`RunnerError::CommandFailed`] if FFmpeg exits unsuccessfully;
/// [`RunnerError::UnexpectedOutput`] if its output cannot be parsed.
pub async fn probe_capabilities(sidecars: &Sidecars) -> Result<Arc<Capabilities>, RunnerError> {
    let ffmpeg = sidecars.ffmpeg.as_path();
    let cached = cache().get(ffmpeg).cloned();
    if let Some(cached) = cached {
        return Ok(cached);
    }
    let (version, filters, encoders) = tokio::try_join!(
        listing(ffmpeg, "-version", "reading the FFmpeg version"),
        listing(ffmpeg, "-filters", "listing the FFmpeg filters"),
        listing(ffmpeg, "-encoders", "listing the FFmpeg encoders"),
    )?;
    let unexpected = |action| RunnerError::UnexpectedOutput {
        program: ffmpeg.to_path_buf(),
        action,
    };
    let ffmpeg_version =
        parse_version(&version).ok_or_else(|| unexpected("reading the FFmpeg version"))?;
    if !version_meets_minimum(&ffmpeg_version, MIN_FFMPEG) {
        return Err(RunnerError::FfmpegTooOld {
            found: ffmpeg_version,
            required: format!("{}.{}", MIN_FFMPEG.0, MIN_FFMPEG.1),
        });
    }
    let filters = parse_filters(&filters);
    if filters.is_empty() {
        return Err(unexpected("listing the FFmpeg filters"));
    }
    let encoders = parse_encoders(&encoders);
    if encoders.is_empty() {
        return Err(unexpected("listing the FFmpeg encoders"));
    }
    let capabilities = Arc::new(Capabilities {
        ffmpeg_version,
        filters,
        encoders,
    });
    cache().insert(ffmpeg.to_path_buf(), Arc::clone(&capabilities));
    Ok(capabilities)
}

fn cache() -> MutexGuard<'static, Cache> {
    CACHE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs `ffmpeg -nostdin -hide_banner <flag>` and returns its stdout.
async fn listing(ffmpeg: &Path, flag: &str, action: &'static str) -> Result<String, RunnerError> {
    let args = ["-nostdin", "-hide_banner", flag].map(OsString::from);
    let output = run_capture(ffmpeg, &args, PROBE_TIMEOUT).await?;
    if !output.success() {
        let stderr = output.stderr_lossy();
        let detail = stderr
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("no error message")
            .to_owned();
        return Err(RunnerError::CommandFailed {
            program: ffmpeg.to_path_buf(),
            action,
            exit_code: output.status_code,
            detail,
        });
    }
    Ok(output.stdout_lossy().into_owned())
}

/// `ffmpeg version 9.0.2-full_build-www.gyan.dev Copyright …` → `9.0.2-full_build-www.gyan.dev`.
fn parse_version(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix("ffmpeg version ")?;
        rest.split_whitespace().next().map(str::to_owned)
    })
}

/// Filter rows look like ` .S zscale            V->V       Apply resizing…`: the third column
/// is the I/O signature containing `->`. Legend lines (`T.. = Timeline support`) never do, so
/// this works with and without the `------` separator of newer FFmpeg versions.
fn parse_filters(text: &str) -> HashSet<String> {
    text.lines()
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            let (_flags, name, io) = (columns.next()?, columns.next()?, columns.next()?);
            io.contains("->").then(|| name.to_owned())
        })
        .collect()
}

/// Encoder rows follow the ` ------` separator: ` V....D libx264  libx264 H.264 …`.
fn parse_encoders(text: &str) -> HashSet<String> {
    text.lines()
        .skip_while(|line| !is_separator(line))
        .skip(1)
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            let _flags = columns.next()?;
            columns.next().map(str::to_owned)
        })
        .collect()
}

fn is_separator(line: &str) -> bool {
    let line = line.trim();
    line.len() >= 3 && line.bytes().all(|b| b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERSION: &str = "ffmpeg version 9.0.2-full_build-www.gyan.dev Copyright (c) 2000-2026 the FFmpeg developers\n\
built with gcc 16.2.0 (Rev3, Built by MSYS2 project)\n";

    const FILTERS: &str = "Filters:
  T.. = Timeline support
  .S. = Slice threading
  A = Audio input/output
  V = Video input/output
  N = Dynamic number and/or type of input/output
  | = Source or sink filter
  ------
 TS aap               AA->A      Apply Affine Projection algorithm to first audio stream.
 .. scale             V->V       Scale the input video size and/or convert the image format.
 .S zscale            V->V       Apply resizing, colorspace and bit depth conversion.
 .. nullsrc           |->V       Null video source, return unprocessed video frames.
 .. amovie            |->N       Read audio from a movie source.
";

    /// FFmpeg 4.x: three flag columns and no separator line.
    const OLD_FILTERS: &str = "Filters:
  T.. = Timeline support
  .S. = Slice threading
  ..C = Command support
  | = Source or sink filter
 ... abench            A->A       Benchmark part of a filtergraph.
 T.C scale             V->V       Scale the input video size and/or convert the image format.
";

    const ENCODERS: &str = "Encoders:
 V..... = Video
 A..... = Audio
 S..... = Subtitle
 .F.... = Frame-level multithreading
 ..S... = Slice-level multithreading
 ...X.. = Codec is experimental
 ....B. = Supports draw_horiz_band
 .....D = Supports direct rendering method 1
 ------
 V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10 (codec h264)
 V....D h264_amf             AMD AMF H.264 Encoder (codec h264)
 V.S..D mpeg4                MPEG-4 part 2
 A....D aac                  AAC (Advanced Audio Coding)
";

    #[test]
    fn minimum_version_is_enforced() {
        assert!(version_meets_minimum(
            "9.0.2-full_build-www.gyan.dev",
            MIN_FFMPEG
        ));
        assert!(version_meets_minimum("6.1", MIN_FFMPEG));
        assert!(version_meets_minimum("7.0.1", MIN_FFMPEG));
        assert!(!version_meets_minimum("6.0.1", MIN_FFMPEG));
        assert!(!version_meets_minimum("4.4.2-0ubuntu", MIN_FFMPEG));
        assert!(!version_meets_minimum("5.1.6", MIN_FFMPEG));
        assert!(version_meets_minimum("N-118000-g1234abcd", MIN_FFMPEG));
    }

    #[test]
    fn parses_the_version_token() {
        assert_eq!(
            parse_version(VERSION).as_deref(),
            Some("9.0.2-full_build-www.gyan.dev")
        );
        assert_eq!(
            parse_version("ffmpeg version N-118000-g1234abcd Copyright").as_deref(),
            Some("N-118000-g1234abcd")
        );
        assert_eq!(parse_version("ffprobe version 9.0"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn parses_filter_names_and_skips_the_legend() {
        let filters = parse_filters(FILTERS);
        let expected: HashSet<String> = ["aap", "scale", "zscale", "nullsrc", "amovie"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(filters, expected);
        let old = parse_filters(OLD_FILTERS);
        assert!(old.contains("scale") && old.contains("abench") && old.len() == 2);
    }

    #[test]
    fn parses_encoder_names_after_the_separator() {
        let encoders = parse_encoders(ENCODERS);
        let expected: HashSet<String> = ["libx264", "h264_amf", "mpeg4", "aac"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(encoders, expected);
        assert!(parse_encoders("no separator here\n V....D x").is_empty());
    }

    #[test]
    fn has_filter_and_has_encoder_check_exact_names() {
        let capabilities = Capabilities {
            ffmpeg_version: "9.0.2".to_owned(),
            filters: parse_filters(FILTERS),
            encoders: parse_encoders(ENCODERS),
        };
        assert!(capabilities.has_filter("zscale"));
        assert!(!capabilities.has_filter("zscal"));
        assert!(capabilities.has_encoder("h264_amf"));
        assert!(!capabilities.has_encoder("h264_nvenc"));
    }
}
