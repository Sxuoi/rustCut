//! Read-only media tools: probe, transcripts, loudness, frames.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use rustcut_media::ProbedSource;
use rustcut_runner::run_capture;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Content, Ctx, ToolOutput, ToolResult, decode};
use crate::env::{Env, filter_path, last_error_line};
use crate::loud::{RankOpts, parse_ametadata, rank_moments};
use crate::projects::find_readonly;
use crate::timeparse::{TimeArg, fmt_hms, fmt_ts, parse_time};
use crate::transcript::{
    Cue, clean_cues, format_lines, is_text_codec, page, parse_srt, render_page,
};

const SHORT: Duration = Duration::from_secs(120);

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn opt_time(t: &Option<TimeArg>) -> Result<Option<f64>, String> {
    t.as_ref().map(parse_time).transpose()
}

/// `{ "path": ... }`.
#[derive(Deserialize)]
pub struct PathArgs {
    /// Video path.
    pub path: String,
}

// ───────────────────────────── probe_video ─────────────────────────────

/// A subtitle stream of the video.
#[derive(Debug, Clone, PartialEq)]
pub struct SubTrack {
    /// 1-based number among the subtitle tracks (what `get_transcript` takes).
    pub track: u32,
    /// FFmpeg's absolute stream index.
    pub stream_index: u32,
    /// Codec name.
    pub codec: String,
    /// Language tag.
    pub language: Option<String>,
    /// Title tag.
    pub title: Option<String>,
    /// Text (true) or bitmap (false).
    pub text: bool,
}

/// Parses `ffprobe -of json -show_streams -select_streams s` output.
pub fn parse_subtitle_probe(text: &str) -> Vec<SubTrack> {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    streams
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let codec = s["codec_name"].as_str().unwrap_or("unknown").to_string();
            let tag = |k: &str| s["tags"][k].as_str().map(str::to_string);
            SubTrack {
                track: u32::try_from(i + 1).unwrap_or(u32::MAX),
                stream_index: s["index"].as_u64().unwrap_or(0) as u32,
                text: is_text_codec(&codec),
                codec,
                language: tag("language"),
                title: tag("title"),
            }
        })
        .collect()
}

async fn list_subtitles(env: &Env, video: &Path) -> Result<Vec<SubTrack>, String> {
    let sidecars = env.sidecars()?;
    let args = vec![
        OsString::from("-v"),
        "error".into(),
        "-select_streams".into(),
        "s".into(),
        "-show_entries".into(),
        "stream=index,codec_name:stream_tags=language,title".into(),
        "-of".into(),
        "json".into(),
        video.as_os_str().to_owned(),
    ];
    let out = run_capture(&sidecars.ffprobe, &args, SHORT)
        .await
        .map_err(|e| format!("ffprobe failed: {e}"))?;
    Ok(parse_subtitle_probe(&out.stdout_lossy()))
}

/// Whether `file` is a sidecar subtitle of the video named `stem`
/// (`stem.srt`, `stem.en.srt`, ...).
pub fn is_sidecar_name(stem: &str, file: &str) -> bool {
    let (stem, file) = (stem.to_lowercase(), file.to_lowercase());
    let Some(rest) = file.strip_prefix(&stem) else {
        return false;
    };
    let Some((middle, ext)) = rest.rsplit_once('.') else {
        return false;
    };
    matches!(ext, "srt" | "vtt" | "ass" | "ssa")
        && (middle.is_empty() || (middle.starts_with('.') && !middle[1..].contains(['/', '\\'])))
}

/// Sidecar subtitle files next to the video, sorted by name.
pub fn find_sidecars(video: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (video.parent(), video.file_stem()) else {
        return Vec::new();
    };
    let stem = stem.to_string_lossy().into_owned();
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| is_sidecar_name(&stem, &e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    found.sort();
    found
}

fn audio_warnings(probed: &ProbedSource) -> Vec<String> {
    let mut w = probed.warnings.clone();
    for (i, a) in probed.source.audio_tracks.iter().enumerate() {
        if !rustcut_media::audio_copyable_in_mp4(&a.codec) {
            w.push(format!(
                "Audio track {} uses {}, which MP4 players do not support: the lossless preset keeps it as is (the output container may differ), the other presets convert it to AAC.",
                i + 1,
                a.codec
            ));
        }
    }
    if probed.source.audio_tracks.len() > 1 {
        w.push(
            "The video has several audio tracks; exports use track 1 unless audio_track is given."
                .into(),
        );
    }
    if probed.source.audio_tracks.is_empty() {
        w.push(
            "The video has no audio track: find_loud_moments and transcribe cannot be used.".into(),
        );
    }
    w
}

pub(crate) async fn probe_video(ctx: &Ctx, args: Value) -> ToolResult {
    let a: PathArgs = decode("probe_video", args)?;
    let env = &ctx.env;
    let video = env.video_path(&a.path)?;
    let probed = env.probe(&video).await?;
    let s = &probed.source;
    let mut warnings = audio_warnings(&probed);
    let subs = match list_subtitles(env, &video).await {
        Ok(s) => s,
        Err(e) => {
            warnings.push(format!("Subtitle tracks could not be listed: {e}"));
            Vec::new()
        }
    };
    let sidecars: Vec<String> = find_sidecars(&video)
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    if subs.iter().any(|t| !t.text) {
        warnings.push("Some subtitle tracks are bitmap (PGS/VobSub/DVB) and cannot be read as text; use transcribe for a transcript.".into());
    }
    let existing = find_readonly(env, &video).ok().flatten();
    let md = std::fs::metadata(&video)
        .map(|m| m.len())
        .unwrap_or(s.fingerprint.size_bytes);
    Ok(ToolOutput::json(&json!({
        "path": super::fwd(&video.to_string_lossy()),
        "file_name": s.file_name(),
        "size_bytes": md,
        "duration_sec": (s.duration_sec * 1000.0).round() / 1000.0,
        "duration": fmt_ts(s.duration_sec),
        "container": s.format_name,
        "bit_rate": s.bit_rate,
        "display_resolution": format!("{}x{}", s.video.display_width, s.video.display_height),
        "fps": s.video.fps().map(|f| (f * 1000.0).round() / 1000.0),
        "variable_frame_rate": s.video.vfr,
        "hdr": s.video.hdr,
        "rotation_degrees": s.video.rotation,
        "video_codec": s.video.codec,
        "audio_tracks": s.audio_tracks.iter().enumerate().map(|(i, a)| json!({
            "track": i + 1, "codec": a.codec, "channels": a.channels,
            "title": a.title, "language": a.language, "sample_rate": a.sample_rate,
            "default": a.is_default,
        })).collect::<Vec<_>>(),
        "subtitle_tracks": subs.iter().map(|t| json!({
            "track": t.track, "codec": t.codec, "language": t.language, "title": t.title,
            "kind": if t.text { "text" } else { "bitmap" },
        })).collect::<Vec<_>>(),
        "sidecar_subtitle_files": sidecars,
        "existing_clips": existing.as_ref().map(|(p, _)| p.clips.len()),
        "warnings": warnings,
    })))
}

// ───────────────────────────── get_transcript ─────────────────────────────

/// Arguments of `get_transcript`.
#[derive(Deserialize)]
pub struct TranscriptArgs {
    path: String,
    track: Option<u32>,
    from: Option<TimeArg>,
    to: Option<TimeArg>,
    offset: Option<usize>,
    limit: Option<usize>,
}

const BITMAP_MSG: &str = "The subtitle track is a bitmap format (PGS, VobSub or DVB), which cannot be read as text. Use the transcribe tool to get a transcript from the audio instead.";

async fn extract_srt(env: &Env, input: &Path, map: Option<u32>) -> Result<String, String> {
    let mut args = vec![OsString::from("-i"), input.as_os_str().to_owned()];
    if let Some(i) = map {
        args.push("-map".into());
        args.push(format!("0:{i}").into());
    }
    args.extend(os(&["-f", "srt", "pipe:1"]));
    let out = env.ffmpeg(args, Duration::from_secs(300)).await?;
    if !out.success() {
        return Err(format!(
            "FFmpeg could not read the subtitles: {}",
            last_error_line(&out)
        ));
    }
    Ok(out.stdout_lossy().into_owned())
}

async fn load_cues(
    env: &Env,
    video: &Path,
    track: Option<u32>,
) -> Result<(Vec<Cue>, String), String> {
    let subs = if env.sidecars.is_ok() {
        list_subtitles(env, video).await.unwrap_or_default()
    } else {
        Vec::new()
    };
    if let Some(n) = track {
        let t = subs.iter().find(|t| t.track == n).ok_or_else(|| {
            format!(
                "The video has no subtitle track {n} ({} embedded track(s)). Run probe_video to see them.",
                subs.len()
            )
        })?;
        if !t.text {
            return Err(BITMAP_MSG.into());
        }
        let srt = extract_srt(env, video, Some(t.stream_index)).await?;
        return Ok((
            clean_cues(parse_srt(&srt, 0.0)),
            format!(
                "embedded subtitle track {n} ({}{})",
                t.codec,
                t.language
                    .as_ref()
                    .map_or(String::new(), |l| format!(", {l}"))
            ),
        ));
    }
    if let Some(t) = subs.iter().find(|t| t.text) {
        let srt = extract_srt(env, video, Some(t.stream_index)).await?;
        return Ok((
            clean_cues(parse_srt(&srt, 0.0)),
            format!(
                "embedded subtitle track {} ({}{})",
                t.track,
                t.codec,
                t.language
                    .as_ref()
                    .map_or(String::new(), |l| format!(", {l}"))
            ),
        ));
    }
    if let Some(side) = find_sidecars(video).into_iter().next() {
        let ext = side
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let name = side
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let srt = if ext == "srt" {
            String::from_utf8_lossy(
                &std::fs::read(&side).map_err(|e| format!("Could not read {name}: {e}"))?,
            )
            .into_owned()
        } else {
            extract_srt(env, &side, None).await?
        };
        return Ok((
            clean_cues(parse_srt(&srt, 0.0)),
            format!("sidecar file {name}"),
        ));
    }
    if subs.iter().any(|t| !t.text) {
        return Err(BITMAP_MSG.into());
    }
    Err("The video has no subtitle track and no sidecar subtitle file (.srt/.vtt/.ass next to it). Use the transcribe tool to create a transcript from the audio.".into())
}

pub(crate) async fn get_transcript(ctx: &Ctx, args: Value) -> ToolResult {
    let a: TranscriptArgs = decode("get_transcript", args)?;
    let video = ctx.env.video_path(&a.path)?;
    let (from, to) = (opt_time(&a.from)?, opt_time(&a.to)?);
    let (cues, source) = load_cues(&ctx.env, &video, a.track).await?;
    let lines = format_lines(&cues, from, to);
    if lines.is_empty() {
        return Ok(ToolOutput::text(format!(
            "Transcript source: {source}\ntotal_lines: 0 (no subtitle lines in the requested range)"
        )));
    }
    let p = page(
        &lines,
        a.offset.unwrap_or(0),
        a.limit.unwrap_or(400).min(2000),
    );
    Ok(ToolOutput::text(render_page(&source, &p)))
}

// ───────────────────────────── transcribe ─────────────────────────────

/// Arguments of `transcribe`.
#[derive(Deserialize)]
pub struct TranscribeArgs {
    path: String,
    language: Option<String>,
    from: Option<TimeArg>,
    to: Option<TimeArg>,
    audio_track: Option<u32>,
    offset: Option<usize>,
    limit: Option<usize>,
}

const WHISPER_HELP: &str = "No whisper model is configured, so speech cannot be transcribed. Download a ggml model file (for example ggml-base.bin from the whisper.cpp project's model downloads at huggingface.co/ggerganov/whisper.cpp), then set the environment variable RUSTCUT_WHISPER_MODEL to its absolute path in the MCP server configuration and restart the server. RustCut never downloads models itself. Alternatives: get_transcript for existing subtitles, find_loud_moments for audio highlights.";

/// The whisper filter graph for `model`, `language` and the destination file.
pub fn whisper_filter(model: &Path, language: &str, destination: &Path) -> String {
    format!(
        "whisper=model={}:language={}:destination={}:format=srt",
        filter_path(model),
        language,
        filter_path(destination)
    )
}

fn valid_language(l: &str) -> bool {
    l == "auto"
        || ((2..=8).contains(&l.len()) && l.chars().all(|c| c.is_ascii_alphabetic() || c == '-'))
}

fn audio_map(probed: &ProbedSource, track: Option<u32>) -> Result<(u32, String), String> {
    let n = track.unwrap_or(1).max(1);
    let have = probed.source.audio_tracks.len();
    if have == 0 {
        return Err("This video has no audio track.".into());
    }
    if n as usize > have {
        return Err(format!(
            "The video has {have} audio track(s); audio_track {n} does not exist."
        ));
    }
    Ok((n, format!("0:a:{}", n - 1)))
}

fn check_range(from: Option<f64>, to: Option<f64>, duration: f64) -> Result<(), String> {
    if let Some(f) = from
        && f >= duration
    {
        return Err(format!(
            "from ({}) is at or after the end of the video ({}).",
            fmt_ts(f),
            fmt_ts(duration)
        ));
    }
    if let (Some(f), Some(t)) = (from, to)
        && t <= f
    {
        return Err("to must be later than from.".into());
    }
    Ok(())
}

/// Checks that `model` looks like a ggml/gguf whisper model before FFmpeg is asked to load it.
pub fn check_whisper_model(model: &Path) -> Result<(), String> {
    use std::io::Read;
    let bad = |why: &str| {
        format!(
            "The whisper model file \"{}\" (RUSTCUT_WHISPER_MODEL) is not a valid ggml whisper model ({why}). Point RUSTCUT_WHISPER_MODEL at a file such as ggml-base.bin from the whisper.cpp project's model downloads and restart the server.",
            model.display()
        )
    };
    let mut head = [0u8; 4];
    let mut f = std::fs::File::open(model).map_err(|e| bad(&format!("cannot be read: {e}")))?;
    f.read_exact(&mut head)
        .map_err(|_| bad("the file is too short"))?;
    // ggml files store the magic 0x67676d6c ("ggml") as a little-endian u32; also ggjt/gguf.
    if matches!(&head, b"lmgg" | b"fmgg" | b"tjgg" | b"GGUF") {
        Ok(())
    } else {
        Err(bad("it does not start with the ggml magic bytes"))
    }
}

pub(crate) async fn transcribe(ctx: &Ctx, args: Value) -> ToolResult {
    let a: TranscribeArgs = decode("transcribe", args)?;
    let env = &ctx.env;
    let video = env.video_path(&a.path)?;
    let model = env.whisper_model.clone().ok_or(WHISPER_HELP)?;
    if !model.is_file() {
        return Err(format!(
            "The whisper model file \"{}\" (RUSTCUT_WHISPER_MODEL) does not exist. Check the path or download a ggml model such as ggml-base.bin from the whisper.cpp project's model downloads; RustCut never downloads models itself.",
            model.display()
        ));
    }
    check_whisper_model(&model)?;
    let language = a.language.clone().unwrap_or_else(|| "auto".into());
    if !valid_language(&language) {
        return Err(format!(
            "\"{language}\" is not a language code. Use \"auto\" or a code like \"en\"."
        ));
    }
    let caps = env.caps().await;
    if !caps.as_ref().is_some_and(|c| c.has_filter("whisper")) {
        return Err("This FFmpeg build has no \"whisper\" filter (it needs FFmpeg 8 or newer, built with whisper.cpp). Install a recent full FFmpeg build or point RUSTCUT_FFMPEG_DIR at one, then restart the server.".into());
    }
    let probed = env.probe(&video).await?;
    let duration = probed.source.duration_sec;
    let (from, to) = (opt_time(&a.from)?, opt_time(&a.to)?);
    check_range(from, to, duration)?;
    let (track, map) = audio_map(&probed, a.audio_track)?;
    let start = from.unwrap_or(0.0);
    let end = to.map_or(duration, |t| t.min(duration));
    let key = cache_key(&format!(
        "{}|{}|{:.3}|{:.3}|{}|{}|{}",
        probed.source.fingerprint.key(),
        track,
        start,
        end,
        language,
        model
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::fs::metadata(&model).map(|m| m.len()).unwrap_or(0)
    ));
    let dir = env.paths.cache.join("mcp-transcripts");
    let cached = dir.join(format!("{key}.srt"));
    if !cached.is_file() {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create the cache folder: {e}"))?;
        let part = dir.join(format!("{key}.srt.part"));
        let _ = std::fs::remove_file(&part);
        let mut ff: Vec<OsString> = Vec::new();
        if start > 0.0 {
            ff.extend(os(&["-ss", &format!("{start:.3}")]));
        }
        if to.is_some() {
            ff.extend(os(&["-t", &format!("{:.3}", end - start)]));
        }
        ff.push("-i".into());
        ff.push(video.as_os_str().to_owned());
        ff.extend(os(&[
            "-map",
            &map,
            "-vn",
            "-af",
            &whisper_filter(&model, &language, &part),
            "-f",
            "null",
            "-",
        ]));
        let out = env.ffmpeg(ff, Duration::from_secs(4 * 3600)).await?;
        if !out.success() || !part.is_file() {
            let _ = std::fs::remove_file(&part);
            let line = last_error_line(&out);
            let hint = if line.contains("Error opening output files") || line.contains("I/O error")
            {
                " (FFmpeg could not start the whisper filter: the model file may be damaged or not a ggml whisper model, or the cache folder is not writable)"
            } else {
                ""
            };
            return Err(format!("Transcription failed: {line}{hint}"));
        }
        std::fs::rename(&part, &cached)
            .map_err(|e| format!("Could not store the transcript: {e}"))?;
    }
    let srt = std::fs::read_to_string(&cached)
        .map_err(|e| format!("Could not read the transcript: {e}"))?;
    let cues = parse_srt(&srt, start);
    let lines = format_lines(&cues, None, None);
    if lines.is_empty() {
        return Ok(ToolOutput::text(
            "Transcript source: whisper\ntotal_lines: 0 (no speech was recognized in this range)",
        ));
    }
    let p = page(
        &lines,
        a.offset.unwrap_or(0),
        a.limit.unwrap_or(400).min(2000),
    );
    Ok(ToolOutput::text(render_page(
        &format!("whisper speech recognition (language {language}, audio track {track})"),
        &p,
    )))
}

fn cache_key(text: &str) -> String {
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(text.as_bytes()))
}

// ───────────────────────────── find_loud_moments ─────────────────────────────

/// Arguments of `find_loud_moments`.
#[derive(Deserialize)]
pub struct LoudArgs {
    path: String,
    top_n: Option<usize>,
    min_gap_sec: Option<f64>,
    window_sec: Option<f64>,
    audio_track: Option<u32>,
}

pub(crate) async fn find_loud_moments(ctx: &Ctx, args: Value) -> ToolResult {
    let a: LoudArgs = decode("find_loud_moments", args)?;
    let env = &ctx.env;
    let video = env.video_path(&a.path)?;
    let top_n = a.top_n.unwrap_or(10);
    let (gap, window) = (a.min_gap_sec.unwrap_or(20.0), a.window_sec.unwrap_or(2.0));
    if !(1..=50).contains(&top_n) {
        return Err("top_n must be between 1 and 50.".into());
    }
    if !gap.is_finite() || gap < 1.0 {
        return Err("min_gap_sec must be at least 1.".into());
    }
    if !window.is_finite() || !(0.5..=10.0).contains(&window) {
        return Err("window_sec must be between 0.5 and 10.".into());
    }
    let probed = env.probe(&video).await?;
    let (track, map) = audio_map(&probed, a.audio_track)?;
    let duration = probed.source.duration_sec;
    let mut ff: Vec<OsString> = vec!["-i".into(), video.as_os_str().to_owned()];
    ff.extend(os(&[
        "-map",
        &map,
        "-vn",
        "-af",
        "ebur128=metadata=1,ametadata=mode=print:key=lavfi.r128.M:file=-",
        "-f",
        "null",
        "-",
    ]));
    let out = env.ffmpeg(ff, Duration::from_secs(3600)).await?;
    if !out.success() {
        return Err(format!(
            "Loudness analysis failed: {}",
            last_error_line(&out)
        ));
    }
    let samples = parse_ametadata(&out.stdout_lossy());
    if samples.is_empty() {
        return Err(
            "The loudness analysis produced no data (is the audio track silent or empty?).".into(),
        );
    }
    let moments = rank_moments(
        &samples,
        RankOpts {
            top_n,
            min_gap_sec: gap,
            window_sec: window,
            duration_sec: duration,
            lead_sec: 10.0,
            tail_sec: 5.0,
        },
    );
    let median = moments.first().map(|m| m.loudness_lufs - m.above_median_db);
    let r1 = |v: f64| (v * 10.0).round() / 10.0;
    Ok(ToolOutput::json(&json!({
        "audio_track": track,
        "duration_sec": duration,
        "median_loudness_lufs": median.map(r1),
        "moments": moments.iter().enumerate().map(|(i, m)| json!({
            "rank": i + 1,
            "time_sec": r1(m.time),
            "time": fmt_hms(m.time),
            "loudness_lufs": r1(m.loudness_lufs),
            "above_median_db": r1(m.above_median_db),
            "suggested_start_sec": r1(m.clip_start),
            "suggested_end_sec": r1(m.clip_end),
            "suggested_range": format!("{}-{}", fmt_hms(m.clip_start), fmt_hms(m.clip_end)),
        })).collect::<Vec<_>>(),
        "note": "Loud does not always mean interesting: check the picture with contact_sheet or the words with get_transcript before adding clips.",
    })))
}

// ───────────────────────────── frames ─────────────────────────────

/// Arguments of `get_frames`.
#[derive(Deserialize)]
pub struct FramesArgs {
    path: String,
    times: Vec<TimeArg>,
    width: Option<u32>,
}

async fn frame_jpeg(env: &Env, video: &Path, t: f64, width: u32) -> Result<Vec<u8>, String> {
    let mut ff = os(&["-ss", &format!("{t:.3}")]);
    ff.push("-i".into());
    ff.push(video.as_os_str().to_owned());
    ff.extend(os(&[
        "-frames:v",
        "1",
        "-an",
        "-sn",
        "-vf",
        &format!("scale={width}:-2"),
        "-pix_fmt",
        "yuvj420p",
        "-c:v",
        "mjpeg",
        "-q:v",
        "5",
        "-f",
        "image2pipe",
        "pipe:1",
    ]));
    let out = env.ffmpeg(ff, SHORT).await?;
    if !out.success() || out.stdout.is_empty() {
        return Err(format!(
            "No frame could be read at {}: {}",
            fmt_ts(t),
            last_error_line(&out)
        ));
    }
    Ok(out.stdout)
}

fn image(bytes: &[u8]) -> Content {
    Content::Image {
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        mime: "image/jpeg",
    }
}

pub(crate) async fn get_frames(ctx: &Ctx, args: Value) -> ToolResult {
    let a: FramesArgs = decode("get_frames", args)?;
    let env = &ctx.env;
    if a.times.is_empty() || a.times.len() > 12 {
        return Err("times must contain between 1 and 12 entries.".into());
    }
    let width = a.width.unwrap_or(384).clamp(64, 1920);
    let video = env.video_path(&a.path)?;
    let duration = env.probe(&video).await?.source.duration_sec;
    let mut times = Vec::new();
    for t in &a.times {
        let t = parse_time(t)?;
        if t > duration + 0.001 {
            return Err(format!(
                "Time {} is after the end of the video ({}).",
                fmt_ts(t),
                fmt_ts(duration)
            ));
        }
        times.push(t.min((duration - 0.05).max(0.0)));
    }
    let mut set = tokio::task::JoinSet::new();
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    for (i, t) in times.iter().copied().enumerate() {
        let (env, video, sem) = (env.clone(), video.clone(), sem.clone());
        set.spawn(async move {
            let _permit = sem.acquire_owned().await;
            (i, frame_jpeg(&env, &video, t, width).await)
        });
    }
    let mut frames: Vec<(usize, Result<Vec<u8>, String>)> = Vec::new();
    while let Some(r) = set.join_next().await {
        frames.push(r.map_err(|e| format!("Frame extraction failed: {e}"))?);
    }
    frames.sort_by_key(|(i, _)| *i);
    let mut content = Vec::new();
    for (i, r) in frames {
        let bytes = r?;
        content.push(Content::Text(format!("Frame at {}", fmt_ts(times[i]))));
        content.push(image(&bytes));
    }
    Ok(ToolOutput {
        content,
        is_error: false,
    })
}

/// Arguments of `contact_sheet`.
#[derive(Deserialize)]
pub struct SheetArgs {
    path: String,
    start: Option<TimeArg>,
    end: Option<TimeArg>,
    count: Option<u32>,
    width: Option<u32>,
}

const FONTS: &[&str] = &[
    "C:/Windows/Fonts/arial.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/Library/Fonts/Arial.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
];

/// Evenly spaced frame times: the centres of `count` equal segments of `[start, end]`.
pub fn sheet_times(start: f64, end: f64, count: u32) -> Vec<f64> {
    let step = (end - start) / f64::from(count);
    (0..count)
        .map(|i| start + (f64::from(i) + 0.5) * step)
        .collect()
}

/// Grid size for `count` tiles: `(columns, rows)`.
pub fn grid(count: u32) -> (u32, u32) {
    let cols = (f64::from(count)).sqrt().ceil() as u32;
    (cols, count.div_ceil(cols))
}

/// The `xstack` layout string for `count` equally sized tiles.
pub fn xstack_layout(count: u32, cols: u32) -> String {
    (0..count)
        .map(|i| {
            let (c, r) = (i % cols, i / cols);
            let x = if c == 0 {
                "0".to_string()
            } else {
                vec!["w0"; c as usize].join("+")
            };
            let y = if r == 0 {
                "0".to_string()
            } else {
                vec!["h0"; r as usize].join("+")
            };
            format!("{x}_{y}")
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn sheet_filter(times: &[f64], tw: u32, th: u32, cols: u32, font: Option<&Path>) -> String {
    let mut parts = Vec::new();
    let fs = (th / 9).max(12);
    for (i, t) in times.iter().enumerate() {
        let mut f = format!(
            "[{i}:v]scale={tw}:{th}:force_original_aspect_ratio=decrease,pad={tw}:{th}:(ow-iw)/2:(oh-ih)/2:black,setsar=1"
        );
        if let Some(font) = font {
            let label = fmt_hms(*t).replace(':', "\\:");
            f.push_str(&format!(
                ",drawtext=fontfile={}:text='{label}':x=6:y=6:fontsize={fs}:fontcolor=white:box=1:boxcolor=black@0.6:boxborderw=4",
                filter_path(font)
            ));
        }
        f.push_str(&format!("[v{i}]"));
        parts.push(f);
    }
    let n = times.len();
    if n == 1 {
        parts.push("[v0]null[out]".into());
    } else {
        let inputs: String = (0..n).map(|i| format!("[v{i}]")).collect();
        parts.push(format!(
            "{inputs}xstack=inputs={n}:layout={}:fill=black[out]",
            xstack_layout(n as u32, cols)
        ));
    }
    parts.join(";")
}

pub(crate) async fn contact_sheet(ctx: &Ctx, args: Value) -> ToolResult {
    let a: SheetArgs = decode("contact_sheet", args)?;
    let env = &ctx.env;
    let video = env.video_path(&a.path)?;
    let count = a.count.unwrap_or(9);
    if !(1..=25).contains(&count) {
        return Err("count must be between 1 and 25.".into());
    }
    let width = a.width.unwrap_or(960).clamp(160, 1920);
    let probed = env.probe(&video).await?;
    let duration = probed.source.duration_sec;
    let start = opt_time(&a.start)?.unwrap_or(0.0);
    let end = opt_time(&a.end)?.map_or(duration, |e| e.min(duration));
    if start >= end {
        return Err(format!(
            "start ({}) must be before end ({}).",
            fmt_ts(start),
            fmt_ts(end)
        ));
    }
    let times = sheet_times(start, end, count);
    let (cols, _rows) = grid(count);
    let tw = ((width / cols) / 2 * 2).max(64);
    let v = &probed.source.video;
    let aspect = f64::from(v.display_height.max(1)) / f64::from(v.display_width.max(1));
    let th = (((f64::from(tw) * aspect) / 2.0).round() as u32 * 2).max(2);
    let font = env
        .caps()
        .await
        .filter(|c| c.has_filter("drawtext"))
        .and_then(|_| {
            FONTS
                .iter()
                .map(Path::new)
                .find(|p| p.is_file())
                .map(Path::to_path_buf)
        });
    let run = |font: Option<&Path>| {
        let mut ff: Vec<OsString> = Vec::new();
        for t in &times {
            ff.extend(os(&["-ss", &format!("{t:.3}")]));
            ff.push("-i".into());
            ff.push(video.as_os_str().to_owned());
        }
        ff.extend(os(&[
            "-filter_complex",
            &sheet_filter(&times, tw, th, cols, font),
            "-map",
            "[out]",
            "-frames:v",
            "1",
            "-pix_fmt",
            "yuvj420p",
            "-c:v",
            "mjpeg",
            "-q:v",
            "4",
            "-f",
            "image2pipe",
            "pipe:1",
        ]));
        env.ffmpeg(ff, Duration::from_secs(180))
    };
    let mut out = run(font.as_deref()).await?;
    if (!out.success() || out.stdout.is_empty()) && font.is_some() {
        out = run(None).await?;
    }
    if !out.success() || out.stdout.is_empty() {
        return Err(format!(
            "The contact sheet could not be made: {}",
            last_error_line(&out)
        ));
    }
    let listing: Vec<String> = times
        .iter()
        .enumerate()
        .map(|(i, t)| {
            format!(
                "tile {} (row {}, column {}): {}",
                i + 1,
                i / cols as usize + 1,
                i % cols as usize + 1,
                fmt_ts(*t)
            )
        })
        .collect();
    let note = if font.is_some() {
        "Times are drawn in the top-left corner of each tile."
    } else {
        "No font was available, so times are not drawn in; use the list."
    };
    Ok(ToolOutput {
        content: vec![
            Content::Text(format!(
                "Contact sheet, {cols} columns, left to right then top to bottom. {note}\n{}",
                listing.join("\n")
            )),
            image(&out.stdout),
        ],
        is_error: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffprobe_subtitle_json() {
        let json = r#"{"streams":[
          {"index":2,"codec_name":"subrip","tags":{"language":"eng","title":"English"}},
          {"index":3,"codec_name":"hdmv_pgs_subtitle","tags":{"language":"fra"}}]}"#;
        let t = parse_subtitle_probe(json);
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].track, t[0].stream_index, t[0].text), (1, 2, true));
        assert_eq!(t[0].language.as_deref(), Some("eng"));
        assert!(!t[1].text);
        assert!(parse_subtitle_probe("garbage").is_empty());
    }

    #[test]
    fn sidecar_names() {
        assert!(is_sidecar_name("movie", "movie.srt"));
        assert!(is_sidecar_name("Movie", "movie.EN.srt"));
        assert!(is_sidecar_name("movie", "movie.pt-BR.vtt"));
        assert!(!is_sidecar_name("movie", "movie2.srt"));
        assert!(!is_sidecar_name("movie", "movie.mp4"));
        assert!(!is_sidecar_name("movie", "other.srt"));
    }

    #[test]
    fn sheet_geometry() {
        assert_eq!(grid(9), (3, 3));
        assert_eq!(grid(25), (5, 5));
        assert_eq!(grid(10), (4, 3));
        assert_eq!(grid(1), (1, 1));
        let t = sheet_times(0.0, 90.0, 9);
        assert_eq!(t.len(), 9);
        assert!((t[0] - 5.0).abs() < 1e-9 && (t[8] - 85.0).abs() < 1e-9);
        assert_eq!(xstack_layout(4, 2), "0_0|w0_0|0_h0|w0_h0");
        assert_eq!(xstack_layout(3, 3), "0_0|w0_0|w0+w0_0");
    }

    #[test]
    fn sheet_filter_mentions_every_input() {
        let f = sheet_filter(
            &[1.0, 2.0, 3.0, 4.0],
            240,
            136,
            2,
            Some(Path::new("C:/Windows/Fonts/arial.ttf")),
        );
        assert!(f.contains("[3:v]") && f.contains("xstack=inputs=4") && f.contains("[out]"));
        assert!(f.contains("fontfile='C\\:/Windows/Fonts/arial.ttf'"));
        assert!(f.contains("text='00\\:00\\:01'"));
    }

    #[test]
    fn whisper_filter_escapes_paths() {
        let f = whisper_filter(
            Path::new("C:/m/ggml-base.bin"),
            "en",
            Path::new("C:/c/x.srt.part"),
        );
        assert_eq!(
            f,
            "whisper=model='C\\:/m/ggml-base.bin':language=en:destination='C\\:/c/x.srt.part':format=srt"
        );
        assert!(valid_language("auto") && valid_language("pt-BR") && !valid_language("e;n"));
    }

    #[test]
    fn whisper_model_magic_is_checked() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("m.bin");
        std::fs::write(&bad, "not a model at all").unwrap();
        assert!(
            check_whisper_model(&bad)
                .unwrap_err()
                .contains("not a valid ggml")
        );
        let good = dir.path().join("g.bin");
        std::fs::write(&good, [0x6c, 0x6d, 0x67, 0x67, 1, 2, 3]).unwrap();
        assert!(check_whisper_model(&good).is_ok());
        std::fs::write(&good, [1u8]).unwrap();
        assert!(check_whisper_model(&good).is_err());
    }

    #[test]
    fn range_checks() {
        assert!(check_range(Some(5.0), Some(3.0), 10.0).is_err());
        assert!(check_range(Some(11.0), None, 10.0).is_err());
        assert!(check_range(Some(1.0), Some(3.0), 10.0).is_ok());
    }
}
