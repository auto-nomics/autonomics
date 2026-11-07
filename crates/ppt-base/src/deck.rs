//! Deck assembly: parsed source slides → typed [`ppt_types`] wire structs.
//!
//! Merges the headmatter defaults under every slide's frontmatter
//! (Slidev `loaders.ts:62` rule), computes click plans, and produces the
//! [`DeckMeta`] the GUI and agent tools consume.

use serde_json::{Map, Value};

use ppt_types::aspect::AspectRatio;
use ppt_types::clicks::{compute_plan, ClickDirective};
use ppt_types::model::{ColorSchema, DeckFeatures, DeckMeta, SlideFrontmatter, SlideSpec};
use ppt_types::range::parse_range_pages;

use crate::parser::{extract_click_directives, headmatter, parse, revision_hash, SourceSlide};

pub mod templates;

/// A fully assembled deck: metadata + visible slides.
#[derive(Debug, Clone)]
pub struct Deck {
    pub meta: DeckMeta,
    pub slides: Vec<SlideSpec>,
}

impl Deck {
    /// 1-based slide count.
    pub fn len(&self) -> usize {
        self.slides.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slides.is_empty()
    }

    /// Look up a slide by 1-based number.
    pub fn slide(&self, no: usize) -> Option<&SlideSpec> {
        self.slides.get(no.checked_sub(1)?)
    }

    /// Selected page indices from a Slidev-style range string
    /// (`"1,4-5"`); `None` = all pages.
    pub fn resolve_range(&self, range: Option<&str>) -> Result<Vec<usize>, ppt_types::ParseRangeError> {
        match range {
            None => Ok((0..self.slides.len()).collect()),
            Some(r) => parse_range_pages(r, self.slides.len()),
        }
    }
}

/// Assemble a deck from canonical markdown.
///
/// `created_at`/`updated_at` are RFC-3339 strings owned by the caller
/// (the store); `id` likewise.
pub fn assemble(markdown: &str, id: &str, created_at: &str, updated_at: &str) -> Deck {
    let parsed = parse(markdown);
    let hm = headmatter(&parsed);

    let defaults = hm
        .get("defaults")
        .map(|v| typed_frontmatter(v))
        .unwrap_or_default();

    let aspect = hm
        .get("aspectRatio")
        .and_then(Value::as_str)
        .and_then(|s| AspectRatio::parse(s).ok())
        .unwrap_or_else(AspectRatio::sixteen_nine);

    let color_schema = hm
        .get("colorSchema")
        .and_then(Value::as_str)
        .and_then(|s| match s {
            "light" => Some(ColorSchema::Light),
            "dark" => Some(ColorSchema::Dark),
            "auto" => Some(ColorSchema::Auto),
            _ => None,
        });

    let mut slides = Vec::new();
    for source in &parsed.slides {
        let slide_fm = typed_frontmatter(&Value::Object(source.frontmatter.clone()));
        let effective = SlideFrontmatter::merged(&defaults, &slide_fm);
        // Slidev: `hide` (and the legacy alias `disabled`) skips the slide
        let hide = effective.hide.unwrap_or(false)
            || source
                .frontmatter
                .get("disabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        if hide {
            continue;
        }

        let directives: Vec<ClickDirective> = extract_click_directives(&source.content);
        let click_plan = if directives.is_empty() {
            None
        } else {
            Some(compute_plan(
                &directives,
                effective.clicks,
                effective.effective_clicks_start(),
            ))
        };

        slides.push(SlideSpec {
            index: slides.len(),
            raw: source.raw.clone(),
            frontmatter: effective,
            content: source.content.clone(),
            note: source.note.clone(),
            title: source.title.clone(),
            level: source.level,
            images: source.images.clone(),
            click_plan,
            revision: source.revision.clone(),
        });
    }

    let features = DeckFeatures {
        katex: parsed.features.katex,
        mermaid: parsed.features.mermaid,
    };

    // Deck title: headmatter `title`, falling back to the first visible
    // slide's title (friendly for agent-authored frontmatter-less decks).
    let title = hm
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| slides.first().and_then(|s| s.title.clone()))
        .unwrap_or_default();

    let meta = DeckMeta {
        id: id.to_string(),
        title,
        aspect_ratio: Some(aspect),
        canvas_width: hm
            .get("canvasWidth")
            .and_then(Value::as_u64)
            .map(|v| v as u32),
        color_schema,
        transition: hm
            .get("transition")
            .and_then(Value::as_str)
            .map(str::to_string),
        export_filename: hm
            .get("exportFilename")
            .and_then(Value::as_str)
            .map(str::to_string),
        defaults: Some(defaults),
        features,
        revision: revision_hash(markdown.trim()),
        slide_count: slides.len(),
        created_at: created_at.to_string(),
        updated_at: updated_at.to_string(),
    };

    Deck { meta, slides }
}

/// Typed frontmatter from a JSON object value (unknown keys preserved
/// in `extras` by the serde flatten).
pub fn typed_frontmatter(v: &Value) -> SlideFrontmatter {
    serde_json::from_value(v.clone()).unwrap_or_default()
}

/// The headmatter map of a markdown source (slide 1's frontmatter).
pub fn deck_headmatter(markdown: &str) -> Map<String, Value> {
    headmatter(&parse(markdown))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn assemble_basics() {
        let md = "---\ntitle: My Deck\naspectRatio: 4/3\ncanvasWidth: 900\n---\n\n# One\n\n---\n\n## Two\n\n<v-click>A</v-click>\n\n<v-click>B</v-click>\n\n---\nhide: true\n---\n\nskipped\n";
        let deck = assemble(md, "deck-1", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z");
        assert_eq!(deck.meta.title, "My Deck");
        assert_eq!(deck.meta.slide_count, 2); // hidden slide excluded
        assert_eq!(deck.len(), 2);
        assert_eq!(deck.meta.aspect_ratio, Some(AspectRatio { width: 4, height: 3 }));
        assert_eq!(deck.meta.canvas_width, Some(900));
        assert_eq!(deck.meta.canvas_height(), 675);

        let s1 = deck.slide(2).unwrap();
        assert_eq!(s1.title.as_deref(), Some("Two"));
        assert_eq!(s1.level, Some(2));
        let plan = s1.click_plan.as_ref().unwrap();
        assert_eq!(plan.total, 2);
        assert_eq!(plan.elements[0].start, 1);
        assert_eq!(plan.elements[1].start, 2);

        // hidden slide is not addressable
        assert!(deck.slide(3).is_none());
    }

    #[test]
    fn defaults_merge_under_slides() {
        // Slide 1's frontmatter IS the headmatter: deck defaults plus its
        // own layout.
        let md = "---\ndefaults:\n  layout: two-cols\n  class: wide\nlayout: cover\n---\n\none\n\n---\n\ntwo\n";
        let deck = assemble(md, "d", "t", "t");
        // slide 1 overrides layout, keeps class
        assert_eq!(deck.slides[0].frontmatter.layout.clone().map(|l| l.to_string()), Some("cover".into()));
        assert_eq!(deck.slides[0].frontmatter.class.as_deref(), Some("wide"));
        assert_eq!(deck.slides[1].frontmatter.layout.clone().map(|l| l.to_string()), Some("two-cols".into()));
        assert_eq!(deck.slides[1].frontmatter.class.as_deref(), Some("wide"));
    }

    #[test]
    fn features_detected_deck_wide() {
        let md = "---\ntitle: T\n---\n\nmath $x^2$ and\n\n```mermaid\ngraph TD\n```\n";
        let deck = assemble(md, "d", "t", "t");
        assert!(deck.meta.features.katex);
        assert!(deck.meta.features.mermaid);
    }

    #[test]
    fn frontmatter_must_be_first_slide() {
        // frontmatter on slide 2 is NOT deck-level config
        let md = "intro\n\n---\ntitle: Not Deck Title\naspectRatio: 1:1\n---\n\nbody\n";
        let deck = assemble(md, "d", "t", "t");
        assert_eq!(deck.meta.title, "");
        assert_eq!(deck.meta.aspect_ratio, Some(AspectRatio::sixteen_nine()));
        // but slide 2 does see its own typed frontmatter
        assert_eq!(deck.slides[1].frontmatter.title.as_deref(), Some("Not Deck Title"));
    }

    #[test]
    fn range_resolution() {
        let md = "a\n\n---\n\nb\n\n---\n\nc\n\n---\n\nd\n\n---\n\ne\n";
        let deck = assemble(md, "d", "t", "t");
        assert_eq!(deck.resolve_range(None).unwrap(), [0, 1, 2, 3, 4]);
        assert_eq!(deck.resolve_range(Some("1,4-5")).unwrap(), [0, 3, 4]);
        assert!(deck.resolve_range(Some("6")).is_err());
    }

    #[test]
    fn extras_survive_assembly() {
        let md = "---\ncustomKey: hello\n---\n\nbody\n";
        let deck = assemble(md, "d", "t", "t");
        assert_eq!(
            deck.slides[0].frontmatter.extras.get("customKey"),
            Some(&json!("hello"))
        );
    }
}
