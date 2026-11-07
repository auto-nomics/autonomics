//! Layout vocabulary.
//!
//! Slidev built-ins (`packages/types/src/builtin-layouts.ts`) minus the
//! app-specific ones (`404`, `error`, `iframe*` — error pages are app
//! concerns and iframes violate the offline/no-CDN constraint), plus
//! biomedical additions for research decks.

use std::fmt;

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layout {
    // ---- Slidev ports -----------------------------------------------------
    /// Title page.
    Cover,
    /// Default content layout.
    Default,
    /// Section divider.
    Section,
    TwoCols,
    TwoColsHeader,
    ImageLeft,
    ImageRight,
    /// Full-bleed content.
    Full,
    Center,
    Quote,
    /// Highlighted fact/statement with large numbers.
    Fact,
    Statement,
    Intro,
    End,
    /// No layout chrome at all — raw content fills the canvas.
    None,
    // ---- Biomedical additions --------------------------------------------
    /// Figure on the left, caption/prose slot on the right.
    FigureLeft,
    /// Figure on the right, caption/prose slot on the left.
    FigureRight,
    /// 2×2 labelled panel grid (frontmatter `panel1..4`), panel letters
    /// A/B/C/D — the SCI multi-panel figure convention.
    PanelGrid,
    /// Publication-style statistics table (coefficient/HR/OR + CI + p).
    StatsTable,
    /// Numbered reference list, small type, two-column overflow.
    References,
    /// Unknown layout name — renders as [`Layout::Default`] with a warning.
    Unknown(String),
}

impl Layout {
    pub fn as_str(&self) -> &str {
        match self {
            Layout::Cover => "cover",
            Layout::Default => "default",
            Layout::Section => "section",
            Layout::TwoCols => "two-cols",
            Layout::TwoColsHeader => "two-cols-header",
            Layout::ImageLeft => "image-left",
            Layout::ImageRight => "image-right",
            Layout::Full => "full",
            Layout::Center => "center",
            Layout::Quote => "quote",
            Layout::Fact => "fact",
            Layout::Statement => "statement",
            Layout::Intro => "intro",
            Layout::End => "end",
            Layout::None => "none",
            Layout::FigureLeft => "figure-left",
            Layout::FigureRight => "figure-right",
            Layout::PanelGrid => "panel-grid",
            Layout::StatsTable => "stats-table",
            Layout::References => "references",
            Layout::Unknown(s) => s,
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "cover" => Layout::Cover,
            "default" => Layout::Default,
            "section" => Layout::Section,
            "two-cols" => Layout::TwoCols,
            "two-cols-header" => Layout::TwoColsHeader,
            "image-left" => Layout::ImageLeft,
            "image-right" => Layout::ImageRight,
            "full" => Layout::Full,
            "center" => Layout::Center,
            "quote" => Layout::Quote,
            "fact" => Layout::Fact,
            "statement" => Layout::Statement,
            "intro" => Layout::Intro,
            "end" => Layout::End,
            "none" => Layout::None,
            "figure-left" => Layout::FigureLeft,
            "figure-right" => Layout::FigureRight,
            "panel-grid" => Layout::PanelGrid,
            "stats-table" => Layout::StatsTable,
            "references" => Layout::References,
            other => Layout::Unknown(other.to_string()),
        }
    }
}

impl fmt::Display for Layout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Layout {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Layout {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Layout::parse(&s))
    }
}

/// A named layout plus any extra class names (frontmatter `class`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedLayout {
    pub layout: Layout,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
}

impl ResolvedLayout {
    /// Slidev's resolution rule: slide 1 defaults to `cover`, others to
    /// `default`; unknown names fall back to `default`.
    pub fn resolve(layout: Option<&Layout>, slide_index: usize) -> Self {
        let layout = match layout {
            Some(Layout::Unknown(_)) | None => {
                if slide_index == 0 {
                    Layout::Cover
                } else {
                    Layout::Default
                }
            }
            Some(l) => l.clone(),
        };
        Self {
            layout,
            class: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_names_round_trip() {
        for name in [
            "cover",
            "default",
            "two-cols",
            "two-cols-header",
            "image-left",
            "figure-left",
            "panel-grid",
            "stats-table",
            "references",
        ] {
            assert_eq!(Layout::parse(name).as_str(), name);
        }
    }

    #[test]
    fn unknown_layout_preserves_name() {
        let l = Layout::parse("fancy-theme");
        assert_eq!(l, Layout::Unknown("fancy-theme".into()));
        assert_eq!(l.as_str(), "fancy-theme");
    }

    #[test]
    fn resolve_first_slide_covers() {
        assert_eq!(ResolvedLayout::resolve(None, 0).layout, Layout::Cover);
        assert_eq!(ResolvedLayout::resolve(None, 1).layout, Layout::Default);
        // Unknown falls back like None
        assert_eq!(
            ResolvedLayout::resolve(Some(&Layout::Unknown("x".into())), 3).layout,
            Layout::Default
        );
        // Explicit layout is kept
        assert_eq!(
            ResolvedLayout::resolve(Some(&Layout::TwoCols), 1).layout,
            Layout::TwoCols
        );
    }

    #[test]
    fn serde_uses_hyphenated_names() {
        assert_eq!(
            serde_json::to_value(Layout::TwoColsHeader).unwrap(),
            serde_json::json!("two-cols-header")
        );
        assert_eq!(
            serde_json::from_value::<Layout>(serde_json::json!("figure-right")).unwrap(),
            Layout::FigureRight
        );
    }
}
