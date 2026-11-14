//! EXP-07 collisions and the Windows path-length edge case (PRD section 9).

use crate::sanitize::{MAX_COMPONENT_BYTES, trim_junk, truncate_bytes};
use rustcut_model::{ConflictPolicy, PlannedAction};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

/// Smallest base name (in characters) path-length truncation may produce.
const MIN_BASE_CHARS: usize = 8;

/// One clip's rendered base name (no extension) awaiting resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameRequest {
    /// The clip.
    pub clip_id: Uuid,
    /// Stable clip number; collisions are resolved in this order.
    pub index: u32,
    /// Sanitized base name from [`crate::render_name`], without extension.
    pub base_name: String,
}

/// The planned output of one clip.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedName {
    /// The clip.
    pub clip_id: Uuid,
    /// Final file name including extension.
    pub file_name: String,
    /// `dir` joined with `file_name`.
    pub path: PathBuf,
    /// What the export will do with the target.
    pub action: PlannedAction,
    /// Set when no valid name exists (path too long even after truncation);
    /// pre-flight turns it into a blocking PathLength error. `action` is then
    /// `Write` and `file_name` is the best-effort truncated name.
    pub error: Option<String>,
}

/// Portable comparison key: NFC then lower-case.
fn key(name: &str) -> String {
    name.nfc().collect::<String>().to_lowercase()
}

fn path_key(p: &Path) -> String {
    key(&p.to_string_lossy().replace('\\', "/"))
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Units of the `.part` suffix of the in-progress file.
const PART_UNITS: usize = 5;
/// Build `base[_suffix].ext` within the 120-byte limit and `max_path_units`.
/// `Err(name)` carries the best-effort name when the path cannot fit.
fn build_name(
    base: &str,
    suffix: Option<u32>,
    ext: &str,
    prefix_units: usize,
    max_path_units: Option<usize>,
) -> Result<String, String> {
    let suffix_s = suffix.map(|n| format!("_{n}")).unwrap_or_default();
    let dot_ext = if ext.is_empty() {
        String::new()
    } else {
        format!(".{ext}")
    };
    let mut base = trim_junk(truncate_bytes(
        base,
        MAX_COMPONENT_BYTES.saturating_sub(suffix_s.len()),
    ))
    .to_owned();
    let mut fits = true;
    if let Some(max) = max_path_units {
        // FFmpeg writes to `<final>.part`: reserve those 5 units too.
        let fixed = prefix_units + utf16_len(&suffix_s) + utf16_len(&dot_ext) + PART_UNITS;
        // The path must stay below `max` units.
        let budget = max.saturating_sub(1).saturating_sub(fixed);
        if utf16_len(&base) > budget {
            let mut units = 0;
            let mut end = 0;
            for (i, c) in base.char_indices() {
                units += c.len_utf16();
                if units > budget {
                    break;
                }
                end = i + c.len_utf8();
            }
            base = trim_junk(&base[..end]).to_owned();
            fits = base.chars().count() >= MIN_BASE_CHARS;
        }
    }
    let name = format!("{base}{suffix_s}{dot_ext}");
    if fits { Ok(name) } else { Err(name) }
}

/// Resolve final output names for a batch (EXP-07).
///
/// * Duplicates inside the batch are compared case-insensitively after NFC and
///   lower-casing. The first clip in clip-number order keeps the name; later
///   ones get `_2`, `_3`, ... Suffixes already taken by another clip in the
///   batch (as its own name) are skipped, and so are, under `Rename`, suffixes
///   that exist on disk.
/// * A file that already exists: if it equals the clip's previous output the
///   action is `ReplacePrevious`; otherwise `Rename` picks the next free
///   `_N`, `Overwrite` plans `Overwrite` and `Skip` plans `Skip`.
/// * Suffixed names keep the 120-byte limit (the base is truncated first).
/// * `max_path_units` (`Some(259)` on Windows without long paths): when
///   `dir + name + ext` reaches that many UTF-16 units the base is truncated
///   further, down to 8 characters; if it still does not fit, the entry gets
///   an `error`.
///
/// `exists` is injected so the function is pure. The result has one entry per
/// request, in request order.
pub fn resolve_outputs(
    requests: &[NameRequest],
    ext: &str,
    dir: &Path,
    policy: ConflictPolicy,
    previous_outputs: &HashMap<Uuid, PathBuf>,
    exists: &dyn Fn(&Path) -> bool,
    max_path_units: Option<usize>,
) -> Vec<ResolvedName> {
    let ext = ext.trim_start_matches('.');
    let prefix_units = utf16_len(&dir.join("x").to_string_lossy()).saturating_sub(1);

    let mut order: Vec<usize> = (0..requests.len()).collect();
    order.sort_by_key(|&i| requests[i].index);

    // Names every clip would take without a suffix: later suffixes avoid them.
    let naturals: HashSet<String> = requests
        .iter()
        .map(|r| {
            key(
                &build_name(&r.base_name, None, ext, prefix_units, max_path_units)
                    .unwrap_or_else(|n| n),
            )
        })
        .collect();

    let mut assigned: HashSet<String> = HashSet::new();
    let mut results: Vec<Option<ResolvedName>> = vec![None; requests.len()];
    for i in order {
        let req = &requests[i];
        let resolved = resolve_one(
            req,
            ext,
            dir,
            policy,
            previous_outputs,
            exists,
            max_path_units,
            prefix_units,
            &naturals,
            &assigned,
        );
        assigned.insert(key(&resolved.file_name));
        results[i] = Some(resolved);
    }
    results.into_iter().flatten().collect()
}

#[allow(clippy::too_many_arguments)]
fn resolve_one(
    req: &NameRequest,
    ext: &str,
    dir: &Path,
    policy: ConflictPolicy,
    previous_outputs: &HashMap<Uuid, PathBuf>,
    exists: &dyn Fn(&Path) -> bool,
    max_path_units: Option<usize>,
    prefix_units: usize,
    naturals: &HashSet<String>,
    assigned: &HashSet<String>,
) -> ResolvedName {
    let previous = previous_outputs.get(&req.clip_id).map(|p| path_key(p));
    let make = |file_name: String, action, error| ResolvedName {
        clip_id: req.clip_id,
        path: dir.join(&file_name),
        file_name,
        action,
        error,
    };
    let mut n: u32 = 1;
    loop {
        let suffix = (n > 1).then_some(n);
        let (name, fit) =
            match build_name(&req.base_name, suffix, ext, prefix_units, max_path_units) {
                Ok(name) => (name, true),
                Err(name) => (name, false),
            };
        if !fit {
            let msg = format!(
                "The output path for clip {} would be {} characters or longer, which Windows cannot handle even after shortening the name. Choose a shorter output folder.",
                req.index,
                max_path_units.unwrap_or(0)
            );
            return make(name, PlannedAction::Write, Some(msg));
        }
        let k = key(&name);
        let taken = assigned.contains(&k) || (n > 1 && naturals.contains(&k));
        if !taken {
            let path = dir.join(&name);
            if !exists(&path) {
                return make(name, PlannedAction::Write, None);
            }
            if previous.as_deref() == Some(path_key(&path).as_str()) {
                return make(name, PlannedAction::ReplacePrevious, None);
            }
            match policy {
                ConflictPolicy::Overwrite => return make(name, PlannedAction::Overwrite, None),
                ConflictPolicy::Skip => return make(name, PlannedAction::Skip, None),
                ConflictPolicy::Rename => {}
            }
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(index: u32, base: &str) -> NameRequest {
        NameRequest {
            clip_id: Uuid::from_u128(index as u128),
            index,
            base_name: base.into(),
        }
    }

    fn run(
        reqs: &[NameRequest],
        policy: ConflictPolicy,
        existing: &[&str],
        prev: &HashMap<Uuid, PathBuf>,
        max: Option<usize>,
    ) -> Vec<ResolvedName> {
        let dir = Path::new("out");
        let set: HashSet<PathBuf> = existing.iter().map(|n| dir.join(n)).collect();
        resolve_outputs(reqs, "mp4", dir, policy, prev, &|p| set.contains(p), max)
    }

    fn names(r: &[ResolvedName]) -> Vec<&str> {
        r.iter().map(|x| x.file_name.as_str()).collect()
    }

    #[test]
    fn unique_names_are_written_as_is() {
        let r = run(
            &[req(1, "a"), req(2, "b")],
            ConflictPolicy::Rename,
            &[],
            &HashMap::new(),
            None,
        );
        assert_eq!(names(&r), ["a.mp4", "b.mp4"]);
        assert!(
            r.iter()
                .all(|x| x.action == PlannedAction::Write && x.error.is_none())
        );
        assert_eq!(r[0].path, Path::new("out").join("a.mp4"));
    }

    #[test]
    fn exp07_in_batch_duplicates_in_clip_number_order() {
        // Requests arrive out of order; clip 1 keeps the name.
        let r = run(
            &[req(3, "Same"), req(1, "Same"), req(2, "Same")],
            ConflictPolicy::Rename,
            &[],
            &HashMap::new(),
            None,
        );
        assert_eq!(names(&r), ["Same_3.mp4", "Same.mp4", "Same_2.mp4"]);
    }

    #[test]
    fn duplicates_are_case_insensitive_and_nfc() {
        let r = run(
            &[
                req(1, "Caf\u{e9}"),
                req(2, "CAFE\u{301}"),
                req(3, "cafe\u{301}"),
            ],
            ConflictPolicy::Rename,
            &[],
            &HashMap::new(),
            None,
        );
        assert_eq!(r[1].file_name, "CAFE\u{301}_2.mp4");
        assert_eq!(r[2].file_name, "cafe\u{301}_3.mp4");
    }

    #[test]
    fn suffix_skips_names_taken_by_other_clips() {
        let r = run(
            &[req(1, "foo"), req(2, "foo"), req(3, "foo_2")],
            ConflictPolicy::Rename,
            &[],
            &HashMap::new(),
            None,
        );
        assert_eq!(names(&r), ["foo.mp4", "foo_3.mp4", "foo_2.mp4"]);
    }

    #[test]
    fn rename_skips_existing_files() {
        let r = run(
            &[req(1, "a"), req(2, "a")],
            ConflictPolicy::Rename,
            &["a.mp4", "a_2.mp4"],
            &HashMap::new(),
            None,
        );
        assert_eq!(names(&r), ["a_3.mp4", "a_4.mp4"]);
    }

    #[test]
    fn overwrite_and_skip_policies() {
        let r = run(
            &[req(1, "a"), req(2, "b")],
            ConflictPolicy::Overwrite,
            &["a.mp4"],
            &HashMap::new(),
            None,
        );
        assert_eq!(
            (r[0].action, r[0].file_name.as_str()),
            (PlannedAction::Overwrite, "a.mp4")
        );
        assert_eq!(r[1].action, PlannedAction::Write);
        let r = run(
            &[req(1, "a"), req(2, "b")],
            ConflictPolicy::Skip,
            &["a.mp4"],
            &HashMap::new(),
            None,
        );
        assert_eq!(
            (r[0].action, r[0].file_name.as_str()),
            (PlannedAction::Skip, "a.mp4")
        );
    }

    #[test]
    fn in_batch_duplicate_under_overwrite_still_gets_suffix() {
        let r = run(
            &[req(1, "a"), req(2, "a")],
            ConflictPolicy::Overwrite,
            &["a.mp4"],
            &HashMap::new(),
            None,
        );
        assert_eq!(names(&r), ["a.mp4", "a_2.mp4"]);
        assert_eq!(r[0].action, PlannedAction::Overwrite);
        assert_eq!(r[1].action, PlannedAction::Write);
    }

    #[test]
    fn previous_output_is_replaced_under_every_policy() {
        let r1 = req(1, "a");
        let mut prev = HashMap::new();
        prev.insert(r1.clip_id, Path::new("out").join("a.mp4"));
        for policy in [
            ConflictPolicy::Rename,
            ConflictPolicy::Overwrite,
            ConflictPolicy::Skip,
        ] {
            let r = run(std::slice::from_ref(&r1), policy, &["a.mp4"], &prev, None);
            assert_eq!(
                (r[0].action, r[0].file_name.as_str()),
                (PlannedAction::ReplacePrevious, "a.mp4"),
                "{policy:?}"
            );
        }
    }

    #[test]
    fn previous_output_compared_case_insensitively() {
        let r1 = req(1, "A");
        let mut prev = HashMap::new();
        prev.insert(r1.clip_id, PathBuf::from("OUT\\a.mp4"));
        let r = run(
            std::slice::from_ref(&r1),
            ConflictPolicy::Rename,
            &["A.mp4"],
            &prev,
            None,
        );
        assert_eq!(r[0].action, PlannedAction::ReplacePrevious);
    }

    #[test]
    fn previous_suffixed_output_is_found_again_under_rename() {
        // Clip 2 exported as a_2.mp4 last time; a.mp4 belongs to clip 1.
        let reqs = [req(1, "a"), req(2, "a")];
        let mut prev = HashMap::new();
        prev.insert(reqs[0].clip_id, Path::new("out").join("a.mp4"));
        prev.insert(reqs[1].clip_id, Path::new("out").join("a_2.mp4"));
        let r = run(
            &reqs,
            ConflictPolicy::Rename,
            &["a.mp4", "a_2.mp4"],
            &prev,
            None,
        );
        assert_eq!(names(&r), ["a.mp4", "a_2.mp4"]);
        assert!(r.iter().all(|x| x.action == PlannedAction::ReplacePrevious));
    }

    #[test]
    fn another_clips_previous_file_is_not_replaced() {
        let reqs = [req(1, "a")];
        let mut prev = HashMap::new();
        prev.insert(Uuid::from_u128(99), Path::new("out").join("a.mp4"));
        let r = run(&reqs, ConflictPolicy::Rename, &["a.mp4"], &prev, None);
        assert_eq!(r[0].file_name, "a_2.mp4");
    }

    #[test]
    fn suffix_keeps_120_byte_limit() {
        let base = "x".repeat(120);
        let r = run(
            &[req(1, &base), req(2, &base)],
            ConflictPolicy::Rename,
            &[],
            &HashMap::new(),
            None,
        );
        assert_eq!(r[0].file_name, format!("{base}.mp4"));
        assert_eq!(r[1].file_name, format!("{}_2.mp4", "x".repeat(118)));
        // Multi-byte base truncated on a char boundary before the suffix.
        let fire = "\u{1f525}".repeat(30);
        let r = run(
            &[req(1, &fire), req(2, &fire)],
            ConflictPolicy::Rename,
            &[],
            &HashMap::new(),
            None,
        );
        let stem2 = r[1].file_name.trim_end_matches(".mp4");
        assert!(stem2.len() <= 120 && stem2.ends_with("_2"), "{stem2}");
        assert_eq!(stem2.chars().filter(|c| *c == '\u{1f525}').count(), 29);
    }

    #[test]
    fn long_path_is_truncated() {
        let base = "y".repeat(100);
        let dir = Path::new("out");
        let prefix = utf16_len(&dir.join("x").to_string_lossy()) - 1;
        let max = prefix + 40 + ".mp4".len() + 1 + 5; // allows a 40-char base
        let r = run(
            &[req(1, &base)],
            ConflictPolicy::Rename,
            &[],
            &HashMap::new(),
            Some(max),
        );
        assert!(r[0].error.is_none());
        assert_eq!(r[0].file_name, format!("{}.mp4", "y".repeat(40)));
        assert!(utf16_len(&r[0].path.to_string_lossy()) < max);
    }

    #[test]
    fn long_path_suffix_still_fits() {
        let base = "y".repeat(100);
        let dir = Path::new("out");
        let prefix = utf16_len(&dir.join("x").to_string_lossy()) - 1;
        let max = prefix + 40 + 4 + 1 + 5;
        let r = run(
            &[req(1, &base), req(2, &base)],
            ConflictPolicy::Rename,
            &[],
            &HashMap::new(),
            Some(max),
        );
        assert_eq!(r[1].file_name, format!("{}_2.mp4", "y".repeat(38)));
        assert!(utf16_len(&r[1].path.to_string_lossy()) < max);
    }

    #[test]
    fn path_exactly_at_limit_is_too_long() {
        let dir = Path::new("out");
        let prefix = utf16_len(&dir.join("x").to_string_lossy()) - 1;
        // A 10-char base gives a path of prefix + 14 units (+5 for `.part`).
        let r = run(
            &[req(1, "abcdefghij")],
            ConflictPolicy::Rename,
            &[],
            &HashMap::new(),
            Some(prefix + 19),
        );
        assert_eq!(r[0].file_name, "abcdefghi.mp4");
        let r = run(
            &[req(1, "abcdefghij")],
            ConflictPolicy::Rename,
            &[],
            &HashMap::new(),
            Some(prefix + 20),
        );
        assert_eq!(r[0].file_name, "abcdefghij.mp4");
    }

    #[test]
    fn part_file_path_stays_below_the_limit() {
        let base = "z".repeat(100);
        let dir = Path::new("out");
        let prefix = utf16_len(&dir.join("x").to_string_lossy()) - 1;
        for max in [prefix + 109, prefix + 110, prefix + 111] {
            let r = run(
                &[req(1, &base)],
                ConflictPolicy::Rename,
                &[],
                &HashMap::new(),
                Some(max),
            );
            assert!(r[0].error.is_none());
            let part = utf16_len(&r[0].path.to_string_lossy()) + ".part".len();
            assert!(part < max, "{part} >= {max}");
        }
    }

    #[test]
    fn path_too_long_even_when_truncated_is_an_error() {
        let dir = Path::new("out");
        let prefix = utf16_len(&dir.join("x").to_string_lossy()) - 1;
        let max = prefix + 4 + 5 + 1 + 5; // room for only 5 characters
        let r = run(
            &[req(1, "a_long_name")],
            ConflictPolicy::Rename,
            &[],
            &HashMap::new(),
            Some(max),
        );
        let err = r[0].error.as_deref().expect("error expected");
        assert!(
            err.contains("clip 1") && err.contains("shorter output folder"),
            "{err}"
        );
        assert_eq!(r[0].action, PlannedAction::Write);
    }

    #[test]
    fn short_name_that_cannot_fit_is_an_error() {
        let r = run(
            &[req(1, "abc")],
            ConflictPolicy::Rename,
            &[],
            &HashMap::new(),
            Some(5),
        );
        assert!(r[0].error.is_some());
    }

    #[test]
    fn truncation_to_equal_names_is_deduplicated() {
        let dir = Path::new("out");
        let prefix = utf16_len(&dir.join("x").to_string_lossy()) - 1;
        let max = prefix + 12 + 4 + 1 + 5;
        let r = run(
            &[req(1, "same_prefix_one"), req(2, "same_prefix_two")],
            ConflictPolicy::Rename,
            &[],
            &HashMap::new(),
            Some(max),
        );
        assert_eq!(r[0].file_name, "same_prefix.mp4");
        assert_eq!(r[1].file_name, "same_prefi_2.mp4");
    }

    #[test]
    fn extension_may_include_dot_and_empty_requests_work() {
        let r = resolve_outputs(
            &[req(1, "a")],
            ".mkv",
            Path::new("o"),
            ConflictPolicy::Rename,
            &HashMap::new(),
            &|_| false,
            None,
        );
        assert_eq!(r[0].file_name, "a.mkv");
        assert!(
            resolve_outputs(
                &[],
                "mp4",
                Path::new("o"),
                ConflictPolicy::Rename,
                &HashMap::new(),
                &|_| false,
                None
            )
            .is_empty()
        );
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn never_two_case_insensitively_equal_names(
                bases in proptest::collection::vec("[a-cA-C_0-9\u{e9}]{1,4}", 1..12),
                existing in proptest::collection::vec("[a-cA-C_0-9]{1,4}(_[2-4])?", 0..6),
                policy in 0u8..3,
            ) {
                let reqs: Vec<_> = bases.iter().enumerate().map(|(i, b)| req(i as u32 + 1, b)).collect();
                let ex: Vec<String> = existing.iter().map(|e| format!("{e}.mp4")).collect();
                let ex_refs: Vec<&str> = ex.iter().map(String::as_str).collect();
                let policy = [ConflictPolicy::Rename, ConflictPolicy::Overwrite, ConflictPolicy::Skip][policy as usize];
                let r = run(&reqs, policy, &ex_refs, &HashMap::new(), None);
                let mut seen = HashSet::new();
                for x in &r {
                    prop_assert!(seen.insert(key(&x.file_name)), "duplicate {}", x.file_name);
                    prop_assert!(x.file_name.len() <= 120 + 4);
                }
            }
        }
    }
}
