//! Tauri shell for RustCut: IPC commands over rustcut-core.
//!
//! The frontend never touches the filesystem directly for project state -
//! everything goes through the commands below, which delegate to
//! `rustcut-core` (atomic saves, schema validation).

use std::path::Path;

use rustcut_core::ProjectState;

mod exporter;

#[tauri::command]
fn load_project(path: String) -> Result<ProjectState, String> {
    ProjectState::load(Path::new(&path)).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_project(path: String, state: ProjectState) -> Result<(), String> {
    state.save_atomic(Path::new(&path)).map_err(|e| e.to_string())
}

#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            load_project,
            save_project,
            app_version,
            exporter::probe_media,
            exporter::start_export
        ])
        .run(tauri::generate_context!())
        .expect("error while running RustCut");
}
