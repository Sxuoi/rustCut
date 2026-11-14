//! Child process of the AC-04 / §8.2 kill test (`tests/kill.rs`); not part
//! of the app.
//!
//! `rustcut-project-killtest <project> <data dir> <seed> <marker base> [max seconds] [mixed|debounced]`
//!
//! Opens or creates the project the way the app does (lock with retry, load
//! with backup recovery, `normalize_on_load`), then performs random edits
//! with random pauses, auto-saving through [`AutoSaver`]. After each edit it
//! stores a unique marker (`marker base + revision`) in
//! `view_state.playhead_sec` and prints `EDIT <marker> <µs since start>
//! <digest>`; each successful write prints `SAVED <marker>`. The parent
//! kills it at a random moment and checks the file on disk against these
//! lines. It exits by itself after `max seconds` (default 30) so it can
//! never outlive a crashed test.

use rustcut_model::time::to_ms;
use rustcut_model::{
    AddClipRequest, ClipEdit, Fingerprint, Project, SourceInfo, VideoInfo, ViewState,
};
use rustcut_project::{
    AutoSaver, BackupStore, LockOutcome, ProjectLock, ProjectStore, SaveOutcome, StoreOptions,
    load_with_recovery,
};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};
use xxhash_rust::xxh3::xxh3_64;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!(
            "usage: rustcut-project-killtest <project> <data dir> <seed> <marker base> [max seconds] [mixed|debounced]"
        );
        return ExitCode::from(2);
    }
    let project_path = PathBuf::from(&args[1]);
    let data_dir = PathBuf::from(&args[2]);
    let seed: u64 = args[3].parse().unwrap_or(1);
    let base: u64 = args[4].parse().unwrap_or(0);
    let max_secs: u64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(30);
    let debounced_only = args.get(6).is_some_and(|m| m == "debounced");
    match run(
        &project_path,
        &data_dir,
        seed,
        base,
        Duration::from_secs(max_secs),
        debounced_only,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            println!("ERROR {message}");
            ExitCode::from(3)
        }
    }
}

fn run(
    project_path: &Path,
    data_dir: &Path,
    seed: u64,
    base: u64,
    max_time: Duration,
    debounced_only: bool,
) -> Result<(), String> {
    let started = Instant::now();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;

    // The killed predecessor's OS lock is released asynchronously; retry briefly.
    let _lock = loop {
        match ProjectLock::try_acquire(project_path).map_err(|e| format!("lock: {e}"))? {
            LockOutcome::Acquired(lock) => break lock,
            LockOutcome::HeldElsewhere { holder } if started.elapsed() > Duration::from_secs(5) => {
                return Err(format!("lock held by {holder:?}"));
            }
            LockOutcome::HeldElsewhere { .. } => std::thread::sleep(Duration::from_millis(20)),
        }
    };

    let backups = BackupStore::in_data_dir(data_dir);
    let project = if project_path.exists() {
        let loaded =
            load_with_recovery(project_path, &backups).map_err(|e| format!("load: {e}"))?;
        if loaded.recovered_from.is_some() {
            println!("RECOVERED");
        }
        loaded.project
    } else {
        Project::new(synthetic_source(project_path))
    };
    let mut store = ProjectStore::new(project, None);
    store.normalize_on_load();
    let _ = store.take_save_request();

    let on_result = Arc::new(|outcome: SaveOutcome| match outcome.error {
        None => println!("SAVED {}", outcome.revision),
        Some(error) => println!("SAVEERR {} {error}", outcome.revision),
    });
    let saver = AutoSaver::spawn(runtime.handle(), Some(backups), on_result);
    println!("START {base}");

    let mut rng = Rng::new(seed);
    while started.elapsed() < max_time {
        let before = store.revision();
        random_edit(&mut store, &mut rng, debounced_only);
        if store.revision() != before {
            let marker = base + store.revision();
            let view = ViewState {
                playhead_sec: marker as f64,
                ..store.project().view_state.clone()
            };
            store.set_view_state(view);
            if let Some(request) = store.take_save_request() {
                saver.schedule(
                    project_path.to_path_buf(),
                    request.project,
                    marker,
                    request.urgency,
                );
            }
            println!(
                "EDIT {marker} {} {}",
                started.elapsed().as_micros(),
                digest(store.project())
            );
        }
        std::thread::sleep(Duration::from_millis(rng.below(13)));
    }
    runtime.block_on(saver.flush());
    println!("DONE");
    Ok(())
}

/// `debounced_only`: after the first add, only edits that save debounced
/// (rename, retime, tag, include, crop), so the 1 s cap path is exercised.
fn random_edit(store: &mut ProjectStore, rng: &mut Rng, debounced_only: bool) {
    let opts = StoreOptions::default();
    let ids: Vec<_> = store.project().clips.iter().map(|c| c.id).collect();
    let tags = [None, Some("funny"), Some("edu"), Some("highlight")];
    let roll = if debounced_only {
        35 + rng.below(45)
    } else {
        rng.below(100)
    };
    if ids.is_empty() || roll < 35 {
        let start = rng.below(9_000_000) as f64 / 1000.0;
        let request = AddClipRequest {
            start_sec: start,
            end_sec: start + 0.5 + rng.below(60_000) as f64 / 1000.0,
            name: Some(format!("clip {}", rng.below(1000))),
            tag_id: tags[rng.below(4) as usize].map(str::to_string),
        };
        let _ = store.add_clip(&request, &opts);
        return;
    }
    let id = ids[rng.below(ids.len() as u64) as usize];
    let edit = match roll {
        35..=49 => ClipEdit::Rename {
            name: format!("renamed {}", rng.below(1000)),
        },
        50..=59 => ClipEdit::Retime {
            start_sec: Some(rng.below(9_000_000) as f64 / 1000.0),
            end_sec: None,
        },
        60..=67 => ClipEdit::SetTag {
            tag_id: tags[rng.below(4) as usize].map(str::to_string),
        },
        68..=74 => ClipEdit::SetIncluded {
            included: rng.below(2) == 0,
        },
        75..=79 => ClipEdit::SetCrop {
            crop_x_offset: rng.below(20_001) as f64 / 10_000.0 - 1.0,
        },
        80..=89 => {
            let _ = store.delete_clips(&[id]);
            return;
        }
        90..=96 => {
            let _ = store.undo();
            return;
        }
        _ => {
            let _ = store.redo();
            return;
        }
    };
    let _ = store.edit_clips(&[id], &edit, &opts);
}

/// Content digest of the clips, tags and `next_index` (view state excluded).
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

fn synthetic_source(project_path: &Path) -> SourceInfo {
    SourceInfo {
        path: project_path
            .with_extension("mp4")
            .to_string_lossy()
            .into_owned(),
        relative_path: "session.mp4".into(),
        fingerprint: Fingerprint {
            size_bytes: 123_456_789,
            xxh3_head_tail: "00112233445566aa".into(),
        },
        duration_sec: 10_000_000.0,
        start_time_sec: 0.0,
        video: VideoInfo {
            codec: "h264".into(),
            coded_width: 1920,
            coded_height: 1080,
            display_width: 1920,
            display_height: 1080,
            rotation: 0,
            sar: "1:1".into(),
            frame_rate: "60/1".into(),
            vfr: false,
            hdr: false,
            stream_index: Some(0),
            pix_fmt: None,
            profile: None,
            level: None,
            color_transfer: None,
            bit_depth: None,
        },
        audio_tracks: Vec::new(),
        format_name: None,
        bit_rate: None,
    }
}

/// xorshift64* (no extra dependency needed for a test driver).
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn below(&mut self, n: u64) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        if n == 0 {
            0
        } else {
            x.wrapping_mul(0x2545_F491_4F6C_DD1D) % n
        }
    }
}
