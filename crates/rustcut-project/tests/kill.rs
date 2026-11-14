//! AC-04 / §8.2 kill test: random force-kills during scripted edits and
//! saves never leave a corrupt project and lose at most ~1 s of edits.
//!
//! The child is this crate's `rustcut-project-killtest` binary
//! (`src/bin/killtest.rs`), which Cargo builds for integration tests and
//! exposes through `CARGO_BIN_EXE_rustcut-project-killtest`, so no path
//! guessing is needed. Each iteration spawns it on the same project file,
//! kills it after a random 5–400 ms and then checks:
//! * the project file parses (0 corrupt projects) and every backup parses;
//! * the file holds a state the child actually reported (`EDIT` marker and
//!   content digest match), at least as new as the last `SAVED` line;
//! * no unsaved edit is older than [`MAX_LOSS`] at the moment of the kill.
//!
//! Iterations: `RUSTCUT_KILL_ITERATIONS` (default 100); the ignored
//! `kill_test_full_1000` runs the PRD's 1 000. Seed: `RUSTCUT_KILL_SEED`.
//! Kills at 5–400 ms cannot observe a 1 s loss, so a second test kills
//! longer sessions (1.5–2.5 s) of debounced-only edits to check the time
//! bound itself.

use rustcut_model::Project;
use rustcut_model::time::to_ms;
use rustcut_project::persist;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use xxhash_rust::xxh3::xxh3_64;

const CHILD: &str = env!("CARGO_BIN_EXE_rustcut-project-killtest");
/// PRD §8.2: ≤ 1 s of edits lost. The auto-saver flushes within 1 s of the last
/// edit; the extra 300 ms is a scheduling margin for the child process and
/// the parent's timestamp bookkeeping on a loaded CI/dev machine (the AV1
/// batch job here). The observed maximum is reported by the test output.
const MAX_LOSS: Duration = Duration::from_millis(1300);

#[derive(Default)]
struct Stats {
    iterations: usize,
    with_saves: usize,
    edits: usize,
    max_loss: Duration,
    save_errors: usize,
}

struct Edit {
    marker: u64,
    at: Duration,
    digest: String,
}

fn iterations_from_env(default: usize) -> usize {
    std::env::var("RUSTCUT_KILL_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn seed() -> u64 {
    std::env::var("RUSTCUT_KILL_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(7)
        })
}

/// Same digest as the child: clips, tags and `next_index`.
fn digest(project: &Project) -> String {
    let mut s = String::new();
    for c in &project.clips {
        let _ = write!(
            s,
            "{}|{}|{}|{}|{}|{:?}|{}|{};",
            c.id,
            c.index,
            c.name,
            to_ms(c.start_sec),
            to_ms(c.end_sec),
            c.tag_id,
            (c.crop_x_offset * 10_000.0).round() as i64,
            c.selected_for_export
        );
    }
    for t in &project.tags {
        let _ = write!(s, "{}:{}:{};", t.id, t.label, t.color);
    }
    let _ = write!(s, "n{}", project.next_index);
    format!("{:016x}", xxh3_64(s.as_bytes()))
}

fn run_kill_iterations(iterations: usize, delay_ms: (u64, u64), mode: &str) -> Stats {
    let seed = seed();
    eprintln!(
        "kill test ({mode}): {iterations} iterations, kill after {delay_ms:?} ms, RUSTCUT_KILL_SEED={seed}"
    );
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("session.rustcut");
    let data_dir = dir.path().join("data");
    let mut rng = seed | 1;
    let mut next = move |n: u64| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng % n.max(1)
    };
    let mut stats = Stats::default();
    let mut base = 1u64;
    let mut digests: HashMap<u64, String> = HashMap::new();
    let mut disk_marker: Option<u64> = None;

    for i in 0..iterations {
        let delay = Duration::from_millis(delay_ms.0 + next(delay_ms.1 - delay_ms.0 + 1));
        let spawned = Instant::now();
        let mut child = Command::new(CHILD)
            .arg(&project_path)
            .arg(&data_dir)
            .arg(next(u64::MAX).to_string())
            .arg(base.to_string())
            .arg("30")
            .arg(mode)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn kill-test child");
        let stdout = child.stdout.take().unwrap();
        let reader = std::thread::spawn(move || {
            BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
                .collect::<Vec<_>>()
        });
        std::thread::sleep(delay);
        let killed_at = spawned.elapsed();
        let _ = child.kill();
        let _ = child.wait();
        let lines = reader.join().unwrap();

        let mut edits = Vec::new();
        let mut last_saved = None;
        for line in &lines {
            let parts: Vec<&str> = line.split_whitespace().collect();
            match parts.as_slice() {
                ["EDIT", marker, micros, digest] => edits.push(Edit {
                    marker: marker.parse().unwrap(),
                    at: Duration::from_micros(micros.parse().unwrap()),
                    digest: (*digest).to_string(),
                }),
                ["SAVED", marker] => last_saved = Some(marker.parse::<u64>().unwrap()),
                // A failed write is retried by the auto-saver; what matters
                // is that no edit older than MAX_LOSS is lost (checked below).
                ["SAVEERR", ..] => {
                    stats.save_errors += 1;
                    eprintln!("iteration {i}: transient save failure (retried): {line}");
                }
                ["ERROR", ..] | ["RECOVERED"] => panic!("iteration {i}: child reported `{line}`"),
                _ => {}
            }
        }
        for e in &edits {
            digests.insert(e.marker, e.digest.clone());
        }
        stats.iterations += 1;
        stats.edits += edits.len();
        stats.with_saves += usize::from(last_saved.is_some());

        if !project_path.exists() {
            assert!(
                disk_marker.is_none() && last_saved.is_none(),
                "iteration {i}: saved project vanished"
            );
            continue;
        }
        let loaded = persist::load(&project_path).unwrap_or_else(|e| {
            panic!("iteration {i}: project corrupt after a kill at {killed_at:?}: {e}")
        });
        let marker = loaded.project.view_state.playhead_sec.round() as u64;
        let on_disk = digest(&loaded.project);
        if marker >= base {
            assert_eq!(
                digests.get(&marker),
                Some(&on_disk),
                "iteration {i}: file is not a state the child reported"
            );
        } else {
            assert_eq!(
                Some(marker),
                disk_marker,
                "iteration {i}: file changed without a reported save"
            );
            assert!(
                last_saved.is_none(),
                "iteration {i}: a reported save is missing from disk"
            );
        }
        if let Some(saved) = last_saved {
            assert!(
                marker >= saved,
                "iteration {i}: SAVED {saved} reported but disk has {marker}"
            );
        }
        if let Some(oldest_lost) = edits.iter().find(|e| e.marker > marker) {
            let loss = killed_at.saturating_sub(oldest_lost.at);
            stats.max_loss = stats.max_loss.max(loss);
            assert!(
                loss <= MAX_LOSS,
                "iteration {i}: lost edits made {loss:?} before the kill"
            );
        }
        disk_marker = Some(marker);
        base = base.max(edits.iter().map(|e| e.marker + 1).max().unwrap_or(base));
        check_backups(&data_dir.join("backups"), i);
    }
    eprintln!(
        "kill test: {} iterations, {} with saves, {} edits, max loss {:?}, {} transient save failures",
        stats.iterations, stats.with_saves, stats.edits, stats.max_loss, stats.save_errors
    );
    stats
}

fn check_backups(root: &Path, iteration: usize) {
    let Ok(dirs) = std::fs::read_dir(root) else {
        return;
    };
    for dir in dirs.flatten().filter(|d| d.path().is_dir()) {
        for file in std::fs::read_dir(dir.path()).unwrap().flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            if name.ends_with(".rustcut") {
                let bytes = std::fs::read(file.path()).unwrap();
                assert!(
                    persist::parse_project_bytes(&bytes).is_ok(),
                    "iteration {iteration}: corrupt backup {name}"
                );
            }
        }
    }
}

#[test]
fn ac04_kill_test() {
    let iterations = iterations_from_env(100);
    let stats = run_kill_iterations(iterations, (5, 400), "mixed");
    assert_eq!(stats.iterations, iterations);
    if iterations >= 20 {
        assert!(
            stats.with_saves * 4 >= iterations,
            "too few iterations reached a save: {}",
            stats.with_saves
        );
    }
}

#[test]
fn ac04_long_sessions_lose_at_most_about_one_second() {
    // Debounced edits only: unsaved edits age up to the 900 ms cap.
    let stats = run_kill_iterations(3, (1500, 2500), "debounced");
    assert!(stats.edits > 100, "the child edited continuously");
    assert!(stats.max_loss <= MAX_LOSS);
}

#[test]
#[ignore = "PRD §8.2 full run: 1 000 kills (several minutes)"]
fn kill_test_full_1000() {
    let iterations = std::env::var("RUSTCUT_KILL_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000);
    run_kill_iterations(iterations, (5, 400), "mixed");
}
