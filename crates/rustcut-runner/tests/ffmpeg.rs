//! Integration tests against the real FFmpeg: EXP-08 (argument vectors, Unicode paths),
//! EXP-09 (progress on stdout, cancel), EXP-10 (stderr tail and log), PRD §7.8 / §8.2 (no
//! orphaned FFmpeg processes) and §7.9 (only absolute sidecar paths are spawned).
//!
//! FFmpeg is found with `Sidecars::locate()`. When it is missing, each test that needs it
//! prints a `SKIPPED` message and passes (set `RUSTCUT_FFMPEG_DIR` to run them).

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use rustcut_runner::{
    CancelToken, RunnerError, Sidecars, SpawnOptions, StaleProcess, StreamOptions,
    is_process_running, kill_stale_sidecar, probe_capabilities, run_capture, run_streaming,
    spawn_tracked,
};

/// Upper bound for any single run, so a regression fails instead of hanging the suite.
const TEST_TIMEOUT: Duration = Duration::from_secs(120);
const HELPER_ENV: &str = "RUSTCUT_RUNNER_ORPHAN_HELPER";
const PID_MARKER: &str = "RUSTCUT_ORPHAN_FFMPEG_PID=";

fn sidecars(test: &str) -> Option<Sidecars> {
    match Sidecars::locate() {
        Ok(sidecars) => Some(sidecars),
        Err(err) => {
            eprintln!("SKIPPED {test}: FFmpeg not found ({err})");
            None
        }
    }
}

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// A temporary folder whose path contains spaces and non-ASCII characters (EXP-08).
fn unicode_dir() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::Builder::new()
        .prefix("rustcut-runner-")
        .tempdir()
        .expect("temp dir");
    let dir = root.path().join("naive dir 測試 é");
    std::fs::create_dir_all(&dir).expect("create the Unicode folder");
    (root, dir)
}

/// A ~60 s real-time-paced run: cheap on CPU, long enough to be cancelled or orphaned.
fn long_args() -> Vec<OsString> {
    os(&[
        "-nostdin",
        "-hide_banner",
        "-progress",
        "pipe:1",
        "-nostats",
        "-re",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=160x120:rate=10",
        "-t",
        "60",
        "-f",
        "null",
        "-",
    ])
}

/// A ~60 s real-time-paced encode into `output` (a `.part` file, muxer set with `-f`, EXP-08).
fn long_encode_args(output: &Path) -> Vec<OsString> {
    let mut args = os(&[
        "-nostdin",
        "-hide_banner",
        "-y",
        "-progress",
        "pipe:1",
        "-nostats",
        "-re",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=160x120:rate=10",
        "-t",
        "60",
        "-c:v",
        "mpeg4",
        "-f",
        "mp4",
    ]);
    args.push(output.as_os_str().to_owned());
    args
}

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(TEST_TIMEOUT, future)
        .await
        .expect("the run did not finish in time")
}

fn wait_until_gone(pid: u32, limit: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if !is_process_running(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    !is_process_running(pid)
}

/// The FFmpeg lines of a runner log, without the runner's own `[rustcut]` lines.
fn ffmpeg_log_lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .expect("log file written")
        .lines()
        .filter(|line| !line.starts_with("[rustcut]"))
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn exp08_exp09_exp10_streaming_encode_with_unicode_paths() {
    let Some(sidecars) = sidecars("streaming encode") else {
        return;
    };
    let (_root, dir) = unicode_dir();
    let output = dir.join("clip é 測試.mp4");
    let log = dir.join("logs").join("clip é 測試.log");
    let mut args = os(&[
        "-nostdin",
        "-hide_banner",
        "-y",
        "-progress",
        "pipe:1",
        "-nostats",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=duration=3:size=320x240:rate=25",
        "-c:v",
        "mpeg4",
        "-q:v",
        "5",
        "-f",
        "mp4",
    ]);
    args.push(output.clone().into_os_string());

    let spawned = OnceLock::new();
    let mut lines_before_spawn = 0_usize;
    let mut stdout = Vec::new();
    let outcome = within(run_streaming(
        &sidecars.ffmpeg,
        &args,
        StreamOptions {
            log_file: Some(log.clone()),
            tail_lines: 20,
        },
        |pid| spawned.set(pid).expect("on_spawn is called once"),
        |line| {
            if spawned.get().is_none() {
                lines_before_spawn += 1;
            }
            stdout.push(line.to_owned());
        },
        CancelToken::new(),
    ))
    .await
    .expect("FFmpeg runs");

    assert!(outcome.success, "{outcome:?}");
    assert_eq!(outcome.exit_code, Some(0));
    assert!(outcome.progress_end, "progress=end is reported (PRD §7.8)");
    assert!(!outcome.cancelled);
    assert_eq!(
        spawned.get(),
        Some(&outcome.pid),
        "on_spawn receives the PID"
    );
    assert_eq!(lines_before_spawn, 0, "on_spawn runs before any output");
    assert!(
        stdout.iter().any(|line| line == "progress=end"),
        "{stdout:?}"
    );
    assert!(stdout.iter().any(|line| line.starts_with("out_time_us=")));
    assert!(!outcome.stderr_tail.is_empty() && outcome.stderr_tail.len() <= 20);
    assert_eq!(outcome.log_error, None);
    let logged = ffmpeg_log_lines(&log);
    assert!(
        logged.ends_with(&outcome.stderr_tail),
        "the tail is the end of the log"
    );
    assert!(
        logged.iter().any(|line| line.contains("clip é 測試.mp4")),
        "the Unicode output path reaches FFmpeg and the log unmodified: {logged:?}"
    );
    let size = std::fs::metadata(&output).expect("output written").len();
    assert!(size > 0);

    let mut probe = os(&[
        "-v",
        "error",
        "-show_entries",
        "format=duration",
        "-of",
        "default=noprint_wrappers=1:nokey=1",
    ]);
    probe.push(output.into_os_string());
    let probed = within(run_capture(
        &sidecars.ffprobe,
        &probe,
        Duration::from_secs(30),
    ))
    .await
    .expect("ffprobe runs");
    assert!(probed.success(), "{}", probed.stderr_lossy());
    let duration: f64 = probed.stdout_lossy().trim().parse().expect("a duration");
    assert!((duration - 3.0).abs() < 0.2, "duration {duration}");
}

#[tokio::test]
async fn exp10_large_stderr_and_stdout_never_deadlock_and_tail_is_bounded() {
    let Some(sidecars) = sidecars("large output") else {
        return;
    };
    let (_root, dir) = unicode_dir();
    let chatty = os(&[
        "-nostdin",
        "-hide_banner",
        "-v",
        "debug",
        "-debug_ts",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=duration=4:size=160x120:rate=25",
        "-f",
        "null",
        "-",
    ]);
    for tail_lines in [20, 5] {
        let log = dir.join(format!("debug {tail_lines}.log"));
        let outcome = within(run_streaming(
            &sidecars.ffmpeg,
            &chatty,
            StreamOptions {
                log_file: Some(log.clone()),
                tail_lines,
            },
            |_| {},
            |_| {},
            CancelToken::new(),
        ))
        .await
        .expect("FFmpeg runs");
        assert!(outcome.success, "{outcome:?}");
        let logged = ffmpeg_log_lines(&log);
        assert!(logged.len() > 100, "only {} stderr lines", logged.len());
        assert_eq!(outcome.stderr_tail.len(), tail_lines);
        assert!(logged.ends_with(&outcome.stderr_tail));
    }

    // About 1 MB of raw video on stdout, read concurrently with stderr and split into
    // bounded lines.
    let raw = os(&[
        "-nostdin",
        "-hide_banner",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=duration=2:size=160x120:rate=25",
        "-pix_fmt",
        "gray",
        "-f",
        "rawvideo",
        "pipe:1",
    ]);
    let mut chars = 0_usize;
    let mut longest = 0_usize;
    let outcome = within(run_streaming(
        &sidecars.ffmpeg,
        &raw,
        StreamOptions::default(),
        |_| {},
        |line| {
            let count = line.chars().count();
            chars += count;
            longest = longest.max(count);
        },
        CancelToken::new(),
    ))
    .await
    .expect("FFmpeg runs");
    assert!(outcome.success, "{outcome:?}");
    assert!(chars > 100_000, "only {chars} characters arrived");
    assert!(longest <= 64 * 1024, "line of {longest} characters");
}

#[tokio::test]
async fn exp09_cancel_kills_ffmpeg_within_2_seconds() {
    let Some(sidecars) = sidecars("cancel") else {
        return;
    };
    let (_root, dir) = unicode_dir();
    let part = dir.join("clip é 測試.mp4.part");
    let cancel = CancelToken::new();
    let spawned = OnceLock::new();
    let cancelled_at = OnceLock::new();
    let alive_at_cancel = OnceLock::new();
    let outcome = within(run_streaming(
        &sidecars.ffmpeg,
        &long_encode_args(&part),
        StreamOptions::default(),
        |pid| spawned.set(pid).expect("on_spawn is called once"),
        |line| {
            if line.starts_with("progress=") && cancelled_at.get().is_none() {
                if let Some(&pid) = spawned.get() {
                    let _ = alive_at_cancel.set(is_process_running(pid));
                }
                let _ = cancelled_at.set(Instant::now());
                cancel.cancel();
            }
        },
        cancel.clone(),
    ))
    .await
    .expect("FFmpeg runs");
    let since_cancel = cancelled_at
        .get()
        .expect("a progress block arrived")
        .elapsed();
    assert!(outcome.cancelled && !outcome.success, "{outcome:?}");
    assert!(
        !outcome.progress_end,
        "a killed encode never reports progress=end"
    );
    assert!(
        part.is_file(),
        "the runner leaves the .part to the caller, who deletes it (EXP-09)"
    );
    assert!(
        since_cancel < Duration::from_secs(2),
        "returned {since_cancel:?} after cancel"
    );
    assert_eq!(
        alive_at_cancel.get(),
        Some(&true),
        "FFmpeg was running when cancelled"
    );
    assert!(
        !is_process_running(outcome.pid),
        "the cancelled FFmpeg is gone"
    );
}

#[tokio::test]
async fn exp09_an_already_cancelled_token_kills_right_after_spawn() {
    let Some(sidecars) = sidecars("pre-cancelled") else {
        return;
    };
    let cancel = CancelToken::new();
    cancel.cancel();
    let started = Instant::now();
    let outcome = within(run_streaming(
        &sidecars.ffmpeg,
        &long_args(),
        StreamOptions::default(),
        |_| {},
        |_| {},
        cancel,
    ))
    .await
    .expect("FFmpeg runs");
    assert!(outcome.cancelled && !outcome.success, "{outcome:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(!is_process_running(outcome.pid));
}

#[tokio::test]
async fn run_capture_reports_exit_code_stdout_and_unicode_stderr() {
    let Some(sidecars) = sidecars("run_capture") else {
        return;
    };
    let version = within(run_capture(
        &sidecars.ffmpeg,
        &os(&["-nostdin", "-hide_banner", "-version"]),
        Duration::from_secs(30),
    ))
    .await
    .expect("FFmpeg runs");
    assert!(version.success());
    assert!(version.stdout_lossy().starts_with("ffmpeg version"));

    let (_root, dir) = unicode_dir();
    let mut args = os(&["-nostdin", "-hide_banner", "-i"]);
    args.push(dir.join("missing é 測試.mp4").into_os_string());
    args.extend(os(&["-f", "null", "-"]));
    let failed = within(run_capture(
        &sidecars.ffmpeg,
        &args,
        Duration::from_secs(30),
    ))
    .await
    .expect("a failing FFmpeg is not a runner error");
    assert!(!failed.success());
    assert!(failed.status_code.is_some());
    let stderr = failed.stderr_lossy();
    assert!(stderr.contains("missing é 測試.mp4"), "{stderr}");
}

#[tokio::test]
async fn run_capture_kills_the_process_on_timeout() {
    let Some(sidecars) = sidecars("run_capture timeout") else {
        return;
    };
    let started = Instant::now();
    let err = within(run_capture(
        &sidecars.ffmpeg,
        &long_args(),
        Duration::from_millis(700),
    ))
    .await
    .expect_err("times out");
    assert!(started.elapsed() < Duration::from_secs(10));
    let message = err.to_string();
    let RunnerError::Timeout { pid, timeout, .. } = err else {
        panic!("expected a timeout, got {err:?}");
    };
    assert_eq!(timeout, Duration::from_millis(700));
    assert!(message.contains("did not finish within 0.7 s"), "{message}");
    assert!(!is_process_running(pid), "the timed-out FFmpeg was killed");
}

#[tokio::test]
async fn prd_7_9_only_absolute_program_paths_are_spawned() {
    let err = spawn_tracked("ffmpeg", &[], &SpawnOptions::default()).expect_err("relative path");
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

    let missing = std::env::temp_dir()
        .join("rustcut-runner-no-such-dir")
        .join(format!("ffmpeg{}", std::env::consts::EXE_SUFFIX));
    let err = run_capture(&missing, &[], Duration::from_secs(5))
        .await
        .expect_err("missing program");
    assert!(
        matches!(&err, RunnerError::Spawn { source, .. } if source.kind() == io::ErrorKind::NotFound),
        "{err:?}"
    );
    let err = run_streaming(
        &missing,
        &[],
        StreamOptions::default(),
        |_| panic!("never spawned"),
        |_| {},
        CancelToken::new(),
    )
    .await
    .expect_err("missing program");
    assert!(matches!(err, RunnerError::Spawn { .. }), "{err:?}");
}

#[tokio::test]
async fn spawn_options_set_working_dir_and_environment() {
    let Some(sidecars) = sidecars("spawn options") else {
        return;
    };
    let (_root, dir) = unicode_dir();
    let options = SpawnOptions {
        current_dir: Some(dir.clone()),
        env: vec![("FFREPORT".into(), "file=report.log".into())],
    };
    let child = spawn_tracked(
        &sidecars.ffmpeg,
        &os(&["-nostdin", "-hide_banner", "-version"]),
        &options,
    )
    .expect("spawned");
    let output = within(child.wait_with_output()).await.expect("finished");
    assert!(output.status.success());
    assert!(
        dir.join("report.log").is_file(),
        "FFREPORT (environment) wrote into the working directory"
    );
}

#[tokio::test]
async fn capabilities_report_version_filters_and_encoders_and_are_cached() {
    let Some(sidecars) = sidecars("capabilities") else {
        return;
    };
    let capabilities = within(probe_capabilities(&sidecars)).await.expect("probed");
    assert!(!capabilities.ffmpeg_version.is_empty());
    assert!(capabilities.has_filter("scale") && capabilities.has_filter("nullsrc"));
    assert!(capabilities.has_encoder("mpeg4"));
    assert!(!capabilities.has_filter("no_such_filter"));
    assert!(!capabilities.has_encoder("no_such_encoder"));
    let again = within(probe_capabilities(&sidecars)).await.expect("cached");
    assert!(
        Arc::ptr_eq(&capabilities, &again),
        "the second call is served from the cache"
    );
}

#[tokio::test]
async fn prd_7_8_kill_stale_sidecar_kills_only_sidecars() {
    let Some(sidecars) = sidecars("kill_stale_sidecar") else {
        return;
    };
    let mut child =
        spawn_tracked(&sidecars.ffmpeg, &long_args(), &SpawnOptions::default()).expect("spawned");
    let pid = child.id().expect("running");
    let (_root, dir) = unicode_dir();
    let elsewhere = Sidecars {
        ffmpeg: dir.join("ffmpeg"),
        ffprobe: dir.join("ffprobe"),
    };
    assert_eq!(
        kill_stale_sidecar(pid, &elsewhere).expect("inspected"),
        StaleProcess::NotASidecar,
        "a PID running another program is left alone"
    );
    assert!(is_process_running(pid));
    assert_eq!(
        kill_stale_sidecar(pid, &sidecars).expect("killed"),
        StaleProcess::Killed
    );
    within(child.wait()).await.expect("reaped");
    assert!(wait_until_gone(pid, Duration::from_secs(3)));
    assert_eq!(
        kill_stale_sidecar(pid, &sidecars).expect("inspected"),
        StaleProcess::NotRunning
    );
}

/// Not a test on its own: `prd_8_2_ffmpeg_dies_with_its_parent_process` re-runs this test
/// binary with `RUSTCUT_RUNNER_ORPHAN_HELPER` set, and this function then plays the app: it
/// spawns a long FFmpeg through `spawn_tracked`, prints the PID and sleeps until it is killed.
#[test]
fn orphan_helper_process() {
    if std::env::var_os(HELPER_ENV).is_none() {
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let sidecars = Sidecars::locate().expect("FFmpeg located by the parent test");
        let child = spawn_tracked(&sidecars.ffmpeg, &long_args(), &SpawnOptions::default())
            .expect("spawned");
        println!("{PID_MARKER}{}", child.id().expect("running"));
        io::stdout().flush().expect("flushed");
        tokio::time::sleep(Duration::from_secs(90)).await;
        drop(child);
    });
}

/// PRD §7.8 / §8.2: killing the app (here a helper process, killed without any chance to clean
/// up) takes its FFmpeg down within 3 s: the Job Object on Windows, `PR_SET_PDEATHSIG` on
/// Linux. (macOS relies on journaling plus `kill_stale_sidecar`, tested above.)
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn prd_8_2_ffmpeg_dies_with_its_parent_process() {
    use rustcut_runner::FFMPEG_DIR_ENV;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    let Some(sidecars) = sidecars("orphan") else {
        return;
    };
    let mut helper = Command::new(std::env::current_exe().expect("test binary"));
    helper
        .args([
            "--exact",
            "orphan_helper_process",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HELPER_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(dir) = sidecars.ffmpeg.parent() {
        helper.env(FFMPEG_DIR_ENV, dir);
    }
    let mut helper = helper.spawn().expect("helper started");
    let stdout = helper.stdout.take().expect("piped stdout");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            // libtest may print "test orphan_helper_process ... " on the same line.
            if let Some((_, pid)) = line.split_once(PID_MARKER) {
                let _ = sender.send(pid.trim().parse::<u32>());
                return;
            }
        }
    });
    let pid = match receiver.recv_timeout(Duration::from_secs(60)) {
        Ok(Ok(pid)) => pid,
        other => {
            let _ = helper.kill();
            panic!("the helper did not report an FFmpeg PID: {other:?}");
        }
    };
    assert!(is_process_running(pid), "FFmpeg runs under the helper");

    helper.kill().expect("helper killed");
    helper.wait().expect("helper reaped");
    let gone = wait_until_gone(pid, Duration::from_secs(3));
    if !gone {
        let _ = kill_stale_sidecar(pid, &sidecars);
    }
    assert!(gone, "FFmpeg (PID {pid}) outlived its parent");
}
