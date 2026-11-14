//! Shared helpers of the integration tests (FFmpeg fixtures, sink, engine setup).
#![allow(dead_code)]

use chrono::NaiveDate;
use rustcut_export::{
    BatchRequest, EngineConfig, ExportEngine, ExportError, ExportSink, PreflightInput, RealFs,
    preflight,
};
use rustcut_ffmpeg::probe::parse_probe_json;
use rustcut_ffmpeg::{MediaIndex, packet_index_args, parse_packet_csv};
use rustcut_model::{
    BatchInfo, BatchState, BatchSummary, Clip, ClipFinished, ClipOutcome, ExportProgress,
    ExportRecord, ExportSettings, ExportStatus, HwEncoderInfo, PreflightReport, Project,
    SourceInfo, compute_fingerprint,
};
use rustcut_runner::Sidecars;
use rustcut_testkit::{Fixture, try_ffmpeg, try_ffprobe, try_fixture};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// FFmpeg sidecars of this machine, or `None` (the test then skips).
pub fn sidecars() -> Option<Sidecars> {
    let ff = try_ffmpeg()?;
    try_ffprobe()?;
    Sidecars::from_dir(ff.parent()?).ok()
}

/// Skip the test when FFmpeg is not available.
#[macro_export]
macro_rules! require_ffmpeg {
    () => {
        match $crate::common::sidecars() {
            Some(s) => s,
            None => {
                eprintln!("SKIPPED: FFmpeg not found on this machine");
                return;
            }
        }
    };
}

pub fn fixture(f: Fixture) -> PathBuf {
    try_fixture(f).unwrap_or_else(|e| panic!("fixture {f:?}: {e}"))
}

pub fn probe_source(path: &Path) -> SourceInfo {
    let json = rustcut_testkit::probe_json(path);
    let mut source = parse_probe_json(&json.to_string(), path)
        .expect("probe")
        .source;
    source.path = path.to_string_lossy().into_owned();
    source.relative_path = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    source.fingerprint = compute_fingerprint(path).expect("fingerprint");
    source
}

pub fn media_index(path: &Path, start_time: f64) -> MediaIndex {
    let out = std::process::Command::new(try_ffprobe().expect("ffprobe"))
        .args(packet_index_args(path))
        .output()
        .expect("run ffprobe");
    parse_packet_csv(&String::from_utf8_lossy(&out.stdout), start_time)
}

pub fn make_clip(index: u32, name: &str, start: f64, end: f64, crop: f64) -> Clip {
    Clip {
        id: Uuid::now_v7(),
        index,
        name: name.into(),
        start_sec: start,
        end_sec: end,
        tag_id: None,
        crop_x_offset: crop,
        selected_for_export: true,
        export_status: ExportStatus::Pending,
        last_export: None,
    }
}

pub fn project_with(source: &Path, clips: &[(f64, f64, &str)]) -> Project {
    let mut p = Project::new(probe_source(source));
    for (i, (a, b, name)) in clips.iter().enumerate() {
        p.clips.push(make_clip(i as u32 + 1, name, *a, *b, 0.0));
    }
    p.next_index = clips.len() as u32 + 1;
    p
}

pub fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 13).expect("date")
}

pub fn run_preflight(
    project: &Project,
    settings: &ExportSettings,
    index: Option<&MediaIndex>,
) -> PreflightReport {
    let ids: Vec<Uuid> = project.clips.iter().map(|c| c.id).collect();
    preflight(&PreflightInput {
        project,
        clip_ids: &ids,
        settings,
        media_index: index,
        current_fingerprint: compute_fingerprint(Path::new(&project.source.path)).ok(),
        zscale_available: false,
        today: today(),
        fs: &RealFs,
    })
}

type Hook = Box<dyn Fn(&Path) + Send + Sync>;

/// Records every event; `on_started` runs inside `on_clip_started`.
#[derive(Default)]
pub struct TestSink {
    pub records: Mutex<Vec<(ExportRecord, ClipFinished)>>,
    pub states: Mutex<Vec<BatchState>>,
    pub progress: Mutex<Vec<ExportProgress>>,
    pub started: Mutex<Vec<(Uuid, PathBuf)>>,
    pub pids: Mutex<Vec<u32>>,
    live: AtomicUsize,
    pub peak: AtomicUsize,
    pub on_started: Mutex<Option<Hook>>,
}

impl ExportSink for TestSink {
    fn on_clip_started(&self, _batch_id: &str, clip_id: Uuid, output_path: &Path) {
        self.started
            .lock()
            .unwrap()
            .push((clip_id, output_path.to_path_buf()));
        if let Some(h) = self.on_started.lock().unwrap().as_ref() {
            h(output_path);
        }
    }
    fn on_progress(&self, p: ExportProgress) {
        self.progress.lock().unwrap().push(p);
    }
    fn on_clip_finished(&self, record: ExportRecord, finished: ClipFinished) {
        self.records.lock().unwrap().push((record, finished));
    }
    fn on_batch_state(&self, state: BatchState) {
        self.states.lock().unwrap().push(state);
    }
    fn on_process_spawned(&self, _clip: Uuid, pid: u32) {
        self.pids.lock().unwrap().push(pid);
        let now = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
    }
    fn on_process_exited(&self, _clip: Uuid, _pid: u32) {
        self.live.fetch_sub(1, Ordering::SeqCst);
    }
}

impl TestSink {
    pub fn new() -> Arc<TestSink> {
        Arc::new(TestSink::default())
    }

    pub fn summary(&self) -> Option<BatchSummary> {
        self.states.lock().unwrap().iter().find_map(|s| match s {
            BatchState::Finished { summary } => Some(summary.clone()),
            _ => None,
        })
    }

    pub fn finished_count(&self) -> usize {
        self.states
            .lock()
            .unwrap()
            .iter()
            .filter(|s| matches!(s, BatchState::Finished { .. }))
            .count()
    }

    pub fn paused_count(&self) -> usize {
        self.states
            .lock()
            .unwrap()
            .iter()
            .filter(|s| matches!(s, BatchState::Paused { .. }))
            .count()
    }

    pub fn outcome(&self, clip: Uuid) -> Option<ClipOutcome> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .find(|(r, _)| r.clip_id == clip)
            .map(|(r, _)| r.outcome.clone())
    }

    /// Final path of a finished clip.
    pub fn done_path(&self, clip: Uuid) -> PathBuf {
        match self.outcome(clip) {
            Some(ClipOutcome::Done { output_path, .. }) => PathBuf::from(output_path),
            other => panic!("clip {clip} did not finish Done: {other:?}"),
        }
    }

    pub async fn wait_until(
        &self,
        timeout: Duration,
        what: &str,
        cond: impl Fn(&TestSink) -> bool,
    ) {
        let t0 = Instant::now();
        while !cond(self) {
            assert!(t0.elapsed() < timeout, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }

    pub async fn wait_finished(&self, timeout: Duration) -> BatchSummary {
        self.wait_until(timeout, "the batch to finish", |s| s.finished_count() > 0)
            .await;
        self.summary().expect("summary")
    }
}

/// An engine with its own data/log/output folders.
pub struct Env {
    pub engine: ExportEngine,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub out_dir: PathBuf,
    _tmp: tempfile::TempDir,
}

pub fn env_with(sidecars: Sidecars, hw: Vec<HwEncoderInfo>) -> Env {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().join("data");
    let log_dir = tmp.path().join("logs");
    let out_dir = tmp.path().join("out");
    let engine = ExportEngine::new(EngineConfig {
        sidecars,
        log_dir: log_dir.clone(),
        data_dir: data_dir.clone(),
        cpu_count: 8,
        hw_encoders: hw,
        zscale_available: false,
    });
    Env {
        engine,
        data_dir,
        log_dir,
        out_dir,
        _tmp: tmp,
    }
}

pub fn env(sidecars: Sidecars) -> Env {
    env_with(sidecars, Vec::new())
}

impl Env {
    pub fn settings(&self) -> ExportSettings {
        ExportSettings {
            output_dir: Some(self.out_dir.to_string_lossy().into_owned()),
            ..ExportSettings::default()
        }
    }

    /// Pre-flight (asserting it passes) and start.
    pub fn start(
        &self,
        project: &Project,
        settings: &ExportSettings,
        index: Option<MediaIndex>,
        sink: &Arc<TestSink>,
    ) -> Result<BatchInfo, ExportError> {
        let report = run_preflight(project, settings, index.as_ref());
        self.engine.start(
            BatchRequest {
                project: project.clone(),
                settings: settings.clone(),
                report,
                media_index: index,
            },
            sink.clone(),
        )
    }

    /// Start, wait for Finished, return the summary.
    pub async fn run(
        &self,
        project: &Project,
        settings: &ExportSettings,
        index: Option<MediaIndex>,
        sink: &Arc<TestSink>,
    ) -> BatchSummary {
        let report = run_preflight(project, settings, index.as_ref());
        assert!(report.can_start, "pre-flight blocked: {:?}", report.items);
        self.start(project, settings, index, sink).expect("start");
        sink.wait_finished(Duration::from_secs(240)).await
    }

    pub fn leftover_parts(&self) -> Vec<PathBuf> {
        walk(&self.out_dir)
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "part"))
            .collect()
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                v.extend(walk(&p));
            } else {
                v.push(p);
            }
        }
    }
    v
}

pub fn streams_of(path: &Path, kind: &str) -> Vec<serde_json::Value> {
    let json = rustcut_testkit::probe_json(path);
    json["streams"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|s| s["codec_type"] == kind)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}
