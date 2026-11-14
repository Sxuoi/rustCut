//! Runs every §7.6 command shape with a real FFmpeg (validated on 9.0.2) on
//! small lavfi-generated inputs, and checks the outputs with FFprobe through
//! this crate's own parsers. Skips (never fails) when FFmpeg is absent; set
//! `RUSTCUT_FFMPEG_DIR` (or `FFMPEG_DIR`) to the folder holding the binaries.
//! Paths contain spaces and non-Latin characters (M18).

use rustcut_ffmpeg::compat::{MP4_AUDIO_CODECS, MP4_VIDEO_CODECS};
use rustcut_ffmpeg::failure::{FailureKind, classify, classify_attempt};
use rustcut_ffmpeg::probe::parse_verify_json;
use rustcut_ffmpeg::{
    ExportCommand, JobSpec, Muxer, Probe, ProgressParser, ProgressUpdate, ResolvedEncoder,
    build_export_command, hw_test_args, lossless_container, packet_index_args, parse_packet_csv,
    parse_probe_json, probe_args, verify_args,
};
use rustcut_model::{EncoderChoice, ExportSettings, OutputResolution, Preset, SourceInfo};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

struct Tools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    zscale: bool,
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn works(ffmpeg: &Path) -> bool {
    Command::new(ffmpeg)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn winget_dirs() -> Vec<PathBuf> {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return Vec::new();
    };
    let packages = PathBuf::from(local)
        .join("Microsoft")
        .join("WinGet")
        .join("Packages");
    let mut out = Vec::new();
    for pkg in std::fs::read_dir(packages).into_iter().flatten().flatten() {
        if !pkg.file_name().to_string_lossy().starts_with("Gyan.FFmpeg") {
            continue;
        }
        for build in std::fs::read_dir(pkg.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            out.push(build.path().join("bin"));
        }
    }
    out
}

fn tools() -> Option<&'static Tools> {
    static TOOLS: OnceLock<Option<Tools>> = OnceLock::new();
    TOOLS
        .get_or_init(|| {
            let mut dirs: Vec<PathBuf> = ["RUSTCUT_FFMPEG_DIR", "FFMPEG_DIR"]
                .iter()
                .filter_map(std::env::var_os)
                .map(PathBuf::from)
                .collect();
            dirs.extend(winget_dirs());
            let found = dirs
                .into_iter()
                .map(|d| (d.join(exe("ffmpeg")), d.join(exe("ffprobe"))))
                .find(|(f, p)| f.is_file() && p.is_file())
                .or_else(|| works(Path::new("ffmpeg")).then(|| ("ffmpeg".into(), "ffprobe".into())))
                .filter(|(f, _)| works(f))?;
            let filters = Command::new(&found.0)
                .args(["-hide_banner", "-filters"])
                .output()
                .ok()?;
            let zscale = String::from_utf8_lossy(&filters.stdout).contains(" zscale ");
            Some(Tools {
                ffmpeg: found.0,
                ffprobe: found.1,
                zscale,
            })
        })
        .as_ref()
}

macro_rules! need_ffmpeg {
    () => {
        match tools() {
            Some(t) => t,
            None => {
                eprintln!("skipping: FFmpeg not found (set RUSTCUT_FFMPEG_DIR to the folder with ffmpeg/ffprobe)");
                return;
            }
        }
    };
}

fn workdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("rustcut ffmpeg 🔥 тест ")
        .tempdir()
        .expect("temp dir")
}

/// Generate a lavfi input with `ffmpeg -v error <args> <dir/name>`.
fn generate(t: &Tools, dir: &Path, name: &str, args: &[&str]) -> PathBuf {
    let out = dir.join(name);
    let o = Command::new(&t.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-y", "-v", "error"])
        .args(args)
        .arg(&out)
        .output()
        .expect("spawn ffmpeg");
    assert!(
        o.status.success(),
        "generating {name}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    out
}

/// 6 s 640×360 60 fps H.264 (keyframes every 2 s) + 3 AAC tracks
/// (stereo, mono, 5.1): M01/M02-like.
fn multi_track(t: &Tools, dir: &Path) -> PathBuf {
    generate(
        t,
        dir,
        "Bahas Patch 2.0 «финал».mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=60:duration=6",
            "-f",
            "lavfi",
            "-i",
            "sine=f=300:d=6",
            "-f",
            "lavfi",
            "-i",
            "sine=f=500:d=6",
            "-f",
            "lavfi",
            "-i",
            "sine=f=700:d=6",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "2:a",
            "-map",
            "3:a",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "120",
            "-keyint_min",
            "120",
            "-sc_threshold",
            "0",
            "-c:a",
            "aac",
            "-ac:a:0",
            "2",
            "-ac:a:1",
            "1",
            "-ac:a:2",
            "6",
        ],
    )
}

fn ffprobe(t: &Tools, args: Vec<OsString>) -> Output {
    Command::new(&t.ffprobe)
        .args(args)
        .output()
        .expect("spawn ffprobe")
}

fn probe(t: &Tools, path: &Path) -> Probe {
    let o = ffprobe(t, probe_args(path));
    assert!(
        o.status.success(),
        "probe {}: {}",
        path.display(),
        String::from_utf8_lossy(&o.stderr)
    );
    parse_probe_json(&String::from_utf8_lossy(&o.stdout), path).expect("parse probe")
}

fn raw_video_stream(t: &Tools, path: &Path) -> serde_json::Value {
    let o = ffprobe(t, probe_args(path));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    v["streams"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["codec_type"] == "video").cloned())
        .expect("video")
}

struct Run {
    ok: bool,
    updates: Vec<ProgressUpdate>,
    stderr: String,
}

fn run(t: &Tools, cmd: &ExportCommand) -> Run {
    let o = Command::new(&t.ffmpeg)
        .args(cmd.to_args())
        .output()
        .expect("spawn ffmpeg");
    let mut parser = ProgressParser::new();
    let updates = parser.feed_text(&String::from_utf8_lossy(&o.stdout));
    Run {
        ok: o.status.success(),
        updates,
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

/// Build, run and probe one export; asserts success and `progress=end`.
fn export(
    t: &Tools,
    src: &SourceInfo,
    s: &ExportSettings,
    range: (f64, f64),
    out: &Path,
    enc: ResolvedEncoder,
) -> Probe {
    let container = lossless_container(src, s);
    let cmd = build_export_command(&JobSpec {
        input: Path::new(&src.path),
        output_part: out,
        source: src,
        start_sec: range.0,
        end_sec: range.1,
        crop_x_offset: 0.3,
        settings: s,
        container: &container,
        encoder: enc,
        threads: Some(2),
        zscale_available: t.zscale,
        snap_in_to: None,
    });
    let r = run(t, &cmd);
    assert!(
        r.ok,
        "ffmpeg failed for {:?}:\n{}",
        cmd.to_string_args(),
        r.stderr
    );
    assert!(
        r.updates.last().is_some_and(|u| u.end),
        "progress=end missing: {:?}",
        r.updates
    );
    probe(t, out)
}

fn preset(p: Preset) -> ExportSettings {
    let mut s = ExportSettings::default();
    s.apply_preset(p);
    s
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn b_lossless_starts_on_keyframe_keeps_every_track_and_verifies() {
    let t = need_ffmpeg!();
    let dir = workdir();
    let src = probe(t, &multi_track(t, dir.path())).source;
    // PLY-06 index from a real packet scan: keyframes every 2 s.
    let scan = ffprobe(t, packet_index_args(&src.path));
    let idx = parse_packet_csv(&String::from_utf8_lossy(&scan.stdout), src.start_time_sec);
    assert_eq!(idx.keyframes, vec![0.0, 2.0, 4.0]);
    assert_eq!(idx.frame_times.len(), 360);
    assert!(close(idx.frame_duration.unwrap(), 1.0 / 60.0, 1e-5) && !idx.vfr);
    let lead_in = idx.lead_in(2.5).unwrap();
    assert!(close(lead_in, 0.5, 1e-6));

    let out = dir.path().join("02 Jump Scare 🔥.mp4.part");
    let p = export(
        t,
        &src,
        &preset(Preset::Lossless),
        (2.5, 4.0),
        &out,
        ResolvedEncoder::Libx264,
    );
    // EXP-02: starts on the 2.000 s keyframe, ends on the frame before 4.000 s.
    assert!(
        close(p.source.duration_sec, 1.5 + lead_in, 0.1),
        "duration {}",
        p.source.duration_sec
    );
    assert_eq!(p.source.audio_tracks.len(), 3, "EXP-04 keeps every track");
    let v = ffprobe(t, verify_args(&out));
    let report = parse_verify_json(&String::from_utf8_lossy(&v.stdout), &out).unwrap();
    assert!(
        report.is_playable() && report.audio_channels == vec![2, 1, 6],
        "{report:?}"
    );
}

#[test]
fn c_accurate_is_exact_and_fps_cap_resamples() {
    let t = need_ffmpeg!();
    let dir = workdir();
    let src = probe(t, &multi_track(t, dir.path())).source;
    let out = dir.path().join("c accurate.mp4.part");
    let p = export(
        t,
        &src,
        &preset(Preset::Accurate),
        (2.5, 4.0),
        &out,
        ResolvedEncoder::Libx264,
    );
    assert!(
        close(p.source.duration_sec, 1.5, 0.06),
        "duration {}",
        p.source.duration_sec
    );
    assert_eq!(
        p.source
            .audio_tracks
            .iter()
            .map(|a| a.codec.as_str())
            .collect::<Vec<_>>(),
        ["aac"; 3]
    );
    assert_eq!(p.source.video.frame_rate, "60/1");
    let mut capped = preset(Preset::Accurate);
    capped.fps_cap = Some(30);
    let out = dir.path().join("c capped.mp4.part");
    let p = export(t, &src, &capped, (1.0, 2.0), &out, ResolvedEncoder::Libx264);
    assert_eq!(p.source.video.frame_rate, "30/1", "FRM-06 cap");
}

#[test]
fn d_vertical_crop_track_selection_native_and_mix_all() {
    let t = need_ffmpeg!();
    let dir = workdir();
    let src = probe(t, &multi_track(t, dir.path())).source;
    let mut s = preset(Preset::Vcrop);
    s.audio_track = 2; // the mono track → stereo (-ac 2)
    let out = dir.path().join("d vcrop.mp4.part");
    let p = export(t, &src, &s, (1.0, 2.0), &out, ResolvedEncoder::Libx264);
    let v = &p.source.video;
    assert_eq!(
        (v.coded_width, v.coded_height, v.sar.as_str()),
        (1080, 1920, "1:1")
    );
    assert_eq!(
        p.source
            .audio_tracks
            .iter()
            .map(|a| a.channels)
            .collect::<Vec<_>>(),
        [2]
    );
    s.output_resolution = OutputResolution::Native;
    s.mix_all_audio = true;
    let out = dir.path().join("d native mix.mp4.part");
    let p = export(t, &src, &s, (1.0, 2.0), &out, ResolvedEncoder::Libx264);
    assert_eq!(
        (p.source.video.coded_width, p.source.video.coded_height),
        (202, 360),
        "Native = window size"
    );
    assert_eq!(
        p.source
            .audio_tracks
            .iter()
            .map(|a| a.channels)
            .collect::<Vec<_>>(),
        [2],
        "3 tracks mixed to stereo"
    );
    assert!(close(p.source.duration_sec, 1.0, 0.06));
}

#[test]
fn e_vertical_blur_prd_chain_with_mix_all() {
    let t = need_ffmpeg!();
    let dir = workdir();
    let src = probe(t, &multi_track(t, dir.path())).source;
    let mut s = preset(Preset::Vblur);
    s.mix_all_audio = true;
    let out = dir.path().join("e vblur.mp4.part");
    let p = export(t, &src, &s, (1.0, 1.5), &out, ResolvedEncoder::Libx264);
    assert_eq!(
        (p.source.video.coded_width, p.source.video.coded_height),
        (1080, 1920)
    );
    assert_eq!(p.source.video.pix_fmt.as_deref(), Some("yuv420p"));
    assert_eq!(p.source.audio_tracks.len(), 1);
}

#[test]
fn f_hdr_hlg_rotated_hevc_tonemap_and_lossless_hvc1() {
    let t = need_ffmpeg!();
    let dir = workdir();
    let flat = generate(
        t,
        dir.path(),
        "hlg flat.mov",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=30:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=d=1",
            "-c:v",
            "libx265",
            "-pix_fmt",
            "yuv420p10le",
            "-x265-params",
            "log-level=error:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc",
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "arib-std-b67",
            "-colorspace",
            "bt2020nc",
            "-c:a",
            "aac",
            "-shortest",
        ],
    );
    let flat = flat.to_string_lossy().into_owned();
    let hdr_path = generate(
        t,
        dir.path(),
        "iPhone HLG.mov",
        &["-display_rotation:v:0", "-90", "-i", &flat, "-c", "copy"],
    );
    let src = probe(t, &hdr_path).source;
    let v = &src.video;
    assert_eq!(
        (v.hdr, v.rotation, v.display_width, v.display_height),
        (true, 90, 180, 320),
        "{v:?}"
    );

    // Lossless HEVC into MP4: tagged hvc1, rotation flag kept (§9).
    let out = dir.path().join("lossless hevc.mp4.part");
    let p = export(
        t,
        &src,
        &preset(Preset::Lossless),
        (0.0, 0.8),
        &out,
        ResolvedEncoder::Libx264,
    );
    assert_eq!(raw_video_stream(t, &out)["codec_tag_string"], "hvc1");
    assert_eq!(p.source.video.rotation, 90);

    if !t.zscale {
        eprintln!("skipping chain F: this FFmpeg build has no zscale");
        return;
    }
    for (p_, name) in [
        (Preset::Vcrop, "f vcrop.mp4.part"),
        (Preset::Vblur, "f vblur.mp4.part"),
        (Preset::Accurate, "f c.mp4.part"),
    ] {
        let mut s = preset(p_);
        s.output_resolution = OutputResolution::Hd720x1280;
        let out = dir.path().join(name);
        let p = export(t, &src, &s, (0.0, 0.8), &out, ResolvedEncoder::Libx264);
        let v = &p.source.video;
        assert_eq!(
            v.color_transfer.as_deref(),
            Some("bt709"),
            "{name}: FRM-07 tags"
        );
        assert_eq!(
            (v.pix_fmt.as_deref(), v.bit_depth),
            (Some("yuv420p"), Some(8)),
            "{name}"
        );
        assert!(!v.hdr, "{name}");
        let dims = (v.coded_width, v.coded_height);
        let want = if p_ == Preset::Accurate {
            (180, 320)
        } else {
            (720, 1280)
        };
        assert_eq!(dims, want, "{name}: auto-rotation before filters");
    }
}

#[test]
fn d_non_square_pixels_are_normalised() {
    let t = need_ffmpeg!();
    let dir = workdir();
    let path = generate(
        t,
        dir.path(),
        "pal sar.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=720x576:rate=25:duration=1,setsar=16/15",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
        ],
    );
    let src = probe(t, &path).source;
    assert_eq!(
        (src.video.display_width, src.video.sar.as_str()),
        (768, "16:15")
    );
    let out = dir.path().join("sar vcrop.mp4.part");
    let p = export(
        t,
        &src,
        &preset(Preset::Vcrop),
        (0.0, 0.8),
        &out,
        ResolvedEncoder::Libx264,
    );
    assert_eq!(
        (
            p.source.video.coded_width,
            p.source.video.coded_height,
            p.source.video.sar.as_str()
        ),
        (1080, 1920, "1:1")
    );
    assert!(
        p.source.audio_tracks.is_empty(),
        "no audio → video-only (EXP-04)"
    );
}

#[test]
fn exp03_pcm_mov_lossless_and_aac_offer() {
    let t = need_ffmpeg!();
    let dir = workdir();
    let path = generate(
        t,
        dir.path(),
        "camera pcm.mov",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=30:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=d=2",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "30",
            "-c:a",
            "pcm_s16le",
            "-shortest",
        ],
    );
    let src = probe(t, &path).source;
    let s = preset(Preset::Lossless);
    let d = lossless_container(&src, &s);
    assert_eq!((d.muxer, d.offer_aac_audio), (Muxer::Mov, true));
    let out = dir.path().join(format!("pcm.{}.part", d.ext));
    let p = export(t, &src, &s, (0.5, 1.5), &out, ResolvedEncoder::Libx264);
    assert_eq!(p.source.audio_tracks[0].codec, "pcm_s16le");
    let accepted = ExportSettings {
        lossless_audio_to_aac: true,
        ..s
    };
    let out = dir.path().join("pcm to aac.mp4.part");
    let p = export(
        t,
        &src,
        &accepted,
        (0.5, 1.5),
        &out,
        ResolvedEncoder::Libx264,
    );
    assert_eq!(
        (
            p.source.video.codec.as_str(),
            p.source.audio_tracks[0].codec.as_str()
        ),
        ("h264", "aac")
    );
    assert!(p.source.format_name.unwrap_or_default().contains("mp4"));
}

#[test]
fn exp03_mp4_allow_list_is_accepted_by_the_mp4_muxer() {
    let t = need_ffmpeg!();
    let dir = workdir();
    let encoder = |codec: &'static str| match codec {
        "mp3" => "libmp3lame",
        "opus" => "libopus",
        "h264" => "libx264",
        "hevc" => "libx265",
        "av1" => "libsvtav1",
        "vp9" => "libvpx-vp9",
        other => other,
    };
    let list = Command::new(&t.ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output()
        .expect("encoders");
    let available = String::from_utf8_lossy(&list.stdout).into_owned();
    for codec in MP4_AUDIO_CODECS.iter().chain(MP4_VIDEO_CODECS) {
        let enc = encoder(codec);
        if !available.contains(&format!(" {enc} ")) {
            eprintln!("skipping {codec}: encoder {enc} not in this build");
            continue;
        }
        let audio = MP4_AUDIO_CODECS.contains(codec);
        let input = if audio {
            "sine=d=0.5:sample_rate=48000"
        } else {
            "testsrc2=size=128x96:rate=10:duration=0.5"
        };
        let mut args = vec!["-f", "lavfi", "-i", input, "-c", enc];
        if !audio {
            args.extend(["-pix_fmt", "yuv420p"]);
        }
        let mkv = generate(t, dir.path(), &format!("{codec}.mkv"), &args);
        let out = dir.path().join(format!("{codec}.mp4"));
        let o = Command::new(&t.ffmpeg)
            .args(["-hide_banner", "-nostdin", "-y", "-v", "error", "-i"])
            .arg(&mkv)
            .args(["-c", "copy", "-f", "mp4"])
            .arg(&out)
            .output()
            .expect("spawn");
        assert!(
            o.status.success(),
            "{codec} must stream-copy into MP4: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

#[test]
fn edge_no_audio_source_and_missing_output_folder() {
    let t = need_ffmpeg!();
    let dir = workdir();
    let path = generate(
        t,
        dir.path(),
        "silent.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=30:duration=2",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
        ],
    );
    let src = probe(t, &path).source;
    for p_ in Preset::ALL {
        let out = dir.path().join(format!("silent {}.mp4.part", p_.as_str()));
        let p = export(
            t,
            &src,
            &preset(p_),
            (0.5, 1.5),
            &out,
            ResolvedEncoder::Libx264,
        );
        assert!(p.source.audio_tracks.is_empty(), "{p_:?}");
    }
    // EXP-10: a vanished output folder is classified so the batch pauses.
    let s = preset(Preset::Accurate);
    let container = lossless_container(&src, &s);
    let gone = dir.path().join("unplugged drive").join("clip.mp4.part");
    let cmd = build_export_command(&JobSpec {
        input: Path::new(&src.path),
        output_part: &gone,
        source: &src,
        start_sec: 0.0,
        end_sec: 1.0,
        crop_x_offset: 0.0,
        settings: &s,
        container: &container,
        encoder: ResolvedEncoder::Libx264,
        threads: None,
        zscale_available: false,
        snap_in_to: None,
    });
    let r = run(t, &cmd);
    assert!(!r.ok);
    assert_eq!(
        classify(&r.stderr),
        FailureKind::OutputMissing,
        "{}",
        r.stderr
    );
}

#[test]
fn exp13_hardware_detection_and_export() {
    let t = need_ffmpeg!();
    for enc in EncoderChoice::HARDWARE {
        let args = hw_test_args(enc).expect("hardware args");
        let o = Command::new(&t.ffmpeg).args(args).output().expect("spawn");
        let stderr = String::from_utf8_lossy(&o.stderr);
        if !o.status.success() {
            eprintln!(
                "{enc:?} unavailable here: {}",
                stderr.lines().next().unwrap_or_default()
            );
            assert_ne!(
                classify_attempt(&stderr, true),
                FailureKind::Other,
                "{enc:?} failure should be classified: {stderr}"
            );
            continue;
        }
        // Detected: a real vertical export with this encoder must work too.
        let dir = workdir();
        let src = probe(t, &multi_track(t, dir.path())).source;
        let out = dir.path().join(format!("hw {enc:?}.mp4.part"));
        let p = export(
            t,
            &src,
            &preset(Preset::Vcrop),
            (1.0, 2.0),
            &out,
            ResolvedEncoder::Hardware(enc),
        );
        assert_eq!(
            (p.source.video.codec.as_str(), p.source.video.coded_height),
            ("h264", 1920),
            "{enc:?}"
        );
        eprintln!("{enc:?}: detection encode and vertical export succeeded");
    }
}
