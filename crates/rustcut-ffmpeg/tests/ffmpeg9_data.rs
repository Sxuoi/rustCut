//! Parsers against real FFmpeg/FFprobe 9.0.2 output captured into
//! `tests/data` (fixtures generated with lavfi, mirroring the PRD §11 matrix:
//! M02 3-track MKV with subtitle + attachments, M05 rotated HEVC 10-bit HLG,
//! M10 SAR 16:15, M12 VP9/Opus WebM, M13 start_time ≠ 0, M14 no audio,
//! M15 truncated MP4, M17 MOV with PCM).

use rustcut_ffmpeg::failure::{FailureKind, classify, first_error_line};
use rustcut_ffmpeg::probe::{ProbeWarning, parse_probe_json, probe_failure_message};
use rustcut_ffmpeg::{
    LosslessAudio, Muxer, ProgressParser, lossless_container, mime_type, parse_packet_csv,
};
use rustcut_model::{CutMode, ExportSettings};

macro_rules! data {
    ($name:literal) => {
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/", $name))
    };
}

fn lossless() -> ExportSettings {
    ExportSettings {
        cut_mode: CutMode::Lossless,
        ..ExportSettings::default()
    }
}

#[test]
fn m05_rotated_hevc_hlg() {
    let p = parse_probe_json(data!("probe_m05.json"), "D:/iPhone/m05.mov").unwrap();
    let v = &p.source.video;
    assert_eq!(
        (v.codec.as_str(), v.coded_width, v.coded_height),
        ("hevc", 320, 180)
    );
    assert_eq!(
        (v.rotation, v.display_width, v.display_height),
        (90, 180, 320),
        "display matrix -90 = 90° clockwise"
    );
    assert!(v.hdr);
    assert_eq!(v.color_transfer.as_deref(), Some("arib-std-b67"));
    assert_eq!(
        (v.bit_depth, v.profile.as_deref(), v.level),
        (Some(10), Some("Main 10"), Some(60))
    );
    assert_eq!(v.frame_rate, "30/1");
    assert!(
        p.warnings
            .iter()
            .any(|w| matches!(w, ProbeWarning::Hdr { .. }))
    );
    assert!(
        p.warnings
            .iter()
            .find(|w| matches!(w, ProbeWarning::Hdr { .. }))
            .unwrap()
            .message()
            .contains("HLG")
    );
    assert_eq!(
        mime_type(&p.source),
        "video/mp4; codecs=\"hvc1.2.4.L60.B0, mp4a.40.2\""
    );
    let d = lossless_container(&p.source, &lossless());
    assert_eq!((d.muxer, d.audio), (Muxer::Mp4, LosslessAudio::Copy));
    assert!(d.reason.contains("hvc1"));
}

#[test]
fn m10_non_square_pixels() {
    let p = parse_probe_json(data!("probe_m10.json"), "m10.mp4").unwrap();
    let v = &p.source.video;
    assert_eq!(
        (v.sar.as_str(), v.display_width, v.display_height),
        ("16:15", 768, 576)
    );
    assert_eq!(v.frame_rate, "25/1");
    assert_eq!(
        mime_type(&p.source),
        "video/mp4; codecs=\"avc1.42E01E, mp4a.40.2\""
    );
}

#[test]
fn m02_three_track_mkv_with_subtitle_cover_and_attachment() {
    let p = parse_probe_json(data!("probe_m02.json"), "D:/obs/m02.mkv").unwrap();
    let s = &p.source;
    assert_eq!(s.format_name.as_deref(), Some("matroska,webm"));
    assert_eq!(s.video.stream_index, Some(0));
    assert_eq!(s.video.frame_rate, "60/1");
    let tracks: Vec<_> = s
        .audio_tracks
        .iter()
        .map(|a| {
            (
                a.stream_index,
                a.title.as_deref(),
                a.language.as_deref(),
                a.channels,
                a.sample_rate,
                a.is_default,
            )
        })
        .collect();
    assert_eq!(
        tracks,
        vec![
            (1, Some("Game"), Some("eng"), 2, Some(44_100), true),
            (2, Some("Mic"), Some("ger"), 1, Some(44_100), false),
            (3, Some("Music"), None, 6, Some(48_000), false),
        ]
    );
    let c = p.streams;
    assert_eq!(
        (
            c.video,
            c.audio,
            c.subtitle,
            c.attachment,
            c.attached_pictures
        ),
        (1, 3, 1, 1, 1)
    );
    assert!(p.warnings.contains(&ProbeWarning::DroppedStreams {
        subtitle: 1,
        data: 0,
        attachment: 1
    }));
    assert!((s.duration_sec - 1.023).abs() < 1e-9);
    assert_eq!(s.bit_rate, Some(1_605_990));
    assert_eq!(s.fingerprint.size_bytes, 205_366);
    assert_eq!(
        lossless_container(s, &lossless()).muxer,
        Muxer::Mp4,
        "OBS MKV → MP4 (EXP-03)"
    );
}

#[test]
fn m13_start_time_not_zero_and_packet_index() {
    let p = parse_probe_json(data!("probe_m13.json"), "m13.mp4").unwrap();
    assert!((p.source.start_time_sec - 1.443447).abs() < 1e-9);
    assert!((p.source.duration_sec - 2.043356).abs() < 1e-9);
    // Packet scan of the MPEG-TS original (B-frames → decode order; FFmpeg 9
    // adds a trailing comma). The TS format starts at 1.4 s.
    let idx = parse_packet_csv(data!("packets_m13_ts.csv"), 1.4);
    assert_eq!(idx.frame_times.len(), 60);
    assert!(idx.frame_times.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(idx.keyframes, vec![0.066667, 1.066667]);
    assert!((idx.frame_duration.unwrap() - 1.0 / 30.0).abs() < 1e-5);
    assert!(!idx.vfr);
    assert!((idx.lead_in(1.5).unwrap() - 0.433333).abs() < 1e-6);
}

#[test]
fn m14_no_audio_and_m12_webm() {
    let p = parse_probe_json(data!("probe_m14.json"), "m14.mp4").unwrap();
    assert!(p.source.audio_tracks.is_empty());
    assert!(mime_type(&p.source).starts_with("video/mp4; codecs=\"avc1."));
    assert!(!mime_type(&p.source).contains(','));
    let w = parse_probe_json(data!("probe_m12.json"), "m12.webm").unwrap();
    assert_eq!(
        (
            w.source.video.codec.as_str(),
            w.source.audio_tracks[0].codec.as_str()
        ),
        ("vp9", "opus")
    );
    assert_eq!(
        w.source.video.level, None,
        "ffprobe reports level -99 for VP9"
    );
    assert!((w.source.duration_sec - 1.008).abs() < 1e-9);
    assert_eq!(
        mime_type(&w.source),
        "video/webm; codecs=\"vp09.00.31.08, opus\""
    );
    assert_eq!(lossless_container(&w.source, &lossless()).muxer, Muxer::Mp4);
}

#[test]
fn m17_pcm_mov_offers_aac() {
    let p = parse_probe_json(data!("probe_m17.json"), "m17.mov").unwrap();
    let d = lossless_container(&p.source, &lossless());
    assert_eq!(
        (d.muxer, d.ext.as_str(), d.offer_aac_audio),
        (Muxer::Mov, "mov", true)
    );
    let accepted = ExportSettings {
        lossless_audio_to_aac: true,
        ..lossless()
    };
    let d = lossless_container(&p.source, &accepted);
    assert_eq!(
        (d.muxer, d.audio),
        (Muxer::Mp4, LosslessAudio::AacTranscode)
    );
}

#[test]
fn m15_truncated_and_other_probe_failures() {
    let truncated = data!("stderr_m15_moov.txt");
    let msg = probe_failure_message(truncated, "D:/rec/m15.mp4");
    assert!(msg.starts_with("m15.mp4 is incomplete"), "{msg}");
    assert_eq!(classify(truncated), FailureKind::InvalidData);
    assert!(
        probe_failure_message(data!("stderr_missing.txt"), "missing.mp4").contains("was not found")
    );
    assert!(probe_failure_message(data!("stderr_junk.txt"), "junk.mp4").contains("junk.mp4"));
}

#[test]
fn exp09_real_progress_transcripts() {
    let mut p = ProgressParser::new();
    let ups = p.feed_text(data!("progress_x264.txt"));
    assert_eq!(ups.len(), 2);
    assert_eq!(
        (ups[0].out_time_sec, ups[0].speed, ups[0].end),
        (None, None, false),
        "FFmpeg 9 starts with N/A"
    );
    let last = ups.last().unwrap();
    assert!(
        last.end && last.speed == Some(10.0) && last.out_time_sec.unwrap() > 2.9,
        "{last:?}"
    );

    let mut p = ProgressParser::new();
    let crlf = data!("progress_x264_long.txt").replace('\n', "\r\n");
    let ups = p.feed_text(&crlf);
    assert_eq!(ups.len(), 6);
    assert_eq!(
        (ups[0].out_time_sec, ups[0].speed),
        (Some(0.13932), Some(0.671))
    );
    let times: Vec<f64> = ups.iter().filter_map(|u| u.out_time_sec).collect();
    assert!(times.windows(2).all(|w| w[0] <= w[1]), "{times:?}");
    assert!(ups.iter().take(5).all(|u| !u.end) && ups[5].end);
    assert!(
        ups.iter()
            .all(|u| u.frame.is_some() && u.total_size.is_some())
    );
}

#[test]
fn exp10_exp13_real_failure_tails() {
    let cases = [
        (
            data!("fail_output_missing.txt"),
            FailureKind::OutputMissing,
            "Error opening output no_such_dir/out.mp4.part: No such file or directory",
        ),
        (
            data!("fail_nvenc.txt"),
            FailureKind::HwEncoderFailed,
            "h264_nvenc: Cannot load nvcuda.dll",
        ),
        (
            data!("fail_qsv.txt"),
            FailureKind::HwEncoderFailed,
            "h264_qsv: Error creating a MFX session: -9.",
        ),
        (
            data!("fail_amf_init.txt"),
            FailureKind::HwEncoderFailed,
            "h264_amf: encoder->Init() failed with error 4",
        ),
        (
            data!("fail_invalid.txt"),
            FailureKind::InvalidData,
            "in#0: moov atom not found",
        ),
    ];
    for (tail, kind, line) in cases {
        assert_eq!(classify(tail), kind, "{line}");
        let first = first_error_line(tail).unwrap();
        assert!(first.ends_with(line), "{first} vs {line}");
        assert!(!first.contains(" @ 0"), "pointer stripped: {first}");
    }
    assert!(classify(data!("fail_output_missing.txt")).pauses_batch());
}
