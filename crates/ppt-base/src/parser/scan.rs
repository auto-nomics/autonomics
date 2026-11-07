//! Line scanner — faithful port of Slidev `parseSync`
//! (`packages/parser/src/core.ts:348-425`).
//!
//! Splits deck markdown into slide source slices. A line starting with
//! `---` ends the current slide and starts a new one; if the 4th char is
//! not `-` and the next line is non-empty, it opens a frontmatter block
//! that runs to the next `---` line. `----`-style lines ARE separators
//! (Slidev's section-matter test pins `c/----/d/----/e` → three slides)
//! but never open frontmatter. `---` inside fenced code blocks and inside
//! HTML comments never splits.

/// One scanned source region: `raw = lines[start..end]` (the separator
/// line itself is excluded), with `content_start` pointing past the
/// frontmatter block when one was consumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlideSpan {
    pub start: usize,
    pub content_start: usize,
    /// Exclusive end line (Slidev `end`).
    pub end: usize,
}

/// Scan deck lines into slide spans. `lines` are split on `\r?\n`.
pub fn scan_spans(lines: &[String]) -> Vec<SlideSpan> {
    let mut spans = Vec::new();
    let mut start = 0usize;
    let mut content_start = 0usize;
    let mut in_html_comment = false;

    let mut i = 0usize;
    while i < lines.len() {
        let raw_line = &lines[i];
        let line = raw_line.trim_end();

        if in_html_comment {
            in_html_comment = advance_html_comment_state(raw_line, true);
            i += 1;
            continue;
        }

        if line.starts_with("---") {
            slice_span(&mut spans, &mut start, &mut content_start, i);

            let next_non_empty = lines
                .get(i + 1)
                .is_some_and(|l| !l.trim().is_empty());
            // `line[3] !== '-'` in JS — a line of exactly `---` has no 4th
            // char, which counts as "not '-'".
            let opens_frontmatter =
                line.as_bytes().get(3) != Some(&b'-') && next_non_empty;
            if opens_frontmatter {
                start = i;
                i += 1;
                while i < lines.len() {
                    if lines[i].trim_end() == "---" {
                        break;
                    }
                    i += 1;
                }
                content_start = i + 1;
            }
        } else if line.trim_start().starts_with("```") {
            // Skip the fenced code block: the fence level is the leading
            // whitespace plus the backtick run (`/^\s*`+/`); the closing
            // line must start with the same prefix.
            let level = leading_backtick_run(line);
            let mut j = i + 1;
            while j < lines.len() {
                if lines[j].starts_with(&level) {
                    break;
                }
                j += 1;
            }
            // Update i only when the code block ends (Slidev pins that an
            // unterminated fence does not swallow the rest of the deck).
            if j != lines.len() {
                i = j;
            }
        } else {
            in_html_comment = advance_html_comment_state(raw_line, false);
        }

        i += 1;
    }

    if start <= lines.len().saturating_sub(1) {
        slice_span(&mut spans, &mut start, &mut content_start, lines.len());
    }

    spans
}

/// Slidev `slice(end)`: push a span, advance the region cursors.
fn slice_span(
    spans: &mut Vec<SlideSpan>,
    start: &mut usize,
    content_start: &mut usize,
    end: usize,
) {
    if *start == end {
        return;
    }
    spans.push(SlideSpan {
        start: *start,
        content_start: *content_start,
        end,
    });
    *start = end + 1;
    *content_start = end + 1;
}

/// Port of Slidev `advanceHtmlCommentState`: after processing the line,
/// are we inside an HTML comment?
fn advance_html_comment_state(line: &str, mut in_html_comment: bool) -> bool {
    let bytes = line.as_bytes();
    let mut cursor = 0usize;
    while cursor < line.len() {
        if in_html_comment {
            match find(bytes, b"-->", cursor) {
                None => return true,
                Some(end) => {
                    in_html_comment = false;
                    cursor = end + 3;
                }
            }
        } else {
            match find(bytes, b"<!--", cursor) {
                None => return false,
                Some(start) => match find(bytes, b"-->", start + 4) {
                    None => return true,
                    Some(end) => cursor = end + 3,
                },
            }
        }
    }
    in_html_comment
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// `^\s*`+` — leading whitespace plus the backtick run.
fn leading_backtick_run(line: &str) -> String {
    let ws = line
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    let rest = &line[ws..];
    let ticks = rest.bytes().take_while(|&b| b == b'`').count();
    format!("{}{}", &line[..ws], &rest[..ticks])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.split('\n').map(String::from).collect()
    }

    #[test]
    fn abcdef_fixture() {
        // Slidev "parse" test fixture: `----` separates (no frontmatter),
        // `---` + non-empty next line opens frontmatter. The scan-level
        // raw slice INCLUDES the frontmatter block (extraction happens in
        // parse_slide_raw).
        let md = "\na\n\n---\n\nb\n\n---\nlayout: z\n---\nc\n----\nd\n----\ne\n\n---\n\nf\n\n";
        let ls = lines(md);
        let spans = scan_spans(&ls);
        assert_eq!(spans.len(), 6);
        let contents: Vec<String> = spans
            .iter()
            .map(|s| ls[s.start..s.end].join("\n").trim().to_string())
            .collect();
        assert_eq!(
            contents,
            [
                "a",
                "b",
                "---\nlayout: z\n---\nc",
                "d",
                "e",
                "f"
            ]
        );
    }

    #[test]
    fn section_matter_fixture() {
        // Slidev "parse section matter" fixture: `---name`, `---- name`
        // lines are separators; `---` directly followed by content opens
        // frontmatter.
        let md = "\na\n\n---\n\nb\n\n---section2\nlayout: z\n---\nc\n----   section 3\nd\n---- section-4\ne\n\n---\n\nf\n\n";
        let ls = lines(md);
        let spans = scan_spans(&ls);
        assert_eq!(spans.len(), 6);
        let contents: Vec<String> = spans
            .iter()
            .map(|s| ls[s.start..s.end].join("\n").trim().to_string())
            .collect();
        assert_eq!(
            contents,
            [
                "a",
                "b",
                "---section2\nlayout: z\n---\nc",
                "d",
                "e",
                "f"
            ]
        );
    }

    #[test]
    fn separators_in_html_comments_ignored() {
        let md = "a\n\n<!--\n---\nhidden\n-->\n\n---\n\nb\n";
        let spans = scan_spans(&lines(md));
        assert_eq!(spans.len(), 2);
    }

    #[test]
    fn comment_opening_on_separator_line() {
        // Slidev pins: `----<!--` still separates (2 slides), and because
        // the separator branch never advances the comment state, the
        // following lines are scanned normally.
        let md = "a\n\n----<!--\nhidden\n-->\n\nb\n";
        let spans = scan_spans(&lines(md));
        assert_eq!(spans.len(), 2);
        let ls = lines(md);
        assert_eq!(
            ls[spans[1].start..spans[1].end].join("\n").trim(),
            "hidden\n-->\n\nb"
        );
    }

    #[test]
    fn code_fence_protects_separator() {
        let md = "```\n---\nstill code\n```\n\n---\n\nb\n";
        let spans = scan_spans(&lines(md));
        assert_eq!(spans.len(), 2);
    }

    #[test]
    fn unterminated_fence_does_not_protect() {
        // Slidev: i is only updated when the block ends, so a `---` after
        // an unterminated fence still splits.
        let md = "```\nunclosed\n\n---\n\nb\n";
        let spans = scan_spans(&lines(md));
        assert_eq!(spans.len(), 2);
    }

    #[test]
    fn indented_fence_needs_matching_indent() {
        // The fence level includes leading whitespace.
        let md = "  ```\n  ---\n  ```\n\n---\n\nb\n";
        let spans = scan_spans(&lines(md));
        assert_eq!(spans.len(), 2);
    }

    #[test]
    fn leading_frontmatter_is_slide_one() {
        let md = "---\ntitle: T\n---\n\n# Slide 1\n\n---\n\n# Slide 2\n";
        let spans = scan_spans(&lines(md));
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].start, 0);
        // content starts after the closing `---` line
        assert_eq!(spans[0].content_start, 3);
    }

    #[test]
    fn trailing_empty_region_produces_empty_slide() {
        // Slidev quirk preserved: a deck ending in `---\n` yields a final
        // empty slide (the trailing region has one empty line, which is
        // non-zero-length so the slice is not skipped).
        let md = "a\n\n---\n\nb\n\n---\n";
        let spans = scan_spans(&lines(md));
        assert_eq!(spans.len(), 3);
        let ls = lines(md);
        assert_eq!(ls[spans[2].start..spans[2].end].join("\n"), "");
    }
}
