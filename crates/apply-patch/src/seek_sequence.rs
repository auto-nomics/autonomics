//! Fuzzy line-sequence matching engine.
//!
//! Ported from Codex's `seek_sequence.rs`. Attempts to locate a pattern
//! (a sequence of lines) within a file's lines using four progressively
//! more lenient matching passes.

/// Attempt to find the sequence of `pattern` lines within `lines` beginning at
/// or after `start`.
///
/// Returns the starting index of the match or `None` if not found. Matches are
/// attempted with decreasing strictness:
///
/// 1. **Exact** — byte-for-byte equality.
/// 2. **rstrip** — ignore trailing whitespace on each line.
/// 3. **trim** — ignore leading and trailing whitespace.
/// 4. **Unicode-normalised** — map common typographic punctuation (en-dashes,
///    smart quotes, non-breaking spaces, …) to their ASCII equivalents before
///    trimming, mirroring `git apply`'s tolerance for minor byte differences.
///
/// When `eof` is true the search starts from the end of the file so that
/// patterns intended to match file endings are applied at the end, falling back
/// to a forward search from `start` if needed.
pub(crate) fn seek_sequence(
    lines: &[String],
    pattern: &[String],
    start: usize,
    eof: bool,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }

    // Pattern longer than input — impossible to match. Early-return to avoid
    // an out-of-bounds slice.
    if pattern.len() > lines.len() {
        return None;
    }

    let search_start = if eof && lines.len() >= pattern.len() {
        lines.len() - pattern.len()
    } else {
        start
    };

    // Pass 1: exact match.
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        if lines[i..i + pattern.len()] == *pattern {
            return Some(i);
        }
    }

    // Pass 2: rstrip match (ignore trailing whitespace).
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        let mut ok = true;
        for (p_idx, pat) in pattern.iter().enumerate() {
            if lines[i + p_idx].trim_end() != pat.trim_end() {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }

    // Pass 3: trim both sides.
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        let mut ok = true;
        for (p_idx, pat) in pattern.iter().enumerate() {
            if lines[i + p_idx].trim() != pat.trim() {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }

    // Pass 4: Unicode-normalised match.
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        let mut ok = true;
        for (p_idx, pat) in pattern.iter().enumerate() {
            if normalise(&lines[i + p_idx]) != normalise(pat) {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }

    None
}

/// Normalise common Unicode punctuation to ASCII equivalents, then trim.
fn normalise(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            // Various dash / hyphen code-points → ASCII '-'
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            // Fancy single quotes → '\''
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            // Fancy double quotes → '"'
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            // Non-breaking space and other odd spaces → normal space
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => ' ',
            other => other,
        })
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::seek_sequence;

    fn to_vec(strings: &[&str]) -> Vec<String> {
        strings.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn test_exact_match_finds_sequence() {
        let lines = to_vec(&["foo", "bar", "baz"]);
        let pattern = to_vec(&["bar", "baz"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(1));
    }

    #[test]
    fn test_rstrip_match_ignores_trailing_whitespace() {
        let lines = to_vec(&["foo   ", "bar\t\t"]);
        let pattern = to_vec(&["foo", "bar"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn test_trim_match_ignores_leading_and_trailing_whitespace() {
        let lines = to_vec(&["    foo   ", "   bar\t"]);
        let pattern = to_vec(&["foo", "bar"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn test_pattern_longer_than_input_returns_none() {
        let lines = to_vec(&["just one line"]);
        let pattern = to_vec(&["too", "many", "lines"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), None);
    }

    #[test]
    fn test_unicode_dash_normalisation() {
        // EN DASH \u{2013} in the file, ASCII '-' in the pattern.
        let lines = to_vec(&["import asyncio  # local import \u{2013} avoids dep"]);
        let pattern = to_vec(&["import asyncio  # local import - avoids dep"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn test_eof_match_starts_from_end() {
        let lines = to_vec(&["a", "b", "c", "d"]);
        // With eof=true, search starts from the end; pattern "c\nd" should
        // still match at index 2.
        let pattern = to_vec(&["c", "d"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, true), Some(2));
    }
}
