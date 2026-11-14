//! EXP-05 naming templates.

use crate::error::{TemplateError, TemplateWarning, VARIABLES};
use crate::resolve::{NameRequest, resolve_outputs};
use crate::sanitize::sanitize_component;
use chrono::NaiveDate;
use rustcut_model::time::{format_hms_dashed, format_whole_seconds};
use rustcut_model::{ConflictPolicy, ExportSettings, Preset, Project};
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

/// Everything a template can refer to for one clip (EXP-05).
#[derive(Debug, Clone)]
pub struct NameContext<'a> {
    /// Stable clip number.
    pub index: u32,
    /// Zero-padding width: `max(2, digits of the highest clip number)`.
    pub index_width: usize,
    /// Raw clip name (sanitized by the renderer).
    pub name: &'a str,
    /// Source file name without extension.
    pub source_name: &'a str,
    /// Clip In, seconds.
    pub start_sec: f64,
    /// Clip Out, seconds.
    pub end_sec: f64,
    /// Export date.
    pub date: NaiveDate,
    /// Tag label, if the clip has a tag.
    pub tag_label: Option<&'a str>,
    /// Export preset.
    pub preset: Preset,
}

impl NameContext<'_> {
    fn padded_index(&self) -> String {
        format!("{:0width$}", self.index, width = self.index_width)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Variable(String),
}

/// Split a template into literals and `{variable}` references, checking braces
/// and variable names.
fn parse(template: &str) -> Result<Vec<Segment>, TemplateError> {
    if template.trim().is_empty() {
        return Err(TemplateError::Empty);
    }
    let mut segments = Vec::new();
    let mut literal = String::new();
    let mut var: Option<String> = None;
    for (i, c) in template.chars().enumerate() {
        let position = i + 1;
        match (c, var.as_mut()) {
            ('{', Some(_)) => return Err(TemplateError::NestedBraces { position }),
            ('{', None) => {
                if !literal.is_empty() {
                    segments.push(Segment::Literal(std::mem::take(&mut literal)));
                }
                var = Some(String::new());
            }
            ('}', Some(_)) => {
                let name = var.take().unwrap_or_default();
                if !VARIABLES.contains(&name.as_str()) {
                    return Err(TemplateError::UnknownVariable { name });
                }
                segments.push(Segment::Variable(name));
            }
            ('}', None) => return Err(TemplateError::UnbalancedBraces { position }),
            (c, Some(v)) => v.push(c),
            (c, None) => literal.push(c),
        }
    }
    if var.is_some() {
        let position = template.chars().count() - var.map_or(0, |v| v.chars().count());
        return Err(TemplateError::UnbalancedBraces { position });
    }
    if !literal.is_empty() {
        segments.push(Segment::Literal(literal));
    }
    Ok(segments)
}

const MEDIA_EXTENSIONS: [&str; 6] = [".mp4", ".mkv", ".mov", ".webm", ".flv", ".ts"];

/// Check a template (EXP-05): errors for an empty template, unknown variables
/// (named in the error) and unbalanced or nested braces; a warning when it ends
/// with a media extension, because the extension is appended automatically.
pub fn validate_template(template: &str) -> Result<Vec<TemplateWarning>, TemplateError> {
    parse(template)?;
    let lower = template.trim_end().to_lowercase();
    Ok(MEDIA_EXTENSIONS
        .iter()
        .find(|e| lower.ends_with(**e))
        .map(|e| TemplateWarning::EndsWithExtension {
            extension: (*e).to_owned(),
        })
        .into_iter()
        .collect())
}

/// Render the base file name (without extension) for one clip.
///
/// Each variable value is sanitized (EXP-06), an empty `{name}` becomes
/// `Clip_{index}`, then the whole rendered string is sanitized and an empty
/// result becomes `Clip_{index}` (EXP-06 step 7).
pub fn render_name(
    template: &str,
    ctx: &NameContext<'_>,
    spaces_to_underscores: bool,
) -> Result<String, TemplateError> {
    let segments = parse(template)?;
    let san = |s: &str| sanitize_component(s, spaces_to_underscores);
    let fallback = format!("Clip_{}", ctx.padded_index());
    let mut out = String::new();
    for seg in &segments {
        match seg {
            Segment::Literal(l) => out.push_str(l),
            Segment::Variable(v) => match v.as_str() {
                "index" => out.push_str(&ctx.padded_index()),
                "name" => {
                    let n = san(ctx.name);
                    out.push_str(if n.is_empty() { &fallback } else { &n });
                }
                "source_name" => out.push_str(&san(ctx.source_name)),
                "start" => out.push_str(&format_hms_dashed(ctx.start_sec)),
                "end" => out.push_str(&format_hms_dashed(ctx.end_sec)),
                "duration" => out.push_str(&format_whole_seconds(ctx.end_sec - ctx.start_sec)),
                "date" => out.push_str(&ctx.date.format("%Y-%m-%d").to_string()),
                "tag" => out.push_str(&san(ctx.tag_label.unwrap_or(""))),
                "preset" => out.push_str(ctx.preset.as_str()),
                _ => {}
            },
        }
    }
    let whole = san(&out);
    Ok(if whole.is_empty() { fallback } else { whole })
}

/// Final file names (with extension) the export dialog previews for the given
/// clips, in clip-number order, with in-batch duplicates already numbered.
/// The dialog shows the first three. An invalid template yields an empty list
/// (the dialog shows [`validate_template`]'s error instead). `ext` may be given
/// with or without the leading dot.
pub fn preview_names(
    project: &Project,
    clip_ids: &[Uuid],
    settings: &ExportSettings,
    ext: &str,
    date: NaiveDate,
) -> Vec<String> {
    let ext = ext.trim_start_matches('.');
    let source_name = project.source.stem();
    let width = project.index_width();
    let mut clips: Vec<_> = project
        .clips
        .iter()
        .filter(|c| clip_ids.contains(&c.id))
        .collect();
    clips.sort_by_key(|c| c.index);
    let mut requests = Vec::with_capacity(clips.len());
    for clip in clips {
        let tag_label = clip
            .tag_id
            .as_deref()
            .and_then(|id| project.tag(id))
            .map(|t| t.label.as_str());
        let ctx = NameContext {
            index: clip.index,
            index_width: width,
            name: &clip.name,
            source_name: &source_name,
            start_sec: clip.start_sec,
            end_sec: if project.source.duration_sec.is_finite()
                && project.source.duration_sec > 0.0
                && clip.end_sec > project.source.duration_sec
            {
                project.source.duration_sec
            } else {
                clip.end_sec
            },
            date,
            tag_label,
            preset: settings.preset(),
        };
        match render_name(
            &settings.naming_template,
            &ctx,
            settings.spaces_to_underscores,
        ) {
            Ok(base_name) => requests.push(NameRequest {
                clip_id: clip.id,
                index: clip.index,
                base_name,
            }),
            Err(_) => return Vec::new(),
        }
    }
    resolve_outputs(
        &requests,
        ext,
        Path::new(""),
        ConflictPolicy::Rename,
        &HashMap::new(),
        &|_| false,
        None,
    )
    .into_iter()
    .map(|r| r.file_name)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(name: &'a str, preset: Preset) -> NameContext<'a> {
        NameContext {
            index: 2,
            index_width: 2,
            name,
            source_name: "livestream_2026_10_21",
            start_sec: 920.12,
            end_sec: 955.8,
            date: NaiveDate::from_ymd_opt(2026, 11, 13).unwrap(),
            tag_label: Some("Funny"),
            preset,
        }
    }

    fn r(t: &str, name: &str) -> String {
        render_name(t, &ctx(name, Preset::Vcrop), true).unwrap()
    }

    #[test]
    fn exp05_default_template() {
        assert_eq!(
            r("{index}_{name}", "Jump Scare Reaction"),
            "02_Jump_Scare_Reaction"
        );
    }

    #[test]
    fn exp05_source_start_name() {
        assert_eq!(
            r("{source_name}_{start}_{name}", "Jump Scare Reaction"),
            "livestream_2026_10_21_00-15-20_Jump_Scare_Reaction"
        );
    }

    #[test]
    fn exp05_preset_variable() {
        assert_eq!(
            r("{index}_{name}_{preset}", "Jump Scare Reaction"),
            "02_Jump_Scare_Reaction_vcrop"
        );
        for (p, s) in [
            (Preset::Lossless, "lossless"),
            (Preset::Accurate, "accurate"),
            (Preset::Vcrop, "vcrop"),
            (Preset::Vblur, "vblur"),
        ] {
            assert_eq!(render_name("{preset}", &ctx("x", p), true).unwrap(), s);
        }
    }

    #[test]
    fn exp05_all_variables() {
        assert_eq!(
            r("{end}|{duration}|{date}|{tag}", "x"),
            "00-15-55_36_2026-11-13_Funny".replace('|', "_")
        );
    }

    #[test]
    fn exp05_tag_empty_when_none() {
        let mut c = ctx("x", Preset::Lossless);
        c.tag_label = None;
        assert_eq!(render_name("{name}_{tag}", &c, true).unwrap(), "x");
    }

    #[test]
    fn exp05_index_width_grows() {
        let mut c = ctx("x", Preset::Lossless);
        c.index = 7;
        c.index_width = 3;
        assert_eq!(render_name("{index}", &c, true).unwrap(), "007");
        c.index = 123;
        c.index_width = 2;
        assert_eq!(render_name("{index}", &c, true).unwrap(), "123");
    }

    #[test]
    fn exp06_step7_question_marks_clip_7() {
        let mut c = ctx("???", Preset::Lossless);
        c.index = 7;
        assert_eq!(render_name("{name}", &c, true).unwrap(), "Clip_07");
        assert_eq!(
            render_name("{index}_{name}", &c, true).unwrap(),
            "07_Clip_07"
        );
    }

    #[test]
    fn whole_name_empty_falls_back_to_clip_index() {
        let mut c = ctx("x", Preset::Lossless);
        c.tag_label = Some("???");
        assert_eq!(render_name("{tag}", &c, true).unwrap(), "Clip_02");
        assert_eq!(render_name("...", &c, true).unwrap(), "Clip_02");
    }

    #[test]
    fn literals_and_values_are_sanitized() {
        assert_eq!(r("{index}/{name}", "a:b"), "02_a_b");
        assert_eq!(r("{name}", "con"), "con_");
    }

    #[test]
    fn spaces_setting_off() {
        assert_eq!(
            render_name("{name}", &ctx("A B", Preset::Lossless), false).unwrap(),
            "A B"
        );
    }

    #[test]
    fn rendered_name_is_limited_to_120_bytes() {
        let long = "x".repeat(200);
        assert_eq!(r("{name}_{name}", &long).len(), 120);
    }

    #[test]
    fn validate_errors() {
        assert_eq!(validate_template(""), Err(TemplateError::Empty));
        assert_eq!(validate_template("   "), Err(TemplateError::Empty));
        assert_eq!(
            validate_template("{index}_{nmae}"),
            Err(TemplateError::UnknownVariable {
                name: "nmae".into()
            })
        );
        assert_eq!(
            validate_template("{}"),
            Err(TemplateError::UnknownVariable {
                name: String::new()
            })
        );
        assert!(matches!(
            validate_template("{index"),
            Err(TemplateError::UnbalancedBraces { .. })
        ));
        assert!(matches!(
            validate_template("index}"),
            Err(TemplateError::UnbalancedBraces { position: 6 })
        ));
        assert!(matches!(
            validate_template("{{index}}"),
            Err(TemplateError::NestedBraces { position: 2 })
        ));
        assert!(matches!(
            validate_template("{a{b}}"),
            Err(TemplateError::NestedBraces { .. })
        ));
        let msg = validate_template("{foo}").unwrap_err().to_string();
        assert!(
            msg.contains("{foo}") && msg.contains("{source_name}"),
            "{msg}"
        );
    }

    #[test]
    fn validate_ok_and_extension_warning() {
        assert_eq!(validate_template("{index}_{name}"), Ok(vec![]));
        assert_eq!(validate_template("static"), Ok(vec![]));
        for ext in [".mp4", ".mkv", ".mov", ".webm", ".flv", ".ts"] {
            let w = validate_template(&format!("{{name}}{ext}")).unwrap();
            assert_eq!(
                w,
                vec![TemplateWarning::EndsWithExtension {
                    extension: ext.into()
                }]
            );
        }
        assert_eq!(validate_template("{name}.MP4").unwrap().len(), 1);
        assert_eq!(validate_template("{name}mp4").unwrap().len(), 0);
    }

    #[test]
    fn render_rejects_invalid_template() {
        assert!(render_name("{bogus}", &ctx("x", Preset::Lossless), true).is_err());
    }
}
