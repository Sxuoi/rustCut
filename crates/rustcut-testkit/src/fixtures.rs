//! Synthetic golden-set fixtures (PRD section 11), generated on demand with FFmpeg lavfi.
//!
//! Generation is atomic (temp name, then rename), safe across threads and test processes
//! (a `<key>.lock` file created with `create_new`, with a stale-lock timeout) and cached
//! across runs: a sidecar `<key>.json` records [`FIXTURE_VERSION`] and a hash of the exact
//! generation arguments, and a mismatch regenerates the fixture.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::tools::{try_ffmpeg, try_ffprobe};

/// Bump to force every cached fixture to be regenerated.
pub const FIXTURE_VERSION: u32 = 1;

/// A lock older than this is considered abandoned (a crashed generator) and is removed.
const STALE_LOCK: Duration = Duration::from_secs(180);
/// How long a caller waits for another process generating the same fixture.
const LOCK_WAIT: Duration = Duration::from_secs(600);

/// The synthetic golden set (PRD section 11). Doc comments give the PRD media id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fixture {
    /// M01 short: 1920x1080 60 fps H.264 High + AAC stereo MP4, 2 s GOP, 12 s.
    Baseline1080p60,
    /// M02: 1280x720 30 fps H.264 + 4 AAC tracks (Mix/Game/Mic/Music) in MKV, 8 s.
    ObsMkv4Audio,
    /// M03: 640x360 30 fps H.264 MP4 with a 10 s GOP, 30 s.
    LongGop10s,
    /// M04: 640x360 H.264 + AAC MP4 with genuinely variable frame durations, 10 s.
    Vfr,
    /// M05: 640x360 HEVC Main10 HLG (BT.2020) rotated 90 degrees, 4 s, AAC.
    HevcHlgRotated,
    /// 1920x1080 (16:9) luma ramp, reference for the framing tests.
    Ramp16x9,
    /// M06: 1440x1080 (4:3) luma ramp.
    Ramp4x3,
    /// M07: 2560x1080 (21:9) luma ramp.
    Ramp21x9,
    /// M08: 1080x1920 (9:16) luma ramp.
    Ramp9x16,
    /// M09: 1080x2340 (taller than 9:16) luma ramp.
    RampTall,
    /// M10: 720x576 luma ramp with SAR 16:15.
    RampSar,
    /// M11: H.264 + AAC FLV, 6 s.
    Flv,
    /// M12: VP9 + Opus WebM, 6 s.
    WebmVp9Opus,
    /// M13: MP4 remuxed from MPEG-TS with `start_time` of about 10 s.
    StartTimeNonZero,
    /// M14: H.264 MP4 without any audio stream.
    NoAudio,
    /// M15: MP4 (index at the end) truncated to 60 % of its bytes.
    TruncatedMp4,
    /// M16: 640x360 144 fps AV1 MP4, 3 s.
    Av1HighFps,
    /// M17: MOV with H.264 + pcm_s16le, 6 s.
    PcmAudio,
    /// M18: 4 s clip at `.fixtures/M18 ruang kerja n 測試/clip with spaces e.mp4` (with accents).
    UnicodePath,
    /// AC-08: 320x240 30 fps, 2 s GOP, 20 s; frame N has uniform luma `(N*7 mod 200) + 30`.
    FrameNumbered,
    /// Negative test: AAC in M4A, 5 s, no video.
    AudioOnly,
}

impl Fixture {
    /// Every fixture, in PRD order.
    pub const ALL: [Fixture; 21] = [
        Fixture::Baseline1080p60,
        Fixture::ObsMkv4Audio,
        Fixture::LongGop10s,
        Fixture::Vfr,
        Fixture::HevcHlgRotated,
        Fixture::Ramp16x9,
        Fixture::Ramp4x3,
        Fixture::Ramp21x9,
        Fixture::Ramp9x16,
        Fixture::RampTall,
        Fixture::RampSar,
        Fixture::Flv,
        Fixture::WebmVp9Opus,
        Fixture::StartTimeNonZero,
        Fixture::NoAudio,
        Fixture::TruncatedMp4,
        Fixture::Av1HighFps,
        Fixture::PcmAudio,
        Fixture::UnicodePath,
        Fixture::FrameNumbered,
        Fixture::AudioOnly,
    ];

    /// Stable short key, also the cache/lock file stem (for example `M05_hevc_hlg_rotated`).
    pub fn key(self) -> &'static str {
        self.spec_head().0
    }

    /// File name relative to [`fixtures_dir`].
    fn rel_path(self) -> PathBuf {
        match self {
            Fixture::UnicodePath => {
                Path::new("M18 ruang kerja ñ 測試").join("clip with spaces é.mp4")
            }
            _ => PathBuf::from(format!("{}.{}", self.key(), self.spec_head().1)),
        }
    }

    /// `(key, extension)`.
    fn spec_head(self) -> (&'static str, &'static str) {
        match self {
            Fixture::Baseline1080p60 => ("M01_baseline_1080p60", "mp4"),
            Fixture::ObsMkv4Audio => ("M02_obs_mkv_4audio", "mkv"),
            Fixture::LongGop10s => ("M03_long_gop_10s", "mp4"),
            Fixture::Vfr => ("M04_vfr", "mp4"),
            Fixture::HevcHlgRotated => ("M05_hevc_hlg_rotated", "mp4"),
            Fixture::Ramp16x9 => ("R16x9_ramp_1920x1080", "mp4"),
            Fixture::Ramp4x3 => ("M06_ramp_1440x1080", "mp4"),
            Fixture::Ramp21x9 => ("M07_ramp_2560x1080", "mp4"),
            Fixture::Ramp9x16 => ("M08_ramp_1080x1920", "mp4"),
            Fixture::RampTall => ("M09_ramp_1080x2340", "mp4"),
            Fixture::RampSar => ("M10_ramp_720x576_sar", "mp4"),
            Fixture::Flv => ("M11_flv", "flv"),
            Fixture::WebmVp9Opus => ("M12_webm_vp9_opus", "webm"),
            Fixture::StartTimeNonZero => ("M13_start_time_nonzero", "mp4"),
            Fixture::NoAudio => ("M14_no_audio", "mp4"),
            Fixture::TruncatedMp4 => ("M15_truncated", "mp4"),
            Fixture::Av1HighFps => ("M16_av1_144fps", "mp4"),
            Fixture::PcmAudio => ("M17_pcm_audio", "mov"),
            Fixture::UnicodePath => ("M18_unicode_path", "mp4"),
            Fixture::FrameNumbered => ("AC08_frame_numbered", "mp4"),
            Fixture::AudioOnly => ("NEG_audio_only", "m4a"),
        }
    }
}

/// Why a fixture could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    /// FFmpeg or FFprobe is not installed where the testkit looks.
    #[error("ffmpeg/ffprobe not found: put them on PATH or set RUSTCUT_FFMPEG_DIR")]
    ToolsMissing,
    /// A filesystem operation failed.
    #[error("{context}: {source}")]
    Io {
        /// What was being done.
        context: String,
        /// The OS error.
        source: std::io::Error,
    },
    /// An FFmpeg generation step exited with an error.
    #[error("generating fixture {key} failed at step {step}: {stderr}")]
    Ffmpeg {
        /// Fixture key.
        key: &'static str,
        /// Zero-based step index.
        step: usize,
        /// Tail of FFmpeg's stderr.
        stderr: String,
    },
    /// Another process held the generation lock for too long.
    #[error("timed out waiting for another process to generate fixture {0}")]
    LockTimeout(&'static str),
}

fn io_err(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> FixtureError {
    let context = context.into();
    move |source| FixtureError::Io { context, source }
}

/// Directory holding generated fixtures: `RUSTCUT_FIXTURES_DIR`, else `<workspace>/.fixtures`.
pub fn fixtures_dir() -> PathBuf {
    match std::env::var_os("RUSTCUT_FIXTURES_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join(".fixtures"),
    }
}

static GENERATIONS: Mutex<Vec<(&'static str, usize)>> = Mutex::new(Vec::new());

/// How many times this process has generated `f` (used to prove the concurrency guarantee).
pub fn generation_count(f: Fixture) -> usize {
    let g = GENERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    g.iter().find(|(k, _)| *k == f.key()).map_or(0, |(_, n)| *n)
}

fn bump_generation(f: Fixture) {
    let mut g = GENERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    match g.iter_mut().find(|(k, _)| *k == f.key()) {
        Some((_, n)) => *n += 1,
        None => g.push((f.key(), 1)),
    }
}

// ---------------------------------------------------------------------------------------
// Generation recipes
// ---------------------------------------------------------------------------------------

/// What to do after the FFmpeg steps.
#[derive(Debug, Clone, Copy)]
enum Post {
    /// The last step wrote `{out}` itself.
    None,
    /// Copy `{mid}` to `{out}`, keeping only this percentage of its bytes (M15).
    Truncate(u64),
}

#[derive(Debug)]
struct Recipe {
    /// Extension of the intermediate file `{mid}`.
    mid_ext: &'static str,
    /// FFmpeg argument lists; `{mid}` and `{out}` are substituted with paths.
    steps: Vec<Vec<String>>,
    post: Post,
}

fn s(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_owned()).collect()
}

fn lavfi_video(size: &str, rate: u32, secs: u32) -> Vec<String> {
    s(&[
        "-f",
        "lavfi",
        "-i",
        &format!("testsrc2=size={size}:rate={rate}:duration={secs}"),
    ])
}

fn lavfi_sine(freq: u32, secs: u32) -> Vec<String> {
    s(&[
        "-f",
        "lavfi",
        "-i",
        &format!("sine=frequency={freq}:sample_rate=48000:duration={secs}"),
    ])
}

/// Cheap H.264 output args.
fn x264_fast(extra: &[&str]) -> Vec<String> {
    let mut v = s(&[
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "28",
        "-pix_fmt",
        "yuv420p",
    ]);
    v.extend(s(extra));
    v
}

fn aac(extra: &[&str]) -> Vec<String> {
    let mut v = s(&["-c:a", "aac", "-b:a", "96k"]);
    v.extend(s(extra));
    v
}

fn cat(parts: Vec<Vec<String>>) -> Vec<String> {
    parts.into_iter().flatten().collect()
}

fn single(steps: Vec<String>) -> Recipe {
    Recipe {
        mid_ext: "mp4",
        steps: vec![steps],
        post: Post::None,
    }
}

/// 2 s, 30 fps luma ramp `20 + 210 * X / W` (min luma well above 16), H.264 + AAC.
fn ramp(w: u32, h: u32, sar: &str) -> Recipe {
    let src = format!(
        "nullsrc=size={w}x{h}:rate=30:duration=2,format=yuv420p,geq=lum='20+X*210/W':cb=128:cr=128,setsar={sar}"
    );
    single(cat(vec![
        s(&["-f", "lavfi", "-i", &src]),
        lavfi_sine(440, 2),
        x264_fast(&["-crf", "18"]),
        aac(&["-ac", "2", "-shortest", "-movflags", "+faststart", "{out}"]),
    ]))
}

fn recipe(f: Fixture) -> Recipe {
    match f {
        Fixture::Baseline1080p60 => single(cat(vec![
            lavfi_video("1920x1080", 60, 12),
            lavfi_sine(440, 12),
            s(&[
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "30",
                "-profile:v",
                "high",
                "-pix_fmt",
                "yuv420p",
            ]),
            s(&["-g", "120", "-keyint_min", "120", "-sc_threshold", "0"]),
            aac(&["-ac", "2", "-shortest", "-movflags", "+faststart", "{out}"]),
        ])),
        Fixture::ObsMkv4Audio => single(cat(vec![
            lavfi_video("1280x720", 30, 8),
            lavfi_sine(220, 8),
            lavfi_sine(440, 8),
            lavfi_sine(660, 8),
            lavfi_sine(880, 8),
            s(&[
                "-map", "0:v:0", "-map", "1:a", "-map", "2:a", "-map", "3:a", "-map", "4:a",
            ]),
            x264_fast(&[]),
            aac(&["-b:a", "64k"]),
            s(&[
                "-metadata:s:a:0",
                "title=Mix",
                "-metadata:s:a:1",
                "title=Game",
            ]),
            s(&[
                "-metadata:s:a:2",
                "title=Mic",
                "-metadata:s:a:3",
                "title=Music",
                "{out}",
            ]),
        ])),
        Fixture::LongGop10s => single(cat(vec![
            lavfi_video("640x360", 30, 30),
            lavfi_sine(440, 30),
            x264_fast(&["-g", "300", "-keyint_min", "300", "-sc_threshold", "0"]),
            aac(&["-shortest", "{out}"]),
        ])),
        Fixture::Vfr => single(cat(vec![
            lavfi_video("640x360", 30, 10),
            lavfi_sine(440, 10),
            s(&[
                "-vf",
                "select='lt(mod(n,7),4)+eq(mod(n,23),0)'",
                "-fps_mode:v",
                "vfr",
            ]),
            x264_fast(&[]),
            aac(&["-shortest", "{out}"]),
        ])),
        Fixture::HevcHlgRotated => Recipe {
            mid_ext: "mp4",
            steps: vec![
                cat(vec![
                    lavfi_video("640x360", 30, 4),
                    lavfi_sine(440, 4),
                    s(&[
                        "-vf",
                        "format=yuv420p10le",
                        "-c:v",
                        "libx265",
                        "-preset",
                        "ultrafast",
                        "-pix_fmt",
                        "yuv420p10le",
                    ]),
                    s(&[
                        "-x265-params",
                        "log-level=error:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc",
                    ]),
                    s(&[
                        "-color_primaries",
                        "bt2020",
                        "-color_trc",
                        "arib-std-b67",
                        "-colorspace",
                        "bt2020nc",
                    ]),
                    s(&["-tag:v", "hvc1"]),
                    aac(&["-shortest", "{mid}"]),
                ]),
                s(&[
                    "-display_rotation",
                    "90",
                    "-i",
                    "{mid}",
                    "-c",
                    "copy",
                    "-tag:v",
                    "hvc1",
                    "{out}",
                ]),
            ],
            post: Post::None,
        },
        Fixture::Ramp16x9 => ramp(1920, 1080, "1"),
        Fixture::Ramp4x3 => ramp(1440, 1080, "1"),
        Fixture::Ramp21x9 => ramp(2560, 1080, "1"),
        Fixture::Ramp9x16 => ramp(1080, 1920, "1"),
        Fixture::RampTall => ramp(1080, 2340, "1"),
        Fixture::RampSar => ramp(720, 576, "16/15"),
        Fixture::Flv => single(cat(vec![
            lavfi_video("640x360", 30, 6),
            lavfi_sine(440, 6),
            x264_fast(&[]),
            aac(&["-shortest", "-f", "flv", "{out}"]),
        ])),
        Fixture::WebmVp9Opus => single(cat(vec![
            lavfi_video("640x360", 30, 6),
            lavfi_sine(440, 6),
            s(&[
                "-c:v",
                "libvpx-vp9",
                "-deadline",
                "realtime",
                "-cpu-used",
                "8",
                "-b:v",
                "400k",
                "-pix_fmt",
                "yuv420p",
            ]),
            s(&[
                "-c:a",
                "libopus",
                "-b:a",
                "64k",
                "-shortest",
                "-f",
                "webm",
                "{out}",
            ]),
        ])),
        Fixture::StartTimeNonZero => Recipe {
            mid_ext: "ts",
            steps: vec![
                cat(vec![
                    lavfi_video("640x360", 30, 4),
                    lavfi_sine(440, 4),
                    x264_fast(&[]),
                    aac(&["-shortest", "-f", "mpegts", "{mid}"]),
                ]),
                // The MP4 muxer normalises a plain remux to 0; shifting the output timestamps
                // keeps a genuine 10 s time origin in the movie header.
                s(&[
                    "-i",
                    "{mid}",
                    "-c",
                    "copy",
                    "-output_ts_offset",
                    "10",
                    "-avoid_negative_ts",
                    "disabled",
                    "{out}",
                ]),
            ],
            post: Post::None,
        },
        Fixture::NoAudio => single(cat(vec![
            lavfi_video("640x360", 30, 4),
            x264_fast(&[]),
            s(&["-an", "{out}"]),
        ])),
        Fixture::TruncatedMp4 => Recipe {
            mid_ext: "mp4",
            // No +faststart: the moov index is written last, so cutting the tail removes it.
            steps: vec![cat(vec![
                lavfi_video("640x360", 30, 10),
                lavfi_sine(440, 10),
                x264_fast(&[]),
                aac(&["-shortest", "{mid}"]),
            ])],
            post: Post::Truncate(60),
        },
        Fixture::Av1HighFps => single(cat(vec![
            lavfi_video("640x360", 144, 3),
            s(&[
                "-c:v",
                "libsvtav1",
                "-preset",
                "12",
                "-crf",
                "40",
                "-g",
                "144",
                "-pix_fmt",
                "yuv420p",
                "-an",
                "{out}",
            ]),
        ])),
        Fixture::PcmAudio => single(cat(vec![
            lavfi_video("640x360", 30, 6),
            lavfi_sine(440, 6),
            x264_fast(&[]),
            s(&["-c:a", "pcm_s16le", "-shortest", "-f", "mov", "{out}"]),
        ])),
        Fixture::UnicodePath => single(cat(vec![
            lavfi_video("640x360", 30, 4),
            lavfi_sine(440, 4),
            x264_fast(&[]),
            aac(&["-shortest", "-movflags", "+faststart", "{out}"]),
        ])),
        Fixture::FrameNumbered => single(cat(vec![
            s(&[
                "-f",
                "lavfi",
                "-i",
                "nullsrc=size=320x240:rate=30:duration=20,format=yuv420p,geq=lum='mod(N*7,200)+30':cb=128:cr=128",
            ]),
            lavfi_sine(1000, 20),
            x264_fast(&[
                "-crf",
                "12",
                "-g",
                "60",
                "-keyint_min",
                "60",
                "-sc_threshold",
                "0",
            ]),
            aac(&["-shortest", "-movflags", "+faststart", "{out}"]),
        ])),
        Fixture::AudioOnly => single(cat(vec![lavfi_sine(440, 5), aac(&["-f", "ipod", "{out}"])])),
    }
}

/// FNV-1a 64: stable across Rust versions (unlike `DefaultHasher`).
fn fnv1a(data: &str) -> u64 {
    data.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Hash of everything that determines the generated bytes (recipe, key, relative path).
fn args_hash(f: Fixture, r: &Recipe) -> String {
    format!(
        "{:016x}",
        fnv1a(&format!("{}|{}|{:?}", f.key(), f.rel_path().display(), r))
    )
}

fn sidecar_path(f: Fixture) -> PathBuf {
    fixtures_dir().join(format!("{}.json", f.key()))
}

fn lock_path(f: Fixture) -> PathBuf {
    fixtures_dir().join(format!("{}.lock", f.key()))
}

/// Whether the cached fixture exists and matches the current version and arguments.
fn is_current(f: Fixture, hash: &str) -> bool {
    let out = fixtures_dir().join(f.rel_path());
    if !out.is_file() {
        return false;
    }
    let Ok(text) = fs::read_to_string(sidecar_path(f)) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    v["version"] == FIXTURE_VERSION && v["args_hash"] == hash
}

/// RAII lock file; removed on drop.
struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Takes the cross-process generation lock, or returns `Ok(None)` if the fixture became
/// current while waiting (another generator finished).
fn acquire(f: Fixture, hash: &str) -> Result<Option<Lock>, FixtureError> {
    let path = lock_path(f);
    let start = Instant::now();
    loop {
        match File::create_new(&path) {
            Ok(mut file) => {
                let _ = writeln!(file, "pid {}", std::process::id());
                return Ok(Some(Lock(path)));
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                let age = fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok());
                if age.is_some_and(|a| a > STALE_LOCK) {
                    let _ = fs::remove_file(&path);
                    continue;
                }
                if is_current(f, hash) {
                    return Ok(None);
                }
                if start.elapsed() > LOCK_WAIT {
                    return Err(FixtureError::LockTimeout(f.key()));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(io_err(format!("creating lock {}", path.display()))(e)),
        }
    }
}

fn run_step(f: Fixture, index: usize, args: &[OsString]) -> Result<(), FixtureError> {
    let ffmpeg = try_ffmpeg().ok_or(FixtureError::ToolsMissing)?;
    let out = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(io_err("starting ffmpeg"))?;
    if out.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = text.lines().collect();
    Err(FixtureError::Ffmpeg {
        key: f.key(),
        step: index,
        stderr: lines[lines.len().saturating_sub(8)..].join(" | "),
    })
}

/// Generates the fixture into a unique temp name and renames it into place.
fn generate(f: Fixture, r: &Recipe, hash: &str) -> Result<(), FixtureError> {
    static UNIQ: AtomicUsize = AtomicUsize::new(0);
    let final_path = fixtures_dir().join(f.rel_path());
    let dir = final_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(fixtures_dir);
    fs::create_dir_all(&dir).map_err(io_err(format!("creating {}", dir.display())))?;
    let uniq = format!(
        "{}-{}",
        std::process::id(),
        UNIQ.fetch_add(1, Ordering::Relaxed)
    );
    let ext = f.spec_head().1;
    let out_tmp = dir.join(format!("{}.tmp-{uniq}.{ext}", f.key()));
    let mid_tmp = dir.join(format!("{}.mid-{uniq}.{}", f.key(), r.mid_ext));
    let cleanup = || {
        let _ = fs::remove_file(&out_tmp);
        let _ = fs::remove_file(&mid_tmp);
    };

    let _ = fs::remove_file(sidecar_path(f));
    let result = (|| {
        for (i, step) in r.steps.iter().enumerate() {
            let args: Vec<OsString> = step
                .iter()
                .map(|a| match a.as_str() {
                    "{out}" => out_tmp.clone().into_os_string(),
                    "{mid}" => mid_tmp.clone().into_os_string(),
                    other => OsString::from(other),
                })
                .collect();
            run_step(f, i, &args)?;
        }
        if let Post::Truncate(percent) = r.post {
            let bytes = fs::read(&mid_tmp).map_err(io_err("reading intermediate file"))?;
            let keep = bytes.len() as u64 * percent / 100;
            fs::write(&out_tmp, &bytes[..keep as usize])
                .map_err(io_err("writing truncated file"))?;
        }
        fs::rename(&out_tmp, &final_path)
            .map_err(io_err(format!("renaming into {}", final_path.display())))?;
        let sidecar =
            serde_json::json!({ "version": FIXTURE_VERSION, "args_hash": hash, "key": f.key() });
        fs::write(sidecar_path(f), sidecar.to_string()).map_err(io_err("writing fixture sidecar"))
    })();
    cleanup();
    result?;
    bump_generation(f);
    Ok(())
}

/// Returns the path of fixture `f`, generating it first if it is missing or stale.
///
/// Safe to call concurrently from many threads and processes: exactly one generates, the
/// others wait and reuse the result.
pub fn try_fixture(f: Fixture) -> Result<PathBuf, FixtureError> {
    if try_ffmpeg().is_none() || try_ffprobe().is_none() {
        return Err(FixtureError::ToolsMissing);
    }
    let recipe = recipe(f);
    let hash = args_hash(f, &recipe);
    let path = fixtures_dir().join(f.rel_path());
    if is_current(f, &hash) {
        return Ok(path);
    }
    fs::create_dir_all(fixtures_dir()).map_err(io_err("creating the fixtures directory"))?;
    let Some(_lock) = acquire(f, &hash)? else {
        return Ok(path);
    };
    if !is_current(f, &hash) {
        generate(f, &recipe, &hash)?;
    }
    Ok(path)
}

/// Like [`try_fixture`] but panics with the error (the usual call in tests).
pub fn fixture(f: Fixture) -> PathBuf {
    try_fixture(f).unwrap_or_else(|e| panic!("fixture {f:?}: {e}"))
}
