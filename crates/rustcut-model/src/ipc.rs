//! IPC contract between the Rust core and the WebView (PRD §7.2).
//!
//! Commands are listed in `docs/ARCHITECTURE.md`; their argument/return types
//! live here. Backend → UI traffic is a single ordered stream of
//! [`AppEvent`]s delivered over one Tauri `Channel` (`subscribe_events`).
//! JSON field names are snake_case everywhere (commands use
//! `#[tauri::command(rename_all = "snake_case")]`).

use crate::clip::{Clip, ClipEdit};
use crate::export::{ExportSettings, Preset};
use crate::project::{Project, Tag, ViewState};
use crate::settings::ProjectLocation;
use crate::source::Fingerprint;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

// ───────────────────────────── errors & notices ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ErrorCode {
    NoProject,
    NotFound,
    InvalidInput,
    /// Range shorter than the minimum clip duration (MRK-01).
    ClipTooShort,
    /// Out <= In, or In beyond the source end.
    InvalidRange,
    ReadOnly,
    SourceMissing,
    /// ffprobe could not read the file (corrupt/truncated/unsupported).
    Probe,
    Ffmpeg,
    Io,
    /// An export is running and the action needs it stopped first.
    Busy,
    Unsupported,
    Internal,
}

/// Error returned by every command (`Result<T, CommandError>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, thiserror::Error)]
#[error("{message}")]
#[ts(export)]
pub struct CommandError {
    pub code: ErrorCode,
    /// Specific, actionable, user-facing sentence (AC-12).
    pub message: String,
    pub detail: Option<String>,
}

impl CommandError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        CommandError {
            code,
            message: message.into(),
            detail: None,
        }
    }
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum NoticeLevel {
    Info,
    Success,
    Warning,
    Error,
}

/// A user-visible message: banners on open, toasts during work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Notice {
    pub level: NoticeLevel,
    /// Stable machine-readable code, e.g. `restored`, `stored_in_library`,
    /// `migrated_schema`, `recovered_from_backup`, `read_only_locked`,
    /// `read_only_newer_schema`, `fingerprint_mismatch`, `source_missing`,
    /// `out_of_range_clips`, `hdr_source`, `source_growing`, `out_cleared`,
    /// `in_cleared`, `clip_too_short`, `duplicate_range`, `export_finished`.
    pub code: String,
    pub message: String,
}

impl Notice {
    pub fn new(level: NoticeLevel, code: &str, message: impl Into<String>) -> Self {
        Notice {
            level,
            code: code.into(),
            message: message.into(),
        }
    }
}

// ───────────────────────────── project state ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum ReadOnlyReason {
    /// PRJ-05: another window/instance holds the lock.
    OpenElsewhere { holder: Option<String> },
    /// PRJ-06: file written by a newer schema version.
    NewerSchema { found: u32, supported: u32 },
    /// §6.4: queue read-only until the source is relinked.
    SourceMissing,
    /// The project folder or its lock file cannot be written, so the project
    /// opens read-only instead of failing to open.
    Unwritable { path: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum SourceStatus {
    Ok,
    /// PRJ-04: *Locate Missing Media*; `suggested_path` is a file in the same
    /// folder with a matching fingerprint, if one was found.
    Missing {
        suggested_path: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HistoryState {
    pub can_undo: bool,
    pub can_redo: bool,
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SaveState {
    /// Last successful write (header shows "saved 2s ago").
    pub saved_at: Option<DateTime<Utc>>,
    /// Changes not yet on disk.
    pub pending: bool,
    pub error: Option<String>,
    pub project_path: Option<String>,
    pub location: ProjectLocation,
}

/// Full state sent on open/relink and on request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectSnapshot {
    #[ts(type = "number")]
    pub revision: u64,
    pub project: Project,
    pub history: HistoryState,
    pub save: SaveState,
    pub read_only: Option<ReadOnlyReason>,
    pub source_status: SourceStatus,
}

/// Incremental change. Revisions increase by one per patch; a UI that sees a
/// gap calls `get_snapshot`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StatePatch {
    #[ts(type = "number")]
    pub revision: u64,
    pub upserted: Vec<Clip>,
    pub removed: Vec<Uuid>,
    pub next_index: u32,
    pub tags: Option<Vec<Tag>>,
    pub export_settings: Option<ExportSettings>,
    pub history: HistoryState,
    /// e.g. "Undo Rename clip" (shown as a toast by the UI).
    pub label: Option<String>,
}

// ───────────────────────────── clip commands ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AddClipRequest {
    pub start_sec: f64,
    pub end_sec: f64,
    pub name: Option<String>,
    pub tag_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AddClipResult {
    pub clip: Clip,
    pub patch: StatePatch,
    /// MRK-03 "duplicate" hint: index of a clip with the identical range.
    pub duplicate_of: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EditClipsRequest {
    pub ids: Vec<Uuid>,
    pub edit: ClipEdit,
}

// ───────────────────────────── open / relink ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PlaybackTier {
    /// Tier 1: the file itself over the asset protocol.
    Direct,
    /// Tier 2: stream copy to MP4 in the cache.
    Remux,
    /// Tier 3 (P1): 540p H.264 short-GOP transcode in the cache.
    Proxy,
}

/// What the player should load (§7.3). The UI confirms playability with
/// `canPlayType(mime_type)` and a decoded first frame within 3 s, and calls
/// `request_playback` with `next_tier` on failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlaybackInfo {
    pub tier: PlaybackTier,
    /// File to pass to `convertFileSrc`; `None` while preparing.
    pub media_path: Option<String>,
    /// e.g. `video/mp4; codecs="avc1.640028, mp4a.40.2"`.
    pub mime_type: String,
    pub preparing: bool,
    /// 0..1 while a remux/proxy is being prepared.
    pub progress: Option<f64>,
    pub eta_sec: Option<f64>,
    pub next_tier: Option<PlaybackTier>,
    /// Set when no tier can play the file: which codec, why, what to do.
    pub unsupported_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OpenResult {
    pub snapshot: ProjectSnapshot,
    /// `None` when the source is missing.
    pub playback: Option<PlaybackInfo>,
    pub notices: Vec<Notice>,
}

/// PRJ-04: returned by `relink_source` when the chosen file's fingerprint
/// differs; the UI asks before calling again with `accept_mismatch = true`.
// One-shot IPC message, serialized immediately: variant size is irrelevant.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum RelinkResult {
    Relinked {
        result: OpenResult,
    },
    Mismatch {
        expected: Fingerprint,
        found: Fingerprint,
        found_duration_sec: f64,
        clips_out_of_range: u32,
    },
}

// ───────────────────────────── media ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum IndexSource {
    /// MP4/MOV sample tables (instant).
    SampleTable,
    /// Background packet scan (other containers).
    PacketScan,
    Cache,
}

/// PLY-06 media index summary sent to the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MediaIndexInfo {
    /// Keyframe presentation times (source-relative seconds, ascending).
    pub keyframes: Vec<f64>,
    /// Median frame duration.
    pub frame_duration_sec: Option<f64>,
    pub vfr: bool,
    /// Every frame's presentation time (ascending), sent for VFR sources so
    /// frame stepping lands on real frames (PLY-04); `None` for CFR sources
    /// or when there are more than 500 000 frames.
    pub frame_times: Option<Vec<f64>>,
    pub source: IndexSource,
    pub complete: bool,
}

/// PLY-05 waveform data, delivered progressively.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WaveformChunk {
    /// 1-based audio track.
    pub track: u32,
    pub peaks_per_sec: u32,
    /// Index of the first peak in this chunk (peak i covers
    /// `[i/peaks_per_sec, (i+1)/peaks_per_sec)`).
    pub start_peak: u32,
    /// Interleaved `[min0, max0, min1, max1, …]`, scaled to -127..=127.
    pub peaks: Vec<i8>,
    pub total_peaks: u32,
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CropWindow {
    pub w: u32,
    pub h: u32,
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HwEncoderInfo {
    /// FFmpeg encoder name, e.g. `h264_amf`.
    pub name: String,
    pub label: String,
    pub available: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AppInfo {
    pub app_version: String,
    pub schema_version: u32,
    pub ffmpeg_version: Option<String>,
    pub ffmpeg_found: bool,
    pub ffprobe_found: bool,
    pub data_dir: String,
    pub log_dir: String,
    pub cache_dir: String,
    /// `windows` | `macos` | `linux`.
    pub platform: String,
    pub cpu_count: u32,
    pub hw_encoders: Vec<HwEncoderInfo>,
}

// ───────────────────────────── export ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CheckLevel {
    Ok,
    Warning,
    /// Blocks *Start* (EXP-11).
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PreflightCheck {
    NothingToExport,
    SourcePresent,
    SourceUnchanged,
    OutputWritable,
    FreeSpace,
    NamesResolved,
    PathLength,
    ContainerCompat,
    HdrReencode,
    MaxDuration,
    OutOfRange,
    LosslessLeadIn,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PreflightItem {
    pub check: PreflightCheck,
    pub level: CheckLevel,
    pub message: String,
    pub clip_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PlannedAction {
    /// New file.
    Write,
    /// Existing file replaced (policy Overwrite).
    Overwrite,
    /// Existing file kept, clip not exported (policy Skip).
    Skip,
    /// The clip's own previous export is replaced (EXP-07 exception).
    ReplacePrevious,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlannedOutput {
    pub clip_id: Uuid,
    pub index: u32,
    /// Final file name with extension.
    pub file_name: String,
    pub output_path: String,
    pub action: PlannedAction,
    /// Lossless: seconds of footage before In (keyframe lead-in), if known.
    pub lead_in_sec: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PreflightReport {
    pub items: Vec<PreflightItem>,
    /// False when any item is an Error.
    pub can_start: bool,
    pub outputs: Vec<PlannedOutput>,
    pub output_dir: String,
    /// Output extension without the dot (`mp4`, `mkv`, …).
    pub extension: String,
    pub preset: Preset,
    #[ts(type = "number")]
    pub estimate_bytes: u64,
    #[ts(type = "number | null")]
    pub free_bytes: Option<u64>,
    /// Rough batch duration estimate for the dialog.
    pub estimate_seconds: Option<f64>,
    /// EXP-03 offer: only audio blocks MP4 output.
    pub offer_aac_audio: bool,
    /// Clips that will not be exported (out of range, skipped by policy).
    pub excluded_clip_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BatchInfo {
    pub batch_id: String,
    pub clip_ids: Vec<Uuid>,
    pub total_duration_sec: f64,
    pub output_dir: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PauseReason {
    DiskFull,
    OutputFolderMissing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BatchSummary {
    pub batch_id: String,
    pub done: Vec<Uuid>,
    pub failed: Vec<Uuid>,
    pub skipped: Vec<Uuid>,
    pub cancelled: Vec<Uuid>,
    pub output_dir: String,
    pub elapsed_sec: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(export)]
pub enum BatchState {
    Running {
        batch: BatchInfo,
    },
    /// EXP-10: full disk or vanished output folder.
    Paused {
        batch_id: String,
        reason: PauseReason,
        message: String,
    },
    Finished {
        summary: BatchSummary,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ClipProgress {
    pub clip_id: Uuid,
    /// 0..1 of this clip.
    pub fraction: f64,
    pub out_time_sec: f64,
    pub speed: Option<f64>,
}

/// EXP-09: overall bar weighted by clip duration, plus running clips.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExportProgress {
    pub batch_id: String,
    pub overall_fraction: f64,
    pub eta_sec: Option<f64>,
    pub done: u32,
    pub failed: u32,
    pub total: u32,
    pub running: Vec<ClipProgress>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "outcome", rename_all = "snake_case")]
#[ts(export)]
pub enum ClipOutcome {
    Done {
        output_path: String,
        #[ts(type = "number")]
        size_bytes: u64,
    },
    Failed {
        exit_code: Option<i32>,
        /// Readable reason (first meaningful FFmpeg error line or our own).
        message: String,
        /// Last 20 lines of FFmpeg output (EXP-10).
        log_tail: Vec<String>,
        log_path: Option<String>,
    },
    Skipped {
        reason: String,
    },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ClipFinished {
    pub batch_id: String,
    pub clip_id: Uuid,
    pub outcome: ClipOutcome,
}

// ───────────────────────────── timestamps (P2) ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TimestampFormat {
    /// YouTube chapters, plain text (EXP-16).
    YoutubeChapters,
    Csv,
    /// CMX3600 EDL.
    Edl,
    Fcpxml,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TimestampExport {
    pub format: TimestampFormat,
    pub text: String,
    pub file_extension: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportedClip {
    /// 1-based line in the pasted text / CSV.
    pub line: u32,
    pub start_sec: f64,
    pub end_sec: f64,
    pub name: String,
    /// Set when the line can't become a clip (shown in the preview).
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportPreview {
    pub items: Vec<ImportedClip>,
    pub valid_count: u32,
}

// ───────────────────────────── event stream ─────────────────────────────

/// Every backend → UI message, in order, over the `subscribe_events` Channel.
// Serialized immediately onto the channel: variant size is irrelevant.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum AppEvent {
    /// Incremental project change (user commands are also answered directly;
    /// this carries changes the UI didn't ask for, e.g. export statuses).
    Patch {
        patch: StatePatch,
    },
    /// Whole state replaced (relink, recovery).
    Snapshot {
        snapshot: ProjectSnapshot,
    },
    Saved {
        save: SaveState,
    },
    ExportBatch {
        state: BatchState,
    },
    ExportProgress {
        progress: ExportProgress,
    },
    ExportClipFinished {
        finished: ClipFinished,
    },
    Playback {
        info: PlaybackInfo,
    },
    MediaIndex {
        info: MediaIndexInfo,
    },
    Waveform {
        chunk: WaveformChunk,
    },
    Toast {
        notice: Notice,
    },
}

/// Convenience for commands that need the current view state from the UI.
pub type ViewStateUpdate = ViewState;
