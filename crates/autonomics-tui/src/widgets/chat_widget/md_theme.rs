//! Theme tokens for the markdown renderer.
//!
//! Hardcoded dark-theme values, synthesised from phloem-tui's existing
//! `PhloemStyleSheet` and markdown-reader's `default_()` theme.

use ratatui::style::Color;

/// Surface tiers — backgrounds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MdSurface {
    /// Code block / raised surface background.
    pub raised: Color,
}

/// Text colours.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MdText {
    pub primary: Color,
    pub muted: Color,
}

/// Syntax-highlighting and code-block colours.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MdSyntax {
    pub inline_code: Color,
    /// Default foreground for unhighlighted code.
    pub code_fg: Color,
    /// Border colour for fenced code-box frames.
    pub code_border: Color,
}

/// Heading hierarchy colours.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MdHeading {
    pub h1: Color,
    pub h2: Color,
    pub h3: Color,
    /// h4–h6 and any other headings.
    pub other: Color,
}

/// List markers and block-quote chrome.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MdList {
    pub marker: Color,
    pub task_marker: Color,
    pub block_quote_fg: Color,
    pub block_quote_border: Color,
}

/// Table chrome.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MdTable {
    pub header: Color,
    pub border: Color,
}

/// Accent colours.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MdAccent {
    pub link: Color,
}

/// Top-level design-token bag for the markdown renderer.
#[derive(Debug, Clone, Copy)]
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

    /// Syntect theme name for code-block highlighting.
    pub(crate) fn syntax_theme_name() -> &'static str {
        "base16-ocean.dark"
    }
}
