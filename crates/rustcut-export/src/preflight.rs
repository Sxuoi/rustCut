//! Pre-flight (EXP-11): everything the Export dialog shows before *Start*, and
//! the plan ([`PlannedOutput`]s) the engine executes. Pure apart from the
//! injected [`FsProbe`], so every rule is unit-testable (AC-16).

use chrono::NaiveDate;
use rustcut_ffmpeg::MediaIndex;
use rustcut_ffmpeg::compat::lossless_container;
use rustcut_ffmpeg::estimate::estimate_bytes;
use rustcut_model::{
    CheckLevel, Clip, ExportSettings, Fingerprint, PlannedOutput, PreflightCheck, PreflightItem,
    PreflightReport, Preset, Project,
};
use rustcut_naming::{NameContext, NameRequest, render_name, resolve_outputs};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Free space must be at least this multiple of the estimate (EXP-11).
pub const FREE_SPACE_FACTOR: f64 = 1.2;
/// Windows path limit used without long-path support (exclusive; edge-case table).
pub const WINDOWS_MAX_PATH_UNITS: usize = 259;
/// Rough throughput of a stream-copy export, bytes per second (disk-bound).
const LOSSLESS_BYTES_PER_SEC: f64 = 150e6;
/// Rough speed of a re-encode relative to real time, used for the time estimate.
const REENCODE_SPEED: f64 = 2.0;

/// File-system questions pre-flight asks (injected so tests can fake a full or
/// read-only disk).
pub trait FsProbe {
    /// Available bytes on the volume of `path`, or of its nearest existing
    /// ancestor when `path` does not exist yet. `None` when unknown.
    fn free_space(&self, path: &Path) -> Option<u64>;
    /// Whether `path` exists.
    fn exists(&self, path: &Path) -> bool;
    /// Whether files can be created inside the existing directory `path`.
    fn writable(&self, path: &Path) -> bool;
}

/// [`FsProbe`] backed by the real file system.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealFs;

impl RealFs {
    fn nearest_existing(path: &Path) -> Option<PathBuf> {
        path.ancestors()
            .find(|p| !p.as_os_str().is_empty() && p.exists())
            .map(Path::to_path_buf)
    }
}

impl FsProbe for RealFs {
    fn free_space(&self, path: &Path) -> Option<u64> {
        let anchor = Self::nearest_existing(path)?;
        fs4::available_space(anchor).ok()
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn writable(&self, path: &Path) -> bool {
        if !path.is_dir() {
            return false;
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let probe = path.join(format!(
            ".rustcut-write-test-{}-{nanos}",
            std::process::id()
        ));
        match std::fs::File::create(&probe) {
            Ok(f) => {
                drop(f);
                let _ = std::fs::remove_file(&probe);
                true
            }
            Err(_) => false,
        }
    }
}

/// Everything pre-flight needs.
pub struct PreflightInput<'a> {
    /// The project (source, clips, tags).
    pub project: &'a Project,
    /// Clips selected for export (any order; unknown ids are ignored).
    pub clip_ids: &'a [Uuid],
    /// Settings of this export.
    pub settings: &'a ExportSettings,
    /// Keyframe index of the source, once built (lead-in, EXP-11).
    pub media_index: Option<&'a rustcut_ffmpeg::MediaIndex>,
    /// Fingerprint of the file currently at the source path; `None` = missing.
    pub current_fingerprint: Option<Fingerprint>,
    /// The FFmpeg build has `zscale` (HDR tone-mapping, FRM-07).
    pub zscale_available: bool,
    /// Export date for `{date}`.
    pub today: NaiveDate,
    /// File-system probe.
    pub fs: &'a dyn FsProbe,
}

fn item(
    check: PreflightCheck,
    level: CheckLevel,
    message: impl Into<String>,
    clip_ids: Vec<Uuid>,
) -> PreflightItem {
    PreflightItem {
        check,
        level,
        message: message.into(),
        clip_ids,
    }
}

/// Human-readable byte count (`1.5 GB`).
pub fn fmt_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Required free bytes for an estimate: `ceil(1.2 x estimate)` (EXP-11).
pub fn required_free_bytes(estimate: u64) -> u64 {
    ((u128::from(estimate) * 12).div_ceil(10)).min(u128::from(u64::MAX)) as u64
}

/// The clip's end clamped to the source duration (PRD 9).
pub(crate) fn clamped_end(clip: &Clip, source_duration: f64) -> f64 {
    if source_duration.is_finite() && source_duration > 0.0 && clip.end_sec > source_duration {
        source_duration
    } else {
        clip.end_sec
    }
}

/// Lead-in of `clip` for a lossless export: `Some(0.0)` when *Snap In* is on and
/// the index knows the keyframe, the index lead-in otherwise, `None` when unknown.
pub(crate) fn lossless_lead_in(
    clip: &Clip,
    settings: &ExportSettings,
    index: Option<&MediaIndex>,
) -> Option<f64> {
    let index = index.filter(|i| !i.keyframes.is_empty())?;
    if settings.snap_in_to_keyframe {
        Some(0.0)
    } else {
        index.lead_in(clip.start_sec)
    }
}

/// Run the pre-flight (EXP-11).
pub fn preflight(input: &PreflightInput<'_>) -> PreflightReport {
    let settings = input.settings.clone().normalized();
    let preset = settings.preset();
    let project = input.project;
    let source = &project.source;
    let mut items: Vec<PreflightItem> = Vec::new();

    // Selected clips in clip-number order; out-of-range clips are excluded.
    let mut selected: Vec<&Clip> = input
        .clip_ids
        .iter()
        .filter_map(|id| project.clip(*id))
        .collect();
    selected.sort_by_key(|c| c.index);
    selected.dedup_by_key(|c| c.id);
    let (clips, excluded): (Vec<&Clip>, Vec<&Clip>) = if source.duration_sec > 0.0 {
        selected
            .iter()
            .copied()
            .partition(|c| c.start_sec < source.duration_sec)
    } else {
        (selected.clone(), Vec::new())
    };
    let excluded_ids: Vec<Uuid> = excluded.iter().map(|c| c.id).collect();
    if !excluded.is_empty() {
        let list = excluded
            .iter()
            .map(|c| format!("#{}", c.index))
            .collect::<Vec<_>>()
            .join(", ");
        items.push(item(
            PreflightCheck::OutOfRange,
            CheckLevel::Warning,
            format!(
                "{} clip(s) start at or after the end of the source ({:.1} s) and are not exported: {list}",
                excluded.len(),
                source.duration_sec
            ),
            excluded_ids.clone(),
        ));
    }
    if clips.is_empty() {
        items.push(item(
            PreflightCheck::NothingToExport,
            CheckLevel::Error,
            "There is nothing to export: no included clip lies inside the source. Include at least one clip.",
            Vec::new(),
        ));
    }

    source_checks(input, &mut items);

    // Output folder.
    let output_dir = PathBuf::from(
        settings
            .output_dir
            .clone()
            .unwrap_or_else(|| source.default_output_dir()),
    );
    output_folder_check(input.fs, &output_dir, &mut items);

    // Container (EXP-03) and names (EXP-05..07).
    let decision = lossless_container(source, &settings);
    container_check(&settings, &decision, &mut items);
    let names = resolve_names(
        input,
        &settings,
        &clips,
        &decision.ext,
        &output_dir,
        &mut items,
    );

    // Lead-ins and estimate (EXP-11).
    let lossless = preset == Preset::Lossless;
    let lead_ins: Vec<Option<f64>> = clips
        .iter()
        .map(|c| {
            if lossless {
                lossless_lead_in(c, &settings, input.media_index)
            } else {
                None
            }
        })
        .collect();
    if lossless && !clips.is_empty() {
        items.push(lead_in_item(&clips, &lead_ins));
    }
    let durations: Vec<(f64, Option<f64>)> = clips
        .iter()
        .zip(&lead_ins)
        .map(|(c, l)| {
            (
                (clamped_end(c, source.duration_sec) - c.start_sec).max(0.0),
                *l,
            )
        })
        .collect();
    let estimate = estimate_bytes(source, &settings, &durations);
    let estimate_bytes_value = estimate.unwrap_or(0);
    let free = input.fs.free_space(&output_dir);
    space_check(estimate, free, &mut items);
    let estimate_seconds = estimate_seconds(&settings, estimate_bytes_value, &durations);

    // Warnings.
    if source.video.hdr && preset.is_reencode() {
        items.push(hdr_item(input.zscale_available));
    }
    if let Some(max) = settings.max_duration_sec.filter(|_| preset.is_vertical()) {
        let long: Vec<&&Clip> = clips.iter().filter(|c| c.duration() > max + 1e-6).collect();
        if !long.is_empty() {
            items.push(item(
                PreflightCheck::MaxDuration,
                CheckLevel::Warning,
                format!(
                    "{} clip(s) are longer than the preset maximum of {:.0} s ({}); the platform may reject or trim them",
                    long.len(),
                    max,
                    long.iter().map(|c| format!("#{}", c.index)).collect::<Vec<_>>().join(", ")
                ),
                long.iter().map(|c| c.id).collect(),
            ));
        }
    }

    let outputs: Vec<PlannedOutput> = clips
        .iter()
        .zip(&lead_ins)
        .zip(&names)
        .map(|((c, lead), n)| PlannedOutput {
            clip_id: c.id,
            index: c.index,
            file_name: n.file_name.clone(),
            output_path: n.path.to_string_lossy().into_owned(),
            action: n.action,
            lead_in_sec: if lossless { *lead } else { None },
        })
        .collect();

    let can_start = !items.iter().any(|i| i.level == CheckLevel::Error);
    PreflightReport {
        items,
        can_start,
        outputs,
        output_dir: output_dir.to_string_lossy().into_owned(),
        extension: decision.ext,
        preset,
        estimate_bytes: estimate_bytes_value,
        free_bytes: free,
        estimate_seconds,
        offer_aac_audio: decision.offer_aac_audio,
        excluded_clip_ids: excluded_ids,
    }
}

fn source_checks(input: &PreflightInput<'_>, items: &mut Vec<PreflightItem>) {
    let source = &input.project.source;
    match &input.current_fingerprint {
        None => items.push(item(
            PreflightCheck::SourcePresent,
            CheckLevel::Error,
            format!(
                "The source file {} was not found. Locate the missing media before exporting.",
                source.path
            ),
            Vec::new(),
        )),
        Some(fp) => {
            items.push(item(
                PreflightCheck::SourcePresent,
                CheckLevel::Ok,
                "Source file found",
                Vec::new(),
            ));
            if *fp == source.fingerprint {
                items.push(item(
                    PreflightCheck::SourceUnchanged,
                    CheckLevel::Ok,
                    "Source file unchanged",
                    Vec::new(),
                ));
            } else {
                items.push(item(
                    PreflightCheck::SourceUnchanged,
                    CheckLevel::Error,
                    "The source file changed on disk or a different file is at this path (PRJ-04). \
                     Relink the original file or re-open the project before exporting.",
                    Vec::new(),
                ));
            }
        }
    }
}

fn output_folder_check(fs: &dyn FsProbe, dir: &Path, items: &mut Vec<PreflightItem>) {
    let (anchor, exists) = if fs.exists(dir) {
        (Some(dir.to_path_buf()), true)
    } else {
        (
            dir.ancestors()
                .skip(1)
                .find(|p| !p.as_os_str().is_empty() && fs.exists(p))
                .map(Path::to_path_buf),
            false,
        )
    };
    match anchor {
        Some(a) if fs.writable(&a) => {
            let msg = if exists {
                format!("Output folder {} is writable", dir.display())
            } else {
                format!("Output folder {} will be created", dir.display())
            };
            items.push(item(PreflightCheck::OutputWritable, CheckLevel::Ok, msg, Vec::new()));
        }
        Some(a) => items.push(item(
            PreflightCheck::OutputWritable,
            CheckLevel::Error,
            format!(
                "Cannot write to {}: the folder is read-only or access is denied. Choose another output folder.",
                a.display()
            ),
            Vec::new(),
        )),
        None => items.push(item(
            PreflightCheck::OutputWritable,
            CheckLevel::Error,
            format!(
                "The output folder {} cannot be created because its drive or parent folder does not exist. \
                 Connect the drive or choose another output folder.",
                dir.display()
            ),
            Vec::new(),
        )),
    }
}

fn container_check(
    settings: &ExportSettings,
    decision: &rustcut_ffmpeg::ContainerDecision,
    items: &mut Vec<PreflightItem>,
) {
    let level = if settings.preset() == Preset::Lossless
        && (decision.ext != "mp4" || decision.offer_aac_audio)
    {
        CheckLevel::Warning
    } else {
        CheckLevel::Ok
    };
    let mut msg = format!(
        "Output container: {} ({})",
        decision.ext.to_uppercase(),
        decision.reason
    );
    if decision.offer_aac_audio && !settings.lossless_audio_to_aac {
        msg.push_str(
            ". Tip: choose \"MP4 with AAC audio\" to get an MP4 while the video stays lossless",
        );
    }
    items.push(item(
        PreflightCheck::ContainerCompat,
        level,
        msg,
        Vec::new(),
    ));
}

fn resolve_names(
    input: &PreflightInput<'_>,
    settings: &ExportSettings,
    clips: &[&Clip],
    ext: &str,
    dir: &Path,
    items: &mut Vec<PreflightItem>,
) -> Vec<rustcut_naming::ResolvedName> {
    let project = input.project;
    let preset = settings.preset();
    let stem = project.source.stem();
    let width = project.index_width();
    let mut requests = Vec::with_capacity(clips.len());
    let mut template_error: Option<String> = None;
    for c in clips {
        let tag = c
            .tag_id
            .as_ref()
            .and_then(|id| project.tags.iter().find(|t| &t.id == id))
            .map(|t| t.label.as_str());
        let ctx = NameContext {
            index: c.index,
            index_width: width,
            name: &c.name,
            source_name: &stem,
            start_sec: c.start_sec,
            end_sec: clamped_end(c, project.source.duration_sec),
            date: input.today,
            tag_label: tag,
            preset,
        };
        let base = match render_name(
            &settings.naming_template,
            &ctx,
            settings.spaces_to_underscores,
        ) {
            Ok(n) => n,
            Err(e) => {
                template_error.get_or_insert_with(|| e.to_string());
                format!("Clip_{:0width$}", c.index)
            }
        };
        requests.push(NameRequest {
            clip_id: c.id,
            index: c.index,
            base_name: base,
        });
    }
    if let Some(e) = template_error {
        items.push(item(
            PreflightCheck::NamesResolved,
            CheckLevel::Error,
            format!("The file name template is invalid: {e}"),
            Vec::new(),
        ));
    }
    let previous: HashMap<Uuid, PathBuf> = clips
        .iter()
        .filter_map(|c| {
            c.last_export
                .as_ref()
                .and_then(|l| l.output_path.as_ref())
                .map(|p| (c.id, PathBuf::from(p)))
        })
        .collect();
    let max_units = cfg!(windows).then_some(WINDOWS_MAX_PATH_UNITS);
    let fs = input.fs;
    let resolved = resolve_outputs(
        &requests,
        ext,
        dir,
        settings.on_conflict,
        &previous,
        &|p| fs.exists(p),
        max_units,
    );
    let too_long: Vec<&rustcut_naming::ResolvedName> =
        resolved.iter().filter(|r| r.error.is_some()).collect();
    if !too_long.is_empty() {
        let first = too_long[0].error.clone().unwrap_or_default();
        items.push(item(
            PreflightCheck::PathLength,
            CheckLevel::Error,
            format!(
                "{} output path(s) are too long for Windows even after shortening the name ({first}). \
                 Choose a shorter output folder.",
                too_long.len()
            ),
            too_long.iter().map(|r| r.clip_id).collect(),
        ));
    }
    if !resolved.is_empty() {
        use rustcut_model::PlannedAction as A;
        let count = |a: A| resolved.iter().filter(|r| r.action == a).count();
        let mut parts = vec![format!("{} file name(s) resolved", resolved.len())];
        for (a, label) in [
            (A::ReplacePrevious, "replace the previous export"),
            (A::Overwrite, "overwrite existing files"),
            (A::Skip, "skipped because the file exists"),
        ] {
            let n = count(a);
            if n > 0 {
                parts.push(format!("{n} {label}"));
            }
        }
        items.push(item(
            PreflightCheck::NamesResolved,
            CheckLevel::Ok,
            parts.join("; "),
            Vec::new(),
        ));
    }
    resolved
}

fn lead_in_item(clips: &[&Clip], leads: &[Option<f64>]) -> PreflightItem {
    let known: Vec<f64> = leads.iter().flatten().copied().collect();
    if known.len() < leads.len() {
        return item(
            PreflightCheck::LosslessLeadIn,
            CheckLevel::Ok,
            "Lossless clips start on the keyframe at or before In; the keyframe index is not ready yet, \
             so the size estimate assumes a 5 s lead-in per clip",
            Vec::new(),
        );
    }
    let max = known.iter().copied().fold(0.0_f64, f64::max);
    let longest: Vec<Uuid> = clips
        .iter()
        .zip(&known)
        .filter(|(_, l)| **l >= 0.05)
        .map(|(c, _)| c.id)
        .collect();
    item(
        PreflightCheck::LosslessLeadIn,
        CheckLevel::Ok,
        format!(
            "Lossless clips start on the keyframe at or before In: up to {max:.1} s of lead-in \
             (use Frame-Accurate or Snap In to keyframe to avoid it)"
        ),
        longest,
    )
}

fn space_check(estimate: Option<u64>, free: Option<u64>, items: &mut Vec<PreflightItem>) {
    let Some(est) = estimate else {
        items.push(item(
            PreflightCheck::FreeSpace,
            CheckLevel::Warning,
            "The size of the export could not be estimated (unknown source bit rate), so free space was not checked",
            Vec::new(),
        ));
        return;
    };
    let need = required_free_bytes(est);
    match free {
        None => items.push(item(
            PreflightCheck::FreeSpace,
            CheckLevel::Warning,
            format!("Free disk space could not be determined; the export needs about {}", fmt_bytes(need)),
            Vec::new(),
        )),
        Some(f) if f < need => items.push(item(
            PreflightCheck::FreeSpace,
            CheckLevel::Error,
            format!(
                "Not enough free disk space: {} free, but the export needs about {} (1.2 x the {} estimate). \
                 Free some space or choose another output folder.",
                fmt_bytes(f),
                fmt_bytes(need),
                fmt_bytes(est)
            ),
            Vec::new(),
        )),
        Some(f) => items.push(item(
            PreflightCheck::FreeSpace,
            CheckLevel::Ok,
            format!("{} free; the export needs about {}", fmt_bytes(f), fmt_bytes(need)),
            Vec::new(),
        )),
    }
}

fn hdr_item(zscale: bool) -> PreflightItem {
    let msg = if zscale {
        "The source is HDR (PQ/HLG). Re-encoded clips are tone-mapped to SDR BT.709 (FRM-07). \
         Lossless exports are unaffected."
    } else {
        "The source is HDR (PQ/HLG) and this FFmpeg build has no zscale filter, so no tone-mapping will be \
         applied: re-encoded colours will look washed out. Use Lossless to keep the original colours."
    };
    item(
        PreflightCheck::HdrReencode,
        CheckLevel::Warning,
        msg,
        Vec::new(),
    )
}

fn estimate_seconds(
    settings: &ExportSettings,
    bytes: u64,
    durations: &[(f64, Option<f64>)],
) -> Option<f64> {
    if durations.is_empty() {
        return None;
    }
    let jobs = f64::from(settings.parallel_jobs.clamp(1, 4));
    if settings.preset() == Preset::Lossless {
        Some((bytes as f64 / LOSSLESS_BYTES_PER_SEC).max(1.0))
    } else {
        let total: f64 = durations.iter().map(|d| d.0).sum();
        Some((total / (REENCODE_SPEED * jobs)).max(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rustcut_model::{
        AudioTrack, ConflictPolicy, ExportStatus, LastExport, PlannedAction, SourceInfo, VideoInfo,
    };
    use std::cell::RefCell;
    use std::collections::HashSet;

    #[derive(Default)]
    struct FakeFs {
        existing: RefCell<HashSet<PathBuf>>,
        free: Option<u64>,
        read_only: bool,
    }

    impl FsProbe for FakeFs {
        fn free_space(&self, _: &Path) -> Option<u64> {
            self.free
        }
        fn exists(&self, p: &Path) -> bool {
            self.existing.borrow().contains(p)
        }
        fn writable(&self, _: &Path) -> bool {
            !self.read_only
        }
    }

    pub(crate) fn source(codec: &str, audio: &str, hdr: bool) -> SourceInfo {
        SourceInfo {
            path: "C:/videos/stream.mp4".into(),
            relative_path: "stream.mp4".into(),
            fingerprint: Fingerprint {
                size_bytes: 1_000_000_000,
                xxh3_head_tail: "abc".into(),
            },
            duration_sec: 600.0,
            start_time_sec: 0.0,
            video: VideoInfo {
                codec: codec.into(),
                coded_width: 1920,
                coded_height: 1080,
                display_width: 1920,
                display_height: 1080,
                rotation: 0,
                sar: "1:1".into(),
                frame_rate: "60/1".into(),
                vfr: false,
                hdr,
                stream_index: Some(0),
                pix_fmt: None,
                profile: None,
                level: None,
                color_transfer: None,
                bit_depth: None,
            },
            audio_tracks: vec![AudioTrack {
                stream_index: 1,
                codec: audio.into(),
                channels: 2,
                title: None,
                language: None,
                sample_rate: Some(48000),
                is_default: true,
            }],
            format_name: Some("mov,mp4,m4a,3gp,3g2,mj2".into()),
            bit_rate: Some(8_000_000),
        }
    }

    fn clip(index: u32, name: &str, a: f64, b: f64) -> Clip {
        Clip {
            id: Uuid::now_v7(),
            index,
            name: name.into(),
            start_sec: a,
            end_sec: b,
            tag_id: None,
            crop_x_offset: 0.0,
            selected_for_export: true,
            export_status: ExportStatus::Pending,
            last_export: None,
        }
    }

    fn project(src: SourceInfo, clips: Vec<Clip>) -> Project {
        let mut p = Project::new(src);
        p.next_index = clips.len() as u32 + 1;
        p.clips = clips;
        p
    }

    struct Env {
        project: Project,
        settings: ExportSettings,
        fs: FakeFs,
        fp: Option<Fingerprint>,
        zscale: bool,
        index: Option<MediaIndex>,
    }

    impl Env {
        fn new(src: SourceInfo, clips: Vec<Clip>) -> Env {
            let fp = Some(src.fingerprint.clone());
            let fs = FakeFs {
                free: Some(100_000_000_000),
                ..FakeFs::default()
            };
            fs.existing.borrow_mut().insert(PathBuf::from("C:/videos"));
            fs.existing.borrow_mut().insert(PathBuf::from("C:/"));
            let settings = ExportSettings {
                output_dir: Some("C:/videos/out".into()),
                ..ExportSettings::default()
            };
            Env {
                project: project(src, clips),
                settings,
                fs,
                fp,
                zscale: false,
                index: None,
            }
        }
        fn run(&self) -> PreflightReport {
            let ids: Vec<Uuid> = self.project.clips.iter().map(|c| c.id).collect();
            preflight(&PreflightInput {
                project: &self.project,
                clip_ids: &ids,
                settings: &self.settings,
                media_index: self.index.as_ref(),
                current_fingerprint: self.fp.clone(),
                zscale_available: self.zscale,
                today: NaiveDate::from_ymd_opt(2026, 11, 13).unwrap(),
                fs: &self.fs,
            })
        }
    }

    fn level_of(r: &PreflightReport, c: PreflightCheck) -> Vec<CheckLevel> {
        r.items
            .iter()
            .filter(|i| i.check == c)
            .map(|i| i.level)
            .collect()
    }

    fn basic() -> Env {
        Env::new(
            source("h264", "aac", false),
            vec![
                clip(1, "Intro", 10.0, 20.0),
                clip(2, "Jump Scare", 100.0, 130.0),
            ],
        )
    }

    #[test]
    fn happy_path_plans_outputs_in_clip_order() {
        let r = basic().run();
        assert!(r.can_start, "{:?}", r.items);
        assert_eq!(r.outputs.len(), 2);
        assert_eq!(r.outputs[0].file_name, "01_Intro.mp4");
        assert_eq!(r.outputs[1].file_name, "02_Jump_Scare.mp4");
        assert_eq!(r.extension, "mp4");
        assert_eq!(r.preset, Preset::Lossless);
        assert!(r.outputs.iter().all(|o| o.action == PlannedAction::Write));
        assert_eq!(r.output_dir, "C:/videos/out");
    }

    #[test]
    fn nothing_to_export_is_an_error() {
        let env = Env::new(source("h264", "aac", false), vec![]);
        let r = env.run();
        assert!(!r.can_start);
        assert_eq!(
            level_of(&r, PreflightCheck::NothingToExport),
            vec![CheckLevel::Error]
        );
        assert!(r.outputs.is_empty());
    }

    #[test]
    fn missing_source_and_changed_fingerprint_block() {
        let mut env = basic();
        env.fp = None;
        let r = env.run();
        assert!(!r.can_start);
        assert_eq!(
            level_of(&r, PreflightCheck::SourcePresent),
            vec![CheckLevel::Error]
        );

        let mut env = basic();
        env.fp = Some(Fingerprint {
            size_bytes: 5,
            xxh3_head_tail: "zzz".into(),
        });
        let r = env.run();
        assert!(!r.can_start);
        assert_eq!(
            level_of(&r, PreflightCheck::SourceUnchanged),
            vec![CheckLevel::Error]
        );
    }

    /// AC-16: free space below 1.2 x estimate blocks.
    #[test]
    fn ac16_low_free_space_blocks() {
        let mut env = basic();
        let probe = env.run();
        assert!(probe.estimate_bytes > 0);
        let need = required_free_bytes(probe.estimate_bytes);
        env.fs.free = Some(need - 1);
        let r = env.run();
        assert!(!r.can_start);
        assert_eq!(
            level_of(&r, PreflightCheck::FreeSpace),
            vec![CheckLevel::Error]
        );
        assert_eq!(r.free_bytes, Some(need - 1));
        env.fs.free = Some(need);
        assert!(env.run().can_start, "exactly 1.2 x the estimate is enough");
    }

    /// AC-16: an unwritable folder blocks, including a not-yet-existing folder
    /// whose nearest existing ancestor is read-only.
    #[test]
    fn ac16_unwritable_folder_blocks() {
        let mut env = basic();
        env.fs.read_only = true;
        let r = env.run();
        assert!(!r.can_start);
        assert_eq!(
            level_of(&r, PreflightCheck::OutputWritable),
            vec![CheckLevel::Error]
        );
        // existing folder, read-only
        env.fs
            .existing
            .borrow_mut()
            .insert(PathBuf::from("C:/videos/out"));
        assert!(!env.run().can_start);
    }

    #[test]
    fn vanished_drive_blocks() {
        let mut env = basic();
        env.settings.output_dir = Some("Z:/usb/clips".into());
        let r = env.run();
        assert!(!r.can_start);
        assert!(r.items.iter().any(
            |i| i.check == PreflightCheck::OutputWritable && i.message.contains("Z:/usb/clips")
        ));
    }

    #[test]
    fn estimates_follow_exp11() {
        let mut env = basic();
        env.index = Some(MediaIndex::from_raw(vec![], vec![0.0, 8.0, 96.0, 120.0]));
        let r = env.run();
        // clip1: in 10 -> kf 8 -> lead 2; clip2: in 100 -> kf 96 -> lead 4
        let secs = (10.0 + 2.0) + (30.0 + 4.0);
        let expect = (8_000_000.0 / 8.0 * secs) as u64;
        assert_eq!(r.estimate_bytes, expect);
        assert_eq!(r.outputs[0].lead_in_sec, Some(2.0));
        assert_eq!(r.outputs[1].lead_in_sec, Some(4.0));
        // unknown index: 5 s per clip
        env.index = None;
        let r = env.run();
        assert_eq!(
            r.estimate_bytes,
            (8_000_000.0 / 8.0 * (10.0 + 30.0 + 10.0)) as u64
        );
        assert_eq!(r.outputs[0].lead_in_sec, None);
        // re-encode: (maxrate + audio) x seconds + bufsize x clips
        env.settings.apply_preset(Preset::Vcrop);
        let r = env.run();
        assert!(r.estimate_bytes > 16_000_000 / 8 * 40);
        assert!(r.outputs.iter().all(|o| o.lead_in_sec.is_none()));
        assert_eq!(r.extension, "mp4");
    }

    #[test]
    fn snap_in_reports_zero_lead_in() {
        let mut env = basic();
        env.index = Some(MediaIndex::from_raw(vec![], vec![0.0, 8.0]));
        env.settings.snap_in_to_keyframe = true;
        let r = env.run();
        assert_eq!(r.outputs[0].lead_in_sec, Some(0.0));
    }

    #[test]
    fn container_and_aac_offer() {
        let mut env = Env::new(
            source("h264", "pcm_s16le", false),
            vec![clip(1, "A", 1.0, 5.0)],
        );
        env.project.source.format_name = Some("mov,mp4,m4a,3gp,3g2,mj2".into());
        env.project.source.path = "C:/videos/stream.mov".into();
        let r = env.run();
        assert!(r.offer_aac_audio);
        assert_eq!(r.extension, "mov");
        assert_eq!(
            level_of(&r, PreflightCheck::ContainerCompat),
            vec![CheckLevel::Warning]
        );
        env.settings.lossless_audio_to_aac = true;
        let r = env.run();
        assert_eq!(r.extension, "mp4");
    }

    #[test]
    fn hdr_warning_only_for_reencode_and_mentions_tone_mapping() {
        let mut env = Env::new(source("hevc", "aac", true), vec![clip(1, "A", 1.0, 5.0)]);
        assert!(level_of(&env.run(), PreflightCheck::HdrReencode).is_empty());
        env.settings.apply_preset(Preset::Accurate);
        let r = env.run();
        let it = r
            .items
            .iter()
            .find(|i| i.check == PreflightCheck::HdrReencode)
            .unwrap();
        assert_eq!(it.level, CheckLevel::Warning);
        assert!(it.message.contains("no tone-mapping"));
        assert!(r.can_start, "a warning never blocks");
        env.zscale = true;
        let r = env.run();
        let it = r
            .items
            .iter()
            .find(|i| i.check == PreflightCheck::HdrReencode)
            .unwrap();
        assert!(it.message.contains("tone-mapped"));
    }

    #[test]
    fn max_duration_warning_is_vertical_only() {
        let mut env = basic();
        env.settings.apply_preset(Preset::Lossless);
        env.settings.max_duration_sec = Some(20.0);
        let r = env.run();
        assert!(
            r.items
                .iter()
                .all(|i| i.check != PreflightCheck::MaxDuration)
        );
    }

    #[test]
    fn max_duration_warning_lists_long_clips() {
        let mut env = basic();
        env.settings.apply_preset(Preset::Vcrop);
        env.settings.max_duration_sec = Some(20.0);
        let r = env.run();
        let it = r
            .items
            .iter()
            .find(|i| i.check == PreflightCheck::MaxDuration)
            .unwrap();
        assert_eq!(it.level, CheckLevel::Warning);
        assert_eq!(it.clip_ids, vec![env.project.clips[1].id]);
        assert!(r.can_start);
    }

    #[test]
    fn out_of_range_clips_are_excluded_with_warning() {
        let mut env = basic();
        env.project.clips.push(clip(3, "Late", 600.0, 610.0));
        env.project.clips.push(clip(4, "Past", 700.0, 710.0));
        let late = env.project.clips[2].id;
        let r = env.run();
        assert!(r.can_start);
        assert_eq!(r.outputs.len(), 2);
        assert!(r.excluded_clip_ids.contains(&late));
        assert_eq!(r.excluded_clip_ids.len(), 2);
        assert_eq!(
            level_of(&r, PreflightCheck::OutOfRange),
            vec![CheckLevel::Warning]
        );
        // only out-of-range clips -> nothing to export
        env.project.clips.drain(0..2);
        let r = env.run();
        assert!(!r.can_start);
    }

    #[test]
    fn conflict_policies_and_previous_output() {
        let mut env = basic();
        env.fs
            .existing
            .borrow_mut()
            .insert(PathBuf::from("C:/videos/out/01_Intro.mp4"));
        let r = env.run();
        assert_eq!(r.outputs[0].file_name, "01_Intro_2.mp4");
        env.settings.on_conflict = ConflictPolicy::Overwrite;
        assert_eq!(env.run().outputs[0].action, PlannedAction::Overwrite);
        env.settings.on_conflict = ConflictPolicy::Skip;
        assert_eq!(env.run().outputs[0].action, PlannedAction::Skip);
        // the clip's own previous output is replaced whatever the policy
        env.settings.on_conflict = ConflictPolicy::Rename;
        env.project.clips[0].last_export = Some(LastExport {
            output_path: Some("C:/videos/out/01_Intro.mp4".into()),
            exported_at: Utc::now(),
            settings_hash: String::new(),
            error: None,
            preset: None,
            content_hash: None,
            exit_code: None,
            log_tail: vec![],
            log_path: None,
        });
        let r = env.run();
        assert_eq!(r.outputs[0].file_name, "01_Intro.mp4");
        assert_eq!(r.outputs[0].action, PlannedAction::ReplacePrevious);
    }

    #[test]
    fn duplicate_names_in_batch_get_suffixes() {
        let mut env = Env::new(
            source("h264", "aac", false),
            vec![clip(1, "Same", 1.0, 5.0), clip(2, "Same", 6.0, 9.0)],
        );
        env.settings.naming_template = "{name}".into();
        let r = env.run();
        assert_eq!(r.outputs[0].file_name, "Same.mp4");
        assert_eq!(r.outputs[1].file_name, "Same_2.mp4");
    }

    #[test]
    fn invalid_template_blocks() {
        let mut env = basic();
        env.settings.naming_template = "{bogus}".into();
        let r = env.run();
        assert!(!r.can_start);
        assert_eq!(
            level_of(&r, PreflightCheck::NamesResolved)[0],
            CheckLevel::Error
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_too_long_is_an_error() {
        let mut env = basic();
        let long = format!("C:/videos/{}", "x".repeat(250));
        env.fs.existing.borrow_mut().insert(PathBuf::from(&long));
        env.settings.output_dir = Some(long);
        let r = env.run();
        assert!(!r.can_start);
        assert_eq!(
            level_of(&r, PreflightCheck::PathLength),
            vec![CheckLevel::Error]
        );
    }

    #[test]
    fn required_free_bytes_is_ceil_1_2x() {
        assert_eq!(required_free_bytes(1000), 1200);
        assert_eq!(required_free_bytes(1), 2);
        assert_eq!(required_free_bytes(0), 0);
    }

    #[test]
    fn real_fs_detects_writable_and_missing() {
        let dir = tempfile::tempdir().unwrap();
        let fs = RealFs;
        assert!(fs.exists(dir.path()));
        assert!(fs.writable(dir.path()));
        assert!(!fs.writable(&dir.path().join("nope")));
        let deep = dir.path().join("a/b/c");
        assert!(!fs.exists(&deep));
        assert!(fs.free_space(&deep).is_some_and(|f| f > 0));
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "probe file removed"
        );
    }
}
