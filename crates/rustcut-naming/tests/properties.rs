//! Property tests (EXP-05/06/07) and the PRD preview example.

use chrono::NaiveDate;
use proptest::prelude::*;
use rustcut_model::{ExportSettings, Preset, Project};
use rustcut_naming::{NameContext, render_name, sanitize_component};

fn is_reserved(s: &str) -> bool {
    let stem = s
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_uppercase();
    let com_lpt = |p: &str| {
        stem.strip_prefix(p).is_some_and(|r| {
            let mut it = r.chars();
            matches!(
                (it.next(), it.next()),
                (Some('1'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'), None)
            )
        })
    };
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || com_lpt("COM") || com_lpt("LPT")
}

fn arb_input() -> impl Strategy<Value = String> {
    prop_oneof![
        r"\PC{0,200}",
        "[ a-zA-Z0-9_.<>:\"/\\|?*\t\n-]{0,80}",
        prop::sample::select(vec![
            "con",
            "aux",
            "COM1",
            "lpt9",
            "nul",
            "prn",
            "com\u{b9}"
        ])
        .prop_flat_map(|r| {
            ("[ ._]{0,3}", "[ ._a-z]{0,6}").prop_map(move |(a, b)| format!("{a}{r}{b}"))
        }),
    ]
}

proptest! {
    #[test]
    fn sanitized_is_safe(input in arb_input(), spaces in any::<bool>()) {
        let out = sanitize_component(&input, spaces);
        prop_assert!(out.len() <= 120, "{} bytes", out.len());
        prop_assert!(!out.chars().any(|c| r#"<>:"/\|?*"#.contains(c) || c.is_control()), "{out:?}");
        prop_assert!(!is_reserved(&out), "{out:?}");
        if spaces {
            prop_assert!(!out.chars().any(char::is_whitespace), "{out:?}");
        }
    }

    #[test]
    fn sanitize_is_idempotent(input in arb_input(), spaces in any::<bool>()) {
        let once = sanitize_component(&input, spaces);
        prop_assert_eq!(sanitize_component(&once, spaces), once);
    }

    #[test]
    fn rendered_names_are_never_empty(name in arb_input(), tag in arb_input(), index in 1u32..5000, spaces in any::<bool>()) {
        let ctx = NameContext {
            index,
            index_width: 2,
            name: &name,
            source_name: &tag,
            start_sec: 1.0,
            end_sec: 9.0,
            date: NaiveDate::from_ymd_opt(2026, 11, 13).unwrap(),
            tag_label: Some(&tag),
            preset: Preset::Lossless,
        };
        for t in ["{name}", "{tag}", "{source_name}", "{index}_{name}", "{tag}.{tag}"] {
            let out = render_name(t, &ctx, spaces).unwrap();
            prop_assert!(!out.is_empty());
            prop_assert!(out.len() <= 120);
            prop_assert!(!is_reserved(&out));
        }
    }
}

#[test]
fn preview_names_first_three_follow_template_and_dedupe() {
    use rustcut_model::{Clip, ExportStatus};
    let sample: Project =
        serde_json::from_str(include_str!("../../rustcut-model/tests/prd_sample.rustcut")).unwrap();
    let mut p = sample;
    p.clips.clear();
    for (i, name) in ["Jump Scare Reaction", "jump scare reaction", "???", "Last"]
        .iter()
        .enumerate()
    {
        p.clips.push(Clip {
            id: uuid::Uuid::from_u128(i as u128 + 1),
            index: i as u32 + 1,
            name: (*name).into(),
            start_sec: 10.0 * i as f64,
            end_sec: 10.0 * i as f64 + 5.0,
            tag_id: None,
            crop_x_offset: 0.5,
            selected_for_export: true,
            export_status: ExportStatus::Pending,
            last_export: None,
        });
    }
    let mut settings: ExportSettings = p.export_settings.clone();
    settings.naming_template = "{name}".into();
    let ids: Vec<_> = p.clips.iter().map(|c| c.id).collect();
    let date = NaiveDate::from_ymd_opt(2026, 11, 13).unwrap();
    let names = rustcut_naming::preview_names(&p, &ids, &settings, "mp4", date);
    assert_eq!(
        names,
        [
            "Jump_Scare_Reaction.mp4",
            "jump_scare_reaction_2.mp4",
            "Clip_03.mp4",
            "Last.mp4"
        ]
    );
    settings.naming_template = "{bogus}".into();
    assert!(rustcut_naming::preview_names(&p, &ids, &settings, ".mp4", date).is_empty());
    settings.naming_template = "{index}_{name}".into();
    let names = rustcut_naming::preview_names(&p, &ids[..1], &settings, "mkv", date);
    assert_eq!(names, ["01_Jump_Scare_Reaction.mkv"]);
}
