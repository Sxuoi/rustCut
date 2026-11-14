//! End-to-end tests over stdio: the real binary, a temp data folder, generated media.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

struct Client {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    next_id: u64,
    notifications: Vec<Value>,
}

impl Client {
    fn spawn(data: &Path, roots: Option<&Path>) -> Client {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustcut-mcp"));
        cmd.env("RUSTCUT_DATA_DIR", data)
            .env_remove("RUSTCUT_WHISPER_MODEL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(r) = roots {
            cmd.env("RUSTCUT_MCP_ROOTS", r);
        } else {
            cmd.env_remove("RUSTCUT_MCP_ROOTS");
        }
        if let Some(ff) = rustcut_testkit::try_ffmpeg() {
            cmd.env("RUSTCUT_FFMPEG_DIR", ff.parent().unwrap());
        }
        let mut child = cmd.spawn().expect("spawn rustcut-mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Client {
            child,
            stdin,
            rx,
            next_id: 1,
            notifications: Vec::new(),
        }
    }

    fn send(&mut self, v: &Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .rx
                .recv_timeout(Duration::from_secs(300))
                .expect("server answered in time");
            let v: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout carried a non-JSON line ({e}): {line}"));
            if v["id"] == json!(id) {
                return v;
            }
            self.notifications.push(v);
        }
    }

    fn initialize(&mut self) -> Value {
        let r = self.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "test", "version": "0"}}),
        );
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        r
    }

    /// Calls a tool; returns `(is_error, content blocks)`.
    fn call(&mut self, name: &str, args: Value) -> (bool, Vec<Value>) {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}));
        assert!(r.get("error").is_none(), "protocol error for {name}: {r}");
        let res = &r["result"];
        (
            res["isError"].as_bool().unwrap(),
            res["content"].as_array().unwrap().clone(),
        )
    }

    fn call_json(&mut self, name: &str, args: Value) -> Value {
        let (err, content) = self.call(name, args);
        let text = content[0]["text"].as_str().unwrap().to_string();
        assert!(!err, "{name} failed: {text}");
        serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{name} text is not JSON ({e}): {text}"))
    }

    fn call_text(&mut self, name: &str, args: Value) -> (bool, String) {
        let (err, content) = self.call(name, args);
        (err, content[0]["text"].as_str().unwrap_or("").to_string())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ff(args: &[&str]) {
    let mut v = vec!["-y", "-hide_banner", "-v", "error"];
    v.extend_from_slice(args);
    rustcut_testkit::ffmpeg_ok(v);
}

fn s(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

struct Media {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    videos: PathBuf,
}

/// Generated inputs: `clip.mp4` (10 s, quiet), `talk.mkv` (with an srt track), `loud.mp4`
/// (quiet with a loud burst at 6-8 s).
fn media() -> Option<Media> {
    rustcut_testkit::try_ffmpeg()?;
    let tmp = tempfile::tempdir().unwrap();
    let videos = tmp.path().join("videos");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&videos).unwrap();
    let clip = videos.join("clip.mp4");
    ff(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=640x360:rate=25:duration=10",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=300:duration=10",
        "-c:v",
        "libx264",
        "-g",
        "25",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
        &s(&clip),
    ]);
    let srt = videos.join("subs.srt");
    std::fs::write(
        &srt,
        "1\n00:00:01,000 --> 00:00:03,000\nHello there\n\n2\n00:00:05,000 --> 00:00:07,500\n{\\an8}Second\\Nline\n\n",
    )
    .unwrap();
    ff(&[
        "-i",
        &s(&clip),
        "-i",
        &s(&srt),
        "-map",
        "0",
        "-map",
        "1",
        "-c",
        "copy",
        "-c:s",
        "srt",
        &s(&videos.join("talk.mkv")),
    ]);
    std::fs::remove_file(&srt).unwrap();
    ff(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x180:rate=25:duration=12",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=300:duration=12",
        "-af",
        "volume='if(between(t,6,8),1.0,0.02)':eval=frame",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
        &s(&videos.join("loud.mp4")),
    ]);
    Some(Media {
        _tmp: tmp,
        data,
        videos,
    })
}

fn skip() {
    eprintln!("SKIPPED: FFmpeg not found (set RUSTCUT_FFMPEG_DIR or add it to PATH)");
}

#[test]
fn handshake_lists_every_tool_with_valid_schemas() {
    let tmp = tempfile::tempdir().unwrap();
    let mut c = Client::spawn(&tmp.path().join("data"), None);
    let init = c.initialize();
    assert_eq!(init["result"]["serverInfo"]["name"], "rustcut-mcp");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("probe_video")
    );
    let list = c.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 14);
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object", "{t}");
        assert!(t["annotations"]["readOnlyHint"].is_boolean(), "{t}");
        assert!(t["description"].as_str().unwrap().len() > 20);
    }
    assert!(c.request("ping", json!({}))["result"].is_object());
    let unknown = c.request("nope/nope", json!({}));
    assert_eq!(unknown["error"]["code"], -32601);
    // A bad call is a tool error, not a protocol error.
    let (err, text) = c.call_text("probe_video", json!({}));
    assert!(err && text.contains("Invalid arguments"), "{text}");
    let (err, text) = c.call_text("no_such_tool", json!({}));
    assert!(err && text.contains("Unknown tool"), "{text}");
}

#[test]
fn paths_are_validated_and_confined_to_the_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("videos");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(tmp.path().join("a.mp4"), b"x").unwrap();
    std::fs::write(root.join("notes.srt"), b"x").unwrap();
    let mut c = Client::spawn(&tmp.path().join("data"), Some(&root));
    c.initialize();
    let outside = s(&tmp.path().join("a.mp4"));
    let (err, text) = c.call_text("probe_video", json!({"path": outside}));
    assert!(err && text.contains("outside the folders"), "{text}");
    let (err, text) = c.call_text(
        "add_clips",
        json!({"path": outside, "timestamps": "0:01-0:03"}),
    );
    assert!(err && text.contains("outside"), "{text}");
    let (err, text) = c.call_text("probe_video", json!({"path": "relative/a.mp4"}));
    assert!(err && text.contains("absolute"), "{text}");
    let (err, text) = c.call_text("probe_video", json!({"path": s(&root.join("notes.srt"))}));
    assert!(err && text.contains("subtitle file"), "{text}");
    let (err, text) = c.call_text("probe_video", json!({"path": s(&root.join("missing.mp4"))}));
    assert!(err && text.contains("not found"), "{text}");
    let (err, text) = c.call_text(
        "export_clips",
        json!({"path": s(&root.join("missing.mp4")), "output_dir": s(&tmp.path().join("elsewhere"))}),
    );
    assert!(err, "{text}");
}

#[test]
fn clip_and_export_flow_matches_the_desktop_project() {
    let Some(m) = media() else { return skip() };
    let mut c = Client::spawn(&m.data, Some(&m.videos));
    c.initialize();
    let clip = s(&m.videos.join("clip.mp4"));

    let probe = c.call_json("probe_video", json!({"path": clip}));
    assert!(
        (probe["duration_sec"].as_f64().unwrap() - 10.0).abs() < 0.3,
        "{probe}"
    );
    assert_eq!(probe["display_resolution"], "640x360");
    assert_eq!(probe["audio_tracks"].as_array().unwrap().len(), 1);
    assert_eq!(probe["video_codec"], "h264");

    // Nothing yet: an empty list, and no project file is created.
    let empty = c.call_json("list_clips", json!({"path": clip}));
    assert_eq!(empty["clips"].as_array().unwrap().len(), 0);
    assert!(!m.videos.join("clip.rustcut").exists());

    let added = c.call_json(
        "add_clips",
        json!({"path": clip, "timestamps": "00:01-00:03 First\n00:05-00:08 Second\n00:04-00:04 Zero"}),
    );
    assert_eq!(added["created"].as_array().unwrap().len(), 2, "{added}");
    assert_eq!(added["errors"].as_array().unwrap().len(), 1, "{added}");
    let added2 = c.call_json(
        "add_clips",
        json!({"path": clip, "clips": [{"start": 8.5, "end": "00:09.5", "name": "Third"},
                                       {"start": 5, "end": 4}, {"start": 50, "end": 60}]}),
    );
    assert_eq!(added2["created"].as_array().unwrap().len(), 1, "{added2}");
    assert_eq!(added2["errors"].as_array().unwrap().len(), 2, "{added2}");
    assert_eq!(added2["created"][0]["index"], 3);
    assert!(m.videos.join("clip.rustcut").is_file());
    let listed = c.call_json("list_clips", json!({"path": clip}));
    assert_eq!(listed["clips"].as_array().unwrap().len(), 3);

    let upd = c.call_json(
        "update_clip",
        json!({"path": clip, "index": 3, "name": "Third!", "included": false}),
    );
    assert_eq!(upd["clip"]["included"], false);
    let (err, text) = c.call_text(
        "update_clip",
        json!({"path": clip, "index": 9, "name": "x"}),
    );
    assert!(err && text.contains("no clip number 9"), "{text}");
    let del = c.call_json("delete_clips", json!({"path": clip, "indices": [3]}));
    assert_eq!(del["remaining_clips"], 2);

    let preview = c.call_json(
        "preview_export",
        json!({"path": clip, "preset": "lossless"}),
    );
    assert_eq!(preview["can_start"], true, "{preview}");
    assert_eq!(preview["will_write"].as_array().unwrap().len(), 2);
    assert!(
        !m.videos.join("clip_clips").exists(),
        "preview must not write"
    );

    let done = c.call_json("export_clips", json!({"path": clip, "preset": "lossless"}));
    assert_eq!(done["status"], "finished", "{done}");
    assert_eq!(done["done"], 2, "{done}");
    let mut first_outputs = Vec::new();
    for r in done["clips"].as_array().unwrap() {
        assert_eq!(r["status"], "done", "{r}");
        let p = PathBuf::from(r["output_path"].as_str().unwrap());
        assert!(p.is_file(), "{p:?}");
        assert!(std::fs::metadata(&p).unwrap().len() > 1000);
        first_outputs.push(p);
    }
    assert!(first_outputs[0].starts_with(m.videos.join("clip_clips")));

    // The project file (what the desktop app reads) shows both clips completed.
    let project: Value =
        serde_json::from_slice(&std::fs::read(m.videos.join("clip.rustcut")).unwrap()).unwrap();
    let statuses: Vec<&str> = project["clips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["export_status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["completed", "completed"], "{project}");
    assert!(
        !m.videos.join("clip.rustcut.lock").exists(),
        "lock released"
    );

    // A second export never overwrites: new names are picked.
    let again = c.call_json(
        "export_clips",
        json!({"path": clip, "preset": "lossless", "indices": [1]}),
    );
    let second = PathBuf::from(again["clips"][0]["output_path"].as_str().unwrap());
    assert!(
        second.is_file() && !first_outputs.contains(&second),
        "{second:?}"
    );
    assert!(first_outputs.iter().all(|p| p.is_file()));

    // Lock conflict: another process (here the test) holds the project.
    let lock =
        match rustcut_project::ProjectLock::try_acquire(&m.videos.join("clip.rustcut")).unwrap() {
            rustcut_project::LockOutcome::Acquired(l) => l,
            other => panic!("expected to acquire the lock: {other:?}"),
        };
    let (err, text) = c.call_text(
        "add_clips",
        json!({"path": clip, "timestamps": "00:09-00:09.9"}),
    );
    assert!(
        err && text.contains("open in RustCut or another agent"),
        "{text}"
    );
    let (err, text) = c.call_text("export_clips", json!({"path": clip}));
    assert!(err && text.contains("open in RustCut"), "{text}");
    drop(lock);
    let ok = c.call_json(
        "add_clips",
        json!({"path": clip, "timestamps": "00:09-00:09.9"}),
    );
    assert_eq!(ok["created"].as_array().unwrap().len(), 1);
}

#[test]
fn vertical_export_and_background_job_polling() {
    let Some(m) = media() else { return skip() };
    let mut c = Client::spawn(&m.data, Some(&m.videos));
    c.initialize();
    let clip = s(&m.videos.join("clip.mp4"));
    c.call_json(
        "add_clips",
        json!({"path": clip, "timestamps": "00:01-00:03 Hook"}),
    );
    let started = c.call_json(
        "export_clips",
        json!({"path": clip, "preset": "vertical_blur", "resolution": "720x1280", "wait_sec": 0}),
    );
    let job = started["job_id"].as_str().unwrap().to_string();
    assert_eq!(started["status"], "running", "{started}");
    let mut status = Value::Null;
    for _ in 0..120 {
        status = c.call_json("export_status", json!({"job_id": job}));
        if status["status"] == "finished" {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    assert_eq!(status["status"], "finished", "{status}");
    assert_eq!(status["done"], 1, "{status}");
    let out = PathBuf::from(status["clips"][0]["output_path"].as_str().unwrap());
    let (w, h) = rustcut_testkit::video_size(&out);
    assert_eq!((w, h), (720, 1280));
    let (err, text) = c.call_text("export_status", json!({"job_id": "nope"}));
    assert!(err && text.contains("Unknown job_id"), "{text}");
}

#[test]
fn transcripts_loudness_and_images() {
    let Some(m) = media() else { return skip() };
    let mut c = Client::spawn(&m.data, Some(&m.videos));
    c.initialize();

    let talk = s(&m.videos.join("talk.mkv"));
    let probe = c.call_json("probe_video", json!({"path": talk}));
    assert_eq!(probe["subtitle_tracks"][0]["kind"], "text", "{probe}");
    let (err, text) = c.call_text("get_transcript", json!({"path": talk}));
    assert!(!err, "{text}");
    assert!(text.contains("total_lines: 2"), "{text}");
    assert!(
        text.contains("[00:00:01.000 -> 00:00:03.000] Hello there"),
        "{text}"
    );
    assert!(
        text.contains("Second line"),
        "ASS tags and \\N must be cleaned: {text}"
    );
    let (_, paged) = c.call_text("get_transcript", json!({"path": talk, "limit": 1}));
    assert!(paged.contains("next_offset: 1"), "{paged}");
    let (_, late) = c.call_text("get_transcript", json!({"path": talk, "from": "00:04"}));
    assert!(
        late.contains("total_lines: 1") && late.contains("Second"),
        "{late}"
    );
    // No subtitles at all: a pointer to transcribe.
    let (err, text) = c.call_text(
        "get_transcript",
        json!({"path": s(&m.videos.join("clip.mp4"))}),
    );
    assert!(err && text.contains("transcribe"), "{text}");
    // No whisper model configured: explains how to get one, downloads nothing.
    let (err, text) = c.call_text("transcribe", json!({"path": talk}));
    assert!(
        err && text.contains("RUSTCUT_WHISPER_MODEL") && text.contains("ggml"),
        "{text}"
    );

    let loud = s(&m.videos.join("loud.mp4"));
    let r = c.call_json(
        "find_loud_moments",
        json!({"path": loud, "top_n": 3, "min_gap_sec": 3}),
    );
    let first = &r["moments"][0];
    let t = first["time_sec"].as_f64().unwrap();
    assert!((6.0..=8.0).contains(&t), "{r}");
    assert!(first["above_median_db"].as_f64().unwrap() > 10.0, "{r}");
    assert!(first["suggested_start_sec"].as_f64().unwrap() >= 0.0);
    assert!(first["suggested_end_sec"].as_f64().unwrap() <= 12.0);

    let (err, content) = c.call("contact_sheet", json!({"path": loud, "count": 4}));
    assert!(!err);
    assert!(content.iter().any(|b| b["type"] == "image"
        && b["mimeType"] == "image/jpeg"
        && b["data"].as_str().unwrap().len() > 500));
    assert!(content[0]["text"].as_str().unwrap().contains("tile 4"));
    let (err, content) = c.call(
        "get_frames",
        json!({"path": loud, "times": [1, "00:05", 11.5], "width": 200}),
    );
    assert!(!err);
    assert_eq!(content.iter().filter(|b| b["type"] == "image").count(), 3);
    let (err, text) = c.call_text("get_frames", json!({"path": loud, "times": [99]}));
    assert!(err && text.contains("after the end"), "{text}");
}

#[test]
fn review_regressions_timestamps_export_settings_and_validation() {
    let Some(m) = media() else { return skip() };
    let mut c = Client::spawn(&m.data, Some(&m.videos));
    c.initialize();
    let clip = s(&m.videos.join("clip.mp4"));

    // Transcript-style and arrow lines are ranges, not chapter starts.
    let r = c.call_json(
        "add_clips",
        json!({"path": clip, "timestamps": "00:02 --> 00:04 arrow form\n[00:00:05 -> 00:00:07] bracket line\n8-9 plain seconds"}),
    );
    let made = r["created"].as_array().unwrap();
    assert_eq!(made.len(), 3, "{r}");
    assert_eq!(
        (made[0]["start_sec"].as_f64(), made[0]["end_sec"].as_f64()),
        (Some(2.0), Some(4.0))
    );
    assert_eq!(made[0]["name"], "arrow form");
    assert_eq!(made[1]["name"], "bracket line");
    assert_eq!(
        (made[2]["start_sec"].as_f64(), made[2]["end_sec"].as_f64()),
        (Some(8.0), Some(9.0))
    );

    // A bad clips[] item is reported by number and does not discard the others; long names are cut.
    let long = "x".repeat(300);
    let r = c.call_json(
        "add_clips",
        json!({"path": clip, "clips": [{"start": null, "end": 5}, {"start": 1, "end": 2, "name": long}]}),
    );
    assert_eq!(r["created"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["created"][0]["name"].as_str().unwrap().len(), 100);
    let e = r["errors"][0].to_string();
    assert!(e.contains("clips[1]") && !e.contains("untagged"), "{e}");

    // An end past the video end is clamped and says so.
    let r = c.call_json(
        "add_clips",
        json!({"path": clip, "timestamps": "0:08.5-0:50 late"}),
    );
    assert!(r["warnings"].to_string().contains("clamped"), "{r}");

    // Per-call export options are not saved into the project.
    let out = s(&m.videos.join("out"));
    let (err, text) = c.call_text(
        "export_clips",
        json!({"path": clip, "indices": [1], "output_dir": out, "naming_template": "tpl_{name}",
               "on_conflict": "overwrite", "wait_sec": 120}),
    );
    assert!(!err, "{text}");
    let project: Value =
        serde_json::from_str(&std::fs::read_to_string(m.videos.join("clip.rustcut")).unwrap())
            .unwrap();
    assert_eq!(
        project["export_settings"]["naming_template"],
        "{index}_{name}"
    );
    assert_eq!(project["export_settings"]["on_conflict"], "rename");
    let r = c.call_json(
        "export_clips",
        json!({"path": clip, "indices": [1], "output_dir": out, "wait_sec": 120}),
    );
    assert!(
        r["clips"][0]["output_path"]
            .as_str()
            .unwrap()
            .contains("/01_arrow"),
        "{r}"
    );

    // Arguments outside the published schema are refused.
    let (err, text) = c.call_text("export_clips", json!({"path": clip, "indices": []}));
    assert!(err && text.contains("omit the field"), "{text}");
    let (err, text) = c.call_text("export_clips", json!({"path": clip, "wait_sec": -5}));
    assert!(err && text.contains("wait_sec"), "{text}");
}
