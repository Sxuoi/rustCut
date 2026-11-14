//! LRU cache of remux and proxy files (PRD section 7.3: default cap 50 GB, user-clearable).
//!
//! Layout under the cache directory: `remux/<fingerprint key>.mp4`, `proxy/<fingerprint key>.mp4`;
//! in-progress files are `*.mp4.part`. Recency is the later of the file's modification time
//! (touched on use) and the in-memory last-use stamp, so recency survives file systems that
//! refuse a timestamp update on a file that a player has open.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Sub-folders of the cache directory that hold large playback files.
pub(crate) const PLAYBACK_DIRS: [&str; 2] = ["remux", "proxy"];

/// Stale `.part` files (a crash mid-copy) older than this are removed.
const STALE_PART_AGE: Duration = Duration::from_secs(24 * 3600);

/// One cached playback file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    /// Absolute path of the file.
    pub path: PathBuf,
    /// Size in bytes.
    pub size: u64,
    /// When the file was last used (written or opened by the player).
    pub last_used: SystemTime,
}

/// Which files to delete so that the entries fit into `cap` bytes: least recently used first,
/// skipping `protected` files (in use). Pure; the caller deletes. A protected file may leave the
/// cache above `cap`, which is intended (an open file is never deleted).
pub fn plan_evictions(
    entries: &[CacheEntry],
    cap: u64,
    protected: &HashSet<PathBuf>,
) -> Vec<PathBuf> {
    let mut total: u64 = entries.iter().map(|e| e.size).sum();
    let mut order: Vec<&CacheEntry> = entries.iter().collect();
    order.sort_by(|a, b| {
        a.last_used
            .cmp(&b.last_used)
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut out = Vec::new();
    for e in order {
        if total <= cap {
            break;
        }
        if protected.contains(&e.path) {
            continue;
        }
        total -= e.size;
        out.push(e.path.clone());
    }
    out
}

/// Lists the finished playback files of `cache_dir`, deleting stale `.part` files on the way.
pub(crate) fn scan(cache_dir: &Path, stamps: &HashMap<PathBuf, SystemTime>) -> Vec<CacheEntry> {
    let mut out = Vec::new();
    for sub in PLAYBACK_DIRS {
        let Ok(read) = fs::read_dir(cache_dir.join(sub)) else {
            continue;
        };
        for item in read.flatten() {
            let path = item.path();
            let Ok(meta) = item.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            if path.extension().is_some_and(|e| e == "part") {
                let age = SystemTime::now().duration_since(mtime).unwrap_or_default();
                if age > STALE_PART_AGE {
                    let _ = fs::remove_file(&path);
                }
                continue;
            }
            let last_used = stamps.get(&path).map_or(mtime, |s| (*s).max(mtime));
            out.push(CacheEntry {
                path,
                size: meta.len(),
                last_used,
            });
        }
    }
    out
}

/// Marks `path` as used now (best effort: a file that a player holds open may refuse it).
pub(crate) fn touch(path: &Path) {
    let result = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(SystemTime::now()));
    if let Err(err) = result {
        tracing::debug!(path = %path.display(), %err, "could not update the cache timestamp");
    }
}

/// Deletes `paths`, returning the bytes actually freed (failures are logged and skipped).
pub(crate) fn delete_all(paths: &[PathBuf]) -> u64 {
    let mut freed = 0;
    for p in paths {
        let size = fs::metadata(p).map_or(0, |m| m.len());
        match fs::remove_file(p) {
            Ok(()) => freed += size,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => tracing::warn!(path = %p.display(), %err, "could not delete a cache file"),
        }
    }
    freed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, size: u64, age_secs: u64) -> CacheEntry {
        CacheEntry {
            path: PathBuf::from(name),
            size,
            last_used: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 - age_secs),
        }
    }

    #[test]
    fn evicts_least_recently_used_first_until_under_cap() {
        let entries = [
            entry("new", 40, 10),
            entry("old", 40, 1000),
            entry("mid", 40, 500),
        ];
        let none = HashSet::new();
        assert_eq!(plan_evictions(&entries, 120, &none), Vec::<PathBuf>::new());
        assert_eq!(
            plan_evictions(&entries, 100, &none),
            vec![PathBuf::from("old")]
        );
        assert_eq!(
            plan_evictions(&entries, 50, &none),
            vec![PathBuf::from("old"), PathBuf::from("mid")]
        );
        assert_eq!(plan_evictions(&entries, 0, &none).len(), 3);
    }

    #[test]
    fn files_in_use_are_never_evicted() {
        let entries = [entry("a", 60, 900), entry("b", 60, 100)];
        let protect: HashSet<PathBuf> = [PathBuf::from("a")].into();
        assert_eq!(
            plan_evictions(&entries, 60, &protect),
            vec![PathBuf::from("b")]
        );
        assert_eq!(
            plan_evictions(&entries, 0, &protect),
            vec![PathBuf::from("b")]
        );
    }

    #[test]
    fn scan_uses_the_later_of_mtime_and_stamp_and_skips_part_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("remux")).unwrap();
        let f = dir.path().join("remux/a.mp4");
        fs::write(&f, [0u8; 10]).unwrap();
        fs::write(dir.path().join("remux/b.mp4.part"), [0u8; 10]).unwrap();
        let future = SystemTime::now() + Duration::from_secs(3600);
        let stamps = HashMap::from([(f.clone(), future)]);
        let entries = scan(dir.path(), &stamps);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].last_used, future);
        assert_eq!(entries[0].size, 10);
    }
}
