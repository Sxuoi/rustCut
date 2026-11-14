//! Thin Tauri wrappers over [`AppCore`] (ARCHITECTURE section 4). Every command is
//! `#[tauri::command(rename_all = "snake_case")]` and returns `Result<T, CommandError>`.
//! The list of names lives in `commands.txt` (build.rs app manifest + capability).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use rustcut_model::{
    AddClipRequest, AddClipResult, AppEvent, AppInfo, AppSettings, BatchInfo, CommandError,
    CropWindow, EditClipsRequest, ExportSettings, HwEncoderInfo, ImportPreview, ImportedClip,
    MediaIndexInfo, OpenResult, PlatformPreset, PlaybackInfo, PlaybackTier, PreflightReport,
    ProjectSnapshot, RecentProject, RelinkResult, SaveState, StatePatch, Tag, TimestampExport,
    TimestampFormat, ViewState,
};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::{DialogExt, FilePath};
use uuid::Uuid;

use crate::core::AppCore;
use crate::error::join_error;
use crate::events::ChannelSink;
use crate::launch::OPENABLE_EXTENSIONS;

/// Managed Tauri state.
pub struct AppState {
    /// Command logic.
    pub core: AppCore,
    /// The `subscribe_events` channel.
    pub sink: Arc<ChannelSink>,
    /// The path given on the command line of the first launch, until the UI asks for it.
    pub launch_path: Mutex<Option<String>>,
    /// Graceful shutdown started.
    pub shutting_down: AtomicBool,
    /// Graceful shutdown finished (the next close/exit request goes through).
    pub shutdown_done: AtomicBool,
}

type Res<T> = Result<T, CommandError>;

fn picked(path: Option<FilePath>) -> Option<String> {
    path.and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

/// Replace the event channel (the UI subscribes once at startup).
#[tauri::command(rename_all = "snake_case")]
pub fn subscribe_events(state: State<'_, AppState>, on_event: Channel<AppEvent>) -> Res<()> {
    state.sink.replace(on_event);
    Ok(())
}

/// App, FFmpeg and folder facts.
#[tauri::command(rename_all = "snake_case")]
pub async fn get_app_info(state: State<'_, AppState>) -> Res<AppInfo> {
    Ok(state.core.app_info().await)
}

/// The video or project path of the first launch (once; `None` afterwards).
#[tauri::command(rename_all = "snake_case")]
pub fn get_launch_path(state: State<'_, AppState>) -> Res<Option<String>> {
    Ok(state
        .launch_path
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take())
}

/// Current settings.
#[tauri::command(rename_all = "snake_case")]
pub fn get_settings(state: State<'_, AppState>) -> Res<AppSettings> {
    Ok(state.core.settings())
}

/// Save settings; returns the normalized values.
#[tauri::command(rename_all = "snake_case")]
pub async fn set_settings(state: State<'_, AppState>, settings: AppSettings) -> Res<AppSettings> {
    state.core.set_settings(settings).await
}

/// Recent projects, newest first.
#[tauri::command(rename_all = "snake_case")]
pub fn list_recent_projects(state: State<'_, AppState>) -> Res<Vec<RecentProject>> {
    Ok(state.core.list_recent_projects())
}

/// Native open dialog for a video or a `.rustcut` project.
#[tauri::command(rename_all = "snake_case")]
pub async fn pick_media_file(app: AppHandle) -> Res<Option<String>> {
    let path = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("Videos and RustCut projects", &OPENABLE_EXTENSIONS)
            .blocking_pick_file()
    })
    .await
    .map_err(|e| join_error(&e))?;
    Ok(picked(path))
}

/// Native folder dialog starting at `current`.
#[tauri::command(rename_all = "snake_case")]
pub async fn pick_output_dir(app: AppHandle, current: Option<String>) -> Res<Option<String>> {
    let path = tokio::task::spawn_blocking(move || {
        let mut dialog = app.dialog().file();
        if let Some(dir) = current.map(PathBuf::from).filter(|p| p.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        dialog.blocking_pick_folder()
    })
    .await
    .map_err(|e| join_error(&e))?;
    Ok(picked(path))
}

/// Open a video or a `.rustcut` project.
#[tauri::command(rename_all = "snake_case")]
pub async fn open_path(state: State<'_, AppState>, path: String) -> Res<OpenResult> {
    state.core.open_path(&path).await
}

/// Point the project at a (moved) source video.
#[tauri::command(rename_all = "snake_case")]
pub async fn relink_source(
    state: State<'_, AppState>,
    path: String,
    accept_mismatch: bool,
) -> Res<RelinkResult> {
    state.core.relink_source(&path, accept_mismatch).await
}

/// Flush, release the lock, stop background jobs.
#[tauri::command(rename_all = "snake_case")]
pub async fn close_project(state: State<'_, AppState>) -> Res<()> {
    state.core.close_project().await;
    Ok(())
}

/// The open project, if any.
#[tauri::command(rename_all = "snake_case")]
pub fn get_snapshot(state: State<'_, AppState>) -> Res<Option<ProjectSnapshot>> {
    Ok(state.core.get_snapshot())
}

/// Add a clip.
#[tauri::command(rename_all = "snake_case")]
pub fn add_clip(state: State<'_, AppState>, req: AddClipRequest) -> Res<AddClipResult> {
    state.core.add_clip(&req)
}

/// Edit clips.
#[tauri::command(rename_all = "snake_case")]
pub fn edit_clips(state: State<'_, AppState>, req: EditClipsRequest) -> Res<StatePatch> {
    state.core.edit_clips(&req)
}

/// Apply one atomic bulk edit.
#[tauri::command(rename_all = "snake_case")]
pub fn edit_clips_batch(
    state: State<'_, AppState>,
    batch: Vec<EditClipsRequest>,
) -> Res<StatePatch> {
    state.core.edit_clips_batch(&batch)
}

/// Platform export presets (user file in the data dir, else the bundled list).
#[tauri::command(rename_all = "snake_case")]
pub fn get_platform_presets(state: State<'_, AppState>) -> Res<Vec<PlatformPreset>> {
    Ok(state.core.platform_presets())
}

/// Delete clips.
#[tauri::command(rename_all = "snake_case")]
pub fn delete_clips(state: State<'_, AppState>, ids: Vec<Uuid>) -> Res<StatePatch> {
    state.core.delete_clips(&ids)
}

/// Undo.
#[tauri::command(rename_all = "snake_case")]
pub fn undo(state: State<'_, AppState>) -> Res<Option<StatePatch>> {
    state.core.undo()
}

/// Redo.
#[tauri::command(rename_all = "snake_case")]
pub fn redo(state: State<'_, AppState>) -> Res<Option<StatePatch>> {
    state.core.redo()
}

/// Renumber clips chronologically.
#[tauri::command(rename_all = "snake_case")]
pub fn renumber_clips(state: State<'_, AppState>) -> Res<StatePatch> {
    state.core.renumber_clips()
}

/// Replace the tag palette.
#[tauri::command(rename_all = "snake_case")]
pub fn set_tags(state: State<'_, AppState>, tags: Vec<Tag>) -> Res<StatePatch> {
    state.core.set_tags(tags)
}

/// Replace the export settings.
#[tauri::command(rename_all = "snake_case")]
pub fn set_export_settings(
    state: State<'_, AppState>,
    settings: ExportSettings,
) -> Res<StatePatch> {
    state.core.set_export_settings(settings)
}

/// Remember playhead, zoom and selection.
#[tauri::command(rename_all = "snake_case")]
pub fn set_view_state(state: State<'_, AppState>, view: ViewState) -> Res<()> {
    state.core.set_view_state(view)
}

/// Write pending changes now.
#[tauri::command(rename_all = "snake_case")]
pub async fn save_now(state: State<'_, AppState>) -> Res<SaveState> {
    state.core.save_now().await
}

/// FRM-04 crop window.
#[tauri::command(rename_all = "snake_case")]
pub fn crop_window(
    state: State<'_, AppState>,
    display_width: u32,
    display_height: u32,
    offset: f64,
) -> Res<CropWindow> {
    Ok(state
        .core
        .crop_window(display_width, display_height, offset))
}

/// Pre-flight report.
#[tauri::command(rename_all = "snake_case")]
pub async fn preview_export(
    state: State<'_, AppState>,
    clip_ids: Vec<Uuid>,
    settings: ExportSettings,
) -> Res<PreflightReport> {
    state.core.preview_export(&clip_ids, &settings).await
}

/// Start a batch.
#[tauri::command(rename_all = "snake_case")]
pub async fn start_export(
    state: State<'_, AppState>,
    clip_ids: Vec<Uuid>,
    settings: ExportSettings,
) -> Res<BatchInfo> {
    state.core.start_export(&clip_ids, &settings).await
}

/// Cancel one clip or the batch.
#[tauri::command(rename_all = "snake_case")]
pub fn cancel_export(state: State<'_, AppState>, clip_id: Option<Uuid>) -> Res<()> {
    state.core.cancel_export(clip_id);
    Ok(())
}

/// Resume a paused batch.
#[tauri::command(rename_all = "snake_case")]
pub fn resume_export(state: State<'_, AppState>) -> Res<()> {
    state.core.resume_export()
}

/// Re-export the failed clips.
#[tauri::command(rename_all = "snake_case")]
pub async fn retry_failed(state: State<'_, AppState>) -> Res<Option<BatchInfo>> {
    state.core.retry_failed().await
}

/// Prepare playback at a tier.
#[tauri::command(rename_all = "snake_case")]
pub async fn request_playback(state: State<'_, AppState>, tier: PlaybackTier) -> Res<PlaybackInfo> {
    state.core.request_playback(tier).await
}

/// Stream a waveform.
#[tauri::command(rename_all = "snake_case")]
pub async fn start_waveform(state: State<'_, AppState>, track: u32) -> Res<()> {
    state.core.start_waveform(track)
}

/// The frame index, once built.
#[tauri::command(rename_all = "snake_case")]
pub fn get_media_index(state: State<'_, AppState>) -> Res<Option<MediaIndexInfo>> {
    state.core.get_media_index()
}

/// Build the frame index with a packet scan if the open could not (called after the first frame).
#[tauri::command(rename_all = "snake_case")]
pub async fn start_media_index(state: State<'_, AppState>) -> Res<()> {
    state.core.start_media_index()
}

/// Show an exported file or the output folder in the file manager.
#[tauri::command(rename_all = "snake_case")]
pub fn reveal_path(state: State<'_, AppState>, path: String) -> Res<()> {
    state.core.reveal_path(&path)
}

/// Open the output folder.
#[tauri::command(rename_all = "snake_case")]
pub fn open_output_folder(state: State<'_, AppState>) -> Res<()> {
    state.core.open_output_folder()
}

/// Delete cached playback files; returns bytes freed.
#[tauri::command(rename_all = "snake_case")]
pub async fn clear_cache(state: State<'_, AppState>) -> Res<u64> {
    state.core.clear_cache().await
}

/// Hardware encoders (cached unless `force`).
#[tauri::command(rename_all = "snake_case")]
pub async fn detect_hw_encoders(
    state: State<'_, AppState>,
    force: bool,
) -> Res<Vec<HwEncoderInfo>> {
    state.core.detect_hw_encoders(force).await
}

/// Timestamps of all clips in a text format.
#[tauri::command(rename_all = "snake_case")]
pub fn export_timestamps(
    state: State<'_, AppState>,
    format: TimestampFormat,
) -> Res<TimestampExport> {
    state.core.export_timestamps(format)
}

/// Save dialog and write.
#[tauri::command(rename_all = "snake_case")]
pub async fn save_text_file(
    app: AppHandle,
    state: State<'_, AppState>,
    default_name: String,
    text: String,
) -> Res<Option<String>> {
    let path = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_file_name(default_name)
            .blocking_save_file()
    })
    .await
    .map_err(|e| join_error(&e))?;
    let Some(path) = path.and_then(|p| p.into_path().ok()) else {
        return Ok(None);
    };
    state.core.write_text_file(&path, &text)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Preview of pasted timestamps.
#[tauri::command(rename_all = "snake_case")]
pub fn parse_timestamps(state: State<'_, AppState>, text: String) -> Res<ImportPreview> {
    state.core.parse_timestamps(&text)
}

/// Add the imported clips as one undo step.
#[tauri::command(rename_all = "snake_case")]
pub fn import_timestamps(state: State<'_, AppState>, items: Vec<ImportedClip>) -> Res<StatePatch> {
    state.core.import_timestamps(&items)
}
