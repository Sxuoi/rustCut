//! One test per fixture asserting its key property via ffprobe (PRD section 11).

use rustcut_testkit::*;
use serde_json::Value;

fn streams(v: &Value, kind: &str) -> Vec<Value> {
    v["streams"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["codec_type"] == kind)
        .cloned()
        .collect()
}

fn rate(s: &Value, key: &str) -> f64 {
    let (n, d) = s[key].as_str().unwrap().split_once('/').unwrap();
    n.parse::<f64>().unwrap() / d.parse::<f64>().unwrap()
}

fn dur(v: &Value) -> f64 {
    v["format"]["duration"].as_str().unwrap().parse().unwrap()
}

#[test]
fn tools_are_found() {
    assert!(ffmpeg().is_file() && ffprobe().is_file());
    assert!(
        fixtures_dir().ends_with(".fixtures") || std::env::var_os("RUSTCUT_FIXTURES_DIR").is_some()
    );
}

#[test]
fn m01_baseline_1080p60_high_aac_2s_gop() {
    let p = fixture(Fixture::Baseline1080p60);
    let v = probe_json(&p);
    let vs = &streams(&v, "video")[0];
    assert_eq!(
        (vs["codec_name"].as_str(), vs["profile"].as_str()),
        (Some("h264"), Some("High"))
    );
    assert_eq!(video_size(&p), (1920, 1080));
    assert_eq!(rate(vs, "avg_frame_rate"), 60.0);
    let a = &streams(&v, "audio")[0];
    assert_eq!(
        (a["codec_name"].as_str(), a["channels"].as_i64()),
        (Some("aac"), Some(2))
    );
    assert!((dur(&v) - 12.0).abs() < 0.2);
    let k = keyframe_times(&p);
    assert!(
        k.len() >= 6 && k.windows(2).all(|w| (w[1] - w[0] - 2.0).abs() < 1e-3),
        "keyframes {k:?}"
    );
}

#[test]
fn m02_obs_mkv_has_four_titled_audio_tracks() {
    let v = probe_json(&fixture(Fixture::ObsMkv4Audio));
    assert!(
        v["format"]["format_name"]
            .as_str()
            .unwrap()
            .contains("matroska")
    );
    let titles: Vec<String> = streams(&v, "audio")
        .iter()
        .map(|s| s["tags"]["title"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(titles, ["Mix", "Game", "Mic", "Music"]);
    let vs = &streams(&v, "video")[0];
    assert_eq!(
        (
            vs["width"].as_i64(),
            vs["height"].as_i64(),
            vs["codec_name"].as_str()
        ),
        (Some(1280), Some(720), Some("h264"))
    );
    assert_eq!(rate(vs, "avg_frame_rate"), 30.0);
}

#[test]
fn m03_long_gop_has_10s_keyframes() {
    let p = fixture(Fixture::LongGop10s);
    assert_eq!(video_size(&p), (640, 360));
    let k = keyframe_times(&p);
    assert_eq!(k.len(), 3, "keyframes {k:?}");
    assert!(k.windows(2).all(|w| (w[1] - w[0] - 10.0).abs() < 1e-3));
    assert!((duration_sec(&p) - 30.0).abs() < 0.2);
}

#[test]
fn m04_vfr_has_variable_frame_durations() {
    let p = fixture(Fixture::Vfr);
    let v = probe_json(&p);
    let vs = &streams(&v, "video")[0];
    assert_ne!(
        vs["r_frame_rate"], vs["avg_frame_rate"],
        "r_frame_rate must differ from avg for VFR"
    );
    let t = video_pts_times(&p);
    let deltas: Vec<f64> = t.windows(2).map(|w| w[1] - w[0]).collect();
    let (min, max) = deltas
        .iter()
        .fold((f64::MAX, 0.0_f64), |(a, b), d| (a.min(*d), b.max(*d)));
    assert!(max / min >= 3.0, "spread too small: min {min} max {max}");
    assert!(!streams(&v, "audio").is_empty());
}

#[test]
fn m05_hevc_main10_hlg_rotated_90() {
    let v = probe_json(&fixture(Fixture::HevcHlgRotated));
    let vs = &streams(&v, "video")[0];
    assert_eq!(vs["codec_name"], "hevc");
    assert_eq!(vs["profile"], "Main 10");
    assert_eq!(vs["pix_fmt"], "yuv420p10le");
    assert_eq!(vs["color_transfer"], "arib-std-b67");
    assert_eq!(vs["color_primaries"], "bt2020");
    assert_eq!(vs["color_space"], "bt2020nc");
    let rot = vs["side_data_list"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|d| d["rotation"].as_i64())
        .expect("display matrix side data");
    assert_eq!(rot.abs(), 90);
    assert!((dur(&v) - 4.0).abs() < 0.2);
    assert!(!streams(&v, "audio").is_empty());
}

fn check_ramp(f: Fixture, w: u32, h: u32, sar: &str) {
    let p = fixture(f);
    assert_eq!(video_size(&p), (w, h), "{f:?}");
    let v = probe_json(&p);
    let vs = &streams(&v, "video")[0];
    assert_eq!(vs["codec_name"], "h264");
    assert_eq!(vs["pix_fmt"], "yuv420p");
    assert_eq!(rate(vs, "avg_frame_rate"), 30.0);
    assert_eq!(
        vs["sample_aspect_ratio"].as_str().unwrap_or("1:1"),
        sar,
        "{f:?}"
    );
    assert!((dur(&v) - 2.0).abs() < 0.2);
    assert!(!streams(&v, "audio").is_empty());
    let e = edge_luma(&p, 1.0);
    assert!(e.min() >= 16.0, "{f:?}: black edge {e:?}");
    assert!(
        e.left < 40.0 && e.right > 200.0,
        "{f:?}: not a horizontal ramp {e:?}"
    );
}

#[test]
fn m06_to_m10_ramps() {
    check_ramp(Fixture::Ramp16x9, 1920, 1080, "1:1");
    check_ramp(Fixture::Ramp4x3, 1440, 1080, "1:1");
    check_ramp(Fixture::Ramp21x9, 2560, 1080, "1:1");
    check_ramp(Fixture::Ramp9x16, 1080, 1920, "1:1");
    check_ramp(Fixture::RampTall, 1080, 2340, "1:1");
    check_ramp(Fixture::RampSar, 720, 576, "16:15");
}

#[test]
fn ramp_frame_count_is_60() {
    assert_eq!(frame_count(&fixture(Fixture::Ramp4x3)), 60);
}

#[test]
fn m11_flv_h264_aac() {
    let p = fixture(Fixture::Flv);
    let v = probe_json(&p);
    assert_eq!(v["format"]["format_name"], "flv");
    assert_eq!(streams(&v, "video")[0]["codec_name"], "h264");
    assert_eq!(streams(&v, "audio")[0]["codec_name"], "aac");
    assert!((dur(&v) - 6.0).abs() < 0.3);
}

#[test]
fn flv_concurrent_requests_generate_once() {
    let f = Fixture::Flv;
    let p = fixture(f);
    // Invalidate the cache, then race 8 threads for it.
    let sidecar = p.with_file_name(format!("{}.json", f.key()));
    std::fs::remove_file(&sidecar).unwrap();
    let before = generation_count(f);
    let paths: Vec<_> = (0..8)
        .map(|_| std::thread::spawn(move || fixture(f)))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();
    assert!(paths.iter().all(|q| *q == p));
    assert_eq!(
        generation_count(f) - before,
        1,
        "exactly one generation expected"
    );
    assert!(probe_json(&p)["format"]["format_name"] == "flv");
}

#[test]
fn m12_webm_vp9_opus() {
    let v = probe_json(&fixture(Fixture::WebmVp9Opus));
    assert!(
        v["format"]["format_name"]
            .as_str()
            .unwrap()
            .contains("webm")
    );
    assert_eq!(streams(&v, "video")[0]["codec_name"], "vp9");
    assert_eq!(streams(&v, "audio")[0]["codec_name"], "opus");
}

#[test]
fn m13_start_time_is_not_zero() {
    let v = probe_json(&fixture(Fixture::StartTimeNonZero));
    assert!(v["format"]["format_name"].as_str().unwrap().contains("mp4"));
    let start: f64 = v["format"]["start_time"].as_str().unwrap().parse().unwrap();
    assert!(start > 5.0, "start_time {start}");
}

#[test]
fn m14_no_audio_stream() {
    let v = probe_json(&fixture(Fixture::NoAudio));
    assert!(streams(&v, "audio").is_empty());
    assert_eq!(streams(&v, "video").len(), 1);
}

#[test]
fn m15_truncated_mp4_fails_probe() {
    let p = fixture(Fixture::TruncatedMp4);
    let err = try_probe_json(&p).expect_err("a truncated MP4 without index must not probe");
    assert!(!err.is_empty());
}

#[test]
fn m16_av1_144fps() {
    let p = fixture(Fixture::Av1HighFps);
    let v = probe_json(&p);
    let vs = &streams(&v, "video")[0];
    assert_eq!(vs["codec_name"], "av1");
    assert_eq!(rate(vs, "avg_frame_rate"), 144.0);
    assert_eq!(video_size(&p), (640, 360));
    assert!((dur(&v) - 3.0).abs() < 0.2);
}

#[test]
fn m17_pcm_audio_in_mov() {
    let v = probe_json(&fixture(Fixture::PcmAudio));
    assert_eq!(streams(&v, "audio")[0]["codec_name"], "pcm_s16le");
    assert_eq!(streams(&v, "video")[0]["codec_name"], "h264");
}

#[test]
fn m18_unicode_path_with_spaces() {
    let p = fixture(Fixture::UnicodePath);
    let s = p.to_string_lossy();
    assert!(
        s.contains("M18 ruang kerja ñ 測試") && s.contains("clip with spaces é.mp4"),
        "{s}"
    );
    let v = probe_json(&p);
    assert_eq!(streams(&v, "video")[0]["codec_name"], "h264");
    assert!((dur(&v) - 4.0).abs() < 0.2);
}

#[test]
fn ac08_frame_numbered_luma_sequence() {
    let p = fixture(Fixture::FrameNumbered);
    assert_eq!(video_size(&p), (320, 240));
    let v = probe_json(&p);
    assert!((dur(&v) - 20.0).abs() < 0.2);
    assert_eq!(rate(&streams(&v, "video")[0], "avg_frame_rate"), 30.0);
    assert!(!streams(&v, "audio").is_empty());
    for n in [0_u32, 1, 2, 28, 29, 30, 59, 61, 100, 333, 599] {
        // First frame with pts >= at: aim just before the frame start.
        let at = (f64::from(n) / 30.0 - 0.001).max(0.0);
        let want = i32::try_from((n * 7) % 200 + 30).unwrap();
        let got = i32::from(frame_luma(&p, at));
        assert!(
            (got - want).abs() <= 3,
            "frame {n}: luma {got}, expected {want}"
        );
    }
    let k = keyframe_times(&p);
    assert!(k.windows(2).all(|w| (w[1] - w[0] - 2.0).abs() < 1e-3));
}

#[test]
fn negative_audio_only_has_no_video() {
    let v = probe_json(&fixture(Fixture::AudioOnly));
    assert!(streams(&v, "video").is_empty());
    assert_eq!(streams(&v, "audio")[0]["codec_name"], "aac");
}

#[test]
fn packet_hashes_are_stable_and_per_packet() {
    let p = fixture(Fixture::NoAudio);
    let a = packet_hashes(&p, "0:v:0");
    assert_eq!(a, packet_hashes(&p, "0:v:0"));
    assert_eq!(a.len(), 120);
    assert!(
        a.iter()
            .all(|h| h.len() == 32 && h.chars().all(|c| c.is_ascii_hexdigit()))
    );
}

#[test]
fn temp_dirs_are_unique_and_inside_fixtures() {
    let (a, b) = (temp_dir(), temp_dir());
    assert_ne!(a, b);
    assert!(a.is_dir() && a.starts_with(fixtures_dir().join("tmp")));
}

#[test]
fn ffmpeg_ok_panics_on_failure() {
    assert!(
        std::panic::catch_unwind(|| ffmpeg_ok(["-i", "does-not-exist.mp4", "-f", "null", "-"]))
            .is_err()
    );
}
