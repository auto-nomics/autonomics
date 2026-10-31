//! Server-side quote anchoring for agent annotations.
//!
//! The agent reads the canonical extracted text through `vfs` and copies a
//! quote verbatim; this module verifies the quote actually exists before an
//! annotation may anchor to it. Matching mirrors the GUI's
//! `TextQuoteIndex` (`reading-text-range.ts`): whitespace runs collapse to
//! single spaces so a quote copied across line breaks still matches, while
//! character case, CJK text, and emoji are compared exactly. Matches are
//! resolved to UTF-8 byte offsets of the canonical text — the same
//! coordinate space the semantic evidence uses.

use sha2::{Digest, Sha256};

use bib_types::ResolvedAnchor;

/// Whitespace-folded view of a text: every folded character remembers its
/// byte span in the original, so folded matches convert back to exact
/// slices without re-searching.
pub(crate) struct QuoteLocator {
    folded: Vec<char>,
    starts: Vec<usize>,
    ends: Vec<usize>,
}

/// A quote occurrence as a half-open byte range in the original text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Match {
    pub start_byte: usize,
    pub end_byte: usize,
}

impl QuoteLocator {
    pub fn new(text: &str) -> Self {
        let mut folded = Vec::new();
        let mut starts = Vec::new();
        let mut ends = Vec::new();
        let mut in_whitespace = false;
        for (offset, character) in text.char_indices() {
            if character.is_whitespace() {
                // Collapse runs: only the first whitespace of a run is kept.
                if in_whitespace {
                    continue;
                }
                in_whitespace = true;
                folded.push(' ');
                starts.push(offset);
                ends.push(offset + 1);
            } else {
                in_whitespace = false;
                folded.push(character);
                starts.push(offset);
                ends.push(offset + character.len_utf8());
            }
        }
        Self {
            folded,
            starts,
            ends,
        }
    }

    /// All occurrences of `quote`, whitespace-folded, up to `limit`.
    pub fn find_all(&self, quote: &str, limit: usize) -> Vec<Match> {
        let needle = fold_quote(quote);
        if needle.is_empty() || limit == 0 {
            return Vec::new();
        }
        let first = needle[0];
        let mut out = Vec::new();
        let mut at = 0;
        while at + needle.len() <= self.folded.len() {
            let candidate = match self.folded[at..].iter().position(|c| *c == first) {
                Some(relative) => at + relative,
                None => break,
            };
            if candidate + needle.len() > self.folded.len() {
                break;
            }
            if self.folded[candidate..candidate + needle.len()] == needle[..] {
                out.push(Match {
                    start_byte: self.starts[candidate],
                    end_byte: self.ends[candidate + needle.len() - 1],
                });
                if out.len() >= limit {
                    break;
                }
                at = candidate + 1;
            } else {
                at = candidate + 1;
            }
        }
        out
    }

    /// The longest prefix of the folded quote that still occurs somewhere —
    /// the location hint for "not found" errors. Returns `None` when not
    /// even `min_chars` folded characters match anywhere.
    pub fn longest_prefix_match(&self, quote: &str, min_chars: usize) -> Option<Match> {
        let needle = fold_quote(quote);
        if needle.len() < min_chars || min_chars == 0 {
            return None;
        }
        // Existence of a prefix of length k is monotone: once a prefix
        // fails, every longer one fails too. Binary search the boundary.
        let mut lo = min_chars; // smallest length that may still match
        let mut hi = needle.len(); // known not to match in full (caller path)
        if self.find_first(&needle[..hi]).is_some() {
            return Some(self.find_first(&needle[..hi]).expect("checked"));
        }
        let mut best = None;
        while lo <= hi {
            let mid = (lo + hi) / 2;
            if let Some(found) = self.find_first(&needle[..mid]) {
                best = Some(found);
                lo = mid + 1;
            } else {
                hi = mid - 1;
                if hi < lo {
                    break;
                }
            }
        }
        best
    }

    fn find_first(&self, needle: &[char]) -> Option<Match> {
        if needle.is_empty() {
            return None;
        }
        let first = needle[0];
        let mut at = 0;
        while at + needle.len() <= self.folded.len() {
            let relative = self.folded[at..].iter().position(|c| *c == first)?;
            let candidate = at + relative;
            if candidate + needle.len() > self.folded.len() {
                return None;
            }
            if self.folded[candidate..candidate + needle.len()] == needle[..] {
                return Some(Match {
                    start_byte: self.starts[candidate],
                    end_byte: self.ends[candidate + needle.len() - 1],
                });
            }
            at = candidate + 1;
        }
        None
    }
}

/// Fold a quote the way [`QuoteLocator`] folds text: Unicode whitespace
/// runs become single spaces, trimmed at both ends.
fn fold_quote(quote: &str) -> Vec<char> {
    let mut folded = Vec::new();
    for word in quote.split_whitespace() {
        if !folded.is_empty() {
            folded.push(' ');
        }
        folded.extend(word.chars());
    }
    folded
}

/// 1-based line index over byte offsets of a canonical text.
pub(crate) struct LineIndex {
    /// Byte offset where each line starts; line 1 starts at 0.
    line_starts: Vec<usize>,
    /// Total byte length; only [`Self::byte_range_of_line`] reads it.
    #[cfg_attr(not(test), allow(dead_code))]
    total_bytes: usize,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0usize];
        for (offset, character) in text.char_indices() {
            if character == '\n' {
                line_starts.push(offset + 1);
            }
        }
        Self {
            line_starts,
            total_bytes: text.len(),
        }
    }

    /// 1-based line containing `byte`. Bytes past the end clamp to the
    /// last line.
    pub fn line_of_byte(&self, byte: usize) -> u32 {
        let line = self
            .line_starts
            .partition_point(|start| *start <= byte)
            .max(1);
        line as u32
    }

    /// Half-open byte range of a 1-based line.
    #[cfg_attr(not(test), allow(dead_code))] // exercised by the LineIndex tests
    pub fn byte_range_of_line(&self, line: u32) -> (usize, usize) {
        let index = (line as usize)
            .saturating_sub(1)
            .min(self.line_starts.len() - 1);
        let start = self.line_starts[index];
        let end = self
            .line_starts
            .get(index + 1)
            .copied()
            .unwrap_or(self.total_bytes);
        (start, end)
    }

    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }
}

/// sha256 of the canonical text, hex — the `source_hash` anchors are
/// verified against.
pub(crate) fn hash_text(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Assemble the resolved anchor for a single (already disambiguated) match.
pub(crate) fn resolve_anchor(source: &str, text: &str, matched: Match) -> ResolvedAnchor {
    let lines = LineIndex::new(text);
    let quote = &text[matched.start_byte..matched.end_byte];
    ResolvedAnchor {
        source: source.to_owned(),
        source_hash: hash_text(text),
        quote: quote.to_owned(),
        start_byte: matched.start_byte,
        end_byte: matched.end_byte,
        line_start: lines.line_of_byte(matched.start_byte),
        line_end: lines.line_of_byte(matched.end_byte.saturating_sub(1)),
        context: collapse_context(text, matched.start_byte, matched.end_byte),
    }
}

/// Whitespace-collapsed text around a match: up to `CONTEXT_RADIUS`
/// folded characters on either side, ellipsized.
const CONTEXT_RADIUS: usize = 96;

pub(crate) fn collapse_context(text: &str, start_byte: usize, end_byte: usize) -> String {
    let locator = QuoteLocator::new(text);
    // Locate the match inside the folded view by its start offset.
    let folded_start = locator
        .starts
        .partition_point(|offset| *offset < start_byte)
        .saturating_sub(1);
    let folded_end = locator
        .starts
        .partition_point(|offset| *offset < end_byte)
        .saturating_sub(1);
    let from = folded_start.saturating_sub(CONTEXT_RADIUS);
    let to = (folded_end + CONTEXT_RADIUS).min(locator.folded.len());
    let mut context: String = locator.folded[from..to].iter().collect();
    if from > 0 {
        context.insert(0, '…');
    }
    if to < locator.folded.len() {
        context.push('…');
    }
    context
}

/// UTF-16 code-unit offset of `byte` in `text` — the coordinate the GUI
/// text reader's `space:"char"` rectangles are expressed in.
pub(crate) fn utf16_offset(text: &str, byte: usize) -> usize {
    text[..byte].chars().map(|c| c.len_utf16()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_whitespace_but_respects_case_and_content() {
        let text = "alpha\n  beta   gamma\r\nALPHA beta";
        let locator = QuoteLocator::new(text);
        // A quote copied across the line break and indentation matches.
        let hits = locator.find_all("alpha\n beta gamma", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            &text[hits[0].start_byte..hits[0].end_byte],
            "alpha\n  beta   gamma"
        );
        // Case is exact: the lowercase quote matches only the lowercase
        // occurrence, never the ALPHA one, and vice versa.
        assert_eq!(locator.find_all("alpha beta gamma", 10).len(), 1);
        assert_eq!(locator.find_all("ALPHA beta", 10).len(), 1);
        assert!(locator.find_all("alpha BETA", 10).is_empty());
    }

    #[test]
    fn finds_cjk_and_multibyte_matches_with_exact_byte_slices() {
        let text = "方法显著降低了死亡率。\n方法显著降低了死亡率。";
        let locator = QuoteLocator::new(text);
        let hits = locator.find_all("方法显著降低了死亡率", 10);
        assert_eq!(hits.len(), 2);
        for hit in &hits {
            assert_eq!(&text[hit.start_byte..hit.end_byte], "方法显著降低了死亡率");
        }
        // Second occurrence starts one line later (11 CJK chars + 。 + \n).
        assert!(hits[1].start_byte > hits[0].end_byte);
    }

    #[test]
    fn prefix_hint_locates_the_closest_partial_match() {
        let text = "the treatment reduced mortality significantly across cohorts";
        let locator = QuoteLocator::new(text);
        // Drifted quote: only the leading words exist. The slice is the
        // verbatim text, so the folded space before the divergence is kept.
        let hint = locator
            .longest_prefix_match("the treatment reduced mortality rates dramatically", 4)
            .expect("prefix hint");
        assert_eq!(
            &text[hint.start_byte..hint.end_byte],
            "the treatment reduced mortality "
        );
        // Nothing remotely similar → None.
        assert!(locator.longest_prefix_match("zzz qqq xxx", 4).is_none());
    }

    #[test]
    fn line_index_maps_bytes_and_lines_both_ways() {
        let text = "first\nsecond\nthird";
        let lines = LineIndex::new(text);
        assert_eq!(lines.line_count(), 3);
        assert_eq!(lines.line_of_byte(0), 1);
        assert_eq!(lines.line_of_byte(7), 2); // 'e' of second
        assert_eq!(lines.line_of_byte(13), 3); // 't' of third
        assert_eq!(lines.byte_range_of_line(2), (6, 13));
        assert_eq!(lines.byte_range_of_line(3), (13, 18));
        assert_eq!(lines.byte_range_of_line(99), (13, 18));
    }

    #[test]
    fn utf16_offsets_count_emoji_as_two_units() {
        let text = "a🚀b";
        assert_eq!(utf16_offset(text, 0), 0);
        assert_eq!(utf16_offset(text, 1), 1); // after 'a'
        assert_eq!(utf16_offset(text, 5), 3); // after 🚀 (4 bytes, 2 units)
        assert_eq!(utf16_offset(text, 6), 4); // after 'b'
    }

    #[test]
    fn resolved_anchor_carries_hash_lines_and_context() {
        let text = "intro line\nthe key finding is here at last\noutro";
        let locator = QuoteLocator::new(text);
        let matched = locator.find_all("key finding", 10).remove(0);
        let resolved = resolve_anchor("fulltext", text, matched);
        assert_eq!(resolved.source, "fulltext");
        assert_eq!(resolved.quote, "key finding");
        assert_eq!(resolved.line_start, 2);
        assert_eq!(resolved.line_end, 2);
        assert_eq!(resolved.source_hash, hash_text(text));
        assert!(resolved.context.contains("the key finding is here"));
        assert_eq!(resolved.source_hash, {
            use sha2::Digest;
            let digest = sha2::Sha256::digest(text.as_bytes());
            digest
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        });
    }
}
