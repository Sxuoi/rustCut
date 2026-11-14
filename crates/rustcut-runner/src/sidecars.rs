//! Locating the `ffmpeg`/`ffprobe` sidecars (PRD §7.9: only these two programs are ever run).

use std::env::consts::EXE_SUFFIX;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::RunnerError;

/// Environment variable naming a folder that contains `ffmpeg` and `ffprobe`; in debug builds
/// and CLI tools, a valid value takes precedence over every other location.
pub const FFMPEG_DIR_ENV: &str = "RUSTCUT_FFMPEG_DIR";

/// The target triple this crate was compiled for. Development sidecars are named
/// `ffmpeg-<triple>[.exe]` (Tauri's `externalBin` convention before bundling).
pub const TARGET_TRIPLE: &str = env!("RUSTCUT_TARGET_TRIPLE");

/// Absolute paths of the only two programs RustCut may run (PRD §7.9).
///
/// Both files are taken from the same folder, so FFmpeg and FFprobe always come from the same
/// build.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Sidecars {
    /// Absolute path of `ffmpeg[.exe]` (or `ffmpeg-<triple>[.exe]`).
    pub ffmpeg: PathBuf,
    /// Absolute path of `ffprobe[.exe]` (or `ffprobe-<triple>[.exe]`).
    pub ffprobe: PathBuf,
}

impl Sidecars {
    /// Finds the sidecars. The first folder containing both programs wins, in this order:
    ///
    /// 1. the folder named by [`FFMPEG_DIR_ENV`] (`RUSTCUT_FFMPEG_DIR`), if set; an invalid
    ///    value is reported with a warning and the search continues;
    /// 2. the folder of the running executable: `ffmpeg[.exe]` (Tauri bundles strip the triple),
    ///    then `ffmpeg-<triple>[.exe]` (development builds);
    /// 3. each absolute folder on `PATH`, in order (relative entries are ignored so the working
    ///    directory is never searched);
    /// 4. on Windows, WinGet's Gyan builds:
    ///    `%LOCALAPPDATA%\Microsoft\WinGet\Packages\Gyan.FFmpeg*\*\bin` (the full build first).
    ///
    /// Release builds (not `cfg(debug_assertions)`) only accept the executable's folder:
    /// PRD §7.9 allows only the bundled sidecars, ignoring `RUSTCUT_FFMPEG_DIR`, `PATH`
    /// and WinGet. Use
    /// [`Sidecars::locate_for_cli`] for headless tools that have no bundle.
    ///
    /// # Errors
    /// [`RunnerError::SidecarsNotFound`], listing every place that was searched.
    pub fn locate() -> Result<Self, RunnerError> {
        Self::locate_with(!cfg!(debug_assertions))
    }

    /// Like [`Sidecars::locate`] but always also searches `PATH` and the WinGet folders, in
    /// every build profile. For the headless `rustcut-mcp` server and developer tools.
    ///
    /// # Errors
    /// [`RunnerError::SidecarsNotFound`], listing every place that was searched.
    pub fn locate_for_cli() -> Result<Self, RunnerError> {
        Self::locate_with(false)
    }

    fn locate_with(bundled_only: bool) -> Result<Self, RunnerError> {
        let found = locate_in(&SearchEnv {
            bundled_only,
            ..SearchEnv::from_process()
        });
        if let Ok(sidecars) = &found {
            tracing::debug!(ffmpeg = %sidecars.ffmpeg.display(), "located the FFmpeg sidecars");
        }
        found
    }

    /// Uses `ffmpeg[.exe]` and `ffprobe[.exe]` in `dir`, or failing that
    /// `ffmpeg-<triple>[.exe]` and `ffprobe-<triple>[.exe]`. Both must be existing files; the
    /// returned paths are absolute.
    ///
    /// # Errors
    /// [`RunnerError::SidecarDirIncomplete`] when `dir` lacks either program.
    pub fn from_dir(dir: impl AsRef<Path>) -> Result<Self, RunnerError> {
        let dir = dir.as_ref();
        let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
        for suffix in [String::new(), format!("-{TARGET_TRIPLE}")] {
            let ffmpeg = dir.join(format!("ffmpeg{suffix}{EXE_SUFFIX}"));
            let ffprobe = dir.join(format!("ffprobe{suffix}{EXE_SUFFIX}"));
            if ffmpeg.is_file() && ffprobe.is_file() {
                return Ok(Self { ffmpeg, ffprobe });
            }
        }
        Err(RunnerError::SidecarDirIncomplete { dir })
    }

    /// Whether `path` is the same file as [`ffmpeg`](Self::ffmpeg) or
    /// [`ffprobe`](Self::ffprobe) (symlinks resolved; case-insensitive on Windows).
    pub fn is_sidecar_path(&self, path: &Path) -> bool {
        same_file(path, &self.ffmpeg) || same_file(path, &self.ffprobe)
    }
}

/// The process environment that [`Sidecars::locate`] consults, injectable for tests.
#[derive(Debug, Default)]
struct SearchEnv {
    /// Value of `RUSTCUT_FFMPEG_DIR` (ignored when empty).
    ffmpeg_dir: Option<PathBuf>,
    /// Folder of the running executable.
    exe_dir: Option<PathBuf>,
    /// Value of `PATH`.
    path: Option<OsString>,
    /// `%LOCALAPPDATA%` (Windows only).
    local_app_data: Option<PathBuf>,
    /// Release policy (PRD §7.9): use only the executable's folder.
    bundled_only: bool,
}

impl SearchEnv {
    fn from_process() -> Self {
        let non_empty = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        Self {
            ffmpeg_dir: non_empty(FFMPEG_DIR_ENV).map(PathBuf::from),
            exe_dir: std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf)),
            path: non_empty("PATH"),
            local_app_data: if cfg!(windows) {
                non_empty("LOCALAPPDATA").map(PathBuf::from)
            } else {
                None
            },
            bundled_only: false,
        }
    }
}

fn locate_in(env: &SearchEnv) -> Result<Sidecars, RunnerError> {
    let mut searched = Vec::new();

    if let Some(dir) = &env.ffmpeg_dir
        && !env.bundled_only
    {
        match Sidecars::from_dir(dir) {
            Ok(found) => return Ok(found),
            Err(err) => {
                tracing::warn!(
                    "{FFMPEG_DIR_ENV} does not point to a folder with ffmpeg and ffprobe; \
                     searching elsewhere"
                );
                tracing::debug!(%err, "{FFMPEG_DIR_ENV} rejected");
                searched.push(format!("{FFMPEG_DIR_ENV} ({})", dir.display()));
            }
        }
    }

    if let Some(dir) = &env.exe_dir {
        if let Ok(found) = Sidecars::from_dir(dir) {
            return Ok(found);
        }
        searched.push(format!("the application folder ({})", dir.display()));
    }

    if env.bundled_only {
        searched.push(
            "RUSTCUT_FFMPEG_DIR, PATH and WinGet (not used by release builds: only the bundled sidecars may run)"
                .to_string(),
        );
        return Err(RunnerError::SidecarsNotFound { searched });
    }

    if let Some(path) = &env.path {
        let dirs: Vec<PathBuf> = std::env::split_paths(path)
            .filter(|dir| dir.is_absolute())
            .collect();
        if let Some(found) = dirs.iter().find_map(|dir| Sidecars::from_dir(dir).ok()) {
            return Ok(found);
        }
        searched.push(format!("{} PATH folders", dirs.len()));
    }

    if let Some(local_app_data) = &env.local_app_data {
        let packages = local_app_data
            .join("Microsoft")
            .join("WinGet")
            .join("Packages");
        let found = winget_bin_dirs(&packages)
            .iter()
            .find_map(|dir| Sidecars::from_dir(dir).ok());
        if let Some(found) = found {
            return Ok(found);
        }
        searched.push(format!(
            "WinGet Gyan.FFmpeg packages ({})",
            packages.display()
        ));
    }

    Err(RunnerError::SidecarsNotFound { searched })
}

/// `<packages>\Gyan.FFmpeg*\*\bin`: the full build (`Gyan.FFmpeg_…`) before the Essentials and
/// Shared builds, and within a package the highest-named version folder first.
fn winget_bin_dirs(packages: &Path) -> Vec<PathBuf> {
    let name_of = |path: &Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let mut gyan: Vec<(bool, String, PathBuf)> = subdirs(packages)
        .into_iter()
        .map(|dir| (name_of(&dir), dir))
        .filter(|(name, _)| name.starts_with("Gyan.FFmpeg"))
        .map(|(name, dir)| (!name.starts_with("Gyan.FFmpeg_"), name, dir))
        .collect();
    gyan.sort();
    gyan.into_iter()
        .flat_map(|(_, _, package)| {
            let mut versions = subdirs(&package);
            versions.sort_by(|a, b| b.cmp(a));
            versions.into_iter().map(|version| version.join("bin"))
        })
        .collect()
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

fn same_file(a: &Path, b: &Path) -> bool {
    let canonical =
        |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let (a, b) = (canonical(a), canonical(b));
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn install(dir: &Path, suffix: &str) -> Sidecars {
        fs::create_dir_all(dir).expect("create folder");
        let ffmpeg = dir.join(format!("ffmpeg{suffix}{EXE_SUFFIX}"));
        let ffprobe = dir.join(format!("ffprobe{suffix}{EXE_SUFFIX}"));
        fs::write(&ffmpeg, b"").expect("write ffmpeg");
        fs::write(&ffprobe, b"").expect("write ffprobe");
        Sidecars { ffmpeg, ffprobe }
    }

    fn triple() -> String {
        format!("-{TARGET_TRIPLE}")
    }

    #[test]
    fn prd_7_9_env_dir_takes_precedence_over_app_folder() {
        let root = tempfile::tempdir().expect("temp dir");
        let from_env = install(&root.path().join("env"), "");
        install(&root.path().join("app"), "");
        let env = SearchEnv {
            ffmpeg_dir: Some(root.path().join("env")),
            exe_dir: Some(root.path().join("app")),
            ..SearchEnv::default()
        };
        assert_eq!(locate_in(&env).expect("found"), from_env);
    }

    #[test]
    fn prd_7_9_release_policy_ignores_path_and_winget() {
        let root = tempfile::tempdir().expect("temp dir");
        install(&root.path().join("onpath"), "");
        let winget = root
            .path()
            .join("Microsoft/WinGet/Packages/Gyan.FFmpeg_x/v1/bin");
        install(&winget, "");
        let mut env = SearchEnv {
            path: Some(root.path().join("onpath").into_os_string()),
            local_app_data: Some(root.path().to_path_buf()),
            bundled_only: true,
            ..SearchEnv::default()
        };
        assert!(matches!(
            locate_in(&env),
            Err(RunnerError::SidecarsNotFound { .. })
        ));
        // The CLI policy (and debug builds) still find them.
        env.bundled_only = false;
        assert!(locate_in(&env).is_ok());
    }

    #[test]
    fn prd_7_9_release_policy_ignores_env_dir_and_uses_only_app_folder() {
        let root = tempfile::tempdir().expect("temp dir");
        let app = install(&root.path().join("app"), "");
        install(&root.path().join("env"), "");
        let mut env = SearchEnv {
            exe_dir: Some(root.path().join("app")),
            bundled_only: true,
            ..SearchEnv::default()
        };
        assert_eq!(locate_in(&env).expect("app folder"), app);
        env.ffmpeg_dir = Some(root.path().join("env"));
        assert_eq!(locate_in(&env).expect("bundled dir"), app);
        env.exe_dir = Some(root.path().join("missing-app"));
        assert!(matches!(
            locate_in(&env),
            Err(RunnerError::SidecarsNotFound { .. })
        ));
    }

    #[test]
    fn prd_7_9_invalid_env_dir_falls_back_to_app_folder() {
        let root = tempfile::tempdir().expect("temp dir");
        let app = install(&root.path().join("app"), "");
        let env = SearchEnv {
            ffmpeg_dir: Some(root.path().join("missing")),
            exe_dir: Some(root.path().join("app")),
            ..SearchEnv::default()
        };
        assert_eq!(locate_in(&env).expect("found"), app);
    }

    #[test]
    fn prd_7_9_app_folder_prefers_bundled_names_over_triple_names() {
        let root = tempfile::tempdir().expect("temp dir");
        let plain = install(root.path(), "");
        install(root.path(), &triple());
        let env = SearchEnv {
            exe_dir: Some(root.path().to_path_buf()),
            ..SearchEnv::default()
        };
        assert_eq!(locate_in(&env).expect("found"), plain);
    }

    #[test]
    fn prd_7_9_app_folder_accepts_dev_triple_names() {
        let root = tempfile::tempdir().expect("temp dir");
        let dev = install(root.path(), &triple());
        let env = SearchEnv {
            exe_dir: Some(root.path().to_path_buf()),
            ..SearchEnv::default()
        };
        assert_eq!(locate_in(&env).expect("found"), dev);
        assert!(dev.ffmpeg.to_string_lossy().contains(TARGET_TRIPLE));
    }

    #[test]
    fn both_programs_must_be_files_in_the_same_folder() {
        let root = tempfile::tempdir().expect("temp dir");
        let only_ffmpeg = root.path().join("only-ffmpeg");
        fs::create_dir_all(&only_ffmpeg).expect("create");
        fs::write(only_ffmpeg.join(format!("ffmpeg{EXE_SUFFIX}")), b"").expect("write");
        let probe_is_dir = root.path().join("probe-is-dir");
        fs::create_dir_all(probe_is_dir.join(format!("ffprobe{EXE_SUFFIX}"))).expect("create");
        fs::write(probe_is_dir.join(format!("ffmpeg{EXE_SUFFIX}")), b"").expect("write");
        for dir in [&only_ffmpeg, &probe_is_dir] {
            let err = Sidecars::from_dir(dir).expect_err("incomplete folder");
            assert!(
                matches!(err, RunnerError::SidecarDirIncomplete { .. }),
                "{err}"
            );
        }
        let mixed = root.path().join("mixed");
        fs::create_dir_all(&mixed).expect("create");
        fs::write(mixed.join(format!("ffmpeg{EXE_SUFFIX}")), b"").expect("write");
        let probe = format!("ffprobe{}{EXE_SUFFIX}", triple());
        fs::write(mixed.join(probe), b"").expect("write");
        assert!(
            Sidecars::from_dir(&mixed).is_err(),
            "a bundled ffmpeg is never paired with a dev ffprobe"
        );
    }

    #[test]
    fn from_dir_returns_absolute_paths_even_for_relative_input() {
        let err = Sidecars::from_dir("rustcut-no-such-relative-dir").expect_err("missing");
        match err {
            RunnerError::SidecarDirIncomplete { dir } => assert!(dir.is_absolute()),
            other => panic!("unexpected error {other}"),
        }
    }

    #[test]
    fn prd_7_9_path_is_searched_in_order_and_relative_entries_are_ignored() {
        let root = tempfile::tempdir().expect("temp dir");
        let empty = root.path().join("empty");
        fs::create_dir_all(&empty).expect("create");
        let first = install(&root.path().join("first"), "");
        install(&root.path().join("second"), "");
        let path = std::env::join_paths([
            PathBuf::from("relative-entry"),
            empty,
            root.path().join("first"),
            root.path().join("second"),
        ])
        .expect("join PATH");
        let env = SearchEnv {
            exe_dir: Some(root.path().join("no-app-here")),
            path: Some(path),
            ..SearchEnv::default()
        };
        assert_eq!(locate_in(&env).expect("found"), first);
    }

    #[test]
    fn prd_7_9_winget_gyan_full_build_is_found_first() {
        let root = tempfile::tempdir().expect("temp dir");
        let packages = root
            .path()
            .join("Microsoft")
            .join("WinGet")
            .join("Packages");
        install(
            &packages
                .join("Gyan.FFmpeg.Essentials_Microsoft.Winget.Source_x")
                .join("ffmpeg-8.0-essentials_build")
                .join("bin"),
            "",
        );
        let full = install(
            &packages
                .join("Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe")
                .join("ffmpeg-9.0.2-full_build")
                .join("bin"),
            "",
        );
        install(&packages.join("Other.Tool_x").join("v1").join("bin"), "");
        let env = SearchEnv {
            local_app_data: Some(root.path().to_path_buf()),
            ..SearchEnv::default()
        };
        assert_eq!(locate_in(&env).expect("found"), full);
    }

    #[test]
    fn winget_ignores_packages_that_are_not_gyan_ffmpeg() {
        let root = tempfile::tempdir().expect("temp dir");
        let packages = root
            .path()
            .join("Microsoft")
            .join("WinGet")
            .join("Packages");
        install(&packages.join("Other.Tool_x").join("v1").join("bin"), "");
        let env = SearchEnv {
            local_app_data: Some(root.path().to_path_buf()),
            ..SearchEnv::default()
        };
        assert!(locate_in(&env).is_err());
    }

    #[test]
    fn not_found_error_names_every_searched_location_and_the_fix() {
        let root = tempfile::tempdir().expect("temp dir");
        let env = SearchEnv {
            ffmpeg_dir: Some(root.path().join("env")),
            exe_dir: Some(root.path().join("app")),
            path: Some(std::env::join_paths([root.path().join("p")]).expect("join")),
            local_app_data: Some(root.path().to_path_buf()),
            bundled_only: false,
        };
        let err = locate_in(&env).expect_err("nothing installed");
        let RunnerError::SidecarsNotFound { searched } = &err else {
            panic!("unexpected error {err}");
        };
        assert_eq!(searched.len(), 4);
        let message = err.to_string();
        for needle in [
            FFMPEG_DIR_ENV,
            "application folder",
            "1 PATH folders",
            "WinGet",
        ] {
            assert!(
                message.contains(needle),
                "{needle:?} missing from {message}"
            );
        }
    }

    #[test]
    fn is_sidecar_path_matches_only_the_two_programs() {
        let root = tempfile::tempdir().expect("temp dir");
        let sidecars = install(root.path(), "");
        assert!(sidecars.is_sidecar_path(&sidecars.ffmpeg));
        assert!(sidecars.is_sidecar_path(&sidecars.ffprobe));
        let other = root.path().join(format!("other{EXE_SUFFIX}"));
        fs::write(&other, b"").expect("write");
        assert!(!sidecars.is_sidecar_path(&other));
        if cfg!(windows) {
            let upper = PathBuf::from(sidecars.ffmpeg.to_string_lossy().to_uppercase());
            assert!(
                sidecars.is_sidecar_path(&upper),
                "Windows paths are case-insensitive"
            );
        }
    }
}
