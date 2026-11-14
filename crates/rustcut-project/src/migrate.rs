//! Schema migration (PRJ-06) and lenient project parsing.
//!
//! # v1 (PRD v1.2) → v2
//! v1 files are recognised by `"project_version": "1.0"` and no
//! `schema_version`. No v1 writer is available, so the migration is lenient
//! and rests on these documented assumptions:
//! * Top-level layout as in v2, minus `tags` (v1 had no palette).
//! * Each clip has a `color` (`"red"`, `"#EF4444"`, …, or null) instead of
//!   `tag_id`. Red/`#EF4444` → `funny`, blue/`#3B82F6` → `edu`,
//!   green/`#22C55E` → `highlight` (the default palette); any other colour
//!   becomes a new tag `color-<name or hex>` labelled with the colour name
//!   (or hex), shared by every clip of that colour.
//! * `crop_x_offset` existed per clip *and* in `export_settings`; the
//!   export-settings copy is dropped and clips without their own value
//!   inherit it.
//! * The naming variable `{clip_name}` became `{name}`; a clip field
//!   `clip_name` (if present instead of `name`) becomes `name`.
//! * Statuses `done`/`error`/`queued` map to `completed`/`failed`/`pending`.
//! * Anything else missing gets v2 defaults; unreadable clips are left out
//!   with a warning instead of failing the whole file.

use rustcut_model::{
    Clip, ExportSettings, Fingerprint, Project, SourceInfo, Tag, ViewState, default_tags, now_utc,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use uuid::Uuid;

/// Turn a v1 project document into a v2 document (see the module docs).
pub fn v1_to_v2(v1: Value) -> Result<Value, String> {
    let Value::Object(mut root) = v1 else {
        return Err("a version 1 project must be a JSON object".into());
    };
    root.remove("project_version");
    root.insert("schema_version".into(), json!(2));

    let mut project_crop = 0.0;
    if let Some(Value::Object(settings)) = root.get_mut("export_settings") {
        if let Some(crop) = settings.remove("crop_x_offset").and_then(|v| v.as_f64()) {
            project_crop = crop;
        }
        if let Some(Value::String(template)) = settings.get_mut("naming_template") {
            *template = template.replace("{clip_name}", "{name}");
        }
    }

    let mut tags: Vec<Tag> = root
        .get("tags")
        .and_then(|t| serde_json::from_value(t.clone()).ok())
        .unwrap_or_else(default_tags);
    if let Some(Value::Array(clips)) = root.get_mut("clips") {
        for clip in clips.iter_mut() {
            if let Value::Object(c) = clip {
                migrate_clip(c, project_crop, &mut tags);
            }
        }
    }
    root.insert(
        "tags".into(),
        serde_json::to_value(&tags).map_err(|e| e.to_string())?,
    );
    Ok(Value::Object(root))
}

fn migrate_clip(c: &mut Map<String, Value>, project_crop: f64, tags: &mut Vec<Tag>) {
    if let Some(name) = c.remove("clip_name")
        && !c.contains_key("name")
    {
        c.insert("name".into(), name);
    }
    if !c.get("crop_x_offset").is_some_and(Value::is_number) {
        c.insert("crop_x_offset".into(), json!(project_crop));
    }
    let color = c.remove("color");
    if !c.contains_key("tag_id") {
        let tag_id = color
            .as_ref()
            .and_then(Value::as_str)
            .and_then(|s| tag_for_color(s, tags));
        c.insert("tag_id".into(), json!(tag_id));
    }
    if let Some(Value::String(status)) = c.get_mut("export_status") {
        let mapped = match status.to_ascii_lowercase().as_str() {
            "done" | "complete" | "exported" => Some("completed"),
            "error" => Some("failed"),
            "queued" => Some("pending"),
            _ => None,
        };
        if let Some(m) = mapped {
            *status = m.to_string();
        }
    }
}

/// Map a v1 colour to a tag id, adding a tag to `tags` when needed.
fn tag_for_color(color: &str, tags: &mut Vec<Tag>) -> Option<String> {
    let raw = color.trim();
    if raw.is_empty() {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    let hex = color_to_hex(&lower);
    let default_id = match (lower.as_str(), hex.as_deref()) {
        ("red", _) | (_, Some("#EF4444")) => Some("funny"),
        ("blue", _) | (_, Some("#3B82F6")) => Some("edu"),
        ("green", _) | (_, Some("#22C55E")) => Some("highlight"),
        _ => None,
    };
    if let Some(id) = default_id {
        if !tags.iter().any(|t| t.id == id)
            && let Some(tag) = default_tags().into_iter().find(|t| t.id == id)
        {
            tags.push(tag);
        }
        return Some(id.to_string());
    }
    let (id, label) = match &hex {
        Some(h) if lower.starts_with('#') => {
            (format!("color-{}", h[1..].to_ascii_lowercase()), h.clone())
        }
        _ => {
            let slug: String = lower
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .take(24)
                .collect();
            let slug = if slug.is_empty() {
                "custom".to_string()
            } else {
                slug
            };
            (format!("color-{slug}"), capitalize(raw))
        }
    };
    if !tags.iter().any(|t| t.id == id) {
        tags.push(Tag {
            id: id.clone(),
            label,
            color: hex.unwrap_or_else(|| "#6B7280".into()),
        });
    }
    Some(id)
}

fn color_to_hex(lower: &str) -> Option<String> {
    if let Some(digits) = lower.strip_prefix('#') {
        if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        return match digits.len() {
            6 => Some(format!("#{}", digits.to_ascii_uppercase())),
            3 => Some(format!(
                "#{}",
                digits
                    .chars()
                    .flat_map(|c| [c, c])
                    .collect::<String>()
                    .to_ascii_uppercase()
            )),
            _ => None,
        };
    }
    let hex = match lower {
        "red" => "#EF4444",
        "blue" => "#3B82F6",
        "green" => "#22C55E",
        "yellow" => "#EAB308",
        "orange" => "#F97316",
        "purple" | "violet" => "#A855F7",
        "pink" => "#EC4899",
        "teal" => "#14B8A6",
        "cyan" => "#06B6D4",
        "gray" | "grey" => "#6B7280",
        "white" => "#FFFFFF",
        "black" => "#000000",
        _ => return None,
    };
    Some(hex.to_string())
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    let label: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    };
    label.chars().take(40).collect()
}

/// Build a [`Project`] from a document that does not parse strictly (older
/// or newer schema). Only a readable `source` is required; other blocks fall
/// back to defaults field by field, unreadable clips and tags are skipped.
/// Returns user-facing warnings for what was left out.
pub(crate) fn lenient_project(
    v: &Value,
    schema_version: u32,
) -> Result<(Project, Vec<String>), String> {
    let obj = v
        .as_object()
        .ok_or_else(|| "the file does not contain a project".to_string())?;
    let source = lenient_source(obj.get("source"))
        .ok_or_else(|| "the project has no readable source video information".to_string())?;
    let mut warnings = Vec::new();
    let tags = lenient_tags(obj.get("tags"), &mut warnings);
    let (mut clips, skipped) = lenient_clips(obj.get("clips"));
    if skipped > 0 {
        let what = if skipped == 1 {
            "1 clip".to_string()
        } else {
            format!("{skipped} clips")
        };
        warnings.push(format!(
            "{what} could not be read and {} left out.",
            if skipped == 1 { "was" } else { "were" }
        ));
    }
    let tag_ids: HashSet<&str> = tags.iter().map(|t| t.id.as_str()).collect();
    for clip in &mut clips {
        if clip.tag_id.as_deref().is_some_and(|t| !tag_ids.contains(t)) {
            clip.tag_id = None;
        }
    }
    let max_index = clips.iter().map(|c| c.index).max().unwrap_or(0);
    let stored_next = obj
        .get("next_index")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(1);
    let now = now_utc();
    let created_at = obj
        .get("created_at")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or(now);
    let updated_at = obj
        .get("updated_at")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or(now);
    let project = Project {
        schema_version,
        app_version: obj
            .get("app_version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        created_at,
        updated_at,
        source,
        tags,
        export_settings: lenient_struct::<ExportSettings>(obj.get("export_settings")).normalized(),
        next_index: stored_next.max(max_index.saturating_add(1)).max(1),
        clips,
        view_state: lenient_struct::<ViewState>(obj.get("view_state")),
    };
    Ok((project, warnings))
}

/// Start from `T::default()` and take over every field of `v` that keeps
/// the struct valid.
fn lenient_struct<T: Default + Serialize + DeserializeOwned>(v: Option<&Value>) -> T {
    let Ok(mut base) = serde_json::to_value(T::default()) else {
        return T::default();
    };
    if let (Some(Value::Object(fields)), Value::Object(_)) = (v, &base) {
        for (key, value) in fields {
            let mut trial = base.clone();
            if let Value::Object(map) = &mut trial {
                map.insert(key.clone(), value.clone());
            }
            if serde_json::from_value::<T>(trial.clone()).is_ok() {
                base = trial;
            }
        }
    }
    serde_json::from_value(base).unwrap_or_default()
}

fn lenient_source(v: Option<&Value>) -> Option<SourceInfo> {
    let mut s = v?.as_object()?.clone();
    let path = s.get("path")?.as_str()?.to_string();
    // Either separator: the file may come from another OS.
    let file_name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_string();
    s.entry("relative_path").or_insert_with(|| json!(file_name));
    if !s
        .get("fingerprint")
        .is_some_and(|f| serde_json::from_value::<Fingerprint>(f.clone()).is_ok())
    {
        s.insert(
            "fingerprint".into(),
            json!({ "size_bytes": 0, "xxh3_head_tail": "" }),
        );
    }
    if !s.get("duration_sec").is_some_and(Value::is_number) {
        s.insert("duration_sec".into(), json!(0.0));
    }
    if !s.get("start_time_sec").is_some_and(Value::is_number) {
        s.insert("start_time_sec".into(), json!(0.0));
    }
    if !s.get("audio_tracks").is_some_and(Value::is_array) {
        s.insert("audio_tracks".into(), json!([]));
    }
    let mut video = s
        .get("video")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let coded_w = video.get("coded_width").cloned().unwrap_or(json!(0));
    let coded_h = video.get("coded_height").cloned().unwrap_or(json!(0));
    for (key, default) in [
        ("codec", json!("")),
        ("coded_width", coded_w.clone()),
        ("coded_height", coded_h.clone()),
        ("display_width", coded_w),
        ("display_height", coded_h),
        ("rotation", json!(0)),
        ("sar", json!("1:1")),
        ("frame_rate", json!("0/1")),
        ("vfr", json!(false)),
        ("hdr", json!(false)),
    ] {
        video.entry(key).or_insert(default);
    }
    s.insert("video".into(), Value::Object(video));
    if let Some(Value::Array(tracks)) = s.get_mut("audio_tracks") {
        tracks.retain(|t| serde_json::from_value::<rustcut_model::AudioTrack>(t.clone()).is_ok());
    }
    serde_json::from_value(Value::Object(s)).ok()
}

fn lenient_tags(v: Option<&Value>, warnings: &mut Vec<String>) -> Vec<Tag> {
    let Some(Value::Array(items)) = v else {
        return default_tags();
    };
    let mut seen = HashSet::new();
    let mut tags = Vec::new();
    for item in items {
        match serde_json::from_value::<Tag>(item.clone()) {
            Ok(tag) if !tag.id.trim().is_empty() && seen.insert(tag.id.clone()) => tags.push(tag),
            _ => warnings.push("A tag could not be read and was left out.".into()),
        }
    }
    tags
}

/// Parse clips one by one; returns the readable clips and the number skipped.
fn lenient_clips(v: Option<&Value>) -> (Vec<Clip>, usize) {
    let Some(Value::Array(items)) = v else {
        return (Vec::new(), 0);
    };
    let mut clips = Vec::new();
    let mut skipped = 0;
    let mut ids = HashSet::new();
    for item in items {
        match lenient_clip(item) {
            Some(mut clip) => {
                if !ids.insert(clip.id) {
                    clip.id = Uuid::now_v7();
                    ids.insert(clip.id);
                }
                clips.push(clip);
            }
            None => skipped += 1,
        }
    }
    // Clips without a usable number get fresh ones above every existing number.
    let mut next = clips
        .iter()
        .map(|c| c.index)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    for clip in clips.iter_mut().filter(|c| c.index == 0) {
        clip.index = next;
        next = next.saturating_add(1);
    }
    (clips, skipped)
}

fn lenient_clip(v: &Value) -> Option<Clip> {
    let mut c = v.as_object()?.clone();
    let start = c.get("start_sec")?.as_f64()?;
    let end = c.get("end_sec")?.as_f64()?;
    if !start.is_finite() || !end.is_finite() {
        return None;
    }
    if !c
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|s| Uuid::parse_str(s).is_ok())
    {
        c.insert("id".into(), json!(Uuid::now_v7()));
    }
    if !c
        .get("index")
        .and_then(Value::as_u64)
        .is_some_and(|n| u32::try_from(n).is_ok())
    {
        c.insert("index".into(), json!(0));
    }
    if !c.get("name").is_some_and(Value::is_string) {
        c.insert("name".into(), json!(""));
    }
    if !c
        .get("tag_id")
        .is_some_and(|t| t.is_string() || t.is_null())
    {
        c.insert("tag_id".into(), Value::Null);
    }
    if !c.get("crop_x_offset").is_some_and(Value::is_number) {
        c.insert("crop_x_offset".into(), json!(0.0));
    }
    if !c.get("selected_for_export").is_some_and(Value::is_boolean) {
        c.insert("selected_for_export".into(), json!(true));
    }
    let status_ok = c
        .get("export_status")
        .is_some_and(|s| serde_json::from_value::<rustcut_model::ExportStatus>(s.clone()).is_ok());
    if !status_ok {
        c.insert("export_status".into(), json!("pending"));
    }
    let last_ok = c.get("last_export").is_some_and(|l| {
        l.is_null() || serde_json::from_value::<rustcut_model::LastExport>(l.clone()).is_ok()
    });
    if !last_ok {
        c.insert("last_export".into(), Value::Null);
    }
    serde_json::from_value(Value::Object(c)).ok()
}
