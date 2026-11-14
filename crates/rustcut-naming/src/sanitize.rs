//! EXP-06 sanitization, identical on every OS so files stay portable.

use unicode_normalization::UnicodeNormalization;

/// Maximum size of one sanitized component in UTF-8 bytes (EXP-06 step 6).
pub const MAX_COMPONENT_BYTES: usize = 120;

/// True for characters EXP-06 step 2 replaces with `_`.
fn is_forbidden(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control()
}

fn is_junk(c: char) -> bool {
    matches!(c, '_' | '.' | ' ')
}

/// Step 4 trim: `_`, `.` and spaces from both ends.
pub(crate) fn trim_junk(s: &str) -> &str {
    s.trim_matches(is_junk)
}

/// Windows reserved device names, also the superscript-digit variants.
fn is_reserved(stem: &str) -> bool {
    let upper = stem.to_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    for prefix in ["COM", "LPT"] {
        if let Some(rest) = upper.strip_prefix(prefix) {
            let mut it = rest.chars();
            if let (Some(d), None) = (it.next(), it.next()) {
                return matches!(d, '1'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}');
            }
        }
    }
    false
}

/// Step 5: append `_` to a reserved base name (the part before the first dot).
fn protect_reserved(s: String) -> String {
    let stem_end = s.find('.').unwrap_or(s.len());
    let stem = s[..stem_end].trim_end_matches(' ');
    if is_reserved(stem) {
        let k = stem.len();
        format!("{}_{}", &s[..k], &s[k..])
    } else {
        s
    }
}

/// Step 6: cut to `max` bytes on a character boundary.
pub(crate) fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Sanitize one file-name component, EXP-06 steps 1-6 in order:
///
/// 1. NFC; 2. `< > : " / \ | ? *` and control characters become `_`;
/// 3. Unicode whitespace becomes `_` when `spaces_to_underscores`;
/// 4. repeated `_` collapse and `_`, `.`, space are trimmed from both ends;
/// 5. Windows reserved names get `_` appended (also before a dot);
/// 6. truncate to [`MAX_COMPONENT_BYTES`] bytes, then re-trim.
///
/// The result may be empty; step 7 (`Clip_{index}`) belongs to rendering.
/// The function is idempotent and never returns a reserved name.
pub fn sanitize_component(input: &str, spaces_to_underscores: bool) -> String {
    let mut mapped = String::with_capacity(input.len());
    let mut last_underscore = false;
    for c in input.nfc() {
        let c = if is_forbidden(c) || (spaces_to_underscores && c.is_whitespace()) {
            '_'
        } else {
            c
        };
        if c == '_' {
            if last_underscore {
                continue;
            }
            last_underscore = true;
        } else {
            last_underscore = false;
        }
        mapped.push(c);
    }
    let protected = protect_reserved(trim_junk(&mapped).to_owned());
    let cut = trim_junk(truncate_bytes(&protected, MAX_COMPONENT_BYTES));
    if cut.len() == protected.len() {
        protected
    } else {
        // Truncation can expose a reserved stem (e.g. `aux` cut from `aux .x`).
        protect_reserved(cut.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> String {
        sanitize_component(x, true)
    }

    #[test]
    fn exp06_table_vectors() {
        assert_eq!(s("Jump Scare Reaction"), "Jump_Scare_Reaction");
        assert_eq!(s("What?! Epic/Win: 100%"), "What_!_Epic_Win_100%");
        assert_eq!(s("  ...Trailing dots...  "), "Trailing_dots");
        assert_eq!(s("con"), "con_");
        assert_eq!(s("aux.final"), "aux_.final");
        assert_eq!(
            s("Rekaman: Bahas Patch 2.0 \u{1f525}"),
            "Rekaman_Bahas_Patch_2.0_\u{1f525}"
        );
    }

    #[test]
    fn question_marks_become_empty() {
        // Step 7 (Clip_07) is covered in the template tests.
        assert_eq!(s("???"), "");
    }

    #[test]
    fn reserved_names_all_variants() {
        for n in [
            "CON",
            "prn",
            "Aux",
            "NUL",
            "com1",
            "COM9",
            "lpt1",
            "LPT9",
            "COM\u{b9}",
            "lpt\u{b2}",
            "com\u{b3}",
        ] {
            assert_eq!(s(n), format!("{n}_"), "{n}");
            assert_eq!(s(&format!("{n}.txt")), format!("{n}_.txt"));
        }
        // Not reserved.
        assert_eq!(s("com0"), "com0");
        assert_eq!(s("com10"), "com10");
        assert_eq!(s("console"), "console");
        assert_eq!(s("my.con"), "my.con");
    }

    #[test]
    fn reserved_after_trim_and_with_space() {
        assert_eq!(s("..con.."), "con_");
        assert_eq!(sanitize_component("aux .x", false), "aux_ .x");
    }

    #[test]
    fn nfc_normalisation() {
        assert_eq!(s("e\u{301}"), "\u{e9}");
    }

    #[test]
    fn control_and_forbidden_chars() {
        assert_eq!(s("a<b>c\"d|e\\f\tg\nh\u{0}i"), "a_b_c_d_e_f_g_h_i");
    }

    #[test]
    fn spaces_setting_off_keeps_spaces() {
        assert_eq!(
            sanitize_component("Jump Scare  Reaction", false),
            "Jump Scare  Reaction"
        );
        assert_eq!(sanitize_component("  a/b  ", false), "a_b");
    }

    #[test]
    fn repeated_underscores_collapse() {
        assert_eq!(s("a___b__ c"), "a_b_c");
    }

    #[test]
    fn truncates_on_char_boundary_to_120_bytes() {
        let long = "\u{1f525}".repeat(100); // 4 bytes each
        let out = s(&long);
        assert_eq!(out.len(), 120);
        assert!(out.chars().all(|c| c == '\u{1f525}'));
        let ascii = "a".repeat(300);
        assert_eq!(s(&ascii).len(), 120);
        // 119 ASCII + a 2-byte char straddling the limit.
        let straddle = format!("{}\u{e9}", "a".repeat(119));
        assert_eq!(s(&straddle), "a".repeat(119));
    }

    #[test]
    fn truncation_retrims() {
        let input = format!("{}_tail", "a".repeat(119));
        assert_eq!(s(&input), "a".repeat(119));
        let dots = format!("{}...x", "a".repeat(117));
        assert_eq!(s(&dots), "a".repeat(117));
    }

    #[test]
    fn idempotent_on_samples() {
        for x in [
            "con",
            "aux.final",
            "  ...Trailing dots...  ",
            "a__b",
            "COM1.",
            "\u{1f525}",
        ] {
            let once = s(x);
            assert_eq!(s(&once), once, "{x}");
        }
    }
}
