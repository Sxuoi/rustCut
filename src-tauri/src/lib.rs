//! RustCut desktop shell: Tauri v2 glue around the engine crates.
//!
//! * [`core`]: all command logic over [`core::AppCore`] (no Tauri types; unit/integration
//!   tests drive it directly).
//! * [`commands`]: the thin `#[tauri::command]` wrappers (list in `commands.txt`).
//! * [`shell`]: logging, panic hook, OS integration, WebView tweaks.
//!
//! See `src-tauri/README.md` for the launch-path protocol.

pub mod commands;
pub mod core;
pub mod error;
pub mod events;
pub mod launch;
pub mod paths;
pub mod platform;
pub mod shell;
pub mod testing;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rustcut_runner::Sidecars;
use tauri::{Emitter, Manager, RunEvent, WindowEvent};

use crate::commands::AppState;
use crate::core::{AppCore, CoreConfig};
use crate::events::ChannelSink;
use crate::paths::AppPaths;

/// Event name sent to the UI when a second launch asks to open a path.
pub const OPEN_REQUEST_EVENT: &str = "open-request";

/// Run the app until it exits.
pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    let cwd = std::env::current_dir().unwrap_or_default();
    let launch = launch::launch_path_from_args(&args, &cwd);

    let built = tauri::Builder::default()
        // Must be the first plugin: a second launch forwards its path and exits.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            shell::focus_main_window(app);
            if let Some(path) = launch::launch_path_from_args(&argv, Path::new(&cwd)) {
                // Also keep it for a UI that has not registered its listener yet.
                if let Some(state) = app.try_state::<AppState>() {
                    *state
                        .launch_path
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(path.clone());
                }
                if let Err(e) = app.emit(OPEN_REQUEST_EVENT, path) {
                    tracing::warn!(error = %e, "could not forward the open request to the window");
                }
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            commands::subscribe_events,
            commands::get_app_info,
            commands::get_launch_path,
            commands::get_settings,
            commands::set_settings,
            commands::list_recent_projects,
            commands::pick_media_file,
            commands::pick_output_dir,
            commands::open_path,
            commands::relink_source,
            commands::close_project,
            commands::get_snapshot,
            commands::add_clip,
            commands::edit_clips,
            commands::edit_clips_batch,
            commands::get_platform_presets,
            commands::delete_clips,
            commands::undo,
            commands::redo,
            commands::renumber_clips,
            commands::set_tags,
            commands::set_export_settings,
            commands::set_view_state,
            commands::save_now,
            commands::crop_window,
            commands::preview_export,
            commands::start_export,
            commands::cancel_export,
            commands::resume_export,
            commands::retry_failed,
            commands::request_playback,
            commands::start_waveform,
            commands::get_media_index,
            commands::reveal_path,
            commands::open_output_folder,
            commands::clear_cache,
            commands::detect_hw_encoders,
            commands::export_timestamps,
            commands::save_text_file,
            commands::parse_timestamps,
            commands::import_timestamps,
        ])
        .setup(move |app| {
            let started = std::time::Instant::now();
            let resolver = app.path();
            // RUSTCUT_DATA_DIR relocates data, cache and logs under one folder (isolated
            // end-to-end runs, portable installs); unset, the OS per-user folders are used.
            let paths = match std::env::var_os("RUSTCUT_DATA_DIR").filter(|v| !v.is_empty()) {
                Some(root) => AppPaths::under(std::path::Path::new(&root)),
                None => AppPaths::new(
                    resolver.app_data_dir()?,
                    resolver.app_cache_dir()?,
                    resolver.app_log_dir()?,
                ),
            };
            paths.ensure()?;
            shell::init_logging(&paths.log);
            shell::install_panic_hook();
            tracing::info!(version = rustcut_model::APP_VERSION, "RustCut starting");

            // PRD 7.9: the WebView may read only the opened source and the playback cache.
            if let Err(e) = app
                .asset_protocol_scope()
                .allow_directory(&paths.cache, true)
            {
                tracing::warn!(error = %e, "could not allow the playback cache in the asset scope");
            }
            let sink = Arc::new(ChannelSink::default());
            let platform = Arc::new(shell::TauriPlatform::new(app.handle().clone()));
            let sidecars = Sidecars::locate().map_err(|e| e.to_string());
            let core = AppCore::new(CoreConfig {
                paths,
                sidecars,
                sink: sink.clone(),
                platform,
            });
            app.manage(AppState {
                core: core.clone(),
                sink,
                launch_path: Mutex::new(launch),
                shutting_down: AtomicBool::new(false),
                shutdown_done: AtomicBool::new(false),
            });
            shell::disable_browser_accelerators(app.handle());
            tauri::async_runtime::spawn(async move { core.warm_up().await });
            tracing::info!(elapsed_ms = started.elapsed().as_millis(), "setup finished");
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main"
                && let Some(state) = window.try_state::<AppState>()
            {
                if state.shutdown_done.load(Ordering::SeqCst) {
                    return;
                }
                api.prevent_close();
                if state.shutting_down.swap(true, Ordering::SeqCst) {
                    return;
                }
                let (core, win) = (state.core.clone(), window.clone());
                tauri::async_runtime::spawn(async move {
                    let result = core.try_shutdown().await;
                    if let Some(state) = win.try_state::<AppState>() {
                        if let Err(error) = result {
                            report_save_failure(&state, &error);
                            return;
                        }
                        state.shutdown_done.store(true, Ordering::SeqCst);
                    }
                    let _ = win.close();
                });
            }
        })
        .build(tauri::generate_context!());

    let app = match built {
        Ok(app) => app,
        Err(e) => {
            tracing::error!(error = %e, "RustCut failed to start");
            eprintln!("RustCut failed to start: {e}");
            std::process::exit(1);
        }
    };
    app.run(|handle, event| {
        if let RunEvent::ExitRequested { api, .. } = event {
            let Some(state) = handle.try_state::<AppState>() else {
                return;
            };
            if state.shutdown_done.load(Ordering::SeqCst) {
                return;
            }
            api.prevent_exit();
            if state.shutting_down.swap(true, Ordering::SeqCst) {
                return;
            }
            let (core, handle) = (state.core.clone(), handle.clone());
            tauri::async_runtime::spawn(async move {
                let result = core.try_shutdown().await;
                if let Some(state) = handle.try_state::<AppState>() {
                    if let Err(error) = result {
                        report_save_failure(&state, &error);
                        return;
                    }
                    state.shutdown_done.store(true, Ordering::SeqCst);
                }
                handle.exit(0);
            });
        }
    });
}

/// The final save failed: keep the app open, tell the user and allow another close.
fn report_save_failure(state: &AppState, error: &str) {
    tracing::error!(%error, "final save failed; keeping the window open");
    crate::events::EventSink::send(
        &*state.sink,
        rustcut_model::AppEvent::Toast {
            notice: rustcut_model::Notice::new(
                rustcut_model::NoticeLevel::Error,
                "final_save_failed",
                format!(
                    "Your last changes could not be saved: {error} Fix the problem (free disk space, reconnect the drive) and close again."
                ),
            ),
        },
    );
    state.shutting_down.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod manifest_tests {
    const COMMANDS: &str = include_str!("../commands.txt");
    const CAPABILITY: &str = include_str!("../capabilities/default.json");
    const LIB: &str = include_str!("lib.rs");
    const ARCHITECTURE: &str = include_str!("../../docs/ARCHITECTURE.md");

    fn names() -> Vec<&'static str> {
        COMMANDS
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect()
    }

    /// PRD 7.9: exactly the commands in `commands.txt` are registered and permitted.
    #[test]
    fn every_command_is_registered_and_permitted() {
        for name in names() {
            let handler = format!("commands::{name},");
            assert!(
                LIB.contains(&handler),
                "{name} is missing from generate_handler!"
            );
            let permission = format!("\"allow-{}\"", name.replace('_', "-"));
            assert!(
                CAPABILITY.contains(&permission),
                "{name} has no permission in capabilities/default.json"
            );
        }
        let permitted = CAPABILITY.matches("\"allow-").count();
        assert_eq!(
            permitted,
            names().len(),
            "capability lists permissions beyond commands.txt"
        );
    }

    /// The ARCHITECTURE.md section 4 table is implemented in full.
    #[test]
    fn architecture_commands_are_all_present() {
        let table: Vec<&str> = ARCHITECTURE
            .lines()
            .filter(|l| l.starts_with("| `"))
            .flat_map(|l| l.split('|').nth(1))
            .flat_map(|cell| cell.split('/'))
            .map(|s| s.trim().trim_matches('`'))
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
            .collect();
        assert!(table.len() >= 38, "table parsed wrongly: {table:?}");
        for name in table {
            assert!(
                names().contains(&name),
                "{name} from ARCHITECTURE.md section 4 is not a command"
            );
        }
    }

    /// No filesystem/shell plugin is reachable from JavaScript.
    #[test]
    fn capability_is_minimal() {
        for forbidden in [
            "fs:",
            "shell:",
            "opener:",
            "dialog:",
            "http:",
            "core:default",
        ] {
            assert!(
                !CAPABILITY.contains(forbidden),
                "capability must not grant {forbidden}"
            );
        }
    }
}
