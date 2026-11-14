//! Integration tests against real FFmpeg and the testkit fixtures (PLY-01/05/06, section 7.3,
//! 8.1, 9, EXP-13). They skip with a message when FFmpeg is not installed.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rustcut_media::{
    CancelToken, MediaConfig, MediaError, MediaService, ProbedSource, total_peaks_for,
};
use rustcut_model::{EncoderChoice, IndexSource, PlaybackTier, SourceInfo, WaveformChunk};
use rustcut_runner::Sidecars;
use rustcut_testkit::{
    Fixture, ffmpeg_ok, fixture, frame_count, keyframe_times, temp_dir, try_ffmpeg, video_size,
};

struct Env {
    svc: MediaService,
    dir: PathBuf,
}

impl Env {
    fn cache(&self) -> PathBuf {
        self.dir.join("cache")
    }
}

fn env_with_cap(cap: u64) -> Option<Env> {
    let Some(ffmpeg) = try_ffmpeg() else {
        eprintln!("SKIP: FFmpeg not found (set RUSTCUT_FFMPEG_DIR)");
        return None;
    };
    let sidecars =
        Sidecars::from_dir(ffmpeg.parent().expect("ffmpeg has a folder")).expect("sidecars");
    let dir = temp_dir();
    let svc = MediaService::new(MediaConfig {
        sidecars,
        cache_dir: dir.join("cache"),
        data_dir: dir.join("data"),
        cache_cap_bytes: cap,
    });
    Some(Env { svc, dir })
}

fn env() -> Option<Env> {
    env_with_cap(50 * 1024 * 1024 * 1024)
}

async fn probe(env: &Env, f: Fixture) -> ProbedSource {
    env.svc
        .probe_source(&fixture(f))
        .await
        .unwrap_or_else(|e| panic!("probe {f:?}: {e}"))
}

// ---------------------------------------------------------------- PLY-01 probe

#[tokio::test(flavor = "multi_thread")]
async fn ply01_probe_fixtures_key_expectations() {
    let Some(env) = env() else { return };

    let hevc = probe(&env, Fixture::HevcHlgRotated).await;
    let v = &hevc.source.video;
    // the probe parser reports the clockwise rotation: ffprobe's display-matrix 90 is 270 clockwise
    assert!(matches!(v.rotation, 90 | 270), "{}", v.rotation);
    assert_eq!((v.display_width, v.display_height), (360, 640));
    assert!(v.hdr);
    assert!(
        hevc.warnings.iter().any(|w| w.contains("HDR")),
        "{:?}",
        hevc.warnings
    );
    assert!(
        hevc.mime_type.contains("hvc1") || hevc.mime_type.contains("hev1"),
        "{}",
        hevc.mime_type
    );

    let sar = probe(&env, Fixture::RampSar).await;
    assert_eq!(
        (
            sar.source.video.display_width,
            sar.source.video.display_height
        ),
        (768, 576)
    );

    let st = probe(&env, Fixture::StartTimeNonZero).await;
    assert!(
        (9.5..10.5).contains(&st.source.start_time_sec),
        "{}",
        st.source.start_time_sec
    );

    let obs = probe(&env, Fixture::ObsMkv4Audio).await;
    assert_eq!(obs.source.audio_tracks.len(), 4);
    assert!(
        obs.source.audio_tracks.iter().all(|t| t.title.is_some()),
        "{:?}",
        obs.source.audio_tracks
    );

    assert!(
        probe(&env, Fixture::NoAudio)
            .await
            .source
            .audio_tracks
            .is_empty()
    );
    assert!(probe(&env, Fixture::Vfr).await.source.video.vfr);

    let base = probe(&env, Fixture::Baseline1080p60).await;
    let path = fixture(Fixture::Baseline1080p60);
    assert_eq!(
        base.source.fingerprint.size_bytes,
        std::fs::metadata(&path).unwrap().len()
    );
    assert_eq!(base.source.fingerprint.xxh3_head_tail.len(), 16);
    assert_eq!(base.source.relative_path, "");
    assert!(
        base.mime_type.starts_with("video/mp4"),
        "{}",
        base.mime_type
    );

    // M18: spaces and non-Latin characters in the path
    let uni = probe(&env, Fixture::UnicodePath).await;
    assert!(
        uni.source.path.contains("clip with spaces"),
        "{}",
        uni.source.path
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ply01_audio_only_is_rejected_as_no_video() {
    let Some(env) = env() else { return };
    let err = env
        .svc
        .probe_source(&fixture(Fixture::AudioOnly))
        .await
        .unwrap_err();
    assert!(matches!(err, MediaError::NoVideo { .. }), "{err:?}");
    assert!(err.to_string().contains("no video"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn s9_truncated_mp4_fails_with_a_specific_message_and_hint() {
    let Some(env) = env() else { return };
    let err = env
        .svc
        .probe_source(&fixture(Fixture::TruncatedMp4))
        .await
        .unwrap_err();
    assert!(matches!(err, MediaError::Probe { .. }), "{err:?}");
    let msg = err.to_string();
    assert!(msg.contains("moov"), "{msg}");
    assert!(
        msg.contains("untrunc") || msg.contains("Repair"),
        "recovery hint: {msg}"
    );
    assert!(msg.contains("M15_truncated.mp4"), "names the file: {msg}");
}

#[tokio::test(flavor = "multi_thread")]
async fn s9_missing_file_is_a_clear_io_error() {
    let Some(env) = env() else { return };
    let err = env
        .svc
        .probe_source(&env.dir.join("does-not-exist.mp4"))
        .await
        .unwrap_err();
    assert!(matches!(err, MediaError::Io { .. }), "{err:?}");
    assert!(err.to_string().contains("does-not-exist.mp4"));
}

#[tokio::test(flavor = "multi_thread")]
async fn s9_a_freshly_written_file_gets_a_still_recording_warning() {
    let Some(env) = env() else { return };
    let copy = env.dir.join("fresh.mp4");
    std::fs::write(
        &copy,
        std::fs::read(fixture(Fixture::Baseline1080p60)).unwrap(),
    )
    .unwrap();
    let p = env.svc.probe_source(&copy).await.unwrap();
    assert!(
        p.warnings
            .iter()
            .any(|w| w.contains("modified moments ago")),
        "{:?}",
        p.warnings
    );
    // an old file has no such warning
    let old = probe(&env, Fixture::Baseline1080p60).await;
    assert!(
        !old.warnings.iter().any(|w| w.contains("moments ago")),
        "{:?}",
        old.warnings
    );
}

// ------------------------------------------------------------ section 8.1 budget

struct Counting<R> {
    inner: R,
    bytes: u64,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }
}

impl<R: Seek> Seek for Counting<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

fn top_level_boxes(bytes: &[u8]) -> Vec<(usize, [u8; 4])> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        out.push((pos, bytes[pos + 4..pos + 8].try_into().unwrap()));
        if size < 8 {
            break;
        }
        pos += size;
    }
    out
}

/// An MP4 with `moov` at the end preceded by a `free` box of `pad` bytes (so the file is huge
/// and everything but the headers is padding).
fn padded_moov_at_end(env: &Env, pad: usize) -> PathBuf {
    let plain = env.dir.join("plain.mp4");
    ffmpeg_ok([
        "-y".as_ref(),
        "-i".as_ref(),
        fixture(Fixture::Baseline1080p60).as_os_str(),
        "-c".as_ref(),
        "copy".as_ref(),
        plain.as_os_str(),
    ]);
    let bytes = std::fs::read(&plain).unwrap();
    let boxes = top_level_boxes(&bytes);
    let moov = boxes.iter().find(|(_, k)| k == b"moov").expect("moov").0;
    let mdat = boxes.iter().find(|(_, k)| k == b"mdat").expect("mdat").0;
    assert!(
        moov > mdat,
        "ffmpeg writes moov after mdat without faststart"
    );
    let out = env.dir.join("padded.mp4");
    let mut f = std::fs::File::create(&out).unwrap();
    f.write_all(&bytes[..moov]).unwrap();
    f.write_all(&(pad as u32).to_be_bytes()).unwrap();
    f.write_all(b"free").unwrap();
    let zeros = vec![0u8; 1 << 20];
    let mut left = pad - 8;
    while left > 0 {
        let n = left.min(zeros.len());
        f.write_all(&zeros[..n]).unwrap();
        left -= n;
    }
    f.write_all(&bytes[moov..]).unwrap();
    f.sync_all().unwrap();
    out
}

fn ffprobe_bytes_read(env: &Env, path: &Path) -> u64 {
    let out = std::process::Command::new(&env.svc.sidecars().ffprobe)
        .args([
            "-v",
            "debug",
            "-show_format",
            "-show_streams",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stderr);
    text.lines()
        .filter_map(|l| l.split("Statistics: ").nth(1))
        .filter_map(|rest| rest.split(" bytes read").next()?.trim().parse::<u64>().ok())
        .max()
        .expect("ffprobe -v debug prints 'Statistics: N bytes read'")
}

#[tokio::test(flavor = "multi_thread")]
async fn s81_open_reads_headers_not_the_whole_file() {
    let Some(env) = env() else { return };
    const PAD: usize = 300 * 1024 * 1024;
    let path = padded_moov_at_end(&env, PAD);
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size > PAD as u64);

    // FFprobe: bytes read is a few MiB at most (the 4 MB source file's own moov + first packets)
    let read = ffprobe_bytes_read(&env, &path);
    assert!(
        read < 8 * 1024 * 1024,
        "ffprobe read {read} bytes of a {size} byte file"
    );

    // sample tables: only the moov box
    let mut counting = Counting {
        inner: std::fs::File::open(&path).unwrap(),
        bytes: 0,
    };
    let times = rustcut_media::mp4::read_sample_times(&mut counting).unwrap();
    assert!(!times.frame_times.is_empty());
    assert!(
        counting.bytes < 1024 * 1024,
        "sample tables read {} bytes",
        counting.bytes
    );

    // probe + fingerprint end to end
    let t = Instant::now();
    let probed = env.svc.probe_source(&path).await.unwrap();
    let took = t.elapsed();
    assert_eq!(probed.source.fingerprint.size_bytes, size);
    assert!(
        took < Duration::from_secs(5),
        "open took {took:?} for {size} bytes"
    );
    // fingerprint = head + tail chunk only (model constant), far below the file size
    assert!(
        2 * 1024 * 1024 < size / 50,
        "fingerprint hashes the first and last MiB only"
    );

    // the index of this file is also instant
    let t = Instant::now();
    let built = env
        .svc
        .build_index_cancellable(&path, &probed.source, |_| {}, CancelToken::new())
        .await
        .unwrap();
    assert_eq!(built.source, IndexSource::SampleTable);
    assert!(t.elapsed() < Duration::from_secs(3));
}

// ------------------------------------------------------------- PLY-06 index

fn forced_scan(source: &SourceInfo) -> SourceInfo {
    let mut s = source.clone();
    s.format_name = Some("matroska,webm".into());
    s
}

#[tokio::test(flavor = "multi_thread")]
async fn ply06_sample_table_index_equals_the_packet_scan() {
    let Some(env) = env() else { return };
    for f in [
        Fixture::Baseline1080p60,
        Fixture::LongGop10s,
        Fixture::Vfr,
        Fixture::StartTimeNonZero,
        Fixture::HevcHlgRotated,
        Fixture::FrameNumbered,
        Fixture::Av1HighFps,
    ] {
        let path = fixture(f);
        let source = probe(&env, f).await.source;
        let fast = env
            .svc
            .build_index_cancellable(&path, &source, |_| {}, CancelToken::new())
            .await
            .unwrap();
        assert_eq!(
            fast.source,
            IndexSource::SampleTable,
            "{f:?} should use the sample tables"
        );

        // an independent service (own cache) forced onto the packet scan
        let other = env_with_cap(1 << 30).unwrap();
        let progress = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&progress);
        let scan = other
            .svc
            .build_index_cancellable(
                &path,
                &forced_scan(&source),
                move |p| sink.lock().unwrap().push(p),
                CancelToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(scan.source, IndexSource::PacketScan);
        assert_eq!(
            fast.index.frame_times, scan.index.frame_times,
            "{f:?} frame times"
        );
        assert_eq!(
            fast.index.keyframes, scan.index.keyframes,
            "{f:?} keyframes"
        );
        assert_eq!(fast.index.vfr, scan.index.vfr, "{f:?} vfr flag");
        assert!(!fast.index.frame_times.is_empty());
        assert!(
            fast.index.frame_times[0].abs() < 0.1,
            "{f:?}: first frame near 0"
        );
        let _ = std::fs::remove_dir_all(&other.dir);
    }
}

/// AC-14: `quick_index` never scans packets. It answers from the sample tables or the cache and
/// leaves a container without sample tables to the packet scan of `build_index_cancellable`.
#[tokio::test(flavor = "multi_thread")]
async fn ply06_quick_index_uses_sample_tables_or_cache_but_never_scans() {
    let Some(env) = env() else { return };
    let mp4 = fixture(Fixture::Baseline1080p60);
    let mp4_source = probe(&env, Fixture::Baseline1080p60).await.source;
    let quick = env
        .svc
        .quick_index(&mp4, &mp4_source)
        .await
        .expect("MP4 sample tables");
    assert_eq!(quick.source, IndexSource::SampleTable);

    let mkv = fixture(Fixture::ObsMkv4Audio);
    let mkv_source = probe(&env, Fixture::ObsMkv4Audio).await.source;
    assert!(
        env.svc.quick_index(&mkv, &mkv_source).await.is_none(),
        "Matroska needs the packet scan"
    );
    let scanned = env
        .svc
        .build_index_cancellable(&mkv, &mkv_source, |_| {}, CancelToken::new())
        .await
        .unwrap();
    assert_eq!(scanned.source, IndexSource::PacketScan);
    let cached = env
        .svc
        .quick_index(&mkv, &mkv_source)
        .await
        .expect("cached scan");
    assert_eq!(cached.source, IndexSource::Cache);
    assert_eq!(cached.index, scanned.index);
}

#[tokio::test(flavor = "multi_thread")]
async fn ply06_index_matches_fixture_truth_and_is_cached() {
    let Some(env) = env() else { return };
    let path = fixture(Fixture::LongGop10s);
    let source = probe(&env, Fixture::LongGop10s).await.source;
    let idx = env.svc.build_index(&path, &source, |_| {}).await.unwrap();
    let truth = keyframe_times(&path);
    assert_eq!(idx.keyframes.len(), truth.len());
    for (a, b) in idx.keyframes.iter().zip(&truth) {
        assert!((a + source.start_time_sec - b).abs() < 2e-6, "{a} vs {b}");
    }
    assert!(
        idx.keyframes.len() >= 3 && idx.keyframes[1] > 9.0,
        "10 s GOP: {:?}",
        idx.keyframes
    );

    let again = env
        .svc
        .build_index_cancellable(&path, &source, |_| {}, CancelToken::new())
        .await
        .unwrap();
    assert_eq!(again.source, IndexSource::Cache);
    assert_eq!(again.index, idx);
    let key = source.fingerprint.key();
    assert!(
        env.cache()
            .join("index")
            .join(format!("{key}.json"))
            .is_file()
    );

    // VFR: non-uniform frame times are kept
    let vfr_path = fixture(Fixture::Vfr);
    let vfr_src = probe(&env, Fixture::Vfr).await.source;
    let vfr = env
        .svc
        .build_index(&vfr_path, &vfr_src, |_| {})
        .await
        .unwrap();
    assert!(vfr.vfr);
    assert!(vfr.next_frame_after(0.0).is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn ply06_non_mp4_uses_a_cancellable_packet_scan() {
    let Some(env) = env() else { return };
    let path = fixture(Fixture::ObsMkv4Audio);
    let source = probe(&env, Fixture::ObsMkv4Audio).await.source;
    let cancel = CancelToken::new();
    cancel.cancel();
    let err = env
        .svc
        .build_index_cancellable(&path, &source, |_| {}, cancel)
        .await
        .unwrap_err();
    assert!(matches!(err, MediaError::Cancelled), "{err:?}");
    assert!(
        !env.cache().join("index").exists(),
        "a cancelled scan caches nothing"
    );

    let progress = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&progress);
    let built = env
        .svc
        .build_index_cancellable(
            &path,
            &source,
            move |p| sink.lock().unwrap().push(p),
            CancelToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(built.source, IndexSource::PacketScan);
    assert!(built.index.frame_times.len() > 100);
    let p = progress.lock().unwrap();
    assert!(
        p.windows(2).all(|w| w[0] <= w[1]) && p.iter().all(|v| (0.0..=1.0).contains(v)),
        "{p:?}"
    );
}

// ------------------------------------------------------------- PLY-05 waveform

async fn waveform(
    env: &Env,
    f: Fixture,
    track: u32,
    cancel: CancelToken,
) -> Result<Vec<WaveformChunk>, MediaError> {
    let path = fixture(f);
    let source = probe(env, f).await.source;
    let chunks = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&chunks);
    env.svc
        .generate_waveform(
            &path,
            &source,
            track,
            move |c| sink.lock().unwrap().push(c),
            cancel,
        )
        .await?;
    let out = chunks.lock().unwrap().clone();
    Ok(out)
}

#[tokio::test(flavor = "multi_thread")]
async fn ply05_waveform_has_ceil_duration_times_100_peaks_and_is_cached() {
    let Some(env) = env() else { return };
    let source = probe(&env, Fixture::Baseline1080p60).await.source;
    let chunks = waveform(&env, Fixture::Baseline1080p60, 1, CancelToken::new())
        .await
        .unwrap();
    let total = total_peaks_for(source.duration_sec);
    assert_eq!(
        total,
        (source.duration_sec * 100.0).ceil() as u32,
        "{}",
        source.duration_sec
    );
    let all: Vec<i8> = chunks
        .iter()
        .flat_map(|c| c.peaks.iter().copied())
        .collect();
    assert_eq!(all.len(), total as usize * 2);
    assert!(chunks.len() > 3, "progressive chunks");
    assert!(
        chunks
            .iter()
            .all(|c| c.total_peaks == total && c.peaks_per_sec == 100 && c.track == 1)
    );
    let mut next = 0;
    for c in &chunks {
        assert_eq!(c.start_peak, next);
        next += (c.peaks.len() / 2) as u32;
    }
    assert!(chunks.last().unwrap().done && chunks.iter().filter(|c| c.done).count() == 1);
    assert!(
        all.iter().any(|v| v.unsigned_abs() > 10),
        "tone is not silent"
    );
    assert!(
        all.as_chunks::<2>().0.iter().all(|p| p[0] <= p[1]),
        "min <= max"
    );

    // cached: identical chunks, no FFmpeg needed (point a second service at a broken sidecar dir)
    let key = source.fingerprint.key();
    assert!(
        env.cache()
            .join("waveform")
            .join(format!("{key}-t1.bin"))
            .is_file()
    );
    let again = waveform(&env, Fixture::Baseline1080p60, 1, CancelToken::new())
        .await
        .unwrap();
    assert_eq!(again, chunks);

    // replay needs no FFmpeg at all: a service whose sidecars do not exist serves the cache
    let blind = MediaService::new(MediaConfig {
        sidecars: Sidecars {
            ffmpeg: env.dir.join("missing").join("ffmpeg.exe"),
            ffprobe: env.dir.join("missing").join("ffprobe.exe"),
        },
        cache_dir: env.cache(),
        data_dir: env.dir.join("data"),
        cache_cap_bytes: 1,
    });
    let replayed = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&replayed);
    blind
        .generate_waveform(
            &fixture(Fixture::Baseline1080p60),
            &source,
            1,
            move |c| sink.lock().unwrap().push(c),
            CancelToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(*replayed.lock().unwrap(), chunks);
}

#[tokio::test(flavor = "multi_thread")]
async fn ply05_edge_cases_no_audio_bad_track_and_cancel() {
    let Some(env) = env() else { return };
    let none = waveform(&env, Fixture::NoAudio, 1, CancelToken::new())
        .await
        .unwrap();
    assert!(none.is_empty(), "no audio track: Ok with no chunks");

    let err = waveform(&env, Fixture::Baseline1080p60, 2, CancelToken::new())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            MediaError::NoSuchTrack {
                track: 2,
                available: 1,
                ..
            }
        ),
        "{err:?}"
    );
    let err = waveform(&env, Fixture::Baseline1080p60, 0, CancelToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, MediaError::NoSuchTrack { .. }));

    let cancel = CancelToken::new();
    cancel.cancel();
    let err = waveform(&env, Fixture::Baseline1080p60, 1, cancel)
        .await
        .unwrap_err();
    assert!(matches!(err, MediaError::Cancelled), "{err:?}");
    assert!(
        !env.cache().join("waveform").exists(),
        "cancelled run caches nothing"
    );

    // a different track of the multi-track MKV
    let t3 = waveform(&env, Fixture::ObsMkv4Audio, 3, CancelToken::new())
        .await
        .unwrap();
    assert!(t3.iter().all(|c| c.track == 3) && t3.last().unwrap().done);
}

// ------------------------------------------------------- section 7.3 playback

async fn remux(env: &Env, f: Fixture) -> (SourceInfo, PathBuf) {
    let path = fixture(f);
    let source = probe(env, f).await.source;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let out = env
        .svc
        .prepare(
            PlaybackTier::Remux,
            &path,
            &source,
            move |p, eta| sink.lock().unwrap().push((p, eta)),
            CancelToken::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("remux {f:?}: {e}"));
    assert!(
        out.starts_with(env.cache().join("remux")),
        "{}",
        out.display()
    );
    assert!(!seen.lock().unwrap().is_empty(), "progress reported");
    (source, out)
}

#[tokio::test(flavor = "multi_thread")]
async fn s73_remux_of_mkv_flv_and_pcm_gives_a_readable_mp4_with_the_same_duration() {
    let Some(env) = env() else { return };
    for f in [Fixture::ObsMkv4Audio, Fixture::Flv, Fixture::PcmAudio] {
        let (source, out) = remux(&env, f).await;
        let p = env
            .svc
            .probe_source(&out)
            .await
            .unwrap_or_else(|e| panic!("{f:?}: {e}"));
        assert!(
            p.source.format_name.as_deref().unwrap().contains("mp4"),
            "{f:?}"
        );
        assert!(
            (p.source.duration_sec - source.duration_sec).abs() < 0.1,
            "{f:?}: {} vs {}",
            p.source.duration_sec,
            source.duration_sec
        );
        assert_eq!(p.source.video.codec, "h264");
        assert_eq!(
            p.source.audio_tracks.len(),
            1,
            "{f:?}: video + the preview audio track"
        );
        assert_eq!(
            p.source.audio_tracks[0].codec, "aac",
            "{f:?}: playable in the WebView"
        );
        assert!(
            p.source.start_time_sec.abs() < 0.1,
            "{f:?} starts at 0: {}",
            p.source.start_time_sec
        );
        assert!(p.mime_type.contains("mp4a.40.2"));
    }
    // no leftovers
    let leftovers: Vec<_> = std::fs::read_dir(env.cache().join("remux"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "part"))
        .collect();
    assert!(leftovers.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn s73_remux_time_origin_is_the_first_frame_for_nonzero_start_time() {
    let Some(env) = env() else { return };
    let (source, out) = remux(&env, Fixture::StartTimeNonZero).await;
    assert!(source.start_time_sec > 9.0);
    let p = env.svc.probe_source(&out).await.unwrap();
    assert!(
        p.source.start_time_sec.abs() < 0.1,
        "{}",
        p.source.start_time_sec
    );
    // the index of the remuxed file equals the index of the original (same origin)
    let a = env
        .svc
        .build_index(&fixture(Fixture::StartTimeNonZero), &source, |_| {})
        .await
        .unwrap();
    let b = env.svc.build_index(&out, &p.source, |_| {}).await.unwrap();
    assert_eq!(a.frame_times.len(), b.frame_times.len());
    for (x, y) in a.frame_times.iter().zip(&b.frame_times) {
        assert!((x - y).abs() < 0.002, "{x} vs {y}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn s73_prepare_reuses_the_cache_direct_is_the_original_and_cancel_cleans_up() {
    let Some(env) = env() else { return };
    let (source, out) = remux(&env, Fixture::Flv).await;
    let mtime = std::fs::metadata(&out).unwrap().modified().unwrap();
    let t = Instant::now();
    let again = env
        .svc
        .prepare(
            PlaybackTier::Remux,
            &fixture(Fixture::Flv),
            &source,
            |_, _| {},
            CancelToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(again, out);
    assert!(t.elapsed() < Duration::from_millis(500));
    assert!(
        std::fs::metadata(&out).unwrap().modified().unwrap() >= mtime,
        "touched on use"
    );

    let direct = env
        .svc
        .prepare(
            PlaybackTier::Direct,
            &fixture(Fixture::Flv),
            &source,
            |_, _| {},
            CancelToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(direct, fixture(Fixture::Flv));

    let other = env_with_cap(1 << 30).unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    let err = other
        .svc
        .prepare(
            PlaybackTier::Proxy,
            &fixture(Fixture::Baseline1080p60),
            &source,
            |_, _| {},
            cancel,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, MediaError::Cancelled), "{err:?}");
    let proxy_dir = other.cache().join("proxy");
    let count = std::fs::read_dir(&proxy_dir).map_or(0, Iterator::count);
    assert_eq!(count, 0, "no .part or output left after cancel");
    let _ = std::fs::remove_dir_all(&other.dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn s73_proxy_is_540_lines_with_a_short_gop_and_every_frame() {
    let Some(env) = env() else { return };
    let path = fixture(Fixture::Baseline1080p60);
    let source = probe(&env, Fixture::Baseline1080p60).await.source;
    let out = env
        .svc
        .prepare(
            PlaybackTier::Proxy,
            &path,
            &source,
            |_, _| {},
            CancelToken::new(),
        )
        .await
        .unwrap();
    assert!(out.starts_with(env.cache().join("proxy")));
    let (w, h) = video_size(&out);
    assert_eq!((w, h), (960, 540));
    assert_eq!(
        frame_count(&out),
        frame_count(&path),
        "passthrough keeps every frame"
    );
    let keys = keyframe_times(&out);
    let gaps: Vec<f64> = keys.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        gaps.iter().all(|g| *g <= 0.3),
        "GOP of 15 frames at 60 fps: {gaps:?}"
    );
    let p = env.svc.probe_source(&out).await.unwrap();
    assert!((p.source.duration_sec - source.duration_sec).abs() < 0.1);
    assert_eq!(p.source.audio_tracks[0].codec, "aac");
}

#[tokio::test(flavor = "multi_thread")]
async fn s73_proxy_of_a_rotated_hevc_source_is_upright() {
    let Some(env) = env() else { return };
    let path = fixture(Fixture::HevcHlgRotated);
    let source = probe(&env, Fixture::HevcHlgRotated).await.source;
    let out = env
        .svc
        .prepare(
            PlaybackTier::Proxy,
            &path,
            &source,
            |_, _| {},
            CancelToken::new(),
        )
        .await
        .unwrap();
    let p = env.svc.probe_source(&out).await.unwrap();
    assert_eq!(p.source.video.codec, "h264");
    assert_eq!(p.source.video.rotation, 0);
    assert_eq!(
        (p.source.video.display_width, p.source.video.display_height),
        (304, 540),
        "display 360x640 scaled to 540 lines"
    );
}

fn fake_cache_file(env: &Env, dir: &str, name: &str, size: usize, age: Duration) -> PathBuf {
    let d = env.cache().join(dir);
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join(name);
    std::fs::write(&p, vec![0u8; size]).unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(SystemTime::now() - age).unwrap();
    p
}

#[tokio::test(flavor = "multi_thread")]
async fn s73_lru_evicts_least_recently_used_first_and_never_a_pinned_file() {
    let Some(env) = env_with_cap(250) else { return };
    let oldest = fake_cache_file(&env, "remux", "a.mp4", 100, Duration::from_secs(3000));
    let middle = fake_cache_file(&env, "proxy", "b.mp4", 100, Duration::from_secs(2000));
    let newest = fake_cache_file(&env, "remux", "c.mp4", 100, Duration::from_secs(1000));
    assert_eq!(env.svc.cache_size_bytes().await, 300);
    let freed = env.svc.enforce_cache_cap().await;
    assert_eq!(freed, 100);
    assert!(
        !oldest.exists() && middle.exists() && newest.exists(),
        "oldest goes first"
    );

    // pin the next candidate; the cap forces another eviction, which must skip it
    env.svc.pin_playback(&middle);
    let freed = env.svc.set_cache_cap(150).await;
    assert_eq!(freed, 100);
    assert!(middle.exists(), "pinned file survives");
    assert!(!newest.exists());
    assert_eq!(
        env.svc.set_cache_cap(0).await,
        0,
        "only the pinned file is left"
    );
    assert!(middle.exists());

    // clear_cache frees everything unpinned
    let extra = fake_cache_file(&env, "proxy", "d.mp4", 40, Duration::from_secs(10));
    assert_eq!(env.svc.clear_cache().await, 40);
    assert!(!extra.exists() && middle.exists());
    env.svc.release_playback(&middle);
    assert_eq!(env.svc.clear_cache().await, 100);
    assert_eq!(env.svc.cache_size_bytes().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn s73_preparing_a_new_file_evicts_older_ones_over_the_cap() {
    let Some(env) = env_with_cap(1) else { return };
    let old = fake_cache_file(&env, "remux", "old.mp4", 100, Duration::from_secs(5000));
    let (_, out) = remux(&env, Fixture::Flv).await;
    assert!(
        out.exists(),
        "the file just prepared is pinned even though it exceeds the cap"
    );
    assert!(!old.exists(), "older file evicted");
}

// ------------------------------------------------------------- EXP-13 hardware

#[tokio::test(flavor = "multi_thread")]
async fn exp13_hardware_detection_lists_tests_and_caches() {
    let Some(env) = env() else { return };
    let list = env.svc.detect_hw_encoders(false).await;
    eprintln!("hardware encoders: {list:#?}");
    let known: Vec<&str> = EncoderChoice::HARDWARE
        .iter()
        .filter_map(|e| e.ffmpeg_name())
        .collect();
    for info in &list {
        assert!(known.contains(&info.name.as_str()), "{}", info.name);
        assert_eq!(info.available, info.error.is_none(), "{info:?}");
        assert!(!info.label.is_empty());
    }
    if let Ok(expect) = std::env::var("RUSTCUT_EXPECT_HW") {
        for name in expect.split(',').filter(|n| !n.is_empty()) {
            let info = list
                .iter()
                .find(|i| i.name == name)
                .unwrap_or_else(|| panic!("{name} not listed"));
            assert!(info.available, "{name} should work here: {:?}", info.error);
        }
    }
    // cached on disk, keyed by the FFmpeg version
    let cache_file = env.dir.join("data").join("hw_encoders.json");
    assert!(cache_file.is_file());
    let other = MediaService::new(MediaConfig {
        sidecars: env.svc.sidecars().clone(),
        cache_dir: env.dir.join("cache"),
        data_dir: env.dir.join("data"),
        cache_cap_bytes: 1,
    });
    let t = Instant::now();
    assert_eq!(other.detect_hw_encoders(false).await, list);
    assert!(
        t.elapsed() < Duration::from_secs(2),
        "served from the cache file"
    );
    assert_eq!(
        env.svc.best_hw_encoder().await,
        rustcut_media::pick_best(&list)
    );
    // force re-tests
    let again = env.svc.detect_hw_encoders(true).await;
    assert_eq!(again.len(), list.len());
}
