//! PRJ-01 / AC-01 auto-save timing, flush, failure reporting and retry, backups.

mod common;

use common::*;
use rustcut_project::{AutoSaver, BackupStore, SaveOutcome, Urgency, load};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Log = Arc<Mutex<Vec<(Instant, SaveOutcome)>>>;

fn saver(backups: Option<BackupStore>) -> (AutoSaver, Log) {
    let log: Log = Arc::default();
    let sink = log.clone();
    let saver = AutoSaver::spawn(
        &tokio::runtime::Handle::current(),
        backups,
        Arc::new(move |o| sink.lock().unwrap().push((Instant::now(), o))),
    );
    (saver, log)
}

async fn wait_for(log: &Log, count: usize, limit: Duration) {
    let deadline = Instant::now() + limit;
    while log.lock().unwrap().len() < count && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac01_immediate_save_is_on_disk_well_within_one_second() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.rustcut");
    let (saver, log) = saver(None);
    let mut s = store(100.0);
    add(&mut s, 1.0, 2.0);
    let request = s.take_save_request().unwrap();
    assert_eq!(request.urgency, Urgency::Immediate);
    let t0 = Instant::now();
    saver.schedule(
        path.clone(),
        request.project,
        request.revision,
        request.urgency,
    );
    wait_for(&log, 1, Duration::from_secs(5)).await;
    let (at, outcome) = log.lock().unwrap()[0].clone();
    assert!(
        at - t0 < Duration::from_millis(500),
        "immediate save took {:?}",
        at - t0
    );
    assert_eq!(
        (outcome.revision, outcome.error, outcome.path),
        (1, None, path.clone())
    );
    assert!(outcome.saved_at.is_some());
    assert_eq!(load(&path).unwrap().project.clips.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prj01_debounced_burst_is_written_within_one_second_of_its_first_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.rustcut");
    let (saver, log) = saver(None);
    let project = project(100.0);
    let mut scheduled = Vec::new();
    let start = Instant::now();
    let mut revision = 0;
    while start.elapsed() < Duration::from_millis(2500) {
        revision += 1;
        scheduled.push((Instant::now(), revision));
        saver.schedule(path.clone(), project.clone(), revision, Urgency::Debounced);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    saver.flush().await;
    let log = log.lock().unwrap().clone();
    assert!(log.iter().all(|(_, o)| o.error.is_none()));
    assert!(
        log.len() >= 2 && log.len() <= 6,
        "debounced into a few writes, got {}",
        log.len()
    );
    // Every change is on disk within ~1 s (debounce cap 900 ms + write time).
    for (at, rev) in &scheduled {
        let saved = log
            .iter()
            .find(|(_, o)| o.revision >= *rev)
            .map(|(t, _)| *t)
            .unwrap();
        let delay = saved - *at;
        assert!(
            delay <= Duration::from_millis(1150),
            "revision {rev} saved after {delay:?}"
        );
    }
    let first_write = log[0].0 - scheduled[0].0;
    assert!(
        first_write >= Duration::from_millis(800),
        "a continuous burst is written at the cap, got {first_write:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_debounced_change_waits_for_the_quiet_period() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.rustcut");
    let (saver, log) = saver(None);
    let t0 = Instant::now();
    saver.schedule(path.clone(), project(10.0), 7, Urgency::Debounced);
    wait_for(&log, 1, Duration::from_secs(5)).await;
    let delay = log.lock().unwrap()[0].0 - t0;
    assert!(
        delay >= Duration::from_millis(240) && delay < Duration::from_millis(1000),
        "{delay:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flush_writes_pending_changes_now() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.rustcut");
    let (saver, log) = saver(None);
    assert_eq!(saver.flush().await, None, "nothing pending");
    let t0 = Instant::now();
    saver.schedule(path.clone(), project(10.0), 3, Urgency::Debounced);
    let outcome = saver.flush().await.unwrap();
    assert!(
        t0.elapsed() < Duration::from_millis(240),
        "flush does not wait for the debounce"
    );
    assert_eq!((outcome.revision, outcome.error.is_none()), (3, true));
    assert!(path.exists());
    assert_eq!(log.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_write_is_reported_and_retried_on_the_next_change() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "a file where a folder should be").unwrap();
    let path: PathBuf = blocker.join("p.rustcut");
    let (saver, log) = saver(None);
    saver.schedule(path.clone(), project(10.0), 1, Urgency::Immediate);
    let failed = saver.flush().await.unwrap();
    assert_eq!(failed.saved_at, None);
    assert!(
        failed.error.as_deref().unwrap().contains("Could not"),
        "{:?}",
        failed.error
    );

    // A flush retries the kept snapshot.
    let retried = saver.flush().await.unwrap();
    assert!(retried.error.is_some());

    std::fs::remove_file(&blocker).unwrap();
    saver.schedule(path.clone(), project(10.0), 2, Urgency::Immediate);
    let ok = saver.flush().await.unwrap();
    assert_eq!((ok.revision, ok.error), (2, None));
    assert!(path.exists());
    let log = log.lock().unwrap();
    assert!(log.len() >= 3, "every attempt is reported");
    assert!(log[..log.len() - 1].iter().all(|(_, o)| o.error.is_some()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backups_follow_successful_saves_and_are_throttled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.rustcut");
    let backups = BackupStore::in_data_dir(&dir.path().join("data"));
    let (saver, _log) = saver(Some(backups.clone()));
    let project = project(10.0);
    let key = project.source.fingerprint.key();
    saver.schedule(path.clone(), project.clone(), 1, Urgency::Immediate);
    saver.flush().await.unwrap();
    assert_eq!(backups.list(&key).len(), 1);
    saver.schedule(path.clone(), project, 2, Urgency::Immediate);
    saver.flush().await.unwrap();
    assert_eq!(
        backups.list(&key).len(),
        1,
        "second backup within 30 s is skipped"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_saver_writes_pending_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.rustcut");
    let (saver, log) = saver(None);
    saver.schedule(path.clone(), project(10.0), 1, Urgency::Debounced);
    drop(saver);
    wait_for(&log, 1, Duration::from_secs(5)).await;
    assert!(path.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_write_is_retried_by_itself_after_a_backoff() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "a file where a folder should be").unwrap();
    let path = blocker.join("p.rustcut");
    let (saver, log) = saver(None);
    saver.schedule(path.clone(), project(10.0), 1, Urgency::Immediate);
    wait_for(&log, 1, Duration::from_secs(5)).await;
    assert!(log.lock().unwrap()[0].1.error.is_some());
    std::fs::remove_file(&blocker).unwrap();
    // No new change and no flush: the snapshot is written by the automatic retry.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    wait_for(&log, 2, Duration::from_secs(1)).await;
    assert!(path.exists(), "retried without a new change");
    let log = log.lock().unwrap();
    assert_eq!(log.last().unwrap().1.error, None);
    let gap = log[log.len() - 1].0 - log[0].0;
    assert!(
        gap >= Duration::from_millis(450),
        "the retry waits for the backoff, got {gap:?}"
    );
    drop(saver);
}
