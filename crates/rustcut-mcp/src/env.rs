//! Process-wide state: folders shared with the desktop app, FFmpeg, settings, policy, jobs.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustcut_media::{MediaConfig, MediaService, ProbedSource};
use rustcut_model::AppSettings;
use rustcut_runner::{Capabilities, CapturedOutput, Sidecars, probe_capabilities, run_capture};

use crate::export::Job;
use crate::policy::{Policy, check_video_extension};

/// Tauri identifier of the desktop app; its folders are shared with this server.
pub const APP_IDENTIFIER: &str = "app.rustcut.desktop";
/// Environment variable relocating data, cache and logs under one folder.
pub const DATA_DIR_ENV: &str = "RUSTCUT_DATA_DIR";
/// Environment variable with the whisper ggml model file.
pub const WHISPER_MODEL_ENV: &str = "RUSTCUT_WHISPER_MODEL";
/// Settings file name inside the data folder (as written by the desktop app).
pub const SETTINGS_FILE: &str = "settings.json";

/// Folders the app owns (mirrors `src-tauri/src/paths.rs`).
#[derive(Debug, Clone)]
pub struct AppPaths {
    /// Settings, recent list, export journal, library and backups.
    pub data: PathBuf,
    /// Playback remux/proxy files, frame indexes, waveforms.
    pub cache: PathBuf,
    /// Daily log files and per-clip export logs.
    pub log: PathBuf,
    /// `<data>/library`.
    pub library: PathBuf,
    /// `<data>/backups`.
    pub backups: PathBuf,
}

impl AppPaths {
    /// Derive the folders from the three roots.
    pub fn new(data: PathBuf, cache: PathBuf, log: PathBuf) -> Self {
        AppPaths {
            library: data.join("library"),
            backups: data.join("backups"),
            data,
            cache,
            log,
        }
    }

    /// All folders under one root (`RUSTCUT_DATA_DIR`, tests).
    pub fn under(root: &Path) -> Self {
        Self::new(root.join("data"), root.join("cache"), root.join("logs"))
    }

    /// The per-user folders Tauri uses for [`APP_IDENTIFIER`].
    pub fn os_default() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        if cfg!(windows) {
            let roaming = var("APPDATA").unwrap_or_else(|| PathBuf::from("."));
            let local = var("LOCALAPPDATA").unwrap_or_else(|| roaming.clone());
            Self::new(
                roaming.join(APP_IDENTIFIER),
                local.join(APP_IDENTIFIER),
                local.join(APP_IDENTIFIER).join("logs"),
            )
        } else if cfg!(target_os = "macos") {
            let home = home.unwrap_or_else(|| PathBuf::from("."));
            Self::new(
                home.join("Library/Application Support")
                    .join(APP_IDENTIFIER),
                home.join("Library/Caches").join(APP_IDENTIFIER),
                home.join("Library/Logs").join(APP_IDENTIFIER),
            )
        } else {
            let home = home.unwrap_or_else(|| PathBuf::from("."));
            let data = var("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
            let cache = var("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache"));
            Self::new(
                data.join(APP_IDENTIFIER),
                cache.join(APP_IDENTIFIER),
                data.join(APP_IDENTIFIER).join("logs"),
            )
        }
    }

    /// `RUSTCUT_DATA_DIR` if set, else the OS folders.
    pub fn from_env() -> Self {
        match std::env::var_os(DATA_DIR_ENV).filter(|v| !v.is_empty()) {
            Some(root) => Self::under(Path::new(&root)),
            None => Self::os_default(),
        }
    }

    /// Create every folder.
    ///
    /// # Errors
    /// The first folder that cannot be created.
    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [
            &self.data,
            &self.cache,
            &self.log,
            &self.library,
            &self.backups,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

/// Everything the tools share.
pub struct Env {
    /// Shared folders.
    pub paths: AppPaths,
    /// Path policy.
    pub policy: Policy,
    /// FFmpeg/FFprobe, or why they were not found.
    pub sidecars: Result<Sidecars, String>,
    /// Probe / index service (needs FFmpeg).
    pub media: Option<MediaService>,
    /// Whisper model from `RUSTCUT_WHISPER_MODEL` (not checked yet).
    pub whisper_model: Option<PathBuf>,
    caps: tokio::sync::OnceCell<Option<Arc<Capabilities>>>,
    /// Export jobs started by this process.
    pub jobs: Mutex<HashMap<String, Arc<Job>>>,
}

impl Env {
    /// Builds the environment from the process environment.
    pub fn from_process() -> Arc<Self> {
        let paths = AppPaths::from_env();
        if let Err(e) = paths.ensure() {
            tracing::warn!(error = %e, "could not create the data folders");
        }
        let sidecars = Sidecars::locate_for_cli().map_err(|e| e.to_string());
        Self::new(paths, Policy::from_env(), sidecars)
    }

    /// Builds the environment from explicit parts.
    pub fn new(paths: AppPaths, policy: Policy, sidecars: Result<Sidecars, String>) -> Arc<Self> {
        let settings = load_settings(&paths.data);
        let media = sidecars.as_ref().ok().map(|s| {
            MediaService::new(MediaConfig {
                sidecars: s.clone(),
                cache_dir: paths.cache.clone(),
                data_dir: paths.data.clone(),
                cache_cap_bytes: u64::from(settings.cache_cap_gb) * (1 << 30),
            })
        });
        let whisper_model = std::env::var_os(WHISPER_MODEL_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        Arc::new(Env {
            paths,
            policy,
            sidecars,
            media,
            whisper_model,
            caps: tokio::sync::OnceCell::new(),
            jobs: Mutex::new(HashMap::new()),
        })
    }

    /// The desktop app's settings (defaults when the file is missing or unreadable).
    pub fn settings(&self) -> AppSettings {
        load_settings(&self.paths.data)
    }

    /// FFmpeg and FFprobe paths.
    ///
    /// # Errors
    /// An actionable message when FFmpeg is missing.
    pub fn sidecars(&self) -> Result<&Sidecars, String> {
        self.sidecars.as_ref().map_err(|e| {
            format!(
                "FFmpeg was not found ({e}). Install FFmpeg and put it on PATH, or set RUSTCUT_FFMPEG_DIR to the folder that contains ffmpeg and ffprobe, then restart the server."
            )
        })
    }

    /// The media service.
    ///
    /// # Errors
    /// An actionable message when FFmpeg is missing.
    pub fn media(&self) -> Result<&MediaService, String> {
        self.sidecars()?;
        self.media
            .as_ref()
            .ok_or_else(|| "FFmpeg is not available.".to_string())
    }

    /// FFmpeg capabilities (filters, encoders), cached.
    pub async fn caps(&self) -> Option<Arc<Capabilities>> {
        let sidecars = self.sidecars.as_ref().ok()?;
        self.caps
            .get_or_init(|| async { probe_capabilities(sidecars).await.ok() })
            .await
            .clone()
    }

    /// Validates a video path argument: policy, extension, existence.
    ///
    /// # Errors
    /// A specific message.
    pub fn video_path(&self, raw: &str) -> Result<PathBuf, String> {
        let path = self.policy.check(raw)?;
        check_video_extension(&path)?;
        if !path.is_file() {
            return Err(format!(
                "The video \"{}\" was not found. Check the path (use forward slashes or escape backslashes) and that the drive is connected.",
                path.display()
            ));
        }
        Ok(path)
    }

    /// Probes a video (headers only).
    ///
    /// # Errors
    /// FFmpeg missing, or the file cannot be read as video.
    pub async fn probe(&self, path: &Path) -> Result<ProbedSource, String> {
        self.media()?
            .probe_source(path)
            .await
            .map_err(|e| e.to_string())
    }

    /// Runs FFmpeg with `args` (after the standard quiet prefix) and returns its output.
    ///
    /// # Errors
    /// FFmpeg missing or a spawn/timeout failure. A non-zero exit is reported in the output.
    pub async fn ffmpeg(
        &self,
        args: Vec<OsString>,
        timeout: Duration,
    ) -> Result<CapturedOutput, String> {
        let sidecars = self.sidecars()?;
        let mut full: Vec<OsString> = ["-nostdin", "-hide_banner", "-v", "error"]
            .iter()
            .map(OsString::from)
            .collect();
        full.extend(args);
        run_capture(&sidecars.ffmpeg, &full, timeout)
            .await
            .map_err(|e| format!("FFmpeg could not be run: {e}"))
    }
}

/// The last meaningful line of FFmpeg's stderr.
pub fn last_error_line(out: &CapturedOutput) -> String {
    let text = out.stderr_lossy();
    text.lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no error message")
        .to_string()
}

/// Reads `<data>/settings.json` like the desktop app.
pub fn load_settings(data_dir: &Path) -> AppSettings {
    match std::fs::read(data_dir.join(SETTINGS_FILE)) {
        Ok(bytes) => serde_json::from_slice::<AppSettings>(&bytes)
            .map(AppSettings::normalized)
            .unwrap_or_default(),
        Err(_) => AppSettings::default(),
    }
}

/// FFmpeg filter-argument escaping for a path (`C\:/dir/file`), wrapped in single quotes.
pub fn filter_path(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    format!("'{}'", s.replace(':', "\\:").replace('\'', "\\'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_paths_escape_the_drive_colon() {
        assert_eq!(filter_path(Path::new("C:\\a b\\m.bin")), "'C\\:/a b/m.bin'");
    }

    #[test]
    fn data_dir_env_layout_matches_the_app() {
        let p = AppPaths::under(Path::new("/x"));
        assert_eq!(p.data, Path::new("/x/data"));
        assert_eq!(p.cache, Path::new("/x/cache"));
        assert_eq!(p.log, Path::new("/x/logs"));
        assert_eq!(p.library, Path::new("/x/data/library"));
        assert_eq!(p.backups, Path::new("/x/data/backups"));
    }

    #[test]
    fn missing_settings_use_defaults() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load_settings(d.path()), AppSettings::default());
        std::fs::write(
            d.path().join(SETTINGS_FILE),
            r#"{"min_clip_duration_sec": 2.0}"#,
        )
        .unwrap();
        assert!((load_settings(d.path()).min_clip_duration_sec - 2.0).abs() < 1e-9);
    }
}
