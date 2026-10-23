//! Output file naming: template expansion + sanitization (PRD 4.4 / 9).
//!
//! Template variables: `{index}`, `{clip_name}` (alias `{name}`),
//! `{source_name}`, `{start_time}`, `{end_time}`, `{duration}`.
//! Unknown placeholders are left untouched so the user notices them.

use crate::timecode;

#[derive(Debug, Clone, Copy)]
pub struct NamingContext<'a> {
    pub index: u32,
    pub clip_name: &'a str,
    pub source_name: &'a str,
    pub start_sec: f64,
    pub end_sec: f64,
}

/// Expands the naming template. `{index}` is zero-padded to 2 digits
/// (`01`, `02`, ...), matching the PRD example
/// `{index}_{clip_name}.mp4 -> 01_Jump_Scare_Reaction.mp4`.
pub fn expand_template(template: &str, ctx: &NamingContext) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let key = &after[..close];
                let value = match key {
                    "index" => format!("{:02}", ctx.index),
                    "clip_name" | "name" => ctx.clip_name.to_string(),
                    "source_name" => ctx.source_name.to_string(),
                    "start_time" => timecode::format_tc_filename(ctx.start_sec),
                    "end_time" => timecode::format_tc_filename(ctx.end_sec),
                    "duration" => timecode::format_duration(ctx.end_sec - ctx.start_sec),
                    _ => format!("{{{key}}}"),
                };
                out.push_str(&value);
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Normalization pass: any character outside `[alnum . - _ %]` becomes `_`,
/// runs of `_` collapse, leading/trailing `_` and `.` are trimmed.
///
/// Matches the PRD example: `What?! Epic/Win: 100%` -> `What_Epic_Win_100%`,
/// and `Jump Scare Reaction` -> `Jump_Scare_Reaction`.
pub fn normalize_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pending_sep = false;
    for ch in input.chars() {
        if ch.is_alphanumeric() || matches!(ch, '.' | '-' | '_' | '%') {
            out.push(ch);
            pending_sep = false;
        } else if !pending_sep {
            out.push('_');
            pending_sep = true;
        }
    }
    out.trim_matches(|c| c == '_' || c == '.').to_string()
}

/// Full output-name pipeline: expand template, normalize, then run the
/// `sanitize-filename` crate as a final safety net (reserved device names,
/// trailing dots, length limits). Falls back to `Clip_{index}` when the
/// result would be empty (PRD: blank clip names).
pub fn build_output_stem(template: &str, ctx: &NamingContext) -> String {
    let fallback = format!("Clip_{:02}", ctx.index);
    let expanded = expand_template(template, ctx);
    let normalized = normalize_component(&expanded);
    let candidate = if normalized.is_empty() {
        fallback.clone()
    } else {
        normalized
    };
    let safe = sanitize_filename::sanitize(&candidate);
    if safe.trim().is_empty() {
        fallback
    } else {
        safe
    }
}

/// Appends `_2`, `_3`, ... so batch export never overwrites an earlier clip
/// (PRD: duplicate names must not clobber files).
pub fn dedupe_names(stems: &mut [String]) {
    use std::collections::HashSet;
    let mut used: HashSet<String> = HashSet::new();
    for stem in stems.iter_mut() {
        let base = stem.clone();
        if used.insert(base.clone()) {
            continue;
        }
        let mut n = 2u32;
        loop {
            let candidate = format!("{base}_{n}");
            if used.insert(candidate.clone()) {
                *stem = candidate;
                break;
            }
            n += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(index: u32, clip_name: &'a str, source_name: &'a str) -> NamingContext<'a> {
        NamingContext {
            index,
            clip_name,
            source_name,
            start_sec: 730.0,
            end_sec: 780.0,
        }
    }

    #[test]
    fn expands_the_prd_template_examples() {
        // `{index}_{clip_name}.mp4` -> `01_Jump_Scare_Reaction.mp4`
        let stem = build_output_stem("{index}_{clip_name}", &ctx(1, "Jump Scare Reaction", "livestream"));
        assert_eq!(stem, "01_Jump_Scare_Reaction");

        // `{source_name}_{start_time}_{clip_name}` ->
        // `livestream_00-12-10_Jump_Scare_Reaction.mp4`
        let stem = build_output_stem(
            "{source_name}_{start_time}_{clip_name}",
            &ctx(1, "Jump Scare Reaction", "livestream"),
        );
        assert_eq!(stem, "livestream_00-12-10_Jump_Scare_Reaction");
    }

    #[test]
    fn sanitizes_the_prd_illegal_name_example() {
        // `What?! Epic/Win: 100%` -> `What_Epic_Win_100%`
        assert_eq!(normalize_component("What?! Epic/Win: 100%"), "What_Epic_Win_100%");
    }

    #[test]
    fn keeps_valid_names_intact() {
        assert_eq!(normalize_component("Opening Moment"), "Opening_Moment");
        assert_eq!(normalize_component("jump_scare"), "jump_scare");
        assert_eq!(normalize_component("v2.0-final"), "v2.0-final");
    }

    #[test]
    fn blank_names_fall_back_to_clip_index() {
        let stem = build_output_stem("{clip_name}", &ctx(3, "???", "src"));
        assert_eq!(stem, "Clip_03");
    }

    #[test]
    fn unknown_placeholders_are_kept_visible() {
        assert_eq!(expand_template("{nope}_x", &ctx(1, "a", "b")), "{nope}_x");
    }

    #[test]
    fn dedupe_never_overwrites() {
        let mut names = vec![
            "A".to_string(),
            "A".to_string(),
            "A_2".to_string(),
            "A".to_string(),
        ];
        dedupe_names(&mut names);
        assert_eq!(names, vec!["A", "A_2", "A_2_2", "A_3"]);
    }
}
