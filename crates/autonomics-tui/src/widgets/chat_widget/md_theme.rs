//! Theme tokens for the markdown renderer.
//!
//! Per-preset token sets: `dark()` synthesised from phloem-tui's existing
//! `PhloemStyleSheet` and markdown-reader's `default_()` theme;
//! `claude_code()` ported from cola-runner/awesome-tui-design's
//! `designs/claude-code/DESIGN.md`; every other registered theme derives
//! from its DESIGN.md palette via [`MdTokens::from_palette`]. The active
//! set is carried on the global [`crate::theme::Theme`].

use ratatui::style::Color;

/// Surface tiers — backgrounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MdSurface {
    /// Code block / raised surface background.
    pub raised: Color,
}

/// Text colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MdText {
    pub primary: Color,
    pub muted: Color,
}

/// Syntax-highlighting and code-block colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MdSyntax {
    pub inline_code: Color,
    /// Default foreground for unhighlighted code.
    pub code_fg: Color,
    /// Border colour for fenced code-box frames.
    pub code_border: Color,
}

/// Heading hierarchy colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MdHeading {
    pub h1: Color,
    pub h2: Color,
    pub h3: Color,
    /// h4–h6 and any other headings.
    pub other: Color,
}

/// List markers and block-quote chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MdList {
    pub marker: Color,
    pub task_marker: Color,
    pub block_quote_fg: Color,
    pub block_quote_border: Color,
}

/// Table chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MdTable {
    pub header: Color,
    pub border: Color,
}

/// Accent colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MdAccent {
    pub link: Color,
}

/// Top-level design-token bag for the markdown renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MdTokens {
    pub surface: MdSurface,
    pub text: MdText,
    pub syntax: MdSyntax,
    pub heading: MdHeading,
    pub list: MdList,
    pub table: MdTable,
    pub accent: MdAccent,
}

impl MdTokens {
    /// Return hardcoded dark-theme tokens matching phloem-tui's palette.
    pub(crate) fn dark() -> Self {
        Self {
            surface: MdSurface {
                raised: Color::Rgb(40, 40, 40),
            },
            text: MdText {
                primary: Color::Rgb(220, 220, 220),
                muted: Color::DarkGray,
            },
            syntax: MdSyntax {
                inline_code: Color::Rgb(180, 200, 180),
                code_fg: Color::Rgb(200, 200, 200),
                code_border: Color::Rgb(100, 100, 120),
            },
            heading: MdHeading {
                h1: Color::Rgb(220, 220, 255),
                h2: Color::Rgb(180, 180, 255),
                h3: Color::Rgb(160, 160, 240),
                other: Color::Rgb(140, 140, 220),
            },
            list: MdList {
                marker: Color::Rgb(180, 180, 100),
                task_marker: Color::Rgb(180, 180, 100),
                block_quote_fg: Color::Rgb(180, 180, 100),
                block_quote_border: Color::Rgb(100, 100, 120),
            },
            table: MdTable {
                header: Color::Rgb(180, 180, 255),
                border: Color::Rgb(100, 100, 120),
            },
            accent: MdAccent { link: Color::Cyan },
        }
    }

    /// Claude Code preset, ported from cola-runner/awesome-tui-design's
    /// `designs/claude-code/DESIGN.md`. Accent values are that document's
    /// hex palette; structural neutrals (borders) use its `#505050`
    /// Subtle divider color, and the code surface is its documented
    /// `rgb(65,60,65)` tool-block background.
    pub(crate) fn claude_code() -> Self {
        Self {
            surface: MdSurface {
                raised: Color::Rgb(65, 60, 65), // rgb(65,60,65) bash output bg
            },
            text: MdText {
                primary: Color::Rgb(255, 255, 255), // Foreground
                muted: Color::Rgb(136, 136, 136),   // #888888 Muted
            },
            syntax: MdSyntax {
                inline_code: Color::Rgb(235, 159, 127), // #eb9f7f shimmer
                code_fg: Color::Rgb(255, 255, 255),
                code_border: Color::Rgb(80, 80, 80), // #505050 Subtle
            },
            heading: MdHeading {
                h1: Color::Rgb(215, 119, 87),     // #d77757 Primary (H1 spec)
                h2: Color::Rgb(235, 159, 127),    // #eb9f7f shimmer
                h3: Color::Rgb(255, 255, 255),    // bold modifier carries weight
                other: Color::Rgb(153, 153, 153), // #999999 Inactive
            },
            list: MdList {
                marker: Color::Rgb(215, 119, 87), // #d77757 bullets
                task_marker: Color::Rgb(215, 119, 87),
                block_quote_fg: Color::Rgb(153, 153, 153),
                block_quote_border: Color::Rgb(80, 80, 80),
            },
            table: MdTable {
                header: Color::Rgb(235, 159, 127),
                border: Color::Rgb(80, 80, 80),
            },
            accent: MdAccent {
                link: Color::Rgb(177, 185, 249), // #b1b9f9 lavender
            },
        }
    }

    /// Derive the markdown token set from a DESIGN.md palette. Accents
    /// follow the claude-code mapping (primary headings, secondary-leaning
    /// inline code), structural neutrals blend toward the background.
    pub(crate) fn from_palette(p: &crate::theme::Palette) -> Self {
        let border = mix(p.muted, p.bg, 0.4);
        Self {
            surface: MdSurface {
                raised: p.surface,
            },
            text: MdText {
                primary: p.fg,
                muted: p.muted,
            },
            syntax: MdSyntax {
                inline_code: mix(p.secondary, p.fg, 0.35),
                code_fg: p.fg,
                code_border: border,
            },
            heading: MdHeading {
                h1: p.primary,
                h2: mix(p.primary, p.fg, 0.4),
                h3: p.fg,
                other: p.muted,
            },
            list: MdList {
                marker: p.primary,
                task_marker: p.primary,
                block_quote_fg: p.muted,
                block_quote_border: border,
            },
            table: MdTable {
                header: mix(p.primary, p.fg, 0.4),
                border,
            },
            accent: MdAccent { link: p.accent },
        }
    }

    /// Syntect theme name for code-block highlighting.
    pub(crate) fn syntax_theme_name() -> &'static str {
        "base16-ocean.dark"
    }
}

/// Linear blend of two RGB colors at `t` (0 = `a`, 1 = `b`); non-RGB
/// colors pass through. Local copy of [`crate::theme::mix`] — the token
/// module sits below the theme module and keeps its own tiny helper.
fn mix(a: Color, b: Color, t: f32) -> Color {
    let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (a, b) else {
        return a;
    };
    let blend = |c1: u8, c2: u8| (c1 as f32 + (c2 as f32 - c1 as f32) * t).round() as u8;
    Color::Rgb(blend(r1, r2), blend(g1, g2), blend(b1, b2))
}
