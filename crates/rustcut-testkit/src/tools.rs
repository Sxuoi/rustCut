//! Tool discovery and media-inspection helpers.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

use crate::fixtures::fixtures_dir;

const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

/// Directories searched for the FFmpeg tools, in priority order:
/// `RUSTCUT_FFMPEG_DIR`, `FFMPEG_DIR`, `PATH`, then the WinGet "Gyan.FFmpeg" packages.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for var in ["RUSTCUT_FFMPEG_DIR", "FFMPEG_DIR"] {
        if let Some(v) = std::env::var_os(var) {
            dirs.push(PathBuf::from(v));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.extend(winget_dirs());
    dirs
}

fn winget_dirs() -> Vec<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("AppData").join("Local"))
        });
    let Some(base) = base else { return Vec::new() };
    let packages = base.join("Microsoft").join("WinGet").join("Packages");
    let mut out = Vec::new();
    let Ok(pkgs) = std::fs::read_dir(packages) else {
        return out;
    };
    for pkg in pkgs.flatten() {
        if !pkg.file_name().to_string_lossy().starts_with("Gyan.FFmpeg") {
            continue;
        }
        let Ok(builds) = std::fs::read_dir(pkg.path()) else {
            continue;
        };
        for build in builds.flatten() {
            out.push(build.path().join("bin"));
        }
    }
    out
}

fn locate(tool: &str) -> Option<PathBuf> {
    let file = format!("{tool}{EXE}");
    search_dirs()
        .into_iter()
        .map(|d| d.join(&file))
        .find(|p| p.is_file())
}

/// Path of `ffmpeg`, or `None` if it cannot be found (see [`ffmpeg`] for the search order).
pub fn try_ffmpeg() -> Option<PathBuf> {
    static CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();
    CACHE.get_or_init(|| locate("ffmpeg")).clone()
}

/// Path of `ffprobe`, or `None` if it cannot be found.
pub fn try_ffprobe() -> Option<PathBuf> {
    static CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();
    CACHE.get_or_init(|| locate("ffprobe")).clone()
}

/// Path of `ffmpeg`. Searches `RUSTCUT_FFMPEG_DIR`, `FFMPEG_DIR`, `PATH`, then the WinGet
/// Gyan.FFmpeg package directory.
///
/// # Panics
/// Panics with setup instructions if FFmpeg is not found.
pub fn ffmpeg() -> PathBuf {
    try_ffmpeg().expect("ffmpeg not found: put it on PATH or set RUSTCUT_FFMPEG_DIR to the folder containing ffmpeg and ffprobe")
}

/// Path of `ffprobe`; same search and panic behaviour as [`ffmpeg`].
pub fn ffprobe() -> PathBuf {
    try_ffprobe().expect("ffprobe not found: put it on PATH or set RUSTCUT_FFMPEG_DIR to the folder containing ffmpeg and ffprobe")
}

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn stderr_tail(out: &Output) -> String {
    let text = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(12)..].join("\n")
}

/// Runs `ffmpeg -hide_banner -nostdin <args>` and returns its output.
///
/// # Panics
/// Panics (including the stderr tail) if FFmpeg cannot be started or exits non-zero.
pub fn ffmpeg_ok<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let out = Command::new(ffmpeg())
        .args(["-hide_banner", "-nostdin"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("failed to start ffmpeg");
    assert!(
        out.status.success(),
        "ffmpeg failed ({}):\n{}",
        out.status,
        stderr_tail(&out)
    );
    out
}

/// Runs `ffprobe` on `path` (format and streams, side data included) and returns the JSON,
/// or the ffprobe error text (for example for truncated files).
pub fn try_probe_json(path: &Path) -> Result<Value, String> {
    let out = Command::new(ffprobe())
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot start ffprobe: {e}"))?;
    if !out.status.success() {
        return Err(stderr_tail(&out));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("ffprobe produced invalid JSON: {e}"))
}

/// Like [`try_probe_json`] but panics on failure.
pub fn probe_json(path: &Path) -> Value {
    try_probe_json(path).unwrap_or_else(|e| panic!("ffprobe failed for {}: {e}", path.display()))
}

fn probe_entries(path: &Path, extra: &[&str]) -> String {
    let out = Command::new(ffprobe())
        .args(["-v", "error"])
        .args(extra)
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .expect("failed to start ffprobe");
    assert!(
        out.status.success(),
        "ffprobe failed for {}:\n{}",
        path.display(),
        stderr_tail(&out)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Container duration in seconds (`format.duration`).
pub fn duration_sec(path: &Path) -> f64 {
    probe_entries(
        path,
        &["-show_entries", "format=duration", "-of", "csv=p=0"],
    )
    .trim()
    .parse()
    .unwrap_or_else(|_| panic!("no duration for {}", path.display()))
}

/// Stored (coded) width and height of the first video stream, before any display rotation.
pub fn video_size(path: &Path) -> (u32, u32) {
    let text = probe_entries(
        path,
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0:s=x",
        ],
    );
    let (w, h) = text
        .trim()
        .split_once('x')
        .unwrap_or_else(|| panic!("no video stream in {}", path.display()));
    (w.parse().expect("width"), h.parse().expect("height"))
}

/// Number of decodable video frames in the first video stream (`ffprobe -count_frames`).
pub fn frame_count(path: &Path) -> u64 {
    probe_entries(
        path,
        &[
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_read_frames",
            "-of",
            "csv=p=0",
        ],
    )
    .trim()
    .parse()
    .unwrap_or_else(|_| panic!("could not count frames of {}", path.display()))
}

/// Presentation times (seconds, sorted) of every video packet.
pub fn video_pts_times(path: &Path) -> Vec<f64> {
    let text = probe_entries(
        path,
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time",
            "-of",
            "csv=p=0",
        ],
    );
    let mut v: Vec<f64> = text.lines().filter_map(|l| l.trim().parse().ok()).collect();
    v.sort_by(f64::total_cmp);
    v
}

/// Presentation times (seconds, sorted) of the video keyframe packets.
pub fn keyframe_times(path: &Path) -> Vec<f64> {
    let text = probe_entries(
        path,
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time,flags",
            "-of",
            "csv=p=0",
        ],
    );
    let mut v: Vec<f64> = text
        .lines()
        .filter_map(|l| {
            let (t, flags) = l.trim().split_once(',')?;
            flags.starts_with('K').then(|| t.parse().ok()).flatten()
        })
        .collect();
    v.sort_by(f64::total_cmp);
    v
}

/// Per-packet hashes of one stream (`ffmpeg -i p -map <spec> -c copy -f framemd5 -`),
/// only the hash column, in packet order. Used for the AC-07 bit-identical check.
pub fn packet_hashes(path: &Path, stream_spec: &str) -> Vec<String> {
    let mut args: Vec<OsString> = os(&["-v", "error", "-i"]);
    args.push(path.as_os_str().to_owned());
    args.extend(os(&[
        "-map",
        stream_spec,
        "-c",
        "copy",
        "-f",
        "framemd5",
        "-",
    ]));
    let out = ffmpeg_ok(args);
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| l.rsplit(',').next().map(|h| h.trim().to_owned()))
        .collect()
}

/// Decodes the first video frame whose pts is at or after `at_sec` as raw 8-bit luma (Y plane, no range
/// conversion, no rotation) with its stored dimensions.
fn decode_luma_plane(path: &Path, at_sec: f64) -> (Vec<u8>, usize, usize) {
    let (w, h) = video_size(path);
    let mut args: Vec<OsString> = os(&[
        "-v",
        "error",
        "-noautorotate",
        "-ss",
        &format!("{at_sec:.4}"),
        "-i",
    ]);
    args.push(path.as_os_str().to_owned());
    args.extend(os(&[
        "-frames:v",
        "1",
        "-vf",
        "extractplanes=y",
        "-pix_fmt",
        "gray",
        "-f",
        "rawvideo",
        "-",
    ]));
    let out = ffmpeg_ok(args);
    let (w, h) = (w as usize, h as usize);
    assert!(
        out.stdout.len() >= w * h,
        "no frame decoded at {at_sec}s in {}",
        path.display()
    );
    (out.stdout, w, h)
}

/// Mean luma of the first frame with pts >= `at_sec` (aim just before a frame start for an
/// exact frame): the raw Y value (0-255, in the stream's own range). Uniform-luma fixtures such
/// as `Fixture::FrameNumbered` read back their value.
pub fn frame_luma(path: &Path, at_sec: f64) -> u8 {
    let (y, w, h) = decode_luma_plane(path, at_sec);
    let sum: u64 = y[..w * h].iter().map(|&b| u64::from(b)).sum();
    ((sum as f64 / (w * h) as f64).round()) as u8
}

/// Mean luma of the outermost column/row on each side of a frame (black-bar checks, AC-17).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeLuma {
    /// Mean of the leftmost pixel column.
    pub left: f64,
    /// Mean of the rightmost pixel column.
    pub right: f64,
    /// Mean of the top pixel row.
    pub top: f64,
    /// Mean of the bottom pixel row.
    pub bottom: f64,
}

impl EdgeLuma {
    /// The smallest of the four edge means.
    pub fn min(&self) -> f64 {
        self.left.min(self.right).min(self.top).min(self.bottom)
    }
}

/// Decodes the frame at `at_sec` (stored orientation) and returns the edge means.
pub fn edge_luma(path: &Path, at_sec: f64) -> EdgeLuma {
    let (y, w, h) = decode_luma_plane(path, at_sec);
    let col = |x: usize| (0..h).map(|r| f64::from(y[r * w + x])).sum::<f64>() / h as f64;
    let row = |r: usize| {
        y[r * w..(r + 1) * w]
            .iter()
            .map(|&b| f64::from(b))
            .sum::<f64>()
            / w as f64
    };
    EdgeLuma {
        left: col(0),
        right: col(w - 1),
        top: row(0),
        bottom: row(h - 1),
    }
}

/// Folders left in `<fixtures>/tmp` by earlier runs are removed once they are this old.
const STALE_TEMP_AGE: std::time::Duration = std::time::Duration::from_secs(3 * 60 * 60);

/// Creates a fresh, unique directory under `<fixtures>/tmp` and returns it; safe for parallel
/// tests. A directory is not deleted when its test ends (its files stay inspectable after a
/// failure). Instead the first call in a process removes `temp_dir` folders of earlier runs
/// older than three hours, so repeated runs do not fill the disk.
pub fn temp_dir() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    static SWEEP: std::sync::Once = std::sync::Once::new();
    let root = fixtures_dir().join("tmp");
    SWEEP.call_once(|| sweep_stale_temp_dirs(&root, STALE_TEMP_AGE));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = root.join(format!(
        "t{}-{}-{nanos}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("cannot create {}: {e}", dir.display()));
    dir
}

/// Whether `name` has the shape [`temp_dir`] gives its folders (`t<pid>-<n>-<nanos>`).
fn is_temp_dir_name(name: &str) -> bool {
    name.strip_prefix('t').is_some_and(|rest| {
        rest.split('-').count() == 3
            && rest
                .split('-')
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// Best-effort removal of [`temp_dir`] folders under `root` last modified more than `max_age` ago.
/// Other folders (and anything that cannot be read or removed) are left alone.
fn sweep_stale_temp_dirs(root: &Path, max_age: std::time::Duration) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !is_temp_dir_name(&name.to_string_lossy()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let stale = meta.is_dir()
            && meta
                .modified()
                .ok()
                .and_then(|m| now.duration_since(m).ok())
                .is_some_and(|age| age > max_age);
        if stale {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod temp_dir_tests {
    use super::*;

    #[test]
    fn temp_dir_names_are_recognised_and_nothing_else() {
        assert!(is_temp_dir_name("t9408-12-1794566945045523200"));
        assert!(!is_temp_dir_name("real_smoke"));
        assert!(!is_temp_dir_name("t9408-12"));
        assert!(!is_temp_dir_name("tmp-1-2"));
        assert!(!is_temp_dir_name("t1-2-3x"));
    }

    #[test]
    fn sweep_removes_only_old_temp_dirs() {
        let root = temp_dir();
        let fresh = root.join("t1-1-1");
        let other = root.join("keep_me");
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        // Nothing is older than an hour yet.
        sweep_stale_temp_dirs(&root, std::time::Duration::from_secs(3600));
        assert!(fresh.exists() && other.exists());
        // Everything is older than zero seconds: only the temp-dir-shaped folder goes.
        std::thread::sleep(std::time::Duration::from_millis(20));
        sweep_stale_temp_dirs(&root, std::time::Duration::ZERO);
        assert!(!fresh.exists(), "stale temp dir removed");
        assert!(other.exists(), "other folders are left alone");
        let _ = std::fs::remove_dir_all(&root);
    }
}
