//! FRM-08: platform presets live in an updatable JSON file. A valid
//! `<data_dir>/platform-presets.json` wins; otherwise the bundled defaults are
//! used and the caller gets a warning explaining why.

use rustcut_model::PlatformPreset;
use serde::Deserialize;
use std::path::Path;

/// File name looked up inside the app data directory.
pub const PLATFORM_PRESETS_FILE: &str = "platform-presets.json";

/// Defaults shipped with the app (limits change: see the note inside).
pub const BUNDLED_PLATFORM_PRESETS: &str = include_str!("../data/platform-presets.json");

#[derive(Deserialize)]
struct PresetFile {
    presets: Vec<PlatformPreset>,
}

/// Parse a preset file (`{ "note": …, "presets": [ … ] }` or a bare array).
/// Rejects an empty list, blank/duplicate ids and non-positive limits.
pub fn parse_platform_presets(json: &str) -> Result<Vec<PlatformPreset>, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let presets: Vec<PlatformPreset> = if value.is_array() {
        serde_json::from_value(value).map_err(|e| e.to_string())?
    } else {
        serde_json::from_value::<PresetFile>(value)
            .map_err(|e| e.to_string())?
            .presets
    };
    if presets.is_empty() {
        return Err("the file lists no presets".into());
    }
    let mut seen = std::collections::HashSet::new();
    for p in &presets {
        if p.id.trim().is_empty() || p.label.trim().is_empty() {
            return Err("every preset needs a non-empty id and label".into());
        }
        if !seen.insert(p.id.as_str()) {
            return Err(format!("duplicate preset id \"{}\"", p.id));
        }
        if p.max_duration_sec
            .is_some_and(|d| !d.is_finite() || d <= 0.0)
        {
            return Err(format!(
                "preset \"{}\" has an invalid max_duration_sec",
                p.id
            ));
        }
        if p.fps_cap == Some(0) {
            return Err(format!("preset \"{}\" has an invalid fps_cap", p.id));
        }
    }
    Ok(presets)
}

/// The bundled default presets.
pub fn bundled_platform_presets() -> Vec<PlatformPreset> {
    parse_platform_presets(BUNDLED_PLATFORM_PRESETS)
        .expect("bundled platform-presets.json is valid (checked by a unit test)")
}

/// Load the presets: `<data_dir>/platform-presets.json` when present and valid
/// (no warning), else the bundled list. A present-but-unusable file yields the
/// bundled list plus a warning; an absent file is not a warning.
pub fn load_platform_presets(data_dir: &Path) -> (Vec<PlatformPreset>, Option<String>) {
    let path = data_dir.join(PLATFORM_PRESETS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (bundled_platform_presets(), None);
        }
        Err(e) => {
            return (
                bundled_platform_presets(),
                Some(format!(
                    "Could not read {} ({e}); using the built-in platform presets.",
                    path.display()
                )),
            );
        }
    };
    match parse_platform_presets(&text) {
        Ok(presets) => (presets, None),
        Err(reason) => (
            bundled_platform_presets(),
            Some(format!(
                "{} is not a valid preset file ({reason}); using the built-in platform presets.",
                path.display()
            )),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcut_model::{Framing, OutputResolution};

    #[test]
    fn bundled_defaults_are_valid_and_match_the_prd() {
        let p = bundled_platform_presets();
        let by = |id: &str| p.iter().find(|x| x.id == id).unwrap().clone();
        assert_eq!(by("youtube_shorts").max_duration_sec, Some(180.0));
        assert_eq!(by("tiktok").max_duration_sec, Some(600.0));
        assert_eq!(by("instagram_reels").max_duration_sec, Some(180.0));
        assert_eq!(by("tiktok").framing, Framing::VerticalCrop);
        assert_eq!(
            by("tiktok").output_resolution,
            OutputResolution::Full1080x1920
        );
    }

    #[test]
    fn missing_file_uses_bundled_without_warning() {
        let dir = tempfile::tempdir().unwrap();
        let (p, w) = load_platform_presets(dir.path());
        assert_eq!(p, bundled_platform_presets());
        assert!(w.is_none());
    }

    #[test]
    fn valid_user_file_wins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(PLATFORM_PRESETS_FILE),
            r#"{"presets":[{"id":"x","label":"X","framing":"vertical_blur","output_resolution":"720x1280","max_duration_sec":90,"fps_cap":null}]}"#,
        )
        .unwrap();
        let (p, w) = load_platform_presets(dir.path());
        assert!(w.is_none());
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].framing, Framing::VerticalBlur);
        assert_eq!(p[0].max_duration_sec, Some(90.0));
    }

    #[test]
    fn invalid_user_file_falls_back_with_warning() {
        let dir = tempfile::tempdir().unwrap();
        for bad in [
            "{not json",
            r#"{"presets":[]}"#,
            r#"[{"id":"a","label":"A","framing":"vertical_crop","output_resolution":"native","max_duration_sec":-1}]"#,
        ] {
            std::fs::write(dir.path().join(PLATFORM_PRESETS_FILE), bad).unwrap();
            let (p, w) = load_platform_presets(dir.path());
            assert_eq!(p, bundled_platform_presets(), "{bad}");
            assert!(w.unwrap().contains("built-in"), "{bad}");
        }
    }
}
