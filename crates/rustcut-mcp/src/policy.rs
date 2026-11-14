//! Path policy: absolute paths only, optionally confined to allowed folders (MCP roots).

use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

/// Environment variable with the allowed folders (`;`-separated on Windows, `:` elsewhere).
pub const ROOTS_ENV: &str = "RUSTCUT_MCP_ROOTS";

/// Video extensions the server accepts as input.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "flv", "wmv", "mpg", "mpeg", "ts", "m2ts", "mts",
    "ogv", "3gp", "mxf",
];

/// Where the server may read and write.
#[derive(Debug, Default)]
pub struct Policy {
    env_roots: Option<Vec<PathBuf>>,
    client_roots: Mutex<Option<Vec<PathBuf>>>,
}

impl Policy {
    /// Roots from the process environment.
    pub fn from_env() -> Self {
        let roots = std::env::var_os(ROOTS_ENV)
            .map(|v| {
                std::env::split_paths(&v)
                    .filter(|p| !p.as_os_str().is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty());
        Self::new(roots)
    }

    /// A policy with explicit roots (`None` = unrestricted by the environment).
    pub fn new(env_roots: Option<Vec<PathBuf>>) -> Self {
        Self {
            env_roots,
            client_roots: Mutex::new(None),
        }
    }

    /// Roots advertised by the MCP client (`roots/list`).
    pub fn set_client_roots(&self, roots: Vec<PathBuf>) {
        *self.client_roots.lock().unwrap_or_else(|e| e.into_inner()) = Some(roots);
    }

    fn constraints(&self) -> Vec<Vec<PathBuf>> {
        let mut out = Vec::new();
        if let Some(r) = &self.env_roots {
            out.push(r.clone());
        }
        let client = self
            .client_roots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(r) = client.filter(|r| !r.is_empty()) {
            out.push(r);
        }
        out
    }

    /// Whether any folder restriction is active (environment roots or client roots).
    pub fn is_restricted(&self) -> bool {
        !self.constraints().is_empty()
    }

    /// The folders a path must be inside: with two root sets (environment and client) only the
    /// overlap counts, so the narrower folder of each overlapping pair is listed.
    fn effective_roots(&self) -> Vec<PathBuf> {
        let sets = self.constraints();
        let mut it = sets.into_iter();
        let Some(first) = it.next() else {
            return Vec::new();
        };
        it.fold(first, |acc, set| {
            let mut out: Vec<PathBuf> = Vec::new();
            for a in &acc {
                for b in &set {
                    let narrow = if is_within(b, a) {
                        b
                    } else if is_within(a, b) {
                        a
                    } else {
                        continue;
                    };
                    if !out.contains(narrow) {
                        out.push(narrow.clone());
                    }
                }
            }
            out
        })
    }

    /// Human readable list of the effective roots (for messages).
    pub fn describe(&self) -> String {
        let roots = self.effective_roots();
        if roots.is_empty() && self.is_restricted() {
            return "none: the folders from RUSTCUT_MCP_ROOTS and the client roots do not overlap"
                .into();
        }
        roots
            .iter()
            .map(|r| r.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Validates a path argument: absolute, no `..`, inside every active root set.
    ///
    /// # Errors
    /// A message naming the problem and how to fix it.
    pub fn check(&self, raw: &str) -> Result<PathBuf, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(
                "The path is empty. Pass the absolute path of the video, e.g. D:/Videos/stream.mp4."
                    .into(),
            );
        }
        self.check_path(Path::new(raw))
    }

    /// [`Policy::check`] for a `Path`.
    ///
    /// # Errors
    /// As [`Policy::check`].
    pub fn check_path(&self, path: &Path) -> Result<PathBuf, String> {
        if !path.is_absolute() {
            return Err(format!(
                "\"{}\" is not an absolute path. Pass the full path, e.g. D:/Videos/stream.mp4 (relative paths are refused).",
                path.display()
            ));
        }
        if path.components().any(|c| c == Component::ParentDir) {
            return Err(format!(
                "\"{}\" contains \"..\". Pass a normalized absolute path.",
                path.display()
            ));
        }
        for set in self.constraints() {
            if !set.iter().any(|root| is_within(path, root)) {
                return Err(format!(
                    "\"{}\" is outside the folders this server may use (effective: {}). Choose a path inside them.",
                    path.display(),
                    self.describe()
                ));
            }
        }
        Ok(path.to_path_buf())
    }
}

/// Checks the extension and kind of a video input path.
///
/// # Errors
/// A specific message for non-video inputs.
pub fn check_video_extension(path: &Path) -> Result<(), String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        return Ok(());
    }
    let what = match ext.as_str() {
        "rustcut" => "a RustCut project file; pass the video it belongs to instead",
        "srt" | "vtt" | "ass" | "ssa" => "a subtitle file; pass the video instead",
        "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "opus" => {
            "an audio file; RustCut clips video files"
        }
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" => "an image, not a video",
        "" if path.is_dir() => "a folder, not a video file",
        "" => "a file without an extension",
        _ => "not a known video type",
    };
    Err(format!(
        "\"{}\" is {what}. Supported video types: {}.",
        path.display(),
        VIDEO_EXTENSIONS.join(", ")
    ))
}

/// Resolves symlinks of the deepest existing ancestor, lowercases on Windows.
fn resolve(path: &Path) -> PathBuf {
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = path.to_path_buf();
    let base = loop {
        if let Ok(c) = std::fs::canonicalize(&cur) {
            break c;
        }
        match (cur.file_name().map(|n| n.to_owned()), cur.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                cur = parent.to_path_buf();
            }
            _ => break path.to_path_buf(),
        }
    };
    let mut out = base;
    for part in rest.into_iter().rev() {
        out.push(part);
    }
    let s = out.to_string_lossy().into_owned();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
    if cfg!(windows) {
        PathBuf::from(s.to_lowercase())
    } else {
        PathBuf::from(s)
    }
}

/// Whether `path` is `root` or below it (symlinks resolved, case-insensitive on Windows).
pub fn is_within(path: &Path, root: &Path) -> bool {
    resolve(path).starts_with(resolve(root))
}

/// `file:///D:/Videos` -> `D:/Videos` (percent-decoded).
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let raw = rest.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() + 1 && raw.len() >= i + 3 {
            let hex = std::str::from_utf8(&raw[i + 1..i + 3]).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    let mut s = String::from_utf8(bytes).ok()?;
    if cfg!(windows) {
        let b = s.as_bytes();
        if b.len() > 2 && b[0] == b'/' && b[2] == b':' {
            s.remove(0);
        }
    }
    Some(PathBuf::from(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abs(p: &str) -> String {
        if cfg!(windows) {
            format!("C:/{p}")
        } else {
            format!("/{p}")
        }
    }

    #[test]
    fn relative_and_parent_paths_are_refused() {
        let p = Policy::new(None);
        assert!(p.check("videos/a.mp4").unwrap_err().contains("absolute"));
        assert!(p.check("").is_err());
        assert!(p.check(&abs("v/../x.mp4")).unwrap_err().contains(".."));
        assert!(p.check(&abs("v/a.mp4")).is_ok());
    }

    #[test]
    fn roots_confine_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("videos");
        std::fs::create_dir_all(&root).unwrap();
        let p = Policy::new(Some(vec![root.clone()]));
        assert!(p.check_path(&root.join("a.mp4")).is_ok());
        assert!(p.check_path(&root.join("sub").join("clips")).is_ok());
        let outside = dir.path().join("other").join("a.mp4");
        let err = p.check_path(&outside).unwrap_err();
        assert!(err.contains("outside"), "{err}");
        // A sibling folder sharing a prefix is not inside the root.
        assert!(
            p.check_path(&dir.path().join("videos2").join("a.mp4"))
                .is_err()
        );
    }

    #[test]
    fn client_and_env_roots_both_apply() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let p = Policy::new(Some(vec![dir.path().to_path_buf()]));
        assert!(p.check_path(&a.join("x.mp4")).is_ok());
        p.set_client_roots(vec![b.clone()]);
        assert!(p.check_path(&a.join("x.mp4")).is_err());
        assert!(p.check_path(&b.join("x.mp4")).is_ok());
    }

    #[test]
    fn refusal_lists_only_the_effective_folders() {
        let dir = tempfile::tempdir().unwrap();
        let (review, other) = (
            dir.path().join("review"),
            dir.path().join("review").join("other"),
        );
        std::fs::create_dir_all(&other).unwrap();
        let p = Policy::new(Some(vec![review.clone()]));
        p.set_client_roots(vec![other.clone()]);
        let err = p
            .check_path(&review.join("media").join("a.mp4"))
            .unwrap_err();
        assert!(
            err.contains("other") && !err.contains(&format!("{}, ", review.display())),
            "{err}"
        );
        assert_eq!(p.describe(), other.display().to_string());
        let q = Policy::new(Some(vec![dir.path().join("x")]));
        q.set_client_roots(vec![dir.path().join("y")]);
        assert!(q.describe().contains("do not overlap"));
        assert!(!Policy::new(None).is_restricted());
    }

    #[test]
    fn folder_is_not_called_a_file_without_extension() {
        let dir = tempfile::tempdir().unwrap();
        let e = check_video_extension(dir.path()).unwrap_err();
        assert!(e.contains("folder"), "{e}");
    }

    #[test]
    fn video_extension_messages_are_specific() {
        assert!(check_video_extension(Path::new("a.MKV")).is_ok());
        let e = check_video_extension(Path::new("a.srt")).unwrap_err();
        assert!(e.contains("subtitle"));
        assert!(
            check_video_extension(Path::new("a.mp3"))
                .unwrap_err()
                .contains("audio")
        );
        assert!(
            check_video_extension(Path::new("a.exe"))
                .unwrap_err()
                .contains("not a known video")
        );
    }

    #[test]
    fn file_uris_decode() {
        let got = uri_to_path("file:///D:/My%20Videos").unwrap();
        if cfg!(windows) {
            assert_eq!(got, PathBuf::from("D:/My Videos"));
        } else {
            assert_eq!(got, PathBuf::from("/D:/My Videos"));
        }
        assert!(uri_to_path("http://x").is_none());
        assert!(uri_to_path("file:///a%2").is_some());
    }
}
