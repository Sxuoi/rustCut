//! Golden argument vectors: PRD §7.6 B/C/D/E reproduced exactly (the PRD's
//! quotes are for readability only; argv carries none), plus every variant
//! the PRD names (hvc1, chain F, fps cap, Native, SAR, EXP-04 mapping,
//! EXP-12 threads, EXP-13 hardware).

use rustcut_ffmpeg::args::{packet_index_args, probe_args, verify_args};
use rustcut_ffmpeg::{JobSpec, ResolvedEncoder, build_export_command, lossless_container};
use rustcut_model::{
    AudioTrack, EncoderChoice, ExportSettings, Fingerprint, OutputResolution, Preset, SourceInfo,
    VideoInfo,
};
use std::ffi::OsString;
use std::path::Path;

const IN: &str = "livestream_2026_10_21.mp4";
const CHAIN_E: &str = "[0:v]split=2[bg][fg];[bg]scale=270:480:force_original_aspect_ratio=increase,crop=270:480,boxblur=10:2,scale=1080:1920,setsar=1[bgv];[fg]scale=1080:1920:force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1[fgv];[bgv][fgv]overlay=(W-w)/2:(H-h)/2,format=yuv420p[vout]";
const CHAIN_F: &str = "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p";

fn argv(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

fn livestream() -> SourceInfo {
    SourceInfo {
        path: IN.into(),
        relative_path: String::new(),
        fingerprint: Fingerprint {
            size_bytes: 15_000_000_000,
            xxh3_head_tail: String::new(),
        },
        duration_sec: 3.0 * 3600.0,
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
            pix_fmt: Some("yuv420p".into()),
            profile: Some("High".into()),
            level: Some(42),
            color_transfer: None,
            bit_depth: Some(8),
        },
        audio_tracks: (1..=3)
            .map(|i| AudioTrack {
                stream_index: i,
                codec: "aac".into(),
                channels: 2,
                title: None,
                language: None,
                sample_rate: Some(48_000),
                is_default: i == 1,
            })
            .collect(),
        format_name: Some("mov,mp4,m4a,3gp,3g2,mj2".into()),
        bit_rate: Some(12_000_000),
    }
}

fn settings(preset: Preset) -> ExportSettings {
    let mut s = ExportSettings::default();
    s.apply_preset(preset);
    s
}

struct Job {
    source: SourceInfo,
    settings: ExportSettings,
    offset: f64,
    encoder: ResolvedEncoder,
    threads: Option<u32>,
    zscale: bool,
}

impl Job {
    fn new(preset: Preset) -> Self {
        Job {
            source: livestream(),
            settings: settings(preset),
            offset: 0.0,
            encoder: ResolvedEncoder::Libx264,
            threads: None,
            zscale: true,
        }
    }

    fn args(&self, out: &str) -> Vec<String> {
        let container = lossless_container(&self.source, &self.settings);
        build_export_command(&JobSpec {
            input: Path::new(IN),
            output_part: Path::new(out),
            source: &self.source,
            start_sec: 920.12,
            end_sec: 955.8,
            crop_x_offset: self.offset,
            settings: &self.settings,
            container: &container,
            encoder: self.encoder,
            threads: self.threads,
            zscale_available: self.zscale,
            snap_in_to: None,
        })
        .to_string_args()
    }
}

const HEAD: &str =
    "-hide_banner -nostdin -y -ss 920.120 -to 955.800 -i ./livestream_2026_10_21.mp4";
// Re-encodes bias -ss/-to by half a millisecond (EXP-02, 7.6).
const HEAD_ENC: &str =
    "-hide_banner -nostdin -y -ss 920.119 -to 955.799 -i ./livestream_2026_10_21.mp4";

#[test]
fn prd_7_6_b_lossless_exact() {
    let got = Job::new(Preset::Lossless).args("02_Jump_Scare_Reaction.mp4.part");
    let want = format!(
        "{HEAD} -map 0:v:0 -map 0:a? -c copy -avoid_negative_ts make_zero -progress pipe:1 -nostats \
         -f mp4 ./02_Jump_Scare_Reaction.mp4.part"
    );
    assert_eq!(got, argv(&want));
}

#[test]
fn prd_7_6_b_exp03_hevc_adds_hvc1_tag_after_c_copy() {
    let mut job = Job::new(Preset::Lossless);
    job.source.video.codec = "hevc".into();
    let want = format!(
        "{HEAD} -map 0:v:0 -map 0:a? -c copy -tag:v hvc1 -avoid_negative_ts make_zero -progress pipe:1 -nostats \
         -f mp4 ./o.mp4.part"
    );
    assert_eq!(job.args("o.mp4.part"), argv(&want));
    // HEVC kept in Matroska (PCM audio, offer declined) is not tagged.
    job.source.format_name = Some("matroska,webm".into());
    job.source.audio_tracks[0].codec = "pcm_s16le".into();
    let got = job.args("o.mkv.part").join(" ");
    assert!(
        !got.contains("hvc1") && got.ends_with("-f matroska ./o.mkv.part"),
        "{got}"
    );
}

#[test]
fn prd_7_6_c_accurate_exact() {
    let got = Job::new(Preset::Accurate).args("02_Jump_Scare_Reaction_accurate.mp4.part");
    let want = format!(
        "{HEAD_ENC} -map 0:v:0 -map 0:a? -c:v libx264 -preset fast -crf 18 -pix_fmt yuv420p -fps_mode passthrough \
         -c:a aac -b:a 192k -movflags +faststart -progress pipe:1 -nostats -f mp4 ./02_Jump_Scare_Reaction_accurate.mp4.part"
    );
    assert_eq!(got, argv(&want));
}

#[test]
fn prd_7_6_d_vcrop_exact() {
    let mut job = Job::new(Preset::Vcrop);
    job.offset = 0.3;
    let got = job.args("02_Jump_Scare_Reaction_vcrop.mp4.part");
    let want = format!(
        "{HEAD_ENC} -map 0:v:0 -map 0:a:0? -vf crop=608:1080:852:0,scale=1080:1920:flags=lanczos,setsar=1 \
         -c:v libx264 -preset fast -crf 20 -maxrate 16M -bufsize 32M -pix_fmt yuv420p -fps_mode passthrough \
         -c:a aac -b:a 192k -ac 2 -movflags +faststart -progress pipe:1 -nostats -f mp4 ./02_Jump_Scare_Reaction_vcrop.mp4.part"
    );
    assert_eq!(got, argv(&want));
}

#[test]
fn prd_7_6_e_vblur_exact() {
    let got = Job::new(Preset::Vblur).args("02_Jump_Scare_Reaction_vblur.mp4.part");
    let want = format!(
        "{HEAD_ENC} -filter_complex {CHAIN_E} -map [vout] -map 0:a:0? -c:v libx264 -preset fast -crf 20 -maxrate 16M \
         -bufsize 32M -fps_mode passthrough -c:a aac -b:a 192k -ac 2 -movflags +faststart -progress pipe:1 -nostats \
         -f mp4 ./02_Jump_Scare_Reaction_vblur.mp4.part"
    );
    assert_eq!(got, argv(&want));
}

#[test]
fn prd_7_6_c_gets_vf_only_for_fps_cap_or_tonemap() {
    let mut job = Job::new(Preset::Accurate);
    job.source.video.frame_rate = "144/1".into();
    let got = job.args("o.mp4.part").join(" ");
    assert!(got.contains("-map 0:a? -vf fps=60 -c:v libx264"), "{got}");
    job.settings.fps_cap = None; // FRM-06: cap off → timing untouched
    assert!(!job.args("o.mp4.part").contains(&"-vf".to_string()));
    let mut hdr = Job::new(Preset::Accurate);
    hdr.source.video.hdr = true;
    let got = hdr.args("o.mp4.part").join(" ");
    assert!(
        got.contains(&format!("-vf {CHAIN_F} -c:v libx264")) && got.contains(
            "-b:a 192k -color_primaries bt709 -color_trc bt709 -colorspace bt709 -movflags +faststart"
        ),
        "{got}"
    );
    hdr.zscale = false; // FRM-07 P0: no zscale → no chain F, no tags (pre-flight warns)
    let got = hdr.args("o.mp4.part").join(" ");
    assert!(
        !got.contains("zscale") && !got.contains("bt709") && !got.contains("-vf"),
        "{got}"
    );
}

#[test]
fn prd_7_6_d_sar_normalised_first() {
    let mut job = Job::new(Preset::Vcrop);
    job.source.video.coded_width = 720;
    job.source.video.coded_height = 576;
    job.source.video.display_width = 768;
    job.source.video.display_height = 576;
    job.source.video.sar = "16:15".into();
    job.source.video.frame_rate = "25/1".into();
    let got = job.args("o.mp4.part").join(" ");
    assert!(
        got.contains(
            "-vf scale=768:576,setsar=1,crop=324:576:222:0,scale=1080:1920:flags=lanczos,setsar=1 "
        ),
        "{got}"
    );
}

#[test]
fn prd_7_6_d_native_and_fps_cap_and_chain_f() {
    let mut job = Job::new(Preset::Vcrop);
    job.offset = 0.3;
    job.settings.output_resolution = OutputResolution::Native;
    assert!(
        job.args("o.mp4.part")
            .join(" ")
            .contains("-vf crop=608:1080:852:0,setsar=1 -c:v")
    );
    job.settings.output_resolution = OutputResolution::Full1080x1920;
    job.source.video.frame_rate = "144/1".into();
    assert!(
        job.args("o.mp4.part")
            .join(" ")
            .contains("-vf crop=608:1080:852:0,fps=60,scale=1080:1920:flags=lanczos,setsar=1 ")
    );
    job.source.video.frame_rate = "60/1".into();
    job.source.video.hdr = true;
    let got = job.args("o.mp4.part").join(" ");
    assert!(
        got.contains(&format!(
            "-vf {CHAIN_F},crop=608:1080:852:0,scale=1080:1920"
        )),
        "{got}"
    );
    assert!(
        got.contains(
            "-ac 2 -color_primaries bt709 -color_trc bt709 -colorspace bt709 -movflags +faststart"
        ),
        "{got}"
    );
    let mut blur = Job::new(Preset::Vblur);
    blur.source.video.hdr = true;
    let got = blur.args("o.mp4.part").join(" ");
    assert!(
        got.contains(&format!("-filter_complex [0:v]{CHAIN_F},split=2[bg][fg];")),
        "{got}"
    );
}

#[test]
fn exp12_threads_follow_the_video_codec_options() {
    let mut job = Job::new(Preset::Vcrop);
    job.threads = Some(4);
    let got = job.args("o.mp4.part").join(" ");
    assert!(
        got.contains(
            "-crf 20 -maxrate 16M -bufsize 32M -threads 4 -pix_fmt yuv420p -fps_mode passthrough"
        ),
        "{got}"
    );
    let mut c = Job::new(Preset::Accurate);
    c.threads = Some(2);
    assert!(
        c.args("o.mp4.part")
            .join(" ")
            .contains("-crf 18 -threads 2 -pix_fmt yuv420p")
    );
    let mut lossless = Job::new(Preset::Lossless);
    lossless.threads = Some(2);
    assert!(
        !lossless
            .args("o.mp4.part")
            .contains(&"-threads".to_string())
    );
}

#[test]
fn exp04_vertical_track_and_mix_all() {
    let mut job = Job::new(Preset::Vcrop);
    job.settings.audio_track = 2;
    assert!(
        job.args("o.mp4.part")
            .join(" ")
            .contains("-map 0:v:0 -map 0:a:1? -vf")
    );
    job.settings.mix_all_audio = true;
    let got = job.args("o.mp4.part").join(" ");
    let mix = "[0:a:0][0:a:1][0:a:2]amix=inputs=3:duration=longest:normalize=0[aout]";
    assert!(
        got.contains(&format!(
            "-filter_complex {mix} -map 0:v:0 -map [aout] -vf crop="
        )),
        "{got}"
    );
    assert!(got.contains("-c:a aac -b:a 192k -ac 2"), "{got}");
    let mut blur = Job::new(Preset::Vblur);
    blur.settings.mix_all_audio = true;
    let got = blur.args("o.mp4.part").join(" ");
    assert!(
        got.contains(&format!(
            "-filter_complex {CHAIN_E};{mix} -map [vout] -map [aout] -c:v libx264"
        )),
        "{got}"
    );
    // Frame-Accurate Original keeps every track regardless of the vertical setting.
    let mut c = Job::new(Preset::Accurate);
    c.settings.mix_all_audio = true;
    assert!(
        c.args("o.mp4.part")
            .join(" ")
            .contains("-map 0:v:0 -map 0:a? -c:v")
    );
}

#[test]
fn exp04_sources_without_audio_need_no_special_case() {
    for preset in Preset::ALL {
        let mut job = Job::new(preset);
        job.source.audio_tracks.clear();
        let got = job.args("o.mp4.part").join(" ");
        let map = if preset.is_vertical() {
            "-map 0:a:0?"
        } else {
            "-map 0:a?"
        };
        assert!(
            got.contains(map) && !got.contains("-an"),
            "{preset:?}: {got}"
        );
        assert!(
            !got.contains("0:s") && !got.contains("0:d") && !got.contains("0:t"),
            "EXP-04 never maps subtitles/data"
        );
    }
}

#[test]
fn exp03_lossless_containers_and_aac_offer() {
    let mut job = Job::new(Preset::Lossless);
    job.source.format_name = Some("mov,mp4,m4a,3gp,3g2,mj2".into());
    job.source.path = "cam.mov".into();
    job.source.audio_tracks.truncate(1);
    job.source.audio_tracks[0].codec = "pcm_s16le".into();
    let got = job.args("o.mov.part").join(" ");
    assert!(
        got.contains("-c copy -avoid_negative_ts make_zero")
            && got.ends_with("-f mov ./o.mov.part"),
        "{got}"
    );
    job.settings.lossless_audio_to_aac = true;
    job.source.video.codec = "hevc".into();
    let got = job.args("o.mp4.part").join(" ");
    let want = "-map 0:v:0 -map 0:a? -c:v copy -tag:v hvc1 -c:a aac -b:a 192k -avoid_negative_ts make_zero \
                -progress pipe:1 -nostats -f mp4 ./o.mp4.part";
    assert!(got.ends_with(want), "{got}");
}

#[test]
fn exp13_hardware_encoder_in_vertical_crop() {
    let mut job = Job::new(Preset::Vcrop);
    job.offset = 0.3;
    job.encoder = ResolvedEncoder::Hardware(EncoderChoice::H264Amf);
    job.threads = Some(4);
    let got = job.args("o.mp4.part").join(" ");
    assert!(
        got.contains(
            "-vf crop=608:1080:852:0,scale=1080:1920:flags=lanczos,setsar=1 -c:v h264_amf -quality quality -rc vbr_peak \
             -b:v 10M -maxrate 16M -bufsize 32M -pix_fmt yuv420p -fps_mode passthrough -c:a aac"
        ),
        "{got}"
    );
    job.encoder = ResolvedEncoder::Hardware(EncoderChoice::H264Nvenc);
    let accurate = Job {
        encoder: job.encoder,
        ..Job::new(Preset::Accurate)
    };
    assert!(
        accurate
            .args("o.mp4.part")
            .join(" ")
            .contains("-c:v h264_nvenc -preset p5 -rc vbr -cq 18 -b:v 0 -pix_fmt")
    );
}

#[test]
fn s7_6_paths_pass_through_unmodified() {
    let src = livestream();
    let s = settings(Preset::Lossless);
    let container = lossless_container(&src, &s);
    // Absolute paths in the platform's own form (on Unix `D:/...` is relative and gets `./`).
    let (input, output) = if cfg!(windows) {
        (
            "D:/Rekaman Siaran/Bahas Patch 2.0 🔥 «финал».mkv",
            "D:/Rekaman Siaran/clips/02_Bahas Patch 🔥.mp4.part",
        )
    } else {
        (
            "/media/Rekaman Siaran/Bahas Patch 2.0 🔥 «финал».mkv",
            "/media/Rekaman Siaran/clips/02_Bahas Patch 🔥.mp4.part",
        )
    };
    let (input, output) = (Path::new(input), Path::new(output));
    let args = build_export_command(&JobSpec {
        input,
        output_part: output,
        source: &src,
        start_sec: 1.0,
        end_sec: 2.0,
        crop_x_offset: 0.0,
        settings: &s,
        container: &container,
        encoder: ResolvedEncoder::Libx264,
        threads: None,
        zscale_available: false,
        snap_in_to: None,
    })
    .to_args();
    assert_eq!(args[8], OsString::from(input));
    assert_eq!(args.last(), Some(&OsString::from(output)));
}

#[test]
fn prd_7_6_a_probe_index_and_verify_args() {
    let s = |v: Vec<OsString>| {
        v.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        s(probe_args(IN)),
        argv("-v error -show_format -show_streams -of json ./livestream_2026_10_21.mp4")
    );
    assert_eq!(
        s(packet_index_args(IN)),
        argv(
            "-v error -select_streams v:0 -show_entries packet=pts_time,flags -of csv=p=0 ./livestream_2026_10_21.mp4"
        )
    );
    let v = s(verify_args("o.mp4"));
    assert_eq!(v.first().map(String::as_str), Some("-v"));
    assert!(v.contains(&"json".to_string()) && v.last().map(String::as_str) == Some("./o.mp4"));
}
