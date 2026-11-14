//! Source fingerprint (PRD §7.4 `xxhash-rust`, PRJ-04): xxh3-64 over the
//! file size (u64 little-endian), the first 1 MiB and the last 1 MiB.
//! Reads at most 2 MiB regardless of file size (AC-14 budget).

use crate::source::Fingerprint;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use xxhash_rust::xxh3::Xxh3;

pub const FINGERPRINT_CHUNK: u64 = 1024 * 1024;

pub fn compute_fingerprint(path: &Path) -> io::Result<Fingerprint> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut hasher = Xxh3::new();
    hasher.update(&size.to_le_bytes());

    let head_len = size.min(FINGERPRINT_CHUNK) as usize;
    let mut buf = vec![0u8; head_len];
    file.read_exact(&mut buf)?;
    hasher.update(&buf);

    let tail_len = size.min(FINGERPRINT_CHUNK);
    file.seek(SeekFrom::Start(size - tail_len))?;
    buf.resize(tail_len as usize, 0);
    file.read_exact(&mut buf)?;
    hasher.update(&buf);

    Ok(Fingerprint {
        size_bytes: size,
        xxh3_head_tail: format!("{:016x}", hasher.digest()),
    })
}

impl Fingerprint {
    /// Stable key used for app-library file names and caches: `<hash>-<size>`.
    pub fn key(&self) -> String {
        format!("{}-{}", self.xxh3_head_tail, self.size_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn same_content_same_fingerprint_and_size_matters() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        let b = dir.path().join("b.bin");
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        File::create(&a).unwrap().write_all(&data).unwrap();
        File::create(&b).unwrap().write_all(&data).unwrap();
        let fa = compute_fingerprint(&a).unwrap();
        assert_eq!(fa, compute_fingerprint(&b).unwrap());
        assert_eq!(fa.xxh3_head_tail.len(), 16);
        // Truncation (PRJ-04 "truncated file") changes the fingerprint.
        File::create(&b)
            .unwrap()
            .write_all(&data[..2_500_000])
            .unwrap();
        assert_ne!(fa, compute_fingerprint(&b).unwrap());
    }

    #[test]
    fn tiny_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.bin");
        File::create(&p).unwrap().write_all(b"abc").unwrap();
        let f = compute_fingerprint(&p).unwrap();
        assert_eq!(f.size_bytes, 3);
    }
}
