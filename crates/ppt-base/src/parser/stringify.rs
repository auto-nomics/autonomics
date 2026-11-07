//! Round-trip serialization — ports of Slidev `stringify`, `stringifySlide`,
//! and `prettifySlide` (`packages/parser/src/core.ts:49-77`).
//!
//! Contract: `stringify(parse(md))` == `md` (modulo the trailing newline
//! and leading/trailing blank regions), and untouched slides keep their
//! `raw` verbatim — only patched slides go through [`prettify_slide`].

use crate::parser::matter::FrontmatterStyle;
use crate::parser::SourceSlide;

/// Serialize slides back to deck markdown (Slidev `stringify`).
pub fn stringify(slides: &[SourceSlide]) -> String {
    let joined = slides
        .iter()
        .enumerate()
        .map(|(idx, s)| stringify_slide(s, idx))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}\n", joined.trim())
}

/// Serialize one slide; non-first slides whose raw lacks the leading
/// `---` get the separator prepended (Slidev `stringifySlide`).
pub fn stringify_slide(slide: &SourceSlide, idx: usize) -> String {
    if slide.raw.starts_with("---") || idx == 0 {
        slide.raw.clone()
    } else {
        format!("---\n{}", ensure_prefix('\n', &slide.raw))
    }
}

/// Reformat one slide's raw from its parsed parts (Slidev `prettifySlide`).
///
/// The frontmatter raw text is kept verbatim (comment/order preserving);
/// content is normalized to leading/trailing newlines; the note, when
/// present, becomes a trailing `<!-- … -->` block.
pub fn prettify_slide(slide: &mut SourceSlide) {
    let trimmed = slide.content.trim().to_string();
    slide.content = if trimmed.is_empty() {
        String::new()
    } else {
        format!("\n{}\n", trimmed)
    };

    let mut raw = match (&slide.frontmatter_raw, slide.frontmatter_style) {
        (Some(fm), FrontmatterStyle::YamlCodeblock) => {
            format!("```yaml\n{}\n```\n{}", fm.trim(), slide.content)
        }
        (Some(fm), _) => format!("---\n{}\n---\n{}", fm.trim(), slide.content),
        (None, _) => slide.content.clone(),
    };

    if let Some(note) = &slide.note {
        raw.push_str(&format!("\n<!--\n{}\n-->\n", note.trim()));
    }
    slide.raw = raw;
}

/// Prettify every slide (Slidev `prettify`) — used before full-deck
/// rewrites; slide-level operations prettify only the edited slide.
pub fn prettify_deck(slides: &mut [SourceSlide]) {
    for s in slides {
        prettify_slide(s);
    }
}

fn ensure_prefix(prefix: char, s: &str) -> String {
    if s.starts_with(prefix) {
        s.to_string()
    } else {
        format!("{}{}", prefix, s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    #[test]
    fn stringify_round_trips() {
        let md = "---\ntitle: A\n---\n\n# A\n\n---\n\n# B\n\n---\nlayout: x\n---\nbody\n\n<!--\nnote\n-->\n";
        let deck = parse(md);
        let out = stringify(&deck.slides);
        assert_eq!(out, md);
    }

    #[test]
    fn stringify_prepends_separator_for_slides_without_raw_marker() {
        let mut s: SourceSlide = SourceSlide {
            index: 1,
            raw: "body".into(),
            content: "body".into(),
            note: None,
            frontmatter: Default::default(),
            frontmatter_raw: None,
            frontmatter_style: FrontmatterStyle::Frontmatter,
            title: None,
            level: None,
            images: vec![],
            revision: "x".into(),
        };
        let s0 = parse("first").slides.remove(0);
        let out = stringify(&[s0, s.clone()]);
        assert_eq!(out, "first\n---\n\nbody\n");
        s.raw = "\nalready newlined".to_string();
        let out = stringify(&[parse("first").slides.remove(0), s]);
        assert_eq!(out, "first\n---\n\nalready newlined\n");
    }

    #[test]
    fn prettify_normalizes_content_and_note() {
        let mut s = parse("---\nfm: 1\n---\n\nmessy   content\n\n<!--\nN\n-->").slides.remove(0);
        prettify_slide(&mut s);
        assert_eq!(s.raw, "---\nfm: 1\n---\n\nmessy   content\n\n<!--\nN\n-->\n");
    }

    #[test]
    fn prettify_without_frontmatter_or_note() {
        let mut s = parse("\njust body\n").slides.remove(0);
        prettify_slide(&mut s);
        assert_eq!(s.raw, "\njust body\n");
        assert_eq!(s.content, "\njust body\n");
    }

    #[test]
    fn round_trip_property_on_varied_decks() {
        let cases = [
            "single slide",
            "# A\n\n---\n\n# B",
            "---\ntitle: T\nlayout: cover\n---\n\nc1\n\n---\nlayout: two-cols\n---\n\nc2\n\n<!--\nn1\n-->",
            "```yaml\nlayout: cover\n```\n\ncontent",
            // Slidev quirk: adjacent separators with a blank line between
            // them produce an empty slide, and stringify then emits one
            // more blank line than the input — excluded from the
            // exact-string property.
        ];
        for md in cases {
            let deck = parse(md);
            let out = stringify(&deck.slides);
            assert_eq!(out.trim(), md.trim(), "round-trip failed for {md:?}");
        }
    }
}
