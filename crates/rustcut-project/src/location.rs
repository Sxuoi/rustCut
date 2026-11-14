//! Where project files live (PRJ-02), finding the project of a source
//! (PRJ-03) and relink helpers (PRJ-04).
//!
//! * Next to the video: `<source stem>.rustcut`; if that file belongs to a
//!   different source (e.g. `talk.mp4` and `talk.mkv` side by side),
//!   `<source file name>.rustcut`.
//! * App library: `<data dir>/library/<fingerprint key>.rustcut`, used when
//!   the video's folder is not writable or the user prefers it.

use crate::persist::{parent_dir, with_suffix};
use rustcut_model::{Fingerprint, ProjectLocation, SourceInfo, compute_fingerprint};
use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Project file extension (without the dot).
pub const PROJECT_EXTENSION: &str = "rustcut";
/// [`find_relink_candidate`] gives up in folders with more entries than this.
pub const RELINK_SCAN_LIMIT: usize = 2000;

/// `<dir>/<stem>.rustcut`, or `<dir>/<file name>.rustcut` when the former
/// exists but belongs to a source with a different file name.
pub fn project_file_next_to(source_path: &Path) -> PathBuf {
    let primary = stem_project_file(source_path);
    if primary.exists()
        && project_source_name(&primary).is_some_and(|name| !same_file_name(&name, source_path))
    {
        name_project_file(source_path)
    } else {
        primary
    }
}

/// `<data_dir>/library/<fingerprint key>.rustcut`.
pub fn library_file(data_dir: &Path, fingerprint: &Fingerprint) -> PathBuf {
    data_dir
        .join("library")
        .join(format!("{}.{PROJECT_EXTENSION}", fingerprint.key()))
}

/// True when a file can be created in `dir` (probe file, removed again).
/// Works for read-only drives, camera cards and permission problems alike.
pub fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(
        ".rustcut-write-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let created = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .is_ok();
    if created {
        let _ = fs::remove_file(&probe);
    }
    created
}

/// PRJ-02: location for a new project. Next to the video when preferred and
/// `is_writable(<video folder>)`, else the app library (the status bar then
/// says where). `is_writable` is injectable for tests; the app passes
/// [`dir_is_writable`].
pub fn resolve_new_project_location(
    source_path: &Path,
    fingerprint: &Fingerprint,
    pref: ProjectLocation,
    data_dir: &Path,
    is_writable: &dyn Fn(&Path) -> bool,
) -> (PathBuf, ProjectLocation) {
    if pref == ProjectLocation::NextToVideo && is_writable(&parent_dir(source_path)) {
        (
            project_file_next_to(source_path),
            ProjectLocation::NextToVideo,
        )
    } else {
        (
            library_file(data_dir, fingerprint),
            ProjectLocation::AppLibrary,
        )
    }
}

/// PRJ-03: the existing project of a source: next to the video first
/// (`<stem>.rustcut` when it names this file or carries this fingerprint,
/// then `<file name>.rustcut`), then the app library by fingerprint. A
/// project found next to the video may still have a different fingerprint
/// (re-recorded file); the caller compares and warns (PRJ-04).
pub fn find_existing_project(
    source_path: &Path,
    fingerprint: &Fingerprint,
    data_dir: &Path,
) -> Option<(PathBuf, ProjectLocation)> {
    let primary = stem_project_file(source_path);
    if primary.is_file() {
        let info = project_source_info(&primary);
        let ours = match &info {
            None => true, // unreadable: let loading recover it from backups
            Some((name, fp)) => {
                same_file_name(name, source_path) || fp.as_ref() == Some(fingerprint)
            }
        };
        if ours {
            return Some((primary, ProjectLocation::NextToVideo));
        }
    }
    let by_name = name_project_file(source_path);
    if by_name.is_file() {
        return Some((by_name, ProjectLocation::NextToVideo));
    }
    let library = library_file(data_dir, fingerprint);
    library
        .is_file()
        .then_some((library, ProjectLocation::AppLibrary))
}

/// PRJ-04 *Locate Missing Media* suggestion: a file in `dir` with the same
/// fingerprint (files of the same size are hashed, in name order). Gives up
/// in folders with more than [`RELINK_SCAN_LIMIT`] entries.
pub fn find_relink_candidate(dir: &Path, fingerprint: &Fingerprint) -> Option<PathBuf> {
    find_relink_candidate_limited(dir, fingerprint, RELINK_SCAN_LIMIT)
}

pub(crate) fn find_relink_candidate_limited(
    dir: &Path,
    fingerprint: &Fingerprint,
    limit: usize,
) -> Option<PathBuf> {
    let entries: Vec<fs::DirEntry> = fs::read_dir(dir).ok()?.flatten().take(limit + 1).collect();
    if entries.len() > limit {
        return None;
    }
    let mut same_size: Vec<PathBuf> = entries
        .iter()
        .filter(|e| {
            e.metadata()
                .is_ok_and(|m| m.is_file() && m.len() == fingerprint.size_bytes)
        })
        .map(|e| e.path())
        .collect();
    same_size.sort();
    same_size
        .into_iter()
        .find(|p| compute_fingerprint(p).is_ok_and(|fp| &fp == fingerprint))
}

/// `source.relative_path` for a project at `project_path` (forward slashes;
/// just the file name when the two are on different drives).
pub fn relative_source_path(project_path: &Path, source_path: &Path) -> String {
    let file_name = || {
        source_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    relative_to(&parent_dir(project_path), source_path).unwrap_or_else(file_name)
}

/// PRJ-04: the source file of a project: `relative_path` (from the project's
/// folder; tried first because the whole folder may have moved), then the
/// absolute `path`. `None` when neither exists (*Locate Missing Media*).
pub fn resolve_source(project_path: &Path, source: &SourceInfo) -> Option<PathBuf> {
    let relative = (!source.relative_path.is_empty())
        .then(|| parent_dir(project_path).join(&source.relative_path));
    relative
        .filter(|p| p.is_file())
        .or_else(|| Some(PathBuf::from(&source.path)).filter(|p| p.is_file()))
}

fn stem_project_file(source_path: &Path) -> PathBuf {
    let mut name: OsString = source_path
        .file_stem()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{PROJECT_EXTENSION}"));
    source_path.with_file_name(name)
}

fn name_project_file(source_path: &Path) -> PathBuf {
    with_suffix(source_path, &format!(".{PROJECT_EXTENSION}"))
}

/// File name of the source recorded in a project file, if readable.
fn project_source_name(project_file: &Path) -> Option<String> {
    project_source_info(project_file).map(|(name, _)| name)
}

/// `(source file name, fingerprint)` recorded in a project file.
fn project_source_info(project_file: &Path) -> Option<(String, Option<Fingerprint>)> {
    let bytes = fs::read(project_file).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let source = value.get("source")?;
    let path = source.get("path").and_then(Value::as_str)?;
    let name = file_name_of(path);
    let fingerprint = source
        .get("fingerprint")
        .and_then(|f| serde_json::from_value(f.clone()).ok());
    Some((name, fingerprint))
}

/// File name of a stored path, accepting both separators on every OS.
fn file_name_of(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

fn same_file_name(recorded: &str, source_path: &Path) -> bool {
    source_path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().to_lowercase() == recorded.to_lowercase())
}

fn relative_to(base: &Path, target: &Path) -> Option<String> {
    let base = std::path::absolute(base).ok()?;
    let target = std::path::absolute(target).ok()?;
    let b: Vec<Component> = base.components().collect();
    let t: Vec<Component> = target.components().collect();
    let eq = |x: &Component, y: &Component| {
        if cfg!(windows) {
            x.as_os_str().to_string_lossy().to_lowercase()
                == y.as_os_str().to_string_lossy().to_lowercase()
        } else {
            x == y
        }
    };
    let common = b.iter().zip(&t).take_while(|(x, y)| eq(x, y)).count();
    if common == 0 || !matches!(b.first(), Some(Component::Prefix(_) | Component::RootDir)) {
        return None;
    }
    let mut parts: Vec<String> = vec!["..".to_string(); b.len() - common];
    parts.extend(
        t[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relink_scan_gives_up_in_large_folders() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"same bytes";
        for i in 0..5 {
            fs::write(dir.path().join(format!("f{i}.bin")), data).unwrap();
        }
        let fp = compute_fingerprint(&dir.path().join("f0.bin")).unwrap();
        assert!(find_relink_candidate_limited(dir.path(), &fp, 5).is_some());
        assert_eq!(find_relink_candidate_limited(dir.path(), &fp, 4), None);
    }

    #[test]
    fn relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("a").join("p.rustcut");
        assert_eq!(
            relative_source_path(&project, &dir.path().join("a").join("v.mp4")),
            "v.mp4"
        );
        assert_eq!(
            relative_source_path(&project, &dir.path().join("b").join("v.mp4")),
            "../b/v.mp4"
        );
        assert_eq!(file_name_of("D:\\Rec\\x.mp4"), "x.mp4");
        assert_eq!(file_name_of("/home/u/x.mp4"), "x.mp4");
    }
}
