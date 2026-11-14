//! Running a sidecar to completion: [`run_capture`] for short queries (ffprobe) and
//! [`run_streaming`] for exports (EXP-08, EXP-09, EXP-10, PRD §7.8).

use std::borrow::Cow;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Child;
use tokio::time::Instant;

use crate::error::describe_exit;
use crate::spawn::{SpawnOptions, spawn_tracked};
use crate::{CancelToken, RunnerError};

/// Number of trailing stderr lines kept with a failure (EXP-10, PRD §7.8).
pub const DEFAULT_TAIL_LINES: usize = 20;

/// How long output is still read after the process has exited. Only a grandchild holding the
/// pipes open could make this matter; FFmpeg has none.
const DRAIN_GRACE: Duration = Duration::from_secs(1);

/// Longer lines are split, so a stream without line breaks cannot grow memory without bound.
const MAX_LINE_BYTES: usize = 64 * 1024;

const READ_CHUNK_BYTES: usize = 8 * 1024;
const LOG_BUFFER_BYTES: usize = 64 * 1024;

/// Prefix of the lines the runner itself writes into a log file.
const LOG_MARKER: &str = "[rustcut]";

/// The line FFmpeg's `-progress` output ends with once the job completed (EXP-09).
const PROGRESS_END: &str = "progress=end";

/// Everything a process run by [`run_capture`] printed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapturedOutput {
    /// Exit code; `None` if the process was ended by a signal (Unix).
    pub status_code: Option<i32>,
    /// Raw stdout bytes (ffprobe's JSON and CSV output is UTF-8).
    pub stdout: Vec<u8>,
    /// Raw stderr bytes.
    pub stderr: Vec<u8>,
}

impl CapturedOutput {
    /// `true` when the process exited with code 0.
    pub fn success(&self) -> bool {
        self.status_code == Some(0)
    }

    /// stdout as UTF-8, invalid sequences replaced by U+FFFD.
    pub fn stdout_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.stdout)
    }

    /// stderr as UTF-8, invalid sequences replaced by U+FFFD.
    pub fn stderr_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.stderr)
    }
}

/// Options for [`run_streaming`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamOptions {
    /// Per-clip log (PRD §7.8): every stderr line is appended to this file, framed by
    /// `[rustcut]` lines naming the command and how it ended. Parent folders are created.
    /// `None` disables the log. Logging problems never fail a run; see
    /// [`RunOutcome::log_error`].
    pub log_file: Option<PathBuf>,
    /// How many trailing stderr lines [`RunOutcome::stderr_tail`] keeps (default
    /// [`DEFAULT_TAIL_LINES`], i.e. 20, EXP-10).
    pub tail_lines: usize,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            log_file: None,
            tail_lines: DEFAULT_TAIL_LINES,
        }
    }
}

/// How a process run by [`run_streaming`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    /// Exit code; `None` if the process was ended by a signal (Unix, e.g. after cancellation).
    pub exit_code: Option<i32>,
    /// The process exited with code 0 and was not cancelled. (A clip additionally needs
    /// [`progress_end`](Self::progress_end) and a verified output, PRD §7.8.)
    pub success: bool,
    /// stdout contained the line `progress=end`, FFmpeg's `-progress` end marker (EXP-09,
    /// PRD §7.8 success criterion). Always `false` without `-progress pipe:1`.
    pub progress_end: bool,
    /// The [`CancelToken`] fired while the process was running, and the process was killed
    /// (EXP-09).
    pub cancelled: bool,
    /// The last [`StreamOptions::tail_lines`] non-empty stderr lines, oldest first (EXP-10).
    pub stderr_tail: Vec<String>,
    /// OS process ID (also passed to `on_spawn`).
    pub pid: u32,
    /// Wall-clock time from spawn to exit.
    pub elapsed: Duration,
    /// Why the log file is missing or incomplete, if it is.
    pub log_error: Option<String>,
}

/// Runs `program` to completion and returns everything it printed (ffprobe, `ffmpeg -version`).
///
/// stdout and stderr are read concurrently, so a chatty process can never stall on a full pipe.
/// A non-zero exit is not an error: check [`CapturedOutput::success`].
///
/// # Errors
/// * [`RunnerError::Timeout`] when `timeout` elapses first; the process is killed and reaped.
/// * [`RunnerError::Spawn`] when it cannot be started, [`RunnerError::Io`] when its output
///   cannot be read.
pub async fn run_capture(
    program: impl AsRef<Path>,
    args: &[OsString],
    timeout: Duration,
) -> Result<CapturedOutput, RunnerError> {
    let program = program.as_ref();
    let mut child = spawn(program, args)?;
    let pid = pid_of(&child, program)?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let finished = tokio::time::timeout(timeout, async {
        let (out, err, status) = tokio::join!(
            read_all(stdout.as_mut()),
            read_all(stderr.as_mut()),
            child.wait()
        );
        Ok::<_, io::Error>((out?, err?, status?))
    })
    .await;
    match finished {
        Ok(Ok((stdout, stderr, status))) => Ok(CapturedOutput {
            status_code: status.code(),
            stdout,
            stderr,
        }),
        Ok(Err(source)) => {
            kill_and_reap(&mut child, pid).await;
            Err(RunnerError::Io {
                program: program.to_path_buf(),
                source,
            })
        }
        Err(_elapsed) => {
            kill_and_reap(&mut child, pid).await;
            Err(RunnerError::Timeout {
                program: program.to_path_buf(),
                timeout,
                pid,
            })
        }
    }
}

/// Runs a long FFmpeg job (an export, EXP-08..10), streaming its output.
///
/// * `on_spawn` is called exactly once with the PID, right after the process starts and before
///   any of its output is read: journal it there (PRD §7.8).
/// * `on_stdout_line` receives every non-empty stdout line, split on `\n` **and** `\r` (FFmpeg
///   rewrites status lines with `\r`); invalid UTF-8 is replaced by U+FFFD and lines longer
///   than 64 KiB are split. The caller parses `-progress pipe:1` from it (EXP-09).
/// * stderr is split the same way; every line is appended to [`StreamOptions::log_file`] and
///   the last [`StreamOptions::tail_lines`] are returned in [`RunOutcome::stderr_tail`]
///   (EXP-10). stdout and stderr are read concurrently, so neither can fill up and stall FFmpeg.
/// * When `cancel` fires, the process is killed at once and reaped before this returns
///   ([`RunOutcome::cancelled`], EXP-09); a token cancelled before the call kills the process
///   right after it starts. Deleting the `.part` file is the caller's job.
///
/// Both callbacks run on the task that polls this future; keep them quick (parse and forward,
/// e.g. to a channel), because output is not read while they run.
///
/// A failing FFmpeg is not an error: inspect [`RunOutcome::success`] and
/// [`RunOutcome::exit_code`].
///
/// # Errors
/// [`RunnerError::Spawn`] when the process cannot be started, [`RunnerError::Io`] when waiting
/// for it fails.
pub async fn run_streaming<S, L>(
    program: impl AsRef<Path>,
    args: &[OsString],
    options: StreamOptions,
    on_spawn: S,
    mut on_stdout_line: L,
    cancel: CancelToken,
) -> Result<RunOutcome, RunnerError>
where
    S: FnOnce(u32),
    L: FnMut(&str) + Send,
{
    let program = program.as_ref();
    let mut log = LogSink::open(options.log_file.as_deref());
    log.marker(&format!("run: {}", display_command(program, args)));
    let mut child = match spawn(program, args) {
        Ok(child) => child,
        Err(err) => {
            log.marker(&err.to_string());
            log.finish();
            return Err(err);
        }
    };
    let started = Instant::now();
    let pid = pid_of(&child, program)?;
    on_spawn(pid);
    let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(RunnerError::Io {
            program: program.to_path_buf(),
            source: io::Error::other("the output pipes of the process are missing"),
        });
    };

    let mut tail = Tail::new(options.tail_lines);
    let mut cancelled = false;
    let mut progress_end = false;
    let status: ExitStatus = {
        let pumps = async {
            tokio::join!(
                pump_lines(&mut stdout, |line| {
                    progress_end |= line == PROGRESS_END;
                    on_stdout_line(line);
                }),
                pump_lines(&mut stderr, |line| {
                    tail.push(line);
                    log.line(line);
                }),
            )
        };
        let mut pumps = std::pin::pin!(pumps);
        let mut pumps_done = false;
        let mut exited: Option<ExitStatus> = None;
        let mut drain_deadline: Option<Instant> = None;
        loop {
            let mut drain_expired = false;
            tokio::select! {
                biased;
                () = cancel.cancelled(), if !cancelled && exited.is_none() => {
                    cancelled = true;
                    if let Err(err) = child.start_kill() {
                        tracing::debug!(pid, %err, "killing a cancelled sidecar failed");
                    }
                }
                status = child.wait(), if exited.is_none() => {
                    exited = Some(status.map_err(|source| RunnerError::Io {
                        program: program.to_path_buf(),
                        source,
                    })?);
                    drain_deadline = Some(Instant::now() + DRAIN_GRACE);
                }
                (out, err) = &mut pumps, if !pumps_done => {
                    pumps_done = true;
                    for (stream, result) in [("stdout", out), ("stderr", err)] {
                        if let Err(err) = result {
                            tracing::warn!(pid, stream, %err, "reading sidecar output failed");
                        }
                    }
                }
                () = sleep_until(drain_deadline), if !pumps_done && drain_deadline.is_some() => {
                    tracing::warn!(pid, "sidecar output stayed open after it exited; stopped reading");
                    drain_expired = true;
                }
            }
            if let Some(status) = exited
                && (pumps_done || drain_expired)
            {
                break status;
            }
        }
    };

    let elapsed = started.elapsed();
    let exit_code = status.code();
    let summary = if cancelled {
        format!(
            "cancelled: process killed after {:.2} s",
            elapsed.as_secs_f64()
        )
    } else {
        format!(
            "{} after {:.2} s",
            describe_exit(&exit_code),
            elapsed.as_secs_f64()
        )
    };
    log.marker(&summary);
    Ok(RunOutcome {
        exit_code,
        success: status.success() && !cancelled,
        progress_end,
        cancelled,
        stderr_tail: tail.into_vec(),
        pid,
        elapsed,
        log_error: log.finish(),
    })
}

fn spawn(program: &Path, args: &[OsString]) -> Result<Child, RunnerError> {
    spawn_tracked(program, args, &SpawnOptions::default()).map_err(|source| RunnerError::Spawn {
        program: program.to_path_buf(),
        source,
    })
}

fn pid_of(child: &Child, program: &Path) -> Result<u32, RunnerError> {
    child.id().ok_or_else(|| RunnerError::Spawn {
        program: program.to_path_buf(),
        source: io::Error::other("the process exited before its ID could be read"),
    })
}

async fn kill_and_reap(child: &mut Child, pid: u32) {
    if let Err(err) = child.kill().await {
        tracing::debug!(pid, %err, "killing a sidecar failed (it may have exited already)");
    }
}

async fn read_all<R: AsyncRead + Unpin>(reader: Option<&mut R>) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(reader) = reader {
        reader.read_to_end(&mut bytes).await?;
    }
    Ok(bytes)
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Reads `reader` to its end, handing every line to `on_line`.
async fn pump_lines<R: AsyncRead + Unpin>(
    reader: &mut R,
    mut on_line: impl FnMut(&str),
) -> io::Result<()> {
    let mut splitter = LineSplitter::default();
    let mut chunk = vec![0_u8; READ_CHUNK_BYTES];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        splitter.feed(&chunk[..read], &mut on_line);
    }
    splitter.finish(&mut on_line);
    Ok(())
}

/// Splits a byte stream into non-empty lines at `\n` and `\r`, across chunk boundaries.
#[derive(Debug, Default)]
struct LineSplitter {
    pending: Vec<u8>,
}

impl LineSplitter {
    fn feed(&mut self, mut bytes: &[u8], mut emit: impl FnMut(&str)) {
        while let Some(end) = bytes.iter().position(|&b| b == b'\n' || b == b'\r') {
            self.pending.extend_from_slice(&bytes[..end]);
            self.emit_pending(&mut emit);
            bytes = &bytes[end + 1..];
        }
        self.pending.extend_from_slice(bytes);
        while self.pending.len() >= MAX_LINE_BYTES {
            let rest = self.pending.split_off(MAX_LINE_BYTES);
            self.emit_pending(&mut emit);
            self.pending = rest;
        }
    }

    /// Emits a final line that had no terminator.
    fn finish(&mut self, mut emit: impl FnMut(&str)) {
        self.emit_pending(&mut emit);
    }

    fn emit_pending(&mut self, emit: &mut impl FnMut(&str)) {
        if !self.pending.is_empty() {
            emit(&String::from_utf8_lossy(&self.pending));
            self.pending.clear();
        }
    }
}

/// The last `capacity` lines.
#[derive(Debug)]
struct Tail {
    lines: VecDeque<String>,
    capacity: usize,
}

impl Tail {
    fn new(capacity: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(capacity.min(1024)),
            capacity,
        }
    }

    fn push(&mut self, line: &str) {
        if self.capacity == 0 {
            return;
        }
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(line.to_owned());
    }

    fn into_vec(self) -> Vec<String> {
        self.lines.into()
    }
}

/// Appends lines to the per-clip log (PRD §7.8). The first error disables the sink and is
/// reported through [`RunOutcome::log_error`]; it never fails the run.
///
/// Writes are synchronous into a 64 KiB buffer, so they never suspend the run loop.
#[derive(Debug, Default)]
struct LogSink {
    path: Option<PathBuf>,
    writer: Option<BufWriter<File>>,
    error: Option<String>,
}

impl LogSink {
    fn open(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };
        let mut sink = Self {
            path: Some(path.to_path_buf()),
            ..Self::default()
        };
        match open_append(path) {
            Ok(file) => sink.writer = Some(BufWriter::with_capacity(LOG_BUFFER_BYTES, file)),
            Err(err) => sink.fail(&err),
        }
        sink
    }

    fn line(&mut self, line: &str) {
        self.write(line);
    }

    fn marker(&mut self, text: &str) {
        self.write(&format!("{LOG_MARKER} {text}"));
    }

    fn write(&mut self, line: &str) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        let written = writer
            .write_all(line.as_bytes())
            .and_then(|()| writer.write_all(b"\n"));
        if let Err(err) = written {
            self.fail(&err);
        }
    }

    /// Flushes and returns the first error, if any.
    fn finish(mut self) -> Option<String> {
        if let Some(mut writer) = self.writer.take()
            && let Err(err) = writer.flush()
        {
            self.fail(&err);
        }
        self.error
    }

    fn fail(&mut self, err: &io::Error) {
        // No path at warn level: it may contain the clip name (ARCHITECTURE §2).
        tracing::warn!(%err, "writing an FFmpeg log failed; logging stopped for this run");
        self.writer = None;
        if self.error.is_none() {
            self.error = Some(match &self.path {
                Some(path) => format!("could not write the FFmpeg log {}: {err}", path.display()),
                None => format!("could not write the FFmpeg log: {err}"),
            });
        }
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// The command line for the log header (display only; the process gets an argument vector).
fn display_command(program: &Path, args: &[OsString]) -> String {
    std::iter::once(program.as_os_str())
        .chain(args.iter().map(OsString::as_os_str))
        .map(|arg| quote(&arg.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| c.is_whitespace() || c == '"') {
        arg.to_owned()
    } else {
        format!("\"{}\"", arg.replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(chunks: &[&[u8]]) -> Vec<String> {
        let mut splitter = LineSplitter::default();
        let mut lines = Vec::new();
        for chunk in chunks {
            splitter.feed(chunk, |line| lines.push(line.to_owned()));
        }
        splitter.finish(|line| lines.push(line.to_owned()));
        lines
    }

    #[test]
    fn exp09_lines_split_on_lf_cr_and_crlf() {
        assert_eq!(
            split(&[b"frame=1\rframe=2\r\nout_time_us=5\nprogress=end\n"]),
            ["frame=1", "frame=2", "out_time_us=5", "progress=end"]
        );
    }

    #[test]
    fn lines_are_joined_across_chunk_boundaries() {
        assert_eq!(
            split(&[b"out_ti", b"me_us=1", b"0\r", b"\nspeed=2x\n"]),
            ["out_time_us=10", "speed=2x"]
        );
    }

    #[test]
    fn trailing_line_without_terminator_is_emitted_and_empty_lines_skipped() {
        assert_eq!(split(&[b"\n\r\n\ra\n\n\nb"]), ["a", "b"]);
        assert!(split(&[b"", b"\r\n"]).is_empty());
    }

    #[test]
    fn overlong_lines_are_split_at_the_limit() {
        let long = vec![b'x'; MAX_LINE_BYTES * 2 + 10];
        let lines = split(&[&long[..1000], &long[1000..], b"\nnext"]);
        let lengths: Vec<usize> = lines.iter().map(String::len).collect();
        assert_eq!(lengths, [MAX_LINE_BYTES, MAX_LINE_BYTES, 10, 4]);
    }

    #[test]
    fn invalid_utf8_is_replaced_and_unicode_kept() {
        let lines = split(&[b"bad \xff byte\n", "naive dir 測試 é\n".as_bytes()]);
        assert_eq!(lines, ["bad \u{fffd} byte", "naive dir 測試 é"]);
    }

    #[test]
    fn exp10_tail_keeps_the_last_n_lines_oldest_first() {
        let mut tail = Tail::new(DEFAULT_TAIL_LINES);
        for n in 0..25 {
            tail.push(&format!("line {n}"));
        }
        let kept = tail.into_vec();
        assert_eq!(kept.len(), 20);
        assert_eq!(kept.first().map(String::as_str), Some("line 5"));
        assert_eq!(kept.last().map(String::as_str), Some("line 24"));

        let mut short = Tail::new(20);
        short.push("only");
        assert_eq!(short.into_vec(), ["only"]);

        let mut none = Tail::new(0);
        none.push("dropped");
        assert!(none.into_vec().is_empty());
    }

    #[test]
    fn exp10_default_tail_is_20_lines() {
        assert_eq!(StreamOptions::default().tail_lines, 20);
        assert_eq!(StreamOptions::default().log_file, None);
    }

    #[test]
    fn prd_7_8_log_sink_creates_folders_and_appends() {
        let root = tempfile::tempdir().expect("temp dir");
        let path = root
            .path()
            .join("logs é")
            .join("sub dir")
            .join("clip 測試.log");
        for run in 0..2 {
            let mut sink = LogSink::open(Some(&path));
            sink.marker(&format!("run {run}"));
            sink.line("stderr line");
            assert_eq!(sink.finish(), None);
        }
        let text = fs::read_to_string(&path).expect("log written");
        assert_eq!(
            text,
            "[rustcut] run 0\nstderr line\n[rustcut] run 1\nstderr line\n"
        );
    }

    #[test]
    fn prd_7_8_log_failure_is_reported_not_fatal() {
        let root = tempfile::tempdir().expect("temp dir");
        let blocker = root.path().join("a file");
        fs::write(&blocker, b"").expect("write");
        let mut sink = LogSink::open(Some(&blocker.join("clip.log")));
        sink.line("ignored");
        let error = sink.finish().expect("error reported");
        assert!(error.contains("could not write the FFmpeg log"), "{error}");
        assert!(error.contains("clip.log"), "{error}");
        assert_eq!(LogSink::open(None).finish(), None);
    }

    #[test]
    fn run_futures_are_send_so_they_can_be_spawned() {
        fn assert_send<T: Send>(_: T) {}
        let program = PathBuf::from("ffmpeg");
        let args: Vec<OsString> = Vec::new();
        let sidecars = crate::Sidecars {
            ffmpeg: program.clone(),
            ffprobe: program.clone(),
        };
        assert_send(run_capture(&program, &args, Duration::from_secs(1)));
        assert_send(run_streaming(
            &program,
            &args,
            StreamOptions::default(),
            |_| {},
            |_| {},
            CancelToken::new(),
        ));
        assert_send(crate::probe_capabilities(&sidecars));
    }

    #[test]
    fn display_command_quotes_only_where_needed() {
        let args: Vec<OsString> = ["-i", "naive dir 測試 é/in.mp4", "", "say \"hi\""]
            .into_iter()
            .map(OsString::from)
            .collect();
        assert_eq!(
            display_command(Path::new("ffmpeg"), &args),
            r#"ffmpeg -i "naive dir 測試 é/in.mp4" "" "say \"hi\"""#
        );
    }
}
