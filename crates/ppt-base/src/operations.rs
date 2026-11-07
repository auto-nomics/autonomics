//! Markdown source operations — the Slidev MCP operation model
//! (`slidev/packages/slidev/node/mcp/operations.ts`): patch, insert,
//! remove, and move slides *in the canonical markdown*, never in a
//! derived cache. Slide numbers are 1-based.
//!
//! Implementation strategy: parse the deck, splice the slide list,
//! re-stringify. Untouched slides keep their `raw` verbatim
//! ([`stringify`](crate::parser::stringify) only normalizes the
//! separators BETWEEN slides), so round-trip fidelity holds for every
//! slide the operation did not edit.

use crate::error::{Error, Result};
use crate::parser::{parse, parse_slide_raw, prettify_slide, stringify};

/// The frontmatter of slide 1 is the deck headmatter; replacing or
/// removing it via slide operations is rejected (Slidev
/// `assertNotEntryHeadmatter`).
pub const HEADMATTER_SLIDE: usize = 1;

fn check_slide_no(markdown: &str, deck_id: &str, no: usize) -> Result<()> {
    let total = parse(markdown).slides.len();
    if no == 0 || no > total {
        return Err(Error::SlideOutOfRange {
            deck: deck_id.to_string(),
            no,
            total,
        });
    }
    Ok(())
}

/// Apply a slide patch to the canonical markdown.
///
/// `content` replaces the body; `note` replaces (or clears, with `""`)
/// the speaker note; `frontmatter_raw` replaces the frontmatter YAML
/// verbatim (empty string clears it). Invalid YAML is rejected before
/// any mutation. Slide 1's frontmatter is the headmatter and can only be
/// changed through dedicated deck-level operations.
pub fn apply_slide_patch(
    markdown: &str,
    deck_id: &str,
    no: usize,
    patch: SlidePatch,
) -> Result<String> {
    check_slide_no(markdown, deck_id, no)?;
    if no == HEADMATTER_SLIDE && patch.frontmatter_raw.is_some() {
        return Err(Error::HeadmatterGuard);
    }
    if let Some(fm) = frontmatter_raw_nonempty(&patch.frontmatter_raw) {
        validate_yaml(fm)?;
    }

    let SlidePatch {
        content,
        frontmatter_raw,
        note,
    } = patch;

    let mut deck = parse(markdown);
    {
        let slide = &mut deck.slides[no - 1];

        if let Some(content) = content {
            slide.content = content.trim().to_string();
        }
        if let Some(note) = note {
            let note = note.trim().to_string();
            slide.note = if note.is_empty() { None } else { Some(note) };
        }
        if let Some(fm) = frontmatter_raw_nonempty(&frontmatter_raw) {
            slide.frontmatter_raw = Some(fm.to_string());
            slide.frontmatter = crate::parser::parse_yaml_object(fm);
        } else if matches!(frontmatter_raw, Some(ref s) if s.is_empty()) {
            slide.frontmatter_raw = None;
            slide.frontmatter.clear();
        }

        prettify_slide(slide);
    }

    Ok(stringify(&deck.slides))
}

#[derive(Debug, Clone, Default)]
pub struct SlidePatch {
    pub content: Option<String>,
    pub frontmatter_raw: Option<String>,
    pub note: Option<String>,
}

fn frontmatter_raw_nonempty(raw: &Option<String>) -> Option<&str> {
    match raw {
        Some(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}

/// Insert a slide after 1-based position `after` (0 = prepend). Returns
/// the new canonical markdown.
pub fn insert_slide(
    markdown: &str,
    deck_id: &str,
    after: usize,
    content: &str,
    frontmatter_raw: Option<&str>,
    note: Option<&str>,
) -> Result<String> {
    let total = parse(markdown).slides.len();
    if after > total {
        return Err(Error::SlideOutOfRange {
            deck: deck_id.to_string(),
            no: after,
            total,
        });
    }
    if let Some(fm) = frontmatter_raw {
        validate_yaml(fm)?;
    }

    let raw = build_slide_raw(content, frontmatter_raw, note);
    let new_slide = parse_slide_raw(raw, usize::MAX);

    let mut deck = parse(markdown);
    deck.slides.insert(after, new_slide);
    for (i, s) in deck.slides.iter_mut().enumerate() {
        s.index = i;
    }

    Ok(stringify(&deck.slides))
}

/// Remove slide `no` (1-based). The headmatter slide cannot be removed.
pub fn remove_slide(markdown: &str, deck_id: &str, no: usize) -> Result<String> {
    check_slide_no(markdown, deck_id, no)?;
    if no == HEADMATTER_SLIDE {
        return Err(Error::HeadmatterGuard);
    }

    let mut deck = parse(markdown);
    deck.slides.remove(no - 1);
    for (i, s) in deck.slides.iter_mut().enumerate() {
        s.index = i;
    }

    Ok(stringify(&deck.slides))
}

/// Move slide `from` to position `to` (1-based, both interpreted in the
/// original numbering). Moving the headmatter slide (or anything onto
/// position 1) is rejected — it would reassign the deck headmatter.
pub fn move_slide(markdown: &str, deck_id: &str, from: usize, to: usize) -> Result<String> {
    check_slide_no(markdown, deck_id, from)?;
    check_slide_no(markdown, deck_id, to)?;
    if from == HEADMATTER_SLIDE || to == HEADMATTER_SLIDE {
        return Err(Error::HeadmatterGuard);
    }
    if from == to {
        return Ok(markdown.to_string());
    }

    let mut deck = parse(markdown);
    let slide = deck.slides.remove(from - 1);
    deck.slides.insert(to - 1, slide);
    for (i, s) in deck.slides.iter_mut().enumerate() {
        s.index = i;
    }

    Ok(stringify(&deck.slides))
}

/// Reconstruct a slide's raw from parts (prettify shape).
fn build_slide_raw(content: &str, frontmatter_raw: Option<&str>, note: Option<&str>) -> String {
    let mut raw = match frontmatter_raw {
        Some(fm) if !fm.trim().is_empty() => format!("---\n{}\n---\n", fm.trim()),
        _ => String::new(),
    };
    let body = content.trim();
    if !body.is_empty() {
        raw.push('\n');
        raw.push_str(body);
        raw.push('\n');
    }
    if let Some(note) = note {
        let note = note.trim();
        if !note.is_empty() {
            raw.push_str(&format!("\n<!--\n{}\n-->\n", note));
        }
    }
    raw
}

fn validate_yaml(raw: &str) -> Result<()> {
    serde_yaml_ng::from_str::<serde_yaml_ng::Value>(raw)
        .map(|_| ())
        .map_err(|e| Error::BadFrontmatter(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use serde_json::json;

    const DECK: &str = "one\n\n---\n\ntwo\n\n---\nlayout: x\n---\n\nthree\n\n<!--\nn3\n-->\n";

    #[test]
    fn patch_content_and_note() {
        let md = apply_slide_patch(
            DECK,
            "d",
            2,
            SlidePatch {
                content: Some("replaced".into()),
                note: Some("new note".into()),
                frontmatter_raw: None,
            },
        )
        .unwrap();
        let deck = parse(&md);
        assert_eq!(deck.slides.len(), 3);
        assert_eq!(deck.slides[1].content, "replaced");
        assert_eq!(deck.slides[1].note.as_deref(), Some("new note"));
        // untouched slides keep raw verbatim
        assert_eq!(deck.slides[0].content, "one");
        assert_eq!(deck.slides[2].frontmatter.get("layout").unwrap(), "x");
        assert_eq!(deck.slides[2].note.as_deref(), Some("n3"));
    }

    #[test]
    fn patch_frontmatter_replaces_verbatim() {
        let md = apply_slide_patch(
            DECK,
            "d",
            3,
            SlidePatch {
                content: None,
                note: None,
                frontmatter_raw: Some("layout: y\nrank: 2".into()),
            },
        )
        .unwrap();
        let deck = parse(&md);
        assert_eq!(deck.slides[2].frontmatter.get("layout").unwrap(), "y");
        // YAML scalar 2 coerces to a JSON number (JS toJSON behavior)
        assert_eq!(deck.slides[2].frontmatter.get("rank"), Some(&json!(2)));
        assert_eq!(deck.slides[2].note.as_deref(), Some("n3"));
    }

    #[test]
    fn patch_clears_frontmatter_with_empty_string() {
        let md = apply_slide_patch(
            DECK,
            "d",
            3,
            SlidePatch {
                content: None,
                note: None,
                frontmatter_raw: Some(String::new()),
            },
        )
        .unwrap();
        let deck = parse(&md);
        assert!(deck.slides[2].frontmatter.is_empty());
        assert_eq!(deck.slides[2].content, "three");
    }

    #[test]
    fn headmatter_guard() {
        let err = apply_slide_patch(
            DECK,
            "d",
            1,
            SlidePatch {
                content: None,
                note: None,
                frontmatter_raw: Some("layout: cover".into()),
            },
        )
        .unwrap_err();
        assert!(matches!(err, Error::HeadmatterGuard));
        // slide 1 content CAN be patched
        let md = apply_slide_patch(
            DECK,
            "d",
            1,
            SlidePatch {
                content: Some("new head body".into()),
                note: None,
                frontmatter_raw: None,
            },
        )
        .unwrap();
        assert_eq!(parse(&md).slides[0].content, "new head body");
    }

    #[test]
    fn bad_yaml_rejected_before_mutation() {
        let err = apply_slide_patch(
            DECK,
            "d",
            2,
            SlidePatch {
                content: Some("x".into()),
                note: None,
                frontmatter_raw: Some("layout: [unclosed".into()),
            },
        )
        .unwrap_err();
        assert!(matches!(err, Error::BadFrontmatter(_)));
    }

    #[test]
    fn out_of_range() {
        let err = apply_slide_patch(
            DECK,
            "d",
            9,
            SlidePatch {
                content: None,
                note: None,
                frontmatter_raw: None,
            },
        )
        .unwrap_err();
        assert!(matches!(err, Error::SlideOutOfRange { no: 9, total: 3, .. }));
    }

    #[test]
    fn insert_prepend_and_after() {
        let md = insert_slide(DECK, "d", 0, "new-first", Some("layout: cover"), None).unwrap();
        let deck = parse(&md);
        assert_eq!(deck.slides.len(), 4);
        assert_eq!(deck.slides[0].content, "new-first");
        assert_eq!(deck.slides[0].frontmatter.get("layout").unwrap(), "cover");
        assert_eq!(deck.slides[1].content, "one");
        assert_eq!(deck.slides[3].note.as_deref(), Some("n3"));

        let md = insert_slide(DECK, "d", 2, "between", None, Some("n")).unwrap();
        let deck = parse(&md);
        assert_eq!(deck.slides.len(), 4);
        assert_eq!(deck.slides[1].content, "two");
        assert_eq!(deck.slides[2].content, "between");
        assert_eq!(deck.slides[2].note.as_deref(), Some("n"));
        assert_eq!(deck.slides[3].content, "three");

        // insert past the end is an error
        assert!(matches!(
            insert_slide(DECK, "d", 9, "x", None, None).unwrap_err(),
            Error::SlideOutOfRange { no: 9, total: 3, .. }
        ));
    }

    #[test]
    fn remove_middle_slide() {
        let md = remove_slide(DECK, "d", 2).unwrap();
        let deck = parse(&md);
        assert_eq!(deck.slides.len(), 2);
        assert_eq!(deck.slides[0].content, "one");
        assert_eq!(deck.slides[1].content, "three");
        assert_eq!(deck.slides[1].note.as_deref(), Some("n3"));
        assert_eq!(deck.slides[1].frontmatter.get("layout").unwrap(), "x");
    }

    #[test]
    fn remove_last_slide_has_no_dangling_separator() {
        let md = remove_slide(DECK, "d", 3).unwrap();
        let deck = parse(&md);
        assert_eq!(deck.slides.len(), 2);
        assert_eq!(deck.slides[1].content, "two");
        // reparse is stable (no phantom empty slide from a trailing `---`)
        let reparsed = parse(&md);
        assert_eq!(reparsed.slides.len(), 2);
    }

    #[test]
    fn remove_headmatter_rejected() {
        assert!(matches!(
            remove_slide(DECK, "d", 1).unwrap_err(),
            Error::HeadmatterGuard
        ));
    }

    #[test]
    fn move_slide_reorders() {
        let md = move_slide(DECK, "d", 3, 2).unwrap();
        let deck = parse(&md);
        assert_eq!(deck.slides.len(), 3);
        assert_eq!(deck.slides[1].content, "three");
        assert_eq!(deck.slides[1].note.as_deref(), Some("n3"));
        assert_eq!(deck.slides[2].content, "two");

        let md = move_slide(DECK, "d", 2, 3).unwrap();
        let deck = parse(&md);
        assert_eq!(deck.slides[1].content, "three");
        assert_eq!(deck.slides[2].content, "two");

        // no-op
        assert_eq!(move_slide(DECK, "d", 2, 2).unwrap(), DECK);
    }

    #[test]
    fn move_involving_headmatter_rejected() {
        assert!(matches!(
            move_slide(DECK, "d", 2, 1).unwrap_err(),
            Error::HeadmatterGuard
        ));
        assert!(matches!(
            move_slide(DECK, "d", 1, 2).unwrap_err(),
            Error::HeadmatterGuard
        ));
    }

    #[test]
    fn ops_preserve_untouched_slides_verbatim() {
        let md = remove_slide(DECK, "d", 2).unwrap();
        let deck = parse(&md);
        // slide 1's raw: "one\n" (region includes its trailing blank line)
        assert_eq!(deck.slides[0].raw, "one\n");
        // round-trip stability: stringify(parse(stringify)) == stringify
        assert_eq!(stringify(&parse(&md).slides), md);
    }
}
