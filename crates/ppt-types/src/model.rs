//! Deck and slide wire structs (serde camelCase = GUI/agent JSON format).
//!
//! Ported from Slidev `packages/types/src/{types,frontmatter}.ts`:
//! the headmatter is slide 1's frontmatter; every slide's *effective*
//! frontmatter is `headmatter.defaults ⊕ slide frontmatter`
//! ([`SlideFrontmatter::merged`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::aspect::AspectRatio;
use crate::clicks::ClickPlan;
use crate::layout::Layout;

/// Per-slide typed frontmatter (Slidev `Frontmatter` subset we support).
///
/// Everything defaults; unknown keys are preserved in `extras` (flattened)
/// so round-tripping never drops user data.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SlideFrontmatter {
    pub layout: Option<Layout>,
    pub class: Option<String>,
    /// Manual total click override (`frontmatter.clicks`).
    pub clicks: Option<u32>,
    /// Starting click number (default 0).
    pub clicks_start: Option<u32>,
    /// Skip this slide entirely (`hide` / Slidev also accepts `disabled`).
    pub hide: Option<bool>,
    pub hide_in_toc: Option<bool>,
    pub title: Option<String>,
    /// Heading level for the TOC tree.
    pub level: Option<u8>,
    pub route_alias: Option<String>,
    /// Zoom factor applied to the slide canvas.
    pub zoom: Option<f64>,
    /// Figure layouts: asset reference (URL or `assets://{id}` ref).
    pub image: Option<String>,
    pub background: Option<String>,
    pub background_image: Option<String>,
    /// Panel captions for [`Layout::PanelGrid`] (panel A/B/C/D).
    pub panel1: Option<String>,
    pub panel2: Option<String>,
    pub panel3: Option<String>,
    pub panel4: Option<String>,
    /// Biomedical: statistical table caption (e.g. "Table 2: Cox model").
    pub table_caption: Option<String>,
    /// Slidev-equivalent `src:` import is NOT supported (v1: single-file
    /// decks); the key is still preserved via `extras` for round-trip.
    #[serde(flatten)]
    pub extras: BTreeMap<String, Value>,
}

impl SlideFrontmatter {
    /// `base ⊕ over`: `over`'s `Some` fields win; `extras` merge per-key
    /// with `over` winning.
    pub fn merged(base: &Self, over: &Self) -> Self {
        Self {
            layout: over.layout.clone().or_else(|| base.layout.clone()),
            class: over.class.clone().or_else(|| base.class.clone()),
            clicks: over.clicks.or(base.clicks),
            clicks_start: over.clicks_start.or(base.clicks_start),
            hide: over.hide.or(base.hide),
            hide_in_toc: over.hide_in_toc.or(base.hide_in_toc),
            title: over.title.clone().or_else(|| base.title.clone()),
            level: over.level.or(base.level),
            route_alias: over.route_alias.clone().or_else(|| base.route_alias.clone()),
            zoom: over.zoom.or(base.zoom),
            image: over.image.clone().or_else(|| base.image.clone()),
            background: over.background.clone().or_else(|| base.background.clone()),
            background_image: over
                .background_image
                .clone()
                .or_else(|| base.background_image.clone()),
            panel1: over.panel1.clone().or_else(|| base.panel1.clone()),
            panel2: over.panel2.clone().or_else(|| base.panel2.clone()),
            panel3: over.panel3.clone().or_else(|| base.panel3.clone()),
            panel4: over.panel4.clone().or_else(|| base.panel4.clone()),
            table_caption: over
                .table_caption
                .clone()
                .or_else(|| base.table_caption.clone()),
            extras: {
                let mut m = base.extras.clone();
                for (k, v) in &over.extras {
                    m.insert(k.clone(), v.clone());
                }
                m
            },
        }
    }

    /// Effective clicks_start (default 0).
    pub fn effective_clicks_start(&self) -> u32 {
        self.clicks_start.unwrap_or(0)
    }
}

/// Deck-level feature flags (Slidev `SlidevDetectedFeatures` subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DeckFeatures {
    /// `$…$` / `$$…$$` math detected — frontend loads KaTeX.
    pub katex: bool,
    /// ```` ```mermaid ```` fence detected — frontend lazy-loads Mermaid.
    pub mermaid: bool,
}

/// Deck metadata (Slidev `SlidevConfig` + derived state).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DeckMeta {
    pub id: String,
    pub title: String,
    #[serde(alias = "aspectRatio")]
    pub aspect_ratio: Option<AspectRatio>,
    /// Canvas width in px (Slidev `canvasWidth`, default 980).
    pub canvas_width: Option<u32>,
    pub color_schema: Option<ColorSchema>,
    /// Built-in transition name (slide-up/fade/…); None = none.
    pub transition: Option<String>,
    /// Export filename stem (default: deck id).
    pub export_filename: Option<String>,
    /// Headmatter `defaults` — merged under every slide's frontmatter.
    pub defaults: Option<SlideFrontmatter>,
    /// Regex-detected heavy features.
    pub features: DeckFeatures,
    /// Hash of canonical markdown (cache-busting / optimistic concurrency).
    pub revision: String,
    /// Number of visible slides.
    pub slide_count: usize,
    pub created_at: String,
    pub updated_at: String,
}

impl DeckMeta {
    pub fn aspect(&self) -> AspectRatio {
        self.aspect_ratio.unwrap_or_else(AspectRatio::sixteen_nine)
    }

    pub fn canvas(&self) -> u32 {
        self.canvas_width.unwrap_or(crate::DEFAULT_CANVAS_WIDTH)
    }

    pub fn canvas_height(&self) -> u32 {
        self.aspect().height_for(self.canvas())
    }
}

/// One parsed slide.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SlideSpec {
    /// 0-based index in the *visible* deck.
    pub index: usize,
    /// Raw source slice including its frontmatter block (round-trip).
    pub raw: String,
    /// Effective frontmatter (headmatter defaults merged under).
    pub frontmatter: SlideFrontmatter,
    /// Body markdown (note stripped, frontmatter stripped).
    pub content: String,
    /// Speaker note (trailing `<!-- -->` block).
    pub note: Option<String>,
    pub title: Option<String>,
    /// TOC level (heading depth or frontmatter level).
    pub level: Option<u8>,
    /// Image URLs referenced by this slide (preload hints).
    pub images: Vec<String>,
    /// Computed click plan; `None` when the slide has no click directives.
    pub click_plan: Option<ClickPlan>,
    /// Hash of `raw`.
    pub revision: String,
}

impl SlideSpec {
    /// 1-based slide number (wire/API convention).
    pub fn no(&self) -> usize {
        self.index + 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorSchema {
    Light,
    Dark,
    Auto,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frontmatter_parses_and_preserves_extras() {
        let fm: SlideFrontmatter =
            serde_json::from_value(json!({"layout": "two-cols", "custom": {"a": 1}})).unwrap();
        assert_eq!(fm.layout, Some(Layout::TwoCols));
        assert_eq!(fm.extras.get("custom"), Some(&json!({"a": 1})));
        // Round-trip keeps the extra key
        let rt: SlideFrontmatter = serde_json::from_value(serde_json::to_value(&fm).unwrap()).unwrap();
        assert_eq!(rt, fm);
    }

    #[test]
    fn merged_overrides_wins() {
        let base = SlideFrontmatter {
            layout: Some(Layout::Cover),
            clicks: Some(3),
            extras: BTreeMap::from([("keep".into(), json!(1))]),
            ..Default::default()
        };
        let over = SlideFrontmatter {
            layout: Some(Layout::TwoCols),
            extras: BTreeMap::from([("keep".into(), json!(2)), ("new".into(), json!(true))]),
            ..Default::default()
        };
        let m = SlideFrontmatter::merged(&base, &over);
        assert_eq!(m.layout, Some(Layout::TwoCols));
        assert_eq!(m.clicks, Some(3));
        assert_eq!(m.extras.get("keep"), Some(&json!(2)));
        assert_eq!(m.extras.get("new"), Some(&json!(true)));
    }

    #[test]
    fn frontmatter_aliases_snake_case() {
        let fm: SlideFrontmatter =
            serde_json::from_value(json!({"hideInToc": true, "clicksStart": 2, "backgroundImage": "x"}))
                .unwrap();
        assert!(fm.hide_in_toc.unwrap());
        assert_eq!(fm.clicks_start, Some(2));
        assert_eq!(fm.background_image.as_deref(), Some("x"));
    }

    #[test]
    fn deck_meta_canvas_helpers() {
        let meta = DeckMeta {
            id: "deck-x".into(),
            title: "T".into(),
            aspect_ratio: None,
            canvas_width: None,
            color_schema: None,
            transition: None,
            export_filename: None,
            defaults: None,
            features: DeckFeatures::default(),
            revision: "r".into(),
            slide_count: 0,
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert_eq!(meta.canvas(), 980);
        assert_eq!(meta.canvas_height(), 551);
        assert_eq!(meta.aspect(), AspectRatio::sixteen_nine());
    }

    #[test]
    fn color_schema_is_lowercase() {
        assert_eq!(
            serde_json::to_value(ColorSchema::Dark).unwrap(),
            json!("dark")
        );
        assert_eq!(
            serde_json::from_value::<ColorSchema>(json!("auto")).unwrap(),
            ColorSchema::Auto
        );
    }
}
