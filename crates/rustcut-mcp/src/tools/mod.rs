//! Tool registry: schemas, annotations, dispatch and result types.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::env::Env;

pub mod clips;
pub mod media;

/// The server `instructions` string: a short guide for the model.
pub const INSTRUCTIONS: &str = "\
RustCut clips videos without touching the originals (clips are exported as new files).
Typical workflow:
1. probe_video: duration, tracks, subtitles. Paths must be absolute (e.g. D:/Videos/stream.mp4).
2. Find the moments: get_transcript (embedded or sidecar subtitles) or transcribe (speech-to-text, \
needs a whisper model); find_loud_moments (laughter, shouting, explosions that a transcript misses); \
contact_sheet / get_frames to look at the picture.
3. add_clips with clips [{start,end,name}] or a timestamps text like \"12:10-13:05 Jump scare\".
4. preview_export (nothing is written) then export_clips. Long exports return a job_id: poll export_status.
Times are seconds from the start of the video (number) or \"HH:MM:SS.mmm\" / \"MM:SS\" text.
Clips must be at least the minimum duration (0.5 s by default) and inside the video. Clip numbers (index) \
stay stable: use them in update_clip, delete_clips and export indices.
Presets: lossless = fast stream copy, original quality, starts at the nearest earlier keyframe; \
accurate = re-encode, frame-exact cut, original framing; vertical_crop = 9:16 crop of the video (set crop_offset -1..1 \
to move the crop left/right); vertical_blur = 9:16 with the full video over a blurred background (best for TikTok/Shorts when nothing may be cut off).
Existing files are never overwritten unless on_conflict=\"overwrite\". Export options you pass (naming_template, resolution, on_conflict, ...) apply to that call only and are not saved; omitted options use the defaults shown in the tool description. The clips are stored in the same project \
file as the RustCut desktop app: open the video there to see, edit and export the same clips. \
If a tool says the project is open in RustCut, close it there and retry.
Folders: the server may read and write only inside the folders set by the RUSTCUT_MCP_ROOTS environment variable and the client's roots; if neither is set, EVERY absolute path on this computer is allowed, so keep output_dir deliberate.
The first call of a session can take several seconds (FFmpeg and encoder detection); later calls are faster.";

/// One piece of tool output.
#[derive(Debug, Clone)]
pub enum Content {
    /// Plain text.
    Text(String),
    /// A base64 image.
    Image {
        /// Base64 data.
        data: String,
        /// MIME type.
        mime: &'static str,
    },
}

/// What a tool returns (errors are results with `is_error`, never protocol errors).
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// Content blocks.
    pub content: Vec<Content>,
    /// Whether the call failed.
    pub is_error: bool,
}

impl ToolOutput {
    /// A text result.
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text(s.into())],
            is_error: false,
        }
    }

    /// A compact JSON result.
    pub fn json(v: &Value) -> Self {
        Self::text(v.to_string())
    }

    /// An error result.
    pub fn err(s: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text(s.into())],
            is_error: true,
        }
    }

    /// The MCP `CallToolResult` object.
    pub fn to_result(&self) -> Value {
        let content: Vec<Value> = self
            .content
            .iter()
            .map(|c| match c {
                Content::Text(t) => json!({"type": "text", "text": t}),
                Content::Image { data, mime } => {
                    json!({"type": "image", "data": data, "mimeType": mime})
                }
            })
            .collect();
        json!({"content": content, "isError": self.is_error})
    }
}

/// A tool body's result.
pub type ToolResult = Result<ToolOutput, String>;

/// Sends MCP progress notifications for one request.
#[derive(Clone)]
pub struct Progress {
    /// The client's `progressToken`.
    pub token: Value,
    /// The server's outgoing channel.
    pub tx: UnboundedSender<Value>,
}

impl Progress {
    /// Sends `notifications/progress`.
    pub fn notify(&self, progress: f64, total: f64, message: &str) {
        let _ = self.tx.send(json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": {
                "progressToken": self.token,
                "progress": progress,
                "total": total,
                "message": message,
            }
        }));
    }
}

/// Per-call context.
#[derive(Clone)]
pub struct Ctx {
    /// Shared state.
    pub env: Arc<Env>,
    /// Progress channel when the client asked for progress.
    pub progress: Option<Progress>,
}

/// Path text with forward slashes (Windows accepts them; avoids mixed separators in results).
pub fn fwd(s: &str) -> String {
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.to_string()
    }
}

/// Decodes tool arguments (`null` = no arguments).
///
/// # Errors
/// A message naming the problem.
pub fn decode<T: for<'de> Deserialize<'de>>(tool: &str, args: Value) -> Result<T, String> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|e| {
        format!("Invalid arguments for {tool}: {e}. Check the argument names and types in the tool description.")
    })
}

/// Runs a tool by name.
pub async fn call(ctx: Ctx, name: &str, args: Value) -> ToolOutput {
    let result = match name {
        "probe_video" => media::probe_video(&ctx, args).await,
        "get_transcript" => media::get_transcript(&ctx, args).await,
        "transcribe" => media::transcribe(&ctx, args).await,
        "find_loud_moments" => media::find_loud_moments(&ctx, args).await,
        "get_frames" => media::get_frames(&ctx, args).await,
        "contact_sheet" => media::contact_sheet(&ctx, args).await,
        "list_clips" => clips::list_clips(&ctx, args).await,
        "add_clips" => clips::add_clips(&ctx, args).await,
        "update_clip" => clips::update_clip(&ctx, args).await,
        "delete_clips" => clips::delete_clips(&ctx, args).await,
        "preview_export" => crate::export::preview_export(&ctx, args).await,
        "export_clips" => crate::export::export_clips(&ctx, args).await,
        "export_status" => crate::export::export_status(&ctx, args).await,
        "cancel_export" => crate::export::cancel_export(&ctx, args).await,
        other => Err(format!(
            "Unknown tool \"{other}\". Call tools/list to see the available tools."
        )),
    };
    result.unwrap_or_else(ToolOutput::err)
}

// ───────────────────────────── schemas ─────────────────────────────

fn time(desc: &str) -> Value {
    json!({"type": ["number", "string"], "description": desc})
}

fn path_prop() -> Value {
    json!({"type": "string", "description": "Absolute path of the video file, e.g. D:/Videos/stream.mp4"})
}

fn tool(
    name: &str,
    title: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    hints: (bool, bool, bool),
) -> Value {
    let (read_only, destructive, idempotent) = hints;
    let mut annotations = json!({
        "title": title,
        "readOnlyHint": read_only,
        "idempotentHint": idempotent,
        "openWorldHint": false,
    });
    if !read_only {
        annotations["destructiveHint"] = json!(destructive);
    }
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
        "annotations": annotations,
    })
}

fn export_props() -> Value {
    json!({
        "path": path_prop(),
        "preset": {"type": "string", "enum": ["lossless", "accurate", "vertical_crop", "vertical_blur"],
            "description": "lossless (default): fast copy, original quality, starts at the nearest earlier keyframe. accurate: re-encode, frame-exact, original framing. vertical_crop: 9:16 crop. vertical_blur: 9:16 full frame over a blurred background."},
        "indices": {"type": "array", "items": {"type": "integer", "minimum": 1},
            "description": "Clip numbers to export (default: all included clips)."},
        "output_dir": {"type": "string", "description": "Absolute output folder (default: <video folder>/<video name>_clips)."},
        "naming_template": {"type": "string", "description": "File name template, default \"{index}_{name}\". Variables: {index} {name} {source} {tag} {start} {end} {duration} {date}."},
        "resolution": {"type": "string", "enum": ["1080x1920", "720x1280", "native"], "description": "Output size for the vertical presets (default 1080x1920)."},
        "crop_offset": {"type": "number", "minimum": -1, "maximum": 1,
            "description": "Horizontal crop position for vertical_crop: -1 = left edge, 0 = centre, 1 = right edge. Applied to the exported clips."},
        "audio_track": {"type": "integer", "minimum": 1, "description": "Audio track number to export (default 1)."},
        "on_conflict": {"type": "string", "enum": ["rename", "skip", "overwrite"],
            "description": "When an output file already exists: rename (default, adds a number), skip, or overwrite (the only way to replace files)."},
    })
}

/// The `tools/list` payload.
pub fn definitions() -> Vec<Value> {
    let ro = (true, false, true);
    let mut export_with_wait = export_props();
    export_with_wait["wait_sec"] = json!({"type": "number", "minimum": 0, "maximum": 3600,
        "description": "How long to wait for the export to finish before returning a job_id (default 240). Use export_status to poll."});
    vec![
        tool(
            "probe_video",
            "Probe video",
            "Read a video's facts: duration, size, resolution, fps, variable frame rate, HDR, rotation, video codec, audio tracks (number, codec, channels, language), subtitle tracks (text or bitmap), sidecar subtitle files, existing RustCut clips and warnings. Start here.",
            json!({"path": path_prop()}),
            &["path"],
            ro,
        ),
        tool(
            "get_transcript",
            "Get transcript",
            "Read subtitles as lines \"[HH:MM:SS.mmm -> HH:MM:SS.mmm] text\" from an embedded text subtitle track (subrip, ass, webvtt, mov_text) or a sidecar file next to the video (same name .srt/.vtt/.ass, optionally .en.srt). Markup, drawing commands, watermark lines and duplicate lines are removed. Returns total_lines and next_offset for paging. Bitmap subtitles (PGS, VobSub, DVB) cannot be read: use transcribe instead.",
            json!({
                "path": path_prop(),
                "track": {"type": "integer", "minimum": 1, "description": "Subtitle track number from probe_video (default: first text track, then a sidecar file)."},
                "from": time("Only lines ending after this time."),
                "to": time("Only lines starting before this time."),
                "offset": {"type": "integer", "minimum": 0, "description": "Line to start at (use next_offset of the previous page)."},
                "limit": {"type": "integer", "minimum": 1, "maximum": 2000, "description": "Lines per page (default 400)."},
            }),
            &["path"],
            ro,
        ),
        tool(
            "transcribe",
            "Transcribe speech",
            "Speech-to-text with FFmpeg's whisper filter, for videos without usable subtitles. Needs a ggml whisper model file named by the RUSTCUT_WHISPER_MODEL environment variable (nothing is downloaded). Slow on long videos: use from/to to limit the range. Results are cached. Same output format as get_transcript.",
            json!({
                "path": path_prop(),
                "language": {"type": "string", "description": "Spoken language code such as \"en\" or \"de\" (default \"auto\")."},
                "from": time("Start of the range to transcribe."),
                "to": time("End of the range to transcribe."),
                "audio_track": {"type": "integer", "minimum": 1, "description": "Audio track number (default 1)."},
                "offset": {"type": "integer", "minimum": 0, "description": "Line to start at when paging."},
                "limit": {"type": "integer", "minimum": 1, "maximum": 2000, "description": "Lines per page (default 400)."},
            }),
            &["path"],
            (false, false, true),
        ),
        tool(
            "find_loud_moments",
            "Find loud moments",
            "Rank the loudest moments of the audio (EBU R128 momentary loudness above the track median) and suggest a clip around each (about 10 s before to 5 s after the peak). Catches laughter, shouting, explosions and crowd noise that a transcript misses. Results are clip candidates for add_clips.",
            json!({
                "path": path_prop(),
                "top_n": {"type": "integer", "minimum": 1, "maximum": 50, "description": "How many moments (default 10)."},
                "min_gap_sec": {"type": "number", "minimum": 1, "description": "Minimum seconds between two moments (default 20)."},
                "window_sec": {"type": "number", "minimum": 0.5, "maximum": 10, "description": "Averaging window in seconds (default 2)."},
                "audio_track": {"type": "integer", "minimum": 1, "description": "Audio track number (default 1)."},
            }),
            &["path"],
            ro,
        ),
        tool(
            "get_frames",
            "Get frames",
            "Return still images (JPEG) of the video at the given times, for models that can see images. Up to 12 frames.",
            json!({
                "path": path_prop(),
                "times": {"type": "array", "maxItems": 12, "minItems": 1, "items": time("A time in the video."), "description": "Times of the frames."},
                "width": {"type": "integer", "minimum": 64, "maximum": 1920, "description": "Image width in pixels (default 384)."},
            }),
            &["path", "times"],
            ro,
        ),
        tool(
            "contact_sheet",
            "Contact sheet",
            "ONE tiled JPEG of evenly spaced frames from the video (or from start to end) with the time of each tile drawn in. The cheapest way to look at a video. The frame times are also listed as text.",
            json!({
                "path": path_prop(),
                "start": time("Start of the range (default 0)."),
                "end": time("End of the range (default the end of the video)."),
                "count": {"type": "integer", "minimum": 1, "maximum": 25, "description": "Number of frames (default 9)."},
                "width": {"type": "integer", "minimum": 160, "maximum": 1920, "description": "Width of the whole sheet in pixels (default 960)."},
            }),
            &["path"],
            ro,
        ),
        tool(
            "list_clips",
            "List clips",
            "List the clips of a video: number, name, start, end, duration, tag, included, export status and last output file. Returns an empty list (not an error) when the video has no project yet.",
            json!({"path": path_prop()}),
            &["path"],
            ro,
        ),
        tool(
            "add_clips",
            "Add clips",
            "Add clips to the video's project (the same project the RustCut desktop app uses). Pass clips [{start,end,name?,tag?}] and/or timestamps text such as \"12:10-13:05 Jump scare\" (one per line; also \"[00:10:00 -> 00:10:05] text\" transcript lines, \"90-100\" seconds, \"12m30s-13m\", CSV and chapter lists). Names are limited to 100 characters. Check the warnings in the result (clamped end, unusually long clip, odd name). Returns the created clips and a per-item error for every clip that was too short, out of range or ended before it started. Nothing is exported.",
            json!({
                "path": path_prop(),
                "clips": {"type": "array", "description": "Clips to add.", "items": {
                    "type": "object",
                    "properties": {
                        "start": time("Clip start."),
                        "end": time("Clip end."),
                        "name": {"type": "string", "description": "Clip name, used in the file name."},
                        "tag": {"type": "string", "description": "Tag label from the project's palette."},
                    },
                    "required": ["start", "end"],
                }},
                "timestamps": {"type": "string", "description": "Timestamps text, one clip per line, e.g. \"12:10-13:05 Jump scare\"."},
                "replace": {"type": "boolean", "description": "Delete all existing clips first (default false). Nothing is changed if no new clip is valid."},
            }),
            &["path"],
            (false, true, false),
        ),
        tool(
            "update_clip",
            "Update clip",
            "Change one clip: start, end, name, tag (empty string clears it), included (whether export_clips exports it by default) or crop_offset (-1 left .. 1 right, for vertical_crop).",
            json!({
                "path": path_prop(),
                "index": {"type": "integer", "minimum": 1, "description": "Clip number from list_clips."},
                "start": time("New start."),
                "end": time("New end."),
                "name": {"type": "string", "description": "New name."},
                "tag": {"type": "string", "description": "Tag label, or empty to clear."},
                "included": {"type": "boolean", "description": "Include the clip in default exports."},
                "crop_offset": {"type": "number", "minimum": -1, "maximum": 1, "description": "Horizontal crop position for vertical_crop."},
            }),
            &["path", "index"],
            (false, false, true),
        ),
        tool(
            "delete_clips",
            "Delete clips",
            "Delete clips by number. Numbers of deleted clips are never reused. Exported files are not touched.",
            json!({
                "path": path_prop(),
                "indices": {"type": "array", "items": {"type": "integer", "minimum": 1}, "minItems": 1, "description": "Clip numbers to delete."},
            }),
            &["path", "indices"],
            (false, true, true),
        ),
        tool(
            "preview_export",
            "Preview export",
            "Pre-flight report without exporting: planned file names, size estimate, free disk space, warnings and blocking errors (which export_clips would refuse on). Writes nothing.",
            export_props(),
            &["path"],
            ro,
        ),
        tool(
            "export_clips",
            "Export clips",
            "Export clips as new files (the original is never modified). Runs the pre-flight first and refuses on blocking errors. Waits up to wait_sec for the result with per-clip output path, size and status; otherwise returns a job_id to poll with export_status. Existing files are renamed around, never overwritten, unless on_conflict=\"overwrite\".",
            export_with_wait,
            &["path"],
            (false, true, false),
        ),
        tool(
            "export_status",
            "Export status",
            "Progress and per-clip results of an export started by export_clips.",
            json!({"job_id": {"type": "string", "description": "The job_id returned by export_clips."}}),
            &["job_id"],
            ro,
        ),
        tool(
            "cancel_export",
            "Cancel export",
            "Cancel a running export. Partial files are deleted; clips already finished keep their files.",
            json!({"job_id": {"type": "string", "description": "The job_id returned by export_clips."}}),
            &["job_id"],
            (false, false, true),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_valid_schema_and_annotations() {
        let defs = definitions();
        let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
        for expected in [
            "probe_video",
            "get_transcript",
            "transcribe",
            "find_loud_moments",
            "get_frames",
            "contact_sheet",
            "list_clips",
            "add_clips",
            "update_clip",
            "delete_clips",
            "preview_export",
            "export_clips",
            "export_status",
            "cancel_export",
        ] {
            assert!(names.contains(&expected), "{expected} missing");
        }
        for d in &defs {
            let schema = &d["inputSchema"];
            assert_eq!(schema["type"], "object");
            let props = schema["properties"].as_object().unwrap();
            for r in schema["required"].as_array().unwrap() {
                assert!(props.contains_key(r.as_str().unwrap()), "{d}");
            }
            assert!(d["description"].as_str().unwrap().len() > 20);
            let a = &d["annotations"];
            assert!(a["readOnlyHint"].is_boolean() && a["idempotentHint"].is_boolean());
            if a["readOnlyHint"] == json!(false) {
                assert!(a["destructiveHint"].is_boolean(), "{d}");
            }
        }
    }

    #[test]
    fn decode_reports_bad_arguments() {
        #[derive(Deserialize)]
        struct A {
            #[allow(dead_code)]
            path: String,
        }
        let e = decode::<A>("probe_video", json!({})).err().unwrap();
        assert!(e.contains("Invalid arguments for probe_video"));
        assert!(decode::<A>("x", json!({"path": "a"})).is_ok());
    }

    #[test]
    fn results_serialize_as_mcp_content() {
        let out = ToolOutput::err("bad");
        let v = out.to_result();
        assert_eq!(v["isError"], true);
        assert_eq!(v["content"][0]["type"], "text");
        let img = ToolOutput {
            content: vec![Content::Image {
                data: "AA==".into(),
                mime: "image/jpeg",
            }],
            is_error: false,
        };
        assert_eq!(img.to_result()["content"][0]["mimeType"], "image/jpeg");
    }
}
