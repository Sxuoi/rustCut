//! Process-level glue: logging, panic hook, the Tauri [`Platform`], WebView tweaks.

use std::path::Path;
use std::sync::OnceLock;

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

use crate::platform::Platform;

/// Daily log files kept (PRD 7.8).
pub const LOG_FILES_KEPT: usize = 14;

static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// Daily files `rustcut.<date>.log` in `log_dir` (14 kept), stderr too in debug builds.
/// `RUST_LOG` overrides the default `info` level. Safe to call twice.
pub fn init_logging(log_dir: &Path) {
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("rustcut")
        .filename_suffix("log")
        .max_log_files(LOG_FILES_KEPT)
        .build(log_dir);
    let appender = match appender {
        Ok(a) => a,
        Err(e) => {
            eprintln!("RustCut: cannot write logs to {}: {e}", log_dir.display());
            return;
        }
    };
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let _ = LOG_GUARD.set(guard);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr = cfg!(debug_assertions).then(|| fmt::layer().with_writer(std::io::stderr));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(writer).with_ansi(false))
        .with(stderr)
        .try_init();
}

/// Log panics (with location) before the default handler runs.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map_or_else(String::new, |l| format!("{}:{}", l.file(), l.line()));
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(non-text panic payload)".to_string());
        tracing::error!(%location, "panic: {message}");
        previous(info);
    }));
}

/// [`Platform`] over Tauri's asset scope, notification and opener plugins.
pub struct TauriPlatform {
    app: AppHandle,
}

impl TauriPlatform {
    /// Wrap the app handle.
    pub fn new(app: AppHandle) -> Self {
        TauriPlatform { app }
    }
}

impl Platform for TauriPlatform {
    fn allow_asset(&self, path: &Path) {
        if let Err(e) = self.app.asset_protocol_scope().allow_file(path) {
            tracing::warn!(error = %e, "could not allow the source in the asset scope");
        }
    }

    fn forbid_asset(&self, path: &Path) {
        // Tauri's forbidden patterns are permanent and override allowed ones (and match
        // case-insensitively on Windows), so forbidding would break relinking back to a
        // file. The scope only ever grows; see README (known deviation).
        tracing::debug!(path = %path.display(), "forbid_asset ignored");
    }

    fn notify(&self, title: &str, body: &str) {
        if let Err(e) = self
            .app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
        {
            tracing::debug!(error = %e, "could not show the OS notification");
        }
    }

    fn reveal(&self, path: &Path) -> Result<(), String> {
        self.app
            .opener()
            .reveal_item_in_dir(path)
            .map_err(|e| e.to_string())
    }

    fn open_dir(&self, path: &Path) -> Result<(), String> {
        self.app
            .opener()
            .open_path(path.to_string_lossy().into_owned(), None::<String>)
            .map_err(|e| e.to_string())
    }
}

/// Bring the main window to the front (second launch).
pub fn focus_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// PRD section 5 rule 3: the app owns its shortcuts, so WebView2's browser accelerator keys
/// (F5, Ctrl+R, Ctrl+P, F12, ...) are switched off.
#[cfg(windows)]
pub fn disable_browser_accelerators(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let result = window.with_webview(|webview| {
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
        use windows::core::Interface;
        // SAFETY: the COM calls run on the WebView's own thread (with_webview) on interfaces
        // owned by the controller for the duration of this closure.
        let outcome = unsafe {
            webview
                .controller()
                .CoreWebView2()
                .and_then(|core| core.Settings())
                .and_then(|settings| settings.cast::<ICoreWebView2Settings3>())
                .and_then(|s3| s3.SetAreBrowserAcceleratorKeysEnabled(false))
        };
        match outcome {
            Ok(()) => tracing::debug!("WebView2 browser accelerator keys disabled"),
            Err(e) => tracing::warn!(error = %e, "could not disable the WebView2 accelerator keys"),
        }
    });
    if let Err(e) = result {
        tracing::warn!(error = %e, "could not reach the WebView to disable accelerator keys");
    }
}

/// No-op outside Windows (WebView2 only).
#[cfg(not(windows))]
pub fn disable_browser_accelerators(_app: &AppHandle) {}
