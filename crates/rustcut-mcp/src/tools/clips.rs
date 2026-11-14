//! Clip tools: list, add, update, delete. Mutations go through `ProjectStore` like the GUI.

use serde::Deserialize;
use serde_json::{Value, json};

use rustcut_model::{AddClipRequest, Clip, ClipEdit, Project};
use rustcut_naming::timestamps::parse_import_detailed;

use super::media::PathArgs;
use super::{Ctx, ToolOutput, ToolResult, decode, fwd};
use crate::projects::{clip_by_index, find_readonly, open_for_write, resolve_tag, store_options};
use crate::timeparse::{TimeArg, fmt_ts, normalize_timestamps, parse_time};

/// Longest clip name kept (longer names are shortened with a warning).
const MAX_NAME_CHARS: usize = 100;
/// Clips longer than this get a warning (a mis-read end time is the usual cause).
const LONG_CLIP_SEC: f64 = 600.0;

fn cap_name(name: &str) -> (String, bool) {
    if name.chars().count() <= MAX_NAME_CHARS {
        return (name.to_string(), false);
    }
    let cut: String = name.chars().take(MAX_NAME_CHARS).collect();
    (cut.trim_end().to_string(), true)
}

/// A hint when a clip looks like a mistake.
fn suspicious(c: &Clip) -> Option<String> {
    let mut parts = Vec::new();
    if c.duration() > LONG_CLIP_SEC {
        parts.push(format!(
            "This clip is {:.1} minutes long, which is unusually long; check the end time.",
            c.duration() / 60.0
        ));
    }
    if c.name.starts_with(['>', ']']) {
        parts.push(format!(
            "The name \"{}\" starts with a stray symbol, so a timestamps line was probably misread; check start, end and name.",
            c.name
        ));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn add_warning(v: &mut Value, msg: &str) {
    let text = match v["warning"].as_str() {
        Some(old) => format!("{old} {msg}"),
        None => msg.to_string(),
    };
    v["warning"] = json!(text);
}

const SEE_IN_APP: &str = "These clips are stored in the RustCut project of this video: open the video in the desktop app to see and edit them.";

/// A clip as the tools show it.
pub fn clip_json(project: &Project, c: &Clip) -> Value {
    let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
    json!({
        "index": c.index,
        "name": c.name,
        "start_sec": r3(c.start_sec),
        "end_sec": r3(c.end_sec),
        "start": fmt_ts(c.start_sec),
        "end": fmt_ts(c.end_sec),
        "duration_sec": r3(c.duration()),
        "tag": c.tag_id.as_deref().and_then(|id| project.tag(id)).map(|t| t.label.clone()),
        "included": c.selected_for_export,
        "export_status": serde_json::to_value(c.export_status).unwrap_or(Value::Null),
        "last_output": c.last_export.as_ref().and_then(|l| l.output_path.as_deref()).map(fwd),
        "last_error": c.last_export.as_ref().and_then(|l| l.error.clone()),
    })
}

pub(crate) async fn list_clips(ctx: &Ctx, args: Value) -> ToolResult {
    let a: PathArgs = decode("list_clips", args)?;
    let video = ctx.env.video_path(&a.path)?;
    let Some((project, project_path)) = find_readonly(&ctx.env, &video)? else {
        return Ok(ToolOutput::json(&json!({
            "clips": [],
            "note": "This video has no clips yet (no project). Use add_clips to create some.",
        })));
    };
    let mut clips: Vec<&Clip> = project.clips.iter().collect();
    clips.sort_by_key(|c| c.index);
    Ok(ToolOutput::json(&json!({
        "video_duration_sec": project.source.duration_sec,
        "project_file": fwd(&project_path.to_string_lossy()),
        "clips": clips.iter().map(|c| clip_json(&project, c)).collect::<Vec<_>>(),
    })))
}

/// One clip to add.
#[derive(Deserialize)]
pub struct ClipIn {
    start: TimeArg,
    end: TimeArg,
    name: Option<String>,
    tag: Option<String>,
}

/// Arguments of `add_clips`.
#[derive(Deserialize)]
pub struct AddArgs {
    path: String,
    clips: Option<Vec<Value>>,
    timestamps: Option<String>,
    replace: Option<bool>,
}

pub(crate) async fn add_clips(ctx: &Ctx, args: Value) -> ToolResult {
    let a: AddArgs = decode("add_clips", args)?;
    let has_clips = a.clips.as_ref().is_some_and(|c| !c.is_empty());
    let has_text = a.timestamps.as_ref().is_some_and(|t| !t.trim().is_empty());
    if !has_clips && !has_text {
        return Err("Nothing to add. Pass clips [{\"start\":..,\"end\":..,\"name\":..}] and/or timestamps text such as \"12:10-13:05 Jump scare\".".into());
    }
    let env = &ctx.env;
    let video = env.video_path(&a.path)?;
    let mut opened = open_for_write(env, &video).await?;
    let opts = store_options(env);
    let duration = opened.store.project().source.duration_sec;
    let replaced = if a.replace.unwrap_or(false) {
        let ids: Vec<_> = opened.store.project().clips.iter().map(|c| c.id).collect();
        if !ids.is_empty() {
            opened.store.delete_clips(&ids).map_err(|e| e.to_string())?;
        }
        ids.len()
    } else {
        0
    };
    let mut created: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    let mut warnings: Vec<Value> = Vec::new();
    for (i, raw) in a.clips.iter().flatten().enumerate() {
        let item = i + 1;
        let fail = |msg: String| json!({"item": format!("clips[{}]", item), "error": msg});
        let c: ClipIn = match serde_json::from_value(raw.clone()) {
            Ok(c) => c,
            Err(e) => {
                errors.push(fail(format!(
                    "Each clip must be an object like {{\"start\": \"12:10\", \"end\": \"13:05\", \"name\": \"Jump scare\"}} (start and end are seconds or \"MM:SS\" text; name and tag are optional text): {e}."
                )));
                continue;
            }
        };
        let (start, end) = match (parse_time(&c.start), parse_time(&c.end)) {
            (Ok(s), Ok(e)) => (s, e),
            (Err(m), _) | (_, Err(m)) => {
                errors.push(fail(m));
                continue;
            }
        };
        if end <= start {
            errors.push(fail(format!(
                "end ({}) must be after start ({}).",
                fmt_ts(end),
                fmt_ts(start)
            )));
            continue;
        }
        let tag_id = match c.tag.as_deref().filter(|t| !t.trim().is_empty()) {
            Some(t) => match resolve_tag(opened.store.project(), t) {
                Ok(id) => Some(id),
                Err(m) => {
                    errors.push(fail(m));
                    continue;
                }
            },
            None => None,
        };
        let name = c.name.as_deref().map(cap_name);
        let req = AddClipRequest {
            start_sec: start,
            end_sec: end,
            name: name.as_ref().map(|n| n.0.clone()),
            tag_id,
        };
        match opened.store.add_clip(&req, &opts) {
            Ok(r) => {
                let mut v = clip_json(opened.store.project(), &r.clip);
                if let Some(dup) = r.duplicate_of {
                    add_warning(&mut v, &format!("Overlaps clip {dup} almost exactly."));
                }
                if name.is_some_and(|n| n.1) {
                    add_warning(
                        &mut v,
                        &format!("The name was shortened to {MAX_NAME_CHARS} characters."),
                    );
                }
                if r.clip.end_sec < end - 0.001 {
                    add_warning(
                        &mut v,
                        &format!(
                            "clamped: the end ({}) is past the end of the video, so the clip ends at {}.",
                            fmt_ts(end),
                            fmt_ts(r.clip.end_sec)
                        ),
                    );
                }
                if let Some(m) = suspicious(&r.clip) {
                    add_warning(&mut v, &m);
                }
                created.push(v);
            }
            Err(e) => errors.push(fail(e.to_string())),
        }
    }
    if has_text {
        let text = a.timestamps.as_deref().unwrap_or("");
        let (text, line_errors) = normalize_timestamps(text);
        for (line, msg) in line_errors {
            errors.push(json!({"item": format!("timestamps line {line}"), "error": msg}));
        }
        let (mut preview, notes) = parse_import_detailed(&text, duration, opts.min_duration());
        for n in notes {
            warnings
                .push(json!({"item": format!("timestamps line {}", n.line), "warning": n.message}));
        }
        for it in &mut preview.items {
            let (n, cut) = cap_name(&it.name);
            if cut {
                it.name = n;
                warnings.push(json!({
                    "item": format!("timestamps line {}", it.line),
                    "warning": format!("The name was shortened to {MAX_NAME_CHARS} characters."),
                }));
            }
        }
        match opened.store.import_clips(&preview.items, &opts) {
            Ok(r) => {
                for id in &r.added {
                    if let Some(c) = opened.store.project().clip(*id) {
                        let mut v = clip_json(opened.store.project(), c);
                        if let Some(m) = suspicious(c) {
                            add_warning(&mut v, &m);
                        }
                        created.push(v);
                    }
                }
                for s in &r.skipped {
                    errors.push(json!({
                        "item": format!("timestamps line {}", s.line),
                        "error": s.error.clone().unwrap_or_else(|| "invalid".into()),
                    }));
                }
            }
            Err(e) => errors.push(json!({"item": "timestamps", "error": e.to_string()})),
        }
    }
    if created.is_empty() {
        // Nothing valid: leave the project exactly as it was (also undoes `replace`).
        return Ok(ToolOutput::err(format!(
            "No clip was added and nothing was changed. Problems: {}",
            Value::Array(errors)
        )));
    }
    let was_created = opened.created;
    opened.save(true)?;
    let total = opened.store.project().clips.len();
    Ok(ToolOutput::json(&json!({
        "created": created,
        "errors": errors,
        "warnings": warnings,
        "replaced_clips": replaced,
        "total_clips": total,
        "project_created": was_created,
        "note": SEE_IN_APP,
    })))
}

/// Arguments of `update_clip`.
#[derive(Deserialize)]
pub struct UpdateArgs {
    path: String,
    index: u32,
    start: Option<TimeArg>,
    end: Option<TimeArg>,
    name: Option<String>,
    tag: Option<String>,
    included: Option<bool>,
    crop_offset: Option<f64>,
}

pub(crate) async fn update_clip(ctx: &Ctx, args: Value) -> ToolResult {
    let a: UpdateArgs = decode("update_clip", args)?;
    if a.start.is_none()
        && a.end.is_none()
        && a.name.is_none()
        && a.tag.is_none()
        && a.included.is_none()
        && a.crop_offset.is_none()
    {
        return Err(
            "Nothing to change. Pass at least one of start, end, name, tag, included, crop_offset."
                .into(),
        );
    }
    let (start, end) = (
        a.start.as_ref().map(parse_time).transpose()?,
        a.end.as_ref().map(parse_time).transpose()?,
    );
    if let Some(c) = a.crop_offset
        && !(c.is_finite() && (-1.0..=1.0).contains(&c))
    {
        return Err("crop_offset must be between -1 (left) and 1 (right).".into());
    }
    let env = &ctx.env;
    let video = env.video_path(&a.path)?;
    let mut opened = open_for_write(env, &video).await?;
    if opened.created {
        return Err("This video has no clips yet. Use add_clips first.".into());
    }
    let opts = store_options(env);
    let (id, _) = {
        let c = clip_by_index(opened.store.project(), a.index)?;
        (c.id, c.index)
    };
    let tag = match a.tag.as_deref() {
        None => None,
        Some(t) if t.trim().is_empty() || t.eq_ignore_ascii_case("none") => Some(None),
        Some(t) => Some(Some(resolve_tag(opened.store.project(), t)?)),
    };
    let mut edits = Vec::new();
    if start.is_some() || end.is_some() {
        edits.push(ClipEdit::Retime {
            start_sec: start,
            end_sec: end,
        });
    }
    if let Some(n) = a.name {
        edits.push(ClipEdit::Rename {
            name: cap_name(&n).0,
        });
    }
    if let Some(t) = tag {
        edits.push(ClipEdit::SetTag { tag_id: t });
    }
    if let Some(i) = a.included {
        edits.push(ClipEdit::SetIncluded { included: i });
    }
    if let Some(c) = a.crop_offset {
        edits.push(ClipEdit::SetCrop { crop_x_offset: c });
    }
    for e in &edits {
        // A failure drops `opened` without saving, so a half-applied update never persists.
        opened
            .store
            .edit_clips(&[id], e, &opts)
            .map_err(|err| format!("{err} (no change was saved)"))?;
    }
    opened.save(true)?;
    let project = opened.store.project();
    let c = project.clip(id).ok_or("The clip disappeared.")?;
    Ok(ToolOutput::json(
        &json!({"clip": clip_json(project, c), "note": SEE_IN_APP}),
    ))
}

/// Arguments of `delete_clips`.
#[derive(Deserialize)]
pub struct DeleteArgs {
    path: String,
    indices: Vec<u32>,
}

pub(crate) async fn delete_clips(ctx: &Ctx, args: Value) -> ToolResult {
    let a: DeleteArgs = decode("delete_clips", args)?;
    if a.indices.is_empty() {
        return Err("indices is empty. Pass the clip numbers to delete (see list_clips).".into());
    }
    let env = &ctx.env;
    let video = env.video_path(&a.path)?;
    let mut opened = open_for_write(env, &video).await?;
    if opened.created {
        return Err("This video has no clips yet, so there is nothing to delete.".into());
    }
    let mut ids = Vec::new();
    for i in &a.indices {
        ids.push(clip_by_index(opened.store.project(), *i)?.id);
    }
    opened.store.delete_clips(&ids).map_err(|e| e.to_string())?;
    opened.save(true)?;
    Ok(ToolOutput::json(&json!({
        "deleted": a.indices,
        "remaining_clips": opened.store.project().clips.len(),
    })))
}
