//! preview_export, export_clips, export_status, cancel_export and the job registry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::watch;
use uuid::Uuid;

use rustcut_export::{
    BatchRequest, EngineConfig, ExportEngine, ExportSink, PreflightInput, RealFs, preflight,
};
use rustcut_model::{
    BatchState, BatchSummary, CheckLevel, ClipEdit, ClipFinished, ClipOutcome, ConflictPolicy,
    ExportProgress, ExportRecord, ExportSettings, OutputResolution, PreflightReport, Preset,
    compute_fingerprint, now_utc,
};
use rustcut_project::ProjectStore;

use crate::env::Env;
use crate::projects::{Opened, clip_by_index, find_readonly, open_for_write, store_options};
use crate::tools::{Ctx, Progress, ToolOutput, ToolResult, decode, fwd};

/// Arguments shared by `preview_export` and `export_clips`.
#[derive(Deserialize, Default)]
pub struct ExportArgs {
    path: String,
    preset: Option<String>,
    indices: Option<Vec<u32>>,
    output_dir: Option<String>,
    naming_template: Option<String>,
    resolution: Option<String>,
    crop_offset: Option<f64>,
    audio_track: Option<u32>,
    on_conflict: Option<String>,
    wait_sec: Option<f64>,
}

/// Parses the preset names the tools accept.
pub fn parse_preset(s: Option<&str>) -> Result<Preset, String> {
    match s.map(|v| v.trim().to_lowercase()).as_deref() {
        None | Some("") | Some("lossless") => Ok(Preset::Lossless),
        Some("accurate") => Ok(Preset::Accurate),
        Some("vertical_crop") | Some("vcrop") => Ok(Preset::Vcrop),
        Some("vertical_blur") | Some("vblur") => Ok(Preset::Vblur),
        Some(other) => Err(format!(
            "Unknown preset \"{other}\". Use lossless, accurate, vertical_crop or vertical_blur."
        )),
    }
}

/// Parses the resolution names the tools accept.
pub fn parse_resolution(s: &str) -> Result<OutputResolution, String> {
    match s.trim().to_lowercase().as_str() {
        "1080x1920" => Ok(OutputResolution::Full1080x1920),
        "720x1280" => Ok(OutputResolution::Hd720x1280),
        "native" => Ok(OutputResolution::Native),
        other => Err(format!(
            "Unknown resolution \"{other}\". Use 1080x1920, 720x1280 or native."
        )),
    }
}

/// Parses the conflict policy names the tools accept.
pub fn parse_conflict(s: Option<&str>) -> Result<ConflictPolicy, String> {
    match s.map(|v| v.trim().to_lowercase()).as_deref() {
        None | Some("") | Some("rename") => Ok(ConflictPolicy::Rename),
        Some("skip") => Ok(ConflictPolicy::Skip),
        Some("overwrite") => Ok(ConflictPolicy::Overwrite),
        Some(other) => Err(format!(
            "Unknown on_conflict \"{other}\". Use rename (default), skip or overwrite."
        )),
    }
}

struct Prepared {
    settings: ExportSettings,
    report: PreflightReport,
    project: rustcut_model::Project,
    hw: Vec<rustcut_model::HwEncoderInfo>,
    index: Option<rustcut_ffmpeg::MediaIndex>,
    zscale: bool,
}

fn need_hw(settings: &ExportSettings) -> bool {
    settings.preset() != Preset::Lossless
        && settings.encoder != rustcut_model::EncoderChoice::Libx264
}

fn blocking(report: &PreflightReport) -> Vec<String> {
    report
        .items
        .iter()
        .filter(|i| i.level == CheckLevel::Error)
        .map(|i| i.message.clone())
        .collect()
}

async fn prepare(
    env: &Env,
    store: &mut ProjectStore,
    video: &Path,
    a: &ExportArgs,
) -> Result<Prepared, String> {
    let preset = parse_preset(a.preset.as_deref())?;
    let opts = store_options(env);
    if a.indices.as_ref().is_some_and(Vec::is_empty) {
        return Err("indices is empty; omit the field to export all included clips, or list the clip numbers.".into());
    }
    if a.audio_track == Some(0) {
        return Err("audio_track starts at 1.".into());
    }
    let ids: Vec<Uuid> = match &a.indices {
        Some(list) => {
            let mut v = Vec::new();
            for i in list {
                let id = clip_by_index(store.project(), *i)?.id;
                if !v.contains(&id) {
                    v.push(id);
                }
            }
            v
        }
        _ => {
            let mut included: Vec<_> = store
                .project()
                .clips
                .iter()
                .filter(|c| c.selected_for_export)
                .collect();
            included.sort_by_key(|c| c.index);
            included.iter().map(|c| c.id).collect()
        }
    };
    if ids.is_empty() {
        return Err(if store.project().clips.is_empty() {
            "This video has no clips yet. Use add_clips first.".into()
        } else {
            "No clips to export: none are included. Pass indices or set included=true with update_clip.".into()
        });
    }
    if let Some(c) = a.crop_offset {
        if !(c.is_finite() && (-1.0..=1.0).contains(&c)) {
            return Err("crop_offset must be between -1 (left) and 1 (right).".into());
        }
        store
            .edit_clips(&ids, &ClipEdit::SetCrop { crop_x_offset: c }, &opts)
            .map_err(|e| e.to_string())?;
    }
    // Per-call options apply to this run only (the project file keeps the saved settings).
    // Omitted options use the documented defaults, not whatever the desktop app saved.
    let defaults = ExportSettings::default();
    let mut settings = store.project().export_settings.clone();
    settings.apply_preset(preset);
    settings.output_resolution = match &a.resolution {
        Some(r) => parse_resolution(r)?,
        None => defaults.output_resolution,
    };
    settings.naming_template = match &a.naming_template {
        Some(t) => {
            rustcut_naming::validate_template(t)
                .map_err(|e| format!("Invalid naming_template: {e}"))?;
            t.clone()
        }
        None => defaults.naming_template,
    };
    settings.output_dir = match a.output_dir.as_deref().filter(|d| !d.trim().is_empty()) {
        Some(d) => Some(env.policy.check(d)?.to_string_lossy().into_owned()),
        None => None,
    };
    settings.on_conflict = parse_conflict(a.on_conflict.as_deref())?;
    settings.audio_track = a.audio_track.unwrap_or(defaults.audio_track);
    let settings = settings.normalized();
    let mut probe_project = store.project().clone();
    probe_project.export_settings = settings.clone();
    env.policy
        .check(&probe_project.output_dir())
        .map_err(|e| format!("{e} Pass output_dir inside the allowed folders."))?;
    let media = env.media()?;
    let hw = if need_hw(&settings) {
        media.detect_hw_encoders(false).await
    } else {
        Vec::new()
    };
    let zscale = env.caps().await.is_some_and(|c| c.has_filter("zscale"));
    let index = if preset == Preset::Lossless {
        let source = store.project().source.clone();
        match tokio::time::timeout(
            Duration::from_secs(120),
            media.build_index(video, &source, |_| {}),
        )
        .await
        {
            Ok(Ok(i)) => Some(i),
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "media index unavailable");
                None
            }
            Err(_) => {
                tracing::warn!("media index timed out");
                None
            }
        }
    } else {
        None
    };
    let video_buf = video.to_path_buf();
    let fingerprint = tokio::task::spawn_blocking(move || compute_fingerprint(&video_buf).ok())
        .await
        .unwrap_or(None);
    // Never overwrite: without on_conflict=overwrite a clip's own earlier output is an
    // ordinary existing file (renamed around or skipped), not a file to replace.
    let mut pf_project = store.project().clone();
    if settings.on_conflict != ConflictPolicy::Overwrite {
        for c in pf_project.clips.iter_mut().filter(|c| ids.contains(&c.id)) {
            if let Some(le) = c.last_export.as_mut() {
                le.output_path = None;
            }
        }
    }
    let report = preflight(&PreflightInput {
        project: &pf_project,
        clip_ids: &ids,
        settings: &settings,
        media_index: index.as_ref(),
        current_fingerprint: fingerprint,
        zscale_available: zscale,
        today: chrono::Local::now().date_naive(),
        fs: &RealFs,
    });
    Ok(Prepared {
        project: pf_project,
        settings,
        report,
        hw,
        index,
        zscale,
    })
}

fn report_json(r: &PreflightReport) -> Value {
    let mb = |b: u64| (b as f64 / 1_048_576.0 * 10.0).round() / 10.0;
    let warnings: Vec<&str> = r
        .items
        .iter()
        .filter(|i| i.level == CheckLevel::Warning)
        .map(|i| i.message.as_str())
        .collect();
    json!({
        "can_start": r.can_start,
        "preset": r.preset.as_str(),
        "output_dir": fwd(&r.output_dir),
        "file_extension": r.extension,
        "will_write": r.outputs.iter().map(|o| json!({
            "index": o.index,
            "file_name": o.file_name,
            "action": serde_json::to_value(o.action).unwrap_or(Value::Null),
        })).collect::<Vec<_>>(),
        "estimated_size_mb": mb(r.estimate_bytes),
        "estimated_seconds": r.estimate_seconds.map(|s| s.round()),
        "free_space_mb": r.free_bytes.map(mb),
        "blocking_errors": blocking(r),
        "warnings": warnings,
    })
}

pub(crate) async fn preview_export(ctx: &Ctx, args: Value) -> ToolResult {
    let a: ExportArgs = decode("preview_export", args)?;
    let env = &ctx.env;
    let video = env.video_path(&a.path)?;
    let Some((project, _)) = find_readonly(env, &video)? else {
        return Err("This video has no clips yet. Use add_clips first.".into());
    };
    // An in-memory store lets the preview apply crop_offset without saving anything.
    let mut store = ProjectStore::new(project, None);
    let prepared = prepare(env, &mut store, &video, &a).await?;
    Ok(ToolOutput::json(&report_json(&prepared.report)))
}

// ───────────────────────────── jobs ─────────────────────────────

#[derive(Default)]
struct JobState {
    progress: Option<ExportProgress>,
    results: Vec<Value>,
    summary: Option<BatchSummary>,
    paused: Option<String>,
}

struct JobShared {
    state: Mutex<JobState>,
    done: watch::Sender<bool>,
    progress: Mutex<Option<Progress>>,
    names: HashMap<Uuid, (u32, String, f64)>,
    opened: Mutex<Option<Opened>>,
    engine: OnceLock<ExportEngine>,
}

/// A running or finished export started by this process.
pub struct Job {
    /// The batch id.
    pub id: String,
    /// Project file being written.
    pub project_path: PathBuf,
    /// Output folder.
    pub output_dir: String,
    engine: ExportEngine,
    shared: Arc<JobShared>,
}

fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Job {
    fn finished(&self) -> bool {
        lk(&self.shared.state).summary.is_some()
    }

    fn status_json(&self) -> Value {
        let st = lk(&self.shared.state);
        let mut v = json!({
            "job_id": self.id,
            "status": if st.summary.is_some() { "finished" } else { "running" },
            "output_dir": fwd(&self.output_dir),
            "clips": st.results,
        });
        if let Some(p) = &st.progress {
            v["progress"] = json!({
                "percent": (p.overall_fraction * 1000.0).round() / 10.0,
                "done": p.done, "failed": p.failed, "total": p.total,
                "eta_sec": p.eta_sec.map(f64::round),
            });
        }
        if let Some(m) = &st.paused {
            v["note"] = json!(format!("The export was stopped: {m}"));
        }
        if let Some(s) = &st.summary {
            v["done"] = json!(s.done.len());
            v["failed"] = json!(s.failed.len());
            v["skipped"] = json!(s.skipped.len());
            v["cancelled"] = json!(s.cancelled.len());
            v["elapsed_sec"] = json!(s.elapsed_sec.round());
        }
        v
    }

    async fn wait(&self, timeout: Duration) -> bool {
        let mut rx = self.shared.done.subscribe();
        tokio::time::timeout(timeout, rx.wait_for(|d| *d))
            .await
            .is_ok()
    }
}

struct Bridge {
    shared: Arc<JobShared>,
}

impl Bridge {
    fn apply_and_save(&self, record: &ExportRecord, force: bool) {
        let mut opened = lk(&self.shared.opened);
        if let Some(o) = opened.as_mut() {
            o.store.apply_export_record(record);
            if let Err(e) = o.save(force) {
                tracing::warn!(error = %e, "could not save the project after a clip finished");
            }
        }
    }
}

impl ExportSink for Bridge {
    fn on_clip_started(&self, _batch_id: &str, _clip_id: Uuid, _output_path: &Path) {}

    fn on_progress(&self, progress: ExportProgress) {
        if let Some(p) = lk(&self.shared.progress).as_ref() {
            p.notify(
                (progress.overall_fraction * 100.0).clamp(0.0, 100.0),
                100.0,
                &format!(
                    "{} of {} clips done",
                    progress.done + progress.failed,
                    progress.total
                ),
            );
        }
        lk(&self.shared.state).progress = Some(progress);
    }

    fn on_clip_finished(&self, record: ExportRecord, finished: ClipFinished) {
        self.apply_and_save(&record, false);
        let (index, name, duration) = self
            .shared
            .names
            .get(&finished.clip_id)
            .cloned()
            .unwrap_or((0, String::new(), 0.0));
        let mut v = json!({"index": index, "name": name, "duration_sec": (duration * 1000.0).round() / 1000.0});
        match &finished.outcome {
            ClipOutcome::Done {
                output_path,
                size_bytes,
            } => {
                v["status"] = json!("done");
                v["output_path"] = json!(fwd(&output_path.to_string()));
                v["size_bytes"] = json!(size_bytes);
            }
            ClipOutcome::Failed { message, .. } => {
                v["status"] = json!("failed");
                v["error"] = json!(message);
            }
            ClipOutcome::Skipped { reason } => {
                v["status"] = json!("skipped");
                v["error"] = json!(reason);
            }
            ClipOutcome::Cancelled => v["status"] = json!("cancelled"),
        }
        lk(&self.shared.state).results.push(v);
    }

    fn on_batch_state(&self, state: BatchState) {
        match state {
            BatchState::Running { .. } => {}
            BatchState::Paused { message, .. } => {
                // Nobody can press "resume" here: stop the batch with a clear reason.
                lk(&self.shared.state).paused = Some(message);
                if let Some(engine) = self.shared.engine.get() {
                    engine.cancel_all();
                }
            }
            BatchState::Finished { summary } => {
                {
                    let mut opened = lk(&self.shared.opened);
                    if let Some(o) = opened.as_mut() {
                        if let Err(e) = o.save(true) {
                            tracing::warn!(error = %e, "final project save failed");
                        }
                        o.release();
                    }
                    *opened = None;
                }
                lk(&self.shared.state).summary = Some(summary);
                let _ = self.shared.done.send(true);
            }
        }
    }
}

fn running_job_for(env: &Env, project_path: &Path) -> Option<String> {
    lk(&env.jobs)
        .values()
        .find(|j| j.project_path == project_path && !j.finished())
        .map(|j| j.id.clone())
}

pub(crate) async fn export_clips(ctx: &Ctx, args: Value) -> ToolResult {
    let a: ExportArgs = decode("export_clips", args)?;
    let env = &ctx.env;
    let video = env.video_path(&a.path)?;
    env.sidecars()?;
    if let Some((_, path)) = find_readonly(env, &video)?
        && let Some(id) = running_job_for(env, &path)
    {
        return Err(format!(
            "An export of this video is already running (job_id {id}). Poll it with export_status or stop it with cancel_export."
        ));
    }
    let wait_sec = a.wait_sec.unwrap_or(240.0);
    if !(wait_sec.is_finite() && (0.0..=3600.0).contains(&wait_sec)) {
        return Err("wait_sec must be between 0 and 3600 seconds.".into());
    }
    let wait = Duration::from_secs_f64(wait_sec);
    let mut opened = open_for_write(env, &video).await?;
    let prepared = prepare(env, &mut opened.store, &video, &a).await?;
    let Prepared {
        settings,
        report,
        project,
        hw,
        index,
        zscale,
    } = prepared;
    if !report.can_start {
        let reasons = blocking(&report);
        return Err(format!(
            "The export cannot start: {}",
            if reasons.is_empty() {
                "the pre-flight check failed.".to_string()
            } else {
                reasons.join(" ")
            }
        ));
    }
    let planned: Vec<Uuid> = report.outputs.iter().map(|o| o.clip_id).collect();
    opened.store.mark_exporting(&planned);
    opened.save(false)?;
    let names: HashMap<Uuid, (u32, String, f64)> = opened
        .store
        .project()
        .clips
        .iter()
        .map(|c| (c.id, (c.index, c.name.clone(), c.duration())))
        .collect();
    let project_path = opened.project_path.clone();
    let (done_tx, _) = watch::channel(false);
    let shared = Arc::new(JobShared {
        state: Mutex::new(JobState::default()),
        done: done_tx,
        progress: Mutex::new(ctx.progress.clone()),
        names,
        opened: Mutex::new(Some(opened)),
        engine: OnceLock::new(),
    });
    let cpu_count =
        std::thread::available_parallelism().map_or(4, |n| u32::try_from(n.get()).unwrap_or(4));
    let engine = ExportEngine::new(EngineConfig {
        sidecars: env.sidecars()?.clone(),
        log_dir: env.paths.log.clone(),
        data_dir: env.paths.data.clone(),
        cpu_count,
        hw_encoders: hw,
        zscale_available: zscale,
    });
    let _ = shared.engine.set(engine.clone());
    let started = engine.start(
        BatchRequest {
            project,
            settings,
            report,
            media_index: index,
        },
        Arc::new(Bridge {
            shared: shared.clone(),
        }),
    );
    let info = match started {
        Ok(i) => i,
        Err(e) => {
            // Restore the statuses the clips had before they were marked.
            let mut guard = lk(&shared.opened);
            if let Some(o) = guard.as_mut() {
                for id in &planned {
                    let record = ExportRecord {
                        clip_id: *id,
                        outcome: ClipOutcome::Cancelled,
                        exported_at: now_utc(),
                        preset: o.store.project().export_settings.preset(),
                        settings_hash: String::new(),
                        content_hash: String::new(),
                    };
                    o.store.apply_export_record(&record);
                }
                let _ = o.save(true);
            }
            *guard = None;
            return Err(format!("The export could not start: {e}"));
        }
    };
    let job = Arc::new(Job {
        id: info.batch_id.clone(),
        project_path,
        output_dir: info.output_dir.clone(),
        engine,
        shared: shared.clone(),
    });
    lk(&env.jobs).insert(job.id.clone(), job.clone());
    let finished = job.wait(wait).await;
    *lk(&shared.progress) = None;
    let mut v = job.status_json();
    if !finished {
        v["note"] = json!(format!(
            "Still running. Call export_status with job_id \"{}\" to check progress, or cancel_export to stop it.",
            job.id
        ));
        return Ok(ToolOutput::json(&v));
    }
    let all_failed = v["done"] == json!(0) && v["failed"].as_u64().unwrap_or(0) > 0;
    let out = ToolOutput::json(&v);
    Ok(if all_failed {
        ToolOutput {
            is_error: true,
            ..out
        }
    } else {
        out
    })
}

fn find_job(env: &Env, id: &str) -> Result<Arc<Job>, String> {
    let jobs = lk(&env.jobs);
    jobs.get(id.trim()).cloned().ok_or_else(|| {
        let known: Vec<&str> = jobs.keys().map(String::as_str).collect();
        format!(
            "Unknown job_id \"{id}\". Jobs of this server session: {}.",
            if known.is_empty() {
                "none".to_string()
            } else {
                known.join(", ")
            }
        )
    })
}

/// Arguments of `export_status` / `cancel_export`.
#[derive(Deserialize)]
pub struct JobArgs {
    job_id: String,
}

pub(crate) async fn export_status(ctx: &Ctx, args: Value) -> ToolResult {
    let a: JobArgs = decode("export_status", args)?;
    let job = find_job(&ctx.env, &a.job_id)?;
    Ok(ToolOutput::json(&job.status_json()))
}

pub(crate) async fn cancel_export(ctx: &Ctx, args: Value) -> ToolResult {
    let a: JobArgs = decode("cancel_export", args)?;
    let job = find_job(&ctx.env, &a.job_id)?;
    if job.finished() {
        return Ok(ToolOutput::json(&job.status_json()));
    }
    job.engine.cancel_all();
    job.wait(Duration::from_secs(20)).await;
    let mut v = job.status_json();
    v["note"] = json!("Cancelled; partial files were removed.");
    Ok(ToolOutput::json(&v))
}

/// Cancels running exports and waits briefly (called when the client disconnects).
pub async fn shutdown(env: &Env) {
    let jobs: Vec<Arc<Job>> = lk(&env.jobs).values().cloned().collect();
    for j in jobs.iter().filter(|j| !j.finished()) {
        tracing::info!(job = %j.id, "cancelling export on shutdown");
        j.engine.cancel_all();
    }
    for j in jobs.iter().filter(|j| !j.finished()) {
        j.wait(Duration::from_secs(10)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_names() {
        assert_eq!(parse_preset(None).unwrap(), Preset::Lossless);
        assert_eq!(parse_preset(Some("Vertical_Crop")).unwrap(), Preset::Vcrop);
        assert_eq!(parse_preset(Some("vertical_blur")).unwrap(), Preset::Vblur);
        assert_eq!(parse_preset(Some("accurate")).unwrap(), Preset::Accurate);
        assert!(parse_preset(Some("fast")).unwrap_err().contains("lossless"));
    }

    #[test]
    fn resolution_and_conflict_names() {
        assert_eq!(
            parse_resolution("720x1280").unwrap(),
            OutputResolution::Hd720x1280
        );
        assert_eq!(
            parse_resolution("native").unwrap(),
            OutputResolution::Native
        );
        assert!(parse_resolution("4k").is_err());
        assert_eq!(parse_conflict(None).unwrap(), ConflictPolicy::Rename);
        assert_eq!(
            parse_conflict(Some("overwrite")).unwrap(),
            ConflictPolicy::Overwrite
        );
        assert!(parse_conflict(Some("replace")).is_err());
    }

    #[test]
    fn overwrite_is_never_the_default() {
        let a: ExportArgs = serde_json::from_str(r#"{"path":"/x.mp4"}"#).unwrap();
        assert_eq!(
            parse_conflict(a.on_conflict.as_deref()).unwrap(),
            ConflictPolicy::Rename
        );
    }
}
