//! Real-media smoke test: probe, index, pre-flight and export 3 short clips per preset of every
//! video given on the command line (default: every video in `.fixtures/real`), then verify the
//! outputs with ffprobe/ffmpeg. Run:
//! `cargo run -p rustcut-export --example real_smoke -- [files...]` (set `RUSTCUT_FFMPEG_DIR`).

use rustcut_export::{
    BatchRequest, EngineConfig, ExportEngine, ExportSink, PreflightInput, RealFs, preflight,
};
use rustcut_media::{MediaConfig, MediaService};
use rustcut_model::{
    BatchState, Clip, ClipFinished, ClipOutcome, ExportProgress, ExportRecord, ExportSettings,
    ExportStatus, Preset, Project, SourceInfo,
};
use rustcut_runner::{Sidecars, probe_capabilities};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, mpsc};
use uuid::Uuid;

struct Sink {
    done: Mutex<Vec<(Uuid, ClipOutcome)>>,
    tx: Mutex<mpsc::Sender<()>>,
}
impl ExportSink for Sink {
    fn on_clip_started(&self, _: &str, _: Uuid, _: &Path) {}
    fn on_progress(&self, _: ExportProgress) {}
    fn on_clip_finished(&self, _: ExportRecord, f: ClipFinished) {
        self.done.lock().unwrap().push((f.clip_id, f.outcome));
    }
    fn on_batch_state(&self, s: BatchState) {
        if matches!(s, BatchState::Finished { .. }) {
            let _ = self.tx.lock().unwrap().send(());
        }
    }
}

fn clip(index: u32, name: &str, a: f64, b: f64, crop: f64) -> Clip {
    Clip {
        id: Uuid::now_v7(),
        index,
        name: name.into(),
        start_sec: a,
        end_sec: b,
        tag_id: None,
        crop_x_offset: crop,
        selected_for_export: true,
        export_status: ExportStatus::Pending,
        last_export: None,
    }
}

fn ffprobe_json(ff: &Sidecars, p: &Path) -> Value {
    let o = Command::new(&ff.ffprobe)
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(p)
        .output()
        .expect("ffprobe");
    serde_json::from_slice(&o.stdout).unwrap_or(Value::Null)
}

fn decode_errors(ff: &Sidecars, p: &Path) -> String {
    let o = Command::new(&ff.ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(p)
        .args(["-f", "null", "-"])
        .output()
        .expect("ffmpeg");
    String::from_utf8_lossy(&o.stderr).trim().to_string()
}

fn num(v: &Value) -> f64 {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .or(v.as_f64())
        .unwrap_or(0.0)
}

/// Returns a list of problems (empty = OK) and a short fact string.
fn verify(
    ff: &Sidecars,
    out: &Path,
    src: &SourceInfo,
    preset: Preset,
    clip: &Clip,
    lead_in: f64,
) -> (Vec<String>, String) {
    let mut bad = Vec::new();
    let j = ffprobe_json(ff, out);
    let streams = j["streams"].as_array().cloned().unwrap_or_default();
    let ty = |t: &str| streams.iter().filter(|s| s["codec_type"] == t).count();
    let (nv, na) = (ty("video"), ty("audio"));
    if streams.len() != nv + na {
        bad.push(format!("extra streams: {}", streams.len() - nv - na));
    }
    if nv != 1 {
        bad.push(format!("video streams {nv}"));
    }
    // Audio that FFmpeg can carry at all (apple_apac cannot be decoded or muxed).
    let carriable = src
        .audio_tracks
        .iter()
        .filter(|t| t.codec != "apple_apac")
        .count();
    let want_a = match preset {
        Preset::Lossless | Preset::Accurate => carriable,
        _ => usize::from(carriable > 0),
    };
    if na != want_a {
        bad.push(format!("audio streams {na}, want {want_a}"));
    }
    if preset != Preset::Lossless {
        for s in streams.iter().filter(|s| s["codec_type"] == "audio") {
            if s["codec_name"] != "aac" {
                bad.push(format!("audio codec {}", s["codec_name"]));
            }
            if matches!(preset, Preset::Vcrop | Preset::Vblur) && s["channels"] != 2 {
                bad.push("vertical audio not stereo".into());
            }
        }
    }
    let v = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .cloned()
        .unwrap_or(Value::Null);
    let vc = v["codec_name"].as_str().unwrap_or("?").to_string();
    let fmt = j["format"]["format_name"]
        .as_str()
        .unwrap_or("?")
        .to_string();
    match preset {
        Preset::Lossless => {
            if vc != src.video.codec {
                bad.push(format!("video codec {vc} != {}", src.video.codec));
            }
            if vc == "hevc" && fmt.contains("mp4") && v["codec_tag_string"] != "hvc1" {
                bad.push(format!("hevc tag {}", v["codec_tag_string"]));
            }
        }
        _ => {
            if vc != "h264" {
                bad.push(format!("video codec {vc}"));
            }
            if !fmt.contains("mp4") {
                bad.push(format!("container {fmt}"));
            }
        }
    }
    let (w, h) = (num(&v["width"]) as u32, num(&v["height"]) as u32);
    if matches!(preset, Preset::Vcrop | Preset::Vblur) && (w, h) != (1080, 1920) {
        bad.push(format!("size {w}x{h}"));
    }
    let dur = num(&j["format"]["duration"]);
    let want = clip.end_sec.min(src.duration_sec) - clip.start_sec;
    let (lo, hi) = if preset == Preset::Lossless {
        (want - 0.3, want + lead_in + 0.6)
    } else {
        (want - 0.3, want + 0.3)
    };
    if dur < lo || dur > hi {
        bad.push(format!("duration {dur:.2} not in [{lo:.2},{hi:.2}]"));
    }
    let err = decode_errors(ff, out);
    if !err.is_empty() {
        bad.push(format!("decode: {}", err.lines().next().unwrap_or("")));
    }
    (bad, format!("{fmt} {vc} {w}x{h} v{nv}a{na} {dur:.2}s"))
}

#[tokio::main]
async fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let mut files: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if files.is_empty() {
        let dir = root.join(".fixtures/real");
        let exts = ["mp4", "mov", "mkv", "webm", "avi", "m4v"];
        files = std::fs::read_dir(&dir)
            .expect("read .fixtures/real")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .is_some_and(|x| exts.contains(&x.to_string_lossy().to_lowercase().as_str()))
            })
            .collect();
        files.sort();
    }
    let sidecars = match std::env::var_os("RUSTCUT_FFMPEG_DIR") {
        Some(d) => Sidecars::from_dir(d).expect("RUSTCUT_FFMPEG_DIR"),
        None => Sidecars::locate().expect("ffmpeg not found"),
    };
    let zscale = probe_capabilities(&sidecars)
        .await
        .map(|c| c.has_filter("zscale"))
        .unwrap_or(false);
    let tmp = root.join(".fixtures/tmp/real_smoke");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let media = MediaService::new(MediaConfig {
        sidecars: sidecars.clone(),
        cache_dir: tmp.join("cache"),
        data_dir: tmp.join("data"),
        cache_cap_bytes: 1 << 30,
    });
    let mut table: Vec<String> = Vec::new();
    let mut failures = 0;
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        println!("\n=== {name}");
        let probed = match media.probe_source(path).await {
            Ok(p) => p,
            Err(e) => {
                println!("probe FAILED: {e}");
                table.push(format!("{name} | probe | FAIL | {e}"));
                failures += 1;
                continue;
            }
        };
        let mut src = probed.source.clone();
        src.path = path.to_string_lossy().into_owned();
        let v = &src.video;
        println!(
            "mime={} {} {}x{} rot={} hdr={} vfr={} fps={} dur={:.2}s start={:.3}",
            probed.mime_type,
            v.codec,
            v.display_width,
            v.display_height,
            v.rotation,
            v.hdr,
            v.vfr,
            v.frame_rate,
            src.duration_sec,
            src.start_time_sec
        );
        for w in &probed.warnings {
            println!("  warning: {w}");
        }
        for t in &src.audio_tracks {
            println!("  audio #{} {} ch={}", t.stream_index, t.codec, t.channels);
        }
        let index = media.build_index(path, &src, |_| {}).await.ok();
        if let Some(ix) = &index {
            let mut gops: Vec<f64> = ix.keyframes.windows(2).map(|w| w[1] - w[0]).collect();
            gops.sort_by(|a, b| a.total_cmp(b));
            let typ = gops.get(gops.len() / 2).copied().unwrap_or(0.0);
            let lead = ix.lead_in(src.duration_sec * 0.5).unwrap_or(0.0);
            println!(
                "  index: {} keyframes, {} frames, typical GOP {typ:.2}s, max GOP {:.2}s, lead-in@mid {lead:.2}s",
                ix.keyframes.len(),
                ix.frame_times.len(),
                gops.last().copied().unwrap_or(0.0)
            );
        } else {
            println!("  index: FAILED");
        }
        let d = src.duration_sec;
        let mut project = Project::new(src.clone());
        project.clips = vec![
            clip(1, "start", 0.5, 2.5_f64.min(d), 0.0),
            clip(2, "middle", d * 0.5 - 1.0, d * 0.5 + 1.0, 0.0),
            clip(3, "end", (d - 2.0).max(0.0), d, 0.0),
        ];
        project.next_index = 4;
        let mut cases: Vec<(String, Preset, Vec<Clip>)> = vec![
            ("Lossless".into(), Preset::Lossless, project.clips.clone()),
            ("Accurate".into(), Preset::Accurate, project.clips.clone()),
        ];
        let mut crop = Vec::new();
        for (i, off) in [-1.0, 0.0, 1.0].iter().enumerate() {
            crop.push(clip(
                i as u32 + 1,
                &format!("crop{i}"),
                d * 0.4,
                d * 0.4 + 1.5,
                *off,
            ));
        }
        cases.push(("VCrop".into(), Preset::Vcrop, crop));
        cases.push(("VBlur".into(), Preset::Vblur, project.clips.clone()));
        for (label, preset, clips) in cases {
            let mut p = project.clone();
            p.clips = clips.clone();
            let mut settings = ExportSettings::default();
            settings.apply_preset(preset);
            settings.fps_cap = Some(30);
            let short: String = name
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(12)
                .collect();
            let out_dir = tmp.join(format!("{short}_{label}"));
            settings.output_dir = Some(out_dir.to_string_lossy().into_owned());
            p.export_settings = settings.clone();
            let ids: Vec<Uuid> = clips.iter().map(|c| c.id).collect();
            let report = preflight(&PreflightInput {
                project: &p,
                clip_ids: &ids,
                settings: &settings,
                media_index: index.as_ref(),
                current_fingerprint: Some(src.fingerprint.clone()),
                zscale_available: zscale,
                today: chrono::Utc::now().date_naive(),
                fs: &RealFs,
            });
            println!(
                "-- {label}: ext={} can_start={} offer_aac={}",
                report.extension, report.can_start, report.offer_aac_audio
            );
            for it in &report.items {
                println!("   [{:?}] {:?}: {}", it.level, it.check, it.message);
            }
            if !report.can_start {
                table.push(format!("{name} | {label} | FAIL | preflight blocked"));
                failures += 1;
                continue;
            }
            let engine = ExportEngine::new(EngineConfig {
                sidecars: sidecars.clone(),
                log_dir: tmp.join("logs"),
                data_dir: tmp.join("data"),
                cpu_count: 2,
                hw_encoders: vec![],
                zscale_available: zscale,
            });
            let (tx, rx) = mpsc::channel();
            let sink = Arc::new(Sink {
                done: Mutex::default(),
                tx: Mutex::new(tx),
            });
            let outputs = report.outputs.clone();
            engine
                .start(
                    BatchRequest {
                        project: p.clone(),
                        settings: settings.clone(),
                        report,
                        media_index: index.clone(),
                    },
                    sink.clone(),
                )
                .expect("start");
            tokio::task::spawn_blocking(move || {
                rx.recv_timeout(std::time::Duration::from_secs(900))
            })
            .await
            .unwrap()
            .expect("batch timeout");
            let done = sink.done.lock().unwrap().clone();
            for c in &clips {
                let outcome = done.iter().find(|(id, _)| *id == c.id).map(|d| d.1.clone());
                let planned = outputs.iter().find(|o| o.clip_id == c.id);
                let lead = planned.and_then(|o| o.lead_in_sec).unwrap_or(0.0);
                let tag = format!("{name} | {label} #{} off={}", c.index, c.crop_x_offset);
                match outcome {
                    Some(ClipOutcome::Done { output_path, .. }) => {
                        let (bad, facts) =
                            verify(&sidecars, Path::new(&output_path), &src, preset, c, lead);
                        if bad.is_empty() {
                            table.push(format!("{tag} | OK | {facts}"));
                        } else {
                            failures += 1;
                            table.push(format!("{tag} | FAIL | {facts} | {}", bad.join("; ")));
                        }
                    }
                    Some(ClipOutcome::Failed {
                        message, log_tail, ..
                    }) => {
                        failures += 1;
                        table.push(format!("{tag} | FAIL | export failed: {message}"));
                        for l in log_tail.iter().rev().take(4).rev() {
                            println!("     log: {l}");
                        }
                    }
                    other => {
                        failures += 1;
                        table.push(format!("{tag} | FAIL | outcome {other:?}"));
                    }
                }
            }
        }
    }
    println!("\n==== RESULTS ====");
    for r in &table {
        println!("{r}");
    }
    println!("failures: {failures}");
    if failures > 0 {
        std::process::exit(1);
    }
}
