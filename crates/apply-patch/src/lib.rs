//! Patch-based file editing engine, ported from Codex's `apply_patch`.
//!
//! **Pure computation** — no filesystem I/O. The crate parses structured
//! patches (Codex `***` format) and computes new file contents using fuzzy
//! line-matching. All reads and writes are the caller's responsibility,
//! typically routed through an OpenDAL VFS layer.
//!
//! ## Quick start
//!
//! ```
//! use apply_patch::fuzzy_edit;
//!
//! let outcome = fuzzy_edit("foo\nbar\n", "bar", "QUX", false);
//! // outcome produces "foo\nQUX\n"
//! ```
//!
//! For multi-file Codex-format patches:
//!
//! ```
//! use apply_patch::{parse_patch, compute_updated_content};
//!
//! let patch = "*** Begin Patch
//! *** Update File: src/main.rs
//! @@
//!  fn main
//! -    println!(\"old\");
//! +    println!(\"new\");
//! *** End Patch";
//!
//! let args = parse_patch(patch).unwrap();
//! for hunk in &args.hunks {
//!     // caller reads original content from VFS, then:
//!     // let new = compute_updated_content(&original, chunks)?;
//!     // caller writes `new` back to VFS
//! #     let _ = hunk;
//! }
//! ```

mod parser;
pub mod seek_sequence;

use thiserror::Error;

pub use parser::{Hunk, ParseError, UpdateFileChunk, parse_patch};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ApplyPatchError {
    #[error(transparent)]
    ParseError(#[from] ParseError),
    /// Error while computing replacements for an update chunk (e.g. the
    /// chunk's context or old-lines could not be located in the file).
    #[error("{0}")]
    ComputeReplacements(String),
}

// ---------------------------------------------------------------------------
// Parsed patch data model
// ---------------------------------------------------------------------------

/// Parsed patch: the raw text plus the parsed [`Hunk`]s.
#[derive(Debug, PartialEq)]
pub struct ApplyPatchArgs {
    pub patch: String,
    pub hunks: Vec<Hunk>,
    pub environment_id: Option<String>,
}

// ---------------------------------------------------------------------------
// In-memory: compute updated content from chunks
// ---------------------------------------------------------------------------

/// Compute the result of applying update `chunks` to `original`.
///
/// Reads no files — works purely on the in-memory string. Uses
/// [`seek_sequence`](crate::seek_sequence) fuzzy matching (exact → rstrip →
/// trim → Unicode-normalise) to locate each chunk's `old_lines` within the
/// original content, then splices in the `new_lines`.
///
/// The caller is responsible for reading the original file content and
/// writing the result back (typically through OpenDAL VFS).
pub fn compute_updated_content(
    original: &str,
    chunks: &[UpdateFileChunk],
) -> Result<String, ApplyPatchError> {
    let mut original_lines: Vec<String> = original.split('\n').map(String::from).collect();

    // Drop the trailing empty element from the final newline so line counts
    // match standard `diff` behaviour.
    if original_lines.last().is_some_and(String::is_empty) {
        original_lines.pop();
    }

    let replacements = compute_replacements(&original_lines, "(memory)", chunks)?;
    let mut new_lines = apply_replacements(original_lines, &replacements);

    if !new_lines.last().is_some_and(String::is_empty) {
        new_lines.push(String::new());
    }

    Ok(new_lines.join("\n"))
}

// ---------------------------------------------------------------------------
// In-memory: fuzzy single-replacement edit
// ---------------------------------------------------------------------------

/// Outcome of a [`fuzzy_edit`] call.
#[derive(Debug, Clone, PartialEq)]
pub enum FuzzyEditOutcome {
    /// Replacement was applied successfully.
    Replaced {
        new_content: String,
        count: usize,
        /// `true` when at least one match required fuzzy matching (i.e. was
        /// not a byte-for-byte exact line match).
        fuzzy: bool,
    },
    /// `old` was not found in `original`.
    NotFound,
    /// `old` matched multiple locations and `replace_all` was `false`.
    Ambiguous { count: usize },
}

/// Fuzzy-replace `old` with `new` in `original` using line-based matching
/// with progressive tolerance (exact → rstrip → trim → Unicode-normalise).
///
/// Both `old` and `new` are treated as sequences of complete lines. A single
/// trailing newline in either argument is stripped (so `"bar\n"` and `"bar"`
/// are equivalent patterns for a one-line replacement).
/// If a one-line `old` value has no whole-line match, it is also tried as a
/// substring within each physical line.
///
/// When `replace_all` is `false`, exactly one match location is required;
/// multiple matches return [`FuzzyEditOutcome::Ambiguous`].
pub fn fuzzy_edit(original: &str, old: &str, new: &str, replace_all: bool) -> FuzzyEditOutcome {
    if old == new {
        return FuzzyEditOutcome::Replaced {
            new_content: original.to_string(),
            count: 0,
            fuzzy: false,
        };
    }

    let orig_lines: Vec<String> = split_lines(original);
    let pattern_lines: Vec<String> = split_lines(old);
    let new_lines: Vec<String> = split_lines(new);

    if pattern_lines.is_empty() || pattern_lines.len() > orig_lines.len() {
        return FuzzyEditOutcome::NotFound;
    }

    let positions = seek_sequence::find_all_matches(&orig_lines, &pattern_lines);
    let substring_matches = if positions.is_empty() && pattern_lines.len() == 1 {
        find_substring_matches(&orig_lines, &pattern_lines[0])
    } else {
        Vec::new()
    };

    if positions.is_empty() && substring_matches.is_empty() {
        return FuzzyEditOutcome::NotFound;
    }

    let match_count = positions.len() + substring_matches.len();
    if match_count > 1 && !replace_all {
        return FuzzyEditOutcome::Ambiguous { count: match_count };
    }

    let fuzzy = if substring_matches.is_empty() {
        positions
            .iter()
            .any(|&pos| orig_lines[pos..pos + pattern_lines.len()] != pattern_lines[..])
    } else {
        substring_matches.iter().any(|matched| matched.fuzzy)
    };

    // Apply replacements in descending position order so earlier replacements
    // don't shift indices of later ones.
    let mut result = orig_lines.clone();
    if substring_matches.is_empty() {
        let pattern_len = pattern_lines.len();
        for &pos in positions.iter().rev() {
            for _ in 0..pattern_len {
                if pos < result.len() {
                    result.remove(pos);
                }
            }
            for (offset, line) in new_lines.iter().enumerate() {
                result.insert(pos + offset, line.clone());
            }
        }
    } else {
        let mut by_line: Vec<Vec<SubstringMatch>> = vec![Vec::new(); result.len()];
        for matched in substring_matches {
            by_line[matched.line_index].push(matched);
        }

        for (line_index, matches) in by_line.into_iter().enumerate().rev() {
            if matches.is_empty() {
                continue;
            }
            let line = result[line_index].clone();
            let replacement = splice_substring(&line, &new_lines, &matches);
            result.splice(line_index..line_index + 1, replacement);
        }
    }

    if !result.last().is_some_and(String::is_empty) {
        result.push(String::new());
    }

    FuzzyEditOutcome::Replaced {
        new_content: result.join("\n"),
        count: match_count,
        fuzzy,
    }
}

#[derive(Clone, Copy)]
struct SubstringMatch {
    line_index: usize,
    start: usize,
    end: usize,
    fuzzy: bool,
}

fn find_substring_matches(lines: &[String], pattern: &str) -> Vec<SubstringMatch> {
    if pattern.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        let mut search_from = 0;
        while let Some(offset) = line[search_from..].find(pattern) {
            let start = search_from + offset;
            let end = start + pattern.len();
            matches.push(SubstringMatch {
                line_index,
                start,
                end,
                fuzzy: false,
            });
            search_from = end;
        }
    }

    if !matches.is_empty() {
        return matches;
    }

    let needle = seek_sequence::normalised_spans(pattern);
    if needle.is_empty() {
        return matches;
    }

    for (line_index, line) in lines.iter().enumerate() {
        let haystack = seek_sequence::normalised_spans(line);
        if haystack.len() < needle.len() {
            continue;
        }

        let mut start = 0;
        while start + needle.len() <= haystack.len() {
            let candidate = &haystack[start..start + needle.len()];
            if candidate
                .iter()
                .zip(needle.iter())
                .all(|(actual, expected)| actual.0 == expected.0)
            {
                matches.push(SubstringMatch {
                    line_index,
                    start: candidate[0].1,
                    end: candidate[needle.len() - 1].2,
                    fuzzy: true,
                });
                start += needle.len().max(1);
            } else {
                start += 1;
            }
        }
    }

    matches
}

fn splice_substring(line: &str, new_lines: &[String], matches: &[SubstringMatch]) -> Vec<String> {
    let mut result = vec![String::new()];
    let mut cursor = 0;

    for matched in matches {
        result
            .last_mut()
            .expect("replacement starts with one line")
            .push_str(&line[cursor..matched.start]);
        cursor = matched.end;

        // An empty replacement still leaves one (possibly empty) line so the
        // unmatched prefix and suffix remain joined on the same physical line.
        let replacement: Vec<&str> = if new_lines.is_empty() {
            vec![""]
        } else {
            new_lines.iter().map(String::as_str).collect()
        };
        result
            .last_mut()
            .expect("replacement starts with one line")
            .push_str(replacement[0]);
        result.extend(replacement[1..].iter().map(|line| (*line).to_string()));
    }

    result
        .last_mut()
        .expect("replacement starts with one line")
        .push_str(&line[cursor..]);
    result
}

// ---------------------------------------------------------------------------
// Utility: unified diff
// ---------------------------------------------------------------------------

/// Generate a minimal unified diff between two strings.
pub fn make_unified_diff(old: &str, new: &str) -> String {
    use similar::TextDiff;
    TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .to_string()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Split `s` by `\n` into owned lines, dropping a single trailing empty string
/// produced by a final newline.
fn split_lines(s: &str) -> Vec<String> {
    let mut lines: Vec<String> = s.split('\n').map(String::from).collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// Compute `(start_index, old_len, new_lines)` replacements from the chunk
/// definitions, using [`seek_sequence`] to locate each chunk in the file.
fn compute_replacements(
    original_lines: &[String],
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<Vec<(usize, usize, Vec<String>)>, ApplyPatchError> {
    let mut replacements: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut line_index: usize = 0;

    for chunk in chunks {
        // Use the change_context line to narrow the search position.
        if let Some(ctx_line) = &chunk.change_context {
            if let Some(idx) = seek_sequence::seek_sequence(
                original_lines,
                std::slice::from_ref(ctx_line),
                line_index,
                false,
            ) {
                line_index = idx + 1;
            } else {
                return Err(ApplyPatchError::ComputeReplacements(format!(
                    "Failed to find context '{ctx_line}' in {path}"
                )));
            }
        }

        if chunk.old_lines.is_empty() {
            // Pure addition.
            let insertion_idx = if original_lines.last().is_some_and(String::is_empty) {
                original_lines.len() - 1
            } else {
                original_lines.len()
            };
            replacements.push((insertion_idx, 0, chunk.new_lines.clone()));
            continue;
        }

        let mut pattern: &[String] = &chunk.old_lines;
        let mut found =
            seek_sequence::seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);

        let mut new_slice: &[String] = &chunk.new_lines;

        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            // Retry without the trailing empty line representing the final
            // newline in the file.
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }
            found = seek_sequence::seek_sequence(
                original_lines,
                pattern,
                line_index,
                chunk.is_end_of_file,
            );
        }

        if let Some(start_idx) = found {
            replacements.push((start_idx, pattern.len(), new_slice.to_vec()));
            line_index = start_idx + pattern.len();
        } else {
            return Err(ApplyPatchError::ComputeReplacements(format!(
                "Failed to find expected lines in {}:\n{}",
                path,
                chunk.old_lines.join("\n"),
            )));
        }
    }

    replacements.sort_by_key(|(index, _, _)| *index);
    Ok(replacements)
}

/// Apply `(start_index, old_len, new_lines)` replacements to `lines`.
/// Replacements are applied in descending order so earlier replacements
/// don't shift positions of later ones.
fn apply_replacements(
    mut lines: Vec<String>,
    replacements: &[(usize, usize, Vec<String>)],
) -> Vec<String> {
    for (start_idx, old_len, new_segment) in replacements.iter().rev() {
        let start_idx = *start_idx;
        let old_len = *old_len;

        for _ in 0..old_len {
            if start_idx < lines.len() {
                lines.remove(start_idx);
            }
        }

        for (offset, new_line) in new_segment.iter().enumerate() {
            lines.insert(start_idx + offset, new_line.clone());
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    // ── compute_updated_content tests ──

    #[test]
    fn test_compute_simple_update() {
        let original = "foo\nbar\nbaz\n";
        let chunks = vec![UpdateFileChunk {
            change_context: None,
            old_lines: vec!["bar".into()],
            new_lines: vec!["BAR".into()],
            is_end_of_file: false,
        }];
        let result = compute_updated_content(original, &chunks).unwrap();
        assert_eq!(result, "foo\nBAR\nbaz\n");
    }

    #[test]
    fn test_compute_multi_chunk() {
        let original = "foo\nbar\nbaz\nqux\n";
        let chunks = vec![
            UpdateFileChunk {
                change_context: None,
                old_lines: vec!["bar".into()],
                new_lines: vec!["BAR".into()],
                is_end_of_file: false,
            },
            UpdateFileChunk {
                change_context: None,
                old_lines: vec!["qux".into()],
                new_lines: vec!["QUX".into()],
                is_end_of_file: false,
            },
        ];
        let result = compute_updated_content(original, &chunks).unwrap();
        assert_eq!(result, "foo\nBAR\nbaz\nQUX\n");
    }

    #[test]
    fn test_compute_with_context_anchor() {
        let original = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let chunks = vec![UpdateFileChunk {
            change_context: Some("fn b() {}".into()),
            old_lines: vec!["fn c() {}".into()],
            new_lines: vec!["fn d() {}".into()],
            is_end_of_file: false,
        }];
        let result = compute_updated_content(original, &chunks).unwrap();
        assert_eq!(result, "fn a() {}\nfn b() {}\nfn d() {}\n");
    }

    #[test]
    fn test_compute_pure_addition() {
        let original = "a\nb\n";
        let chunks = vec![UpdateFileChunk {
            change_context: None,
            old_lines: vec![],
            new_lines: vec!["c".into()],
            is_end_of_file: false,
        }];
        let result = compute_updated_content(original, &chunks).unwrap();
        assert_eq!(result, "a\nb\nc\n");
    }

    #[test]
    fn test_compute_fuzzy_whitespace() {
        // Original has trailing spaces; chunk's old_lines don't.
        let original = "foo  \nbar\n";
        let chunks = vec![UpdateFileChunk {
            change_context: None,
            old_lines: vec!["foo".into()],
            new_lines: vec!["FOO".into()],
            is_end_of_file: false,
        }];
        let result = compute_updated_content(original, &chunks).unwrap();
        assert_eq!(result, "FOO\nbar\n");
    }

    #[test]
    fn test_compute_not_found_error() {
        let original = "hello\n";
        let chunks = vec![UpdateFileChunk {
            change_context: None,
            old_lines: vec!["world".into()],
            new_lines: vec!["WORLD".into()],
            is_end_of_file: false,
        }];
        assert!(compute_updated_content(original, &chunks).is_err());
    }

    #[test]
    fn test_compute_multi_line_chunk() {
        let original = "a\nb\nc\nd\n";
        let chunks = vec![UpdateFileChunk {
            change_context: None,
            old_lines: vec!["b".into(), "c".into()],
            new_lines: vec!["X".into(), "Y".into()],
            is_end_of_file: false,
        }];
        let result = compute_updated_content(original, &chunks).unwrap();
        assert_eq!(result, "a\nX\nY\nd\n");
    }

    // ── fuzzy_edit tests ──

    #[test]
    fn test_fuzzy_edit_exact_single_line() {
        let original = "foo\nbar\nbaz\n";
        let outcome = fuzzy_edit(original, "bar", "QUX", false);
        match outcome {
            FuzzyEditOutcome::Replaced {
                new_content,
                count,
                fuzzy,
            } => {
                assert_eq!(new_content, "foo\nQUX\nbaz\n");
                assert_eq!(count, 1);
                assert!(!fuzzy);
            }
            other => panic!("expected Replaced, got {other:?}"),
        }
    }

    #[test]
    fn test_fuzzy_edit_exact_multi_line() {
        let original = "a\nb\nc\nd\n";
        let outcome = fuzzy_edit(original, "b\nc", "X\nY", false);
        match outcome {
            FuzzyEditOutcome::Replaced {
                new_content, count, ..
            } => {
                assert_eq!(new_content, "a\nX\nY\nd\n");
                assert_eq!(count, 1);
            }
            other => panic!("expected Replaced, got {other:?}"),
        }
    }

    #[test]
    fn test_fuzzy_edit_whitespace_tolerance() {
        let original = "foo  \nbar\n";
        let outcome = fuzzy_edit(original, "foo", "FOO", false);
        match outcome {
            FuzzyEditOutcome::Replaced {
                new_content, fuzzy, ..
            } => {
                assert_eq!(new_content, "FOO\nbar\n");
                assert!(fuzzy, "should be flagged as fuzzy match");
            }
            other => panic!("expected Replaced, got {other:?}"),
        }
    }

    #[test]
    fn test_fuzzy_edit_indentation_tolerance() {
        let original = "fn main() {\n    println!(\"hi\");\n}\n";
        let outcome = fuzzy_edit(original, "println!(\"hi\");", "println!(\"bye\");", false);
        match outcome {
            FuzzyEditOutcome::Replaced {
                new_content, fuzzy, ..
            } => {
                assert!(new_content.contains("bye"));
                assert!(fuzzy);
            }
            other => panic!("expected Replaced, got {other:?}"),
        }
    }

    #[test]
    fn test_fuzzy_edit_unicode_dash() {
        let original = "import x  # local \u{2013} fast\n";
        let outcome = fuzzy_edit(original, "import x  # local - fast", "import y", false);
        assert!(matches!(outcome, FuzzyEditOutcome::Replaced { .. }));
    }

    #[test]
    fn test_fuzzy_edit_unicode_substring_in_long_line() {
        let original = format!(
            "{} z = \u{2212}1.96, \u{0394} = 2 \u{00D7} 3, path A \u{2192} B, x\u{2080} \u{2014} y. {}",
            "x".repeat(600),
            "z".repeat(600)
        );
        let old = "z = -1.96, Delta = 2 x 3, path A -> B, x0 - y.";
        let outcome = fuzzy_edit(&original, old, "sensitivity confirmed", false);
        match outcome {
            FuzzyEditOutcome::Replaced {
                new_content,
                count,
                fuzzy,
            } => {
                assert!(fuzzy);
                assert_eq!(count, 1);
                assert!(new_content.starts_with(&"x".repeat(600)));
                assert!(new_content.contains("sensitivity confirmed"));
                assert!(new_content.ends_with(&format!("{}\n", "z".repeat(600))));
            }
            other => panic!("expected Replaced, got {other:?}"),
        }
    }

    #[test]
    fn test_fuzzy_edit_ascii_substring_in_long_line() {
        let original = format!(
            "{} are confirmed correct. {}",
            "a".repeat(800),
            "b".repeat(800)
        );
        let outcome = fuzzy_edit(&original, "are confirmed correct.", "were verified.", false);
        match outcome {
            FuzzyEditOutcome::Replaced {
                new_content,
                count,
                fuzzy,
            } => {
                assert_eq!(fuzzy, false);
                assert_eq!(count, 1);
                assert!(new_content.contains("were verified."));
                assert!(new_content.starts_with(&"a".repeat(800)));
                assert!(new_content.ends_with(&format!("{}\n", "b".repeat(800))));
            }
            other => panic!("expected Replaced, got {other:?}"),
        }
    }

    #[test]
    fn test_fuzzy_edit_not_found() {
        let outcome = fuzzy_edit("foo\nbar\n", "baz", "qux", false);
        assert_eq!(outcome, FuzzyEditOutcome::NotFound);
    }

    #[test]
    fn test_fuzzy_edit_ambiguous_without_replace_all() {
        let outcome = fuzzy_edit("foo\nfoo\n", "foo", "bar", false);
        match outcome {
            FuzzyEditOutcome::Ambiguous { count } => assert_eq!(count, 2),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn test_fuzzy_edit_replace_all() {
        let outcome = fuzzy_edit("foo\nfoo\nbar\n", "foo", "X", true);
        match outcome {
            FuzzyEditOutcome::Replaced {
                new_content, count, ..
            } => {
                assert_eq!(new_content, "X\nX\nbar\n");
                assert_eq!(count, 2);
            }
            other => panic!("expected Replaced, got {other:?}"),
        }
    }

    #[test]
    fn test_fuzzy_edit_delete_line() {
        let outcome = fuzzy_edit("a\nb\nc\n", "b", "", false);
        match outcome {
            FuzzyEditOutcome::Replaced {
                new_content, count, ..
            } => {
                assert_eq!(new_content, "a\nc\n");
                assert_eq!(count, 1);
            }
            other => panic!("expected Replaced, got {other:?}"),
        }
    }

    #[test]
    fn test_fuzzy_edit_same_old_new_is_noop() {
        let outcome = fuzzy_edit("foo\nbar\n", "foo", "foo", false);
        match outcome {
            FuzzyEditOutcome::Replaced { count, fuzzy, .. } => {
                assert_eq!(count, 0);
                assert!(!fuzzy);
            }
            other => panic!("expected Replaced, got {other:?}"),
        }
    }

    #[test]
    fn test_fuzzy_edit_trailing_newline_in_old() {
        let outcome = fuzzy_edit("foo\nbar\nbaz\n", "bar\n", "QUX", false);
        match outcome {
            FuzzyEditOutcome::Replaced { new_content, .. } => {
                assert_eq!(new_content, "foo\nQUX\nbaz\n");
            }
            other => panic!("expected Replaced, got {other:?}"),
        }
    }

    // ── parse + compute integration ──

    #[test]
    fn test_parse_then_compute() {
        let patch = "*** Begin Patch
*** Update File: test.rs
@@
 fn main
-old
+new
*** End Patch";
        let args = parse_patch(patch).unwrap();
        assert_eq!(args.hunks.len(), 1);
        match &args.hunks[0] {
            Hunk::UpdateFile { chunks, .. } => {
                let original = "fn main\nold\n".to_string();
                let result = compute_updated_content(&original, chunks).unwrap();
                assert_eq!(result, "fn main\nnew\n");
            }
            other => panic!("expected UpdateFile, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_add_file() {
        let r = parse_patch("*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch").unwrap();
        assert_eq!(
            r.hunks,
            vec![Hunk::AddFile {
                path: std::path::PathBuf::from("foo"),
                contents: "hi\n".into(),
            }]
        );
    }

    #[test]
    fn test_parse_empty_patch_returns_no_hunks() {
        // The parser accepts an empty patch (no hunks); it's the caller's
        // responsibility to treat zero hunks as a no-op or error.
        let args = parse_patch("*** Begin Patch\n*** End Patch").unwrap();
        assert!(args.hunks.is_empty());
    }

    // ── unified diff utility ──

    #[test]
    fn test_make_unified_diff() {
        let diff = make_unified_diff("a\nb\nc\n", "a\nB\nc\n");
        assert!(diff.contains("-b"));
        assert!(diff.contains("+B"));
    }
}
