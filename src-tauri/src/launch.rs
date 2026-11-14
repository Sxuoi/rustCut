//! Command-line launch: a video or `.rustcut` path passed when starting the app (or
//! when a second instance is started) opens in the running window.

use std::path::{Path, PathBuf};

/// Video extensions the app opens (PRD 3.1), plus the project extension.
pub const OPENABLE_EXTENSIONS: [&str; 10] = [
    "mp4", "mov", "mkv", "webm", "flv", "ts", "m2ts", "m4v", "avi", "rustcut",
];

/// The path to open from `args` (`args[0]` is the executable): the first
/// argument that is not an option and either has an openable extension or
/// names an existing file. Any other file ("Open with RustCut" on an audio
/// file, say) then gets the open flow's specific message, as a dropped file
/// does, instead of being ignored. A relative path is resolved against `cwd`.
pub fn launch_path_from_args(args: &[String], cwd: &Path) -> Option<String> {
    args.iter().skip(1).find_map(|arg| {
        if arg.starts_with('-') || arg.trim().is_empty() {
            return None;
        }
        let path = Path::new(arg);
        let full: PathBuf = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        let openable = path.extension().is_some_and(|ext| {
            OPENABLE_EXTENSIONS.contains(&ext.to_string_lossy().to_ascii_lowercase().as_str())
        });
        (openable || full.is_file()).then(|| full.to_string_lossy().into_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn picks_first_openable_argument() {
        let cwd = std::env::temp_dir();
        assert_eq!(launch_path_from_args(&args(&["rustcut.exe"]), &cwd), None);
        assert_eq!(
            launch_path_from_args(&args(&["rustcut.exe", "--flag", "notes.txt"]), &cwd),
            None
        );
        let abs = cwd.join("a.MP4").to_string_lossy().into_owned();
        assert_eq!(
            launch_path_from_args(&args(&["rustcut.exe", "--x", &abs, "b.mkv"]), &cwd).as_deref(),
            Some(abs.as_str())
        );
        let rel = launch_path_from_args(&args(&["rustcut.exe", "clip one.rustcut"]), &cwd).unwrap();
        assert_eq!(Path::new(&rel), cwd.join("clip one.rustcut"));
    }

    #[test]
    fn an_existing_file_of_any_type_is_forwarded() {
        let dir = std::env::temp_dir().join(format!("rustcut-launch-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let song = dir.join("song.m4a");
        std::fs::write(&song, b"not a video").unwrap();
        let got = launch_path_from_args(&args(&["rustcut.exe", "--x", "song.m4a"]), &dir);
        assert_eq!(got.map(PathBuf::from), Some(song.clone()));
        // A missing file without a video extension is still ignored, and folders are too.
        assert_eq!(
            launch_path_from_args(&args(&["rustcut.exe", "missing.m4a"]), &dir),
            None
        );
        let folder = dir.to_string_lossy().into_owned();
        assert_eq!(
            launch_path_from_args(&args(&["rustcut.exe", &folder]), &dir),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
