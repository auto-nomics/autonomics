//! Central color theme for the TUI.
//!
//! Every widget draws through the semantic tokens in [`Theme`] instead of
//! hardcoding `ratatui::style::Color` values, so the whole surface can be
//! re-skinned at runtime. The `dark` preset reproduces the palette that
//! was previously hardcoded across the widget modules — switching to it
//! renders byte-identical to the pre-theme code.
//!
//! Tokens are grouped by role, not by hue:
//! - **text** — `text` (primary), `text_dim` (secondary), `text_faint`
//!   (hints, separators, disabled), `highlight` (list selection markers),
//!   `active_marker` (the active agent's label in lists),
//!   `on_accent` (foreground on accent-filled chips/rows),
//!   `selected_fg` (foreground on [`Theme::selected_bg`] rows),
//!   `overlay_bg`/`overlay_text` (solid chips such as toasts).
//! - **surfaces** — `band_bg` (status-bar / leaf bands), `strip_bg` /
//!   `strip_active` (tab strip), `selected_bg` (selected rows in pickers).
//! - **accents & semantic** — `accent` (primary), `accent_alt`
//!   (secondary), `success`, `warning`, `error`, `info`.
//! - **editor** — text-selection and scrollbar colors for the
//!   textarea's built-in styles.
//! - **md** — markdown renderer tokens ([`MdTokens`], owned by the chat
//!   widget's markdown module).
//!
//! Preset switching is process-global (`RwLock`): reads take a cheap
//! `Copy` snapshot via [`current`], and [`cycle`] / [`set_by_name`]
//! swap the palette. The switch is session-scoped; persistence is left
//! to the caller.

use std::sync::{OnceLock, RwLock};

use ratatui::style::Color;

use crate::widgets::chat_widget::md_theme::MdTokens;

/// The full palette of semantic color tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    // ── text ──────────────────────────────────────────────
    /// Primary text.
    pub text: Color,
    /// Secondary text (labels, dim rows).
    pub text_dim: Color,
    /// Muted text: hints, placeholders, separators, scrollbars.
    pub text_faint: Color,
    /// Selection markers in lists (bibliography panes, KMS tree).
    pub highlight: Color,
    /// Marks the *active* agent in lists (bold label).
    pub active_marker: Color,
    /// Foreground on accent-filled chips and selected rows.
    pub on_accent: Color,
    /// Foreground on [`Theme::selected_bg`] rows.
    pub selected_fg: Color,
    /// Solid chip background (toast backing).
    pub overlay_bg: Color,
    /// Foreground on [`Theme::overlay_bg`].
    pub overlay_text: Color,

    // ── surfaces ──────────────────────────────────────────
    /// Full-width band behind the status bar and leaf headers.
    pub band_bg: Color,
    /// Tab strip background.
    pub strip_bg: Color,
    /// Active tab background on the strip.
    pub strip_active: Color,
    /// Selected-row background in pickers and popups.
    pub selected_bg: Color,

    // ── accents & semantic ────────────────────────────────
    /// Primary accent (picker borders, cursor hints, tool-running).
    pub accent: Color,
    /// Secondary accent (compacting status, message picker identity).
    pub accent_alt: Color,
    /// Success / streaming.
    pub success: Color,
    /// Warning / requesting / retrying.
    pub warning: Color,
    /// Error / destructive.
    pub error: Color,
    /// Informational / waiting.
    pub info: Color,

    // ── editor (textarea built-in styles) ─────────────────
    pub selection_bg: Color,
    pub selection_fg: Color,
    pub scroll_track: Color,
    pub scroll_thumb: Color,

    // ── markdown renderer tokens ──────────────────────────
    pub(crate) md: MdTokens,
}

/// A named preset entry.
pub struct Preset {
    pub name: &'static str,
    pub theme: Theme,
}

/// The registry of built-in presets, in cycling order.
pub fn presets() -> &'static [Preset] {
    use std::sync::LazyLock;
    static PRESETS: LazyLock<[Preset; 2]> = LazyLock::new(|| {
        [
            Preset {
                name: "dark",
                theme: dark(),
            },
            Preset {
                name: "light",
                theme: light(),
            },
        ]
    });
    &*PRESETS
}

/// The default preset: reproduces the pre-theme hardcoded palette.
pub fn dark() -> Theme {
    Theme {
        text: Color::White,
        text_dim: Color::Gray,
        text_faint: Color::DarkGray,
        highlight: Color::LightYellow,
        active_marker: Color::LightRed,
        on_accent: Color::Black,
        selected_fg: Color::Rgb(200, 200, 200),
        overlay_bg: Color::Black,
        overlay_text: Color::White,

        band_bg: Color::Rgb(24, 34, 46),
        strip_bg: Color::Rgb(24, 24, 28),
        strip_active: Color::Rgb(48, 48, 58),
        selected_bg: Color::Rgb(35, 55, 70),

        accent: Color::Cyan,
        accent_alt: Color::Magenta,
        success: Color::Green,
        warning: Color::Yellow,
        error: Color::Red,
        info: Color::Blue,

        selection_bg: Color::Rgb(49, 62, 115),
        selection_fg: Color::Rgb(192, 202, 245),
        scroll_track: Color::Rgb(32, 35, 53),
        scroll_thumb: Color::Rgb(42, 46, 65),

        md: MdTokens::dark(),
    }
}

/// Light preset: dark-on-light text and toned-down accents readable on a
/// white terminal. Syntax highlighting keeps the dark syntect scheme but
/// renders on a light code surface; the chrome tokens adapt.
pub fn light() -> Theme {
    Theme {
        text: Color::Rgb(33, 37, 41),
        text_dim: Color::Rgb(73, 80, 87),
        text_faint: Color::Rgb(134, 142, 150),
        highlight: Color::Rgb(140, 90, 0),
        active_marker: Color::Rgb(200, 40, 40),
        on_accent: Color::White,
        selected_fg: Color::Rgb(20, 35, 45),
        overlay_bg: Color::Black,
        overlay_text: Color::White,

        band_bg: Color::Rgb(233, 236, 242),
        strip_bg: Color::Rgb(228, 231, 238),
        strip_active: Color::Rgb(206, 212, 222),
        selected_bg: Color::Rgb(197, 228, 243),

        accent: Color::Rgb(0, 110, 130),
        accent_alt: Color::Rgb(140, 30, 150),
        success: Color::Rgb(20, 120, 60),
        warning: Color::Rgb(150, 100, 0),
        error: Color::Rgb(190, 30, 40),
        info: Color::Rgb(20, 80, 180),

        selection_bg: Color::Rgb(180, 200, 255),
        selection_fg: Color::Rgb(20, 30, 60),
        scroll_track: Color::Rgb(235, 237, 240),
        scroll_thumb: Color::Rgb(180, 186, 196),

        md: MdTokens::light(),
    }
}

// ── Global slot ────────────────────────────────────────────────────────

struct Active {
    name: &'static str,
    theme: Theme,
}

fn slot() -> &'static RwLock<Active> {
    static SLOT: OnceLock<RwLock<Active>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(Active {
        name: presets()[0].name,
        theme: presets()[0].theme,
    }))
}

/// Snapshot of the active theme. Widgets call this at render time and
/// read token fields off the copy.
pub fn current() -> Theme {
    slot()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .theme
}

/// Name of the active preset.
pub fn active_name() -> &'static str {
    slot()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .name
}

/// Activate the preset with this name. Returns `false` (unchanged) when
/// no preset carries that name.
pub fn set_by_name(name: &str) -> bool {
    let Some(preset) = presets().iter().find(|p| p.name == name) else {
        return false;
    };
    let mut active = slot()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    active.name = preset.name;
    active.theme = preset.theme;
    true
}

/// Switch to the next preset in [`presets`] order. Returns the new
/// preset's name.
pub fn cycle() -> &'static str {
    let mut active = slot()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let idx = presets()
        .iter()
        .position(|p| p.name == active.name)
        .map_or(0, |i| (i + 1) % presets().len());
    active.name = presets()[idx].name;
    active.theme = presets()[idx].theme;
    active.name
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dark preset must keep the exact colors the widgets used to
    /// hardcode — these assertions pin the migration.
    #[test]
    fn dark_preset_matches_the_pre_theme_palette() {
        let t = dark();
        assert_eq!(t.text, Color::White);
        assert_eq!(t.text_dim, Color::Gray);
        assert_eq!(t.text_faint, Color::DarkGray);
        assert_eq!(t.accent, Color::Cyan);
        assert_eq!(t.success, Color::Green);
        assert_eq!(t.warning, Color::Yellow);
        assert_eq!(t.error, Color::Red);
        assert_eq!(t.info, Color::Blue);
        assert_eq!(t.accent_alt, Color::Magenta);
        assert_eq!(t.on_accent, Color::Black);
        assert_eq!(t.band_bg, Color::Rgb(24, 34, 46));
        assert_eq!(t.strip_bg, Color::Rgb(24, 24, 28));
        assert_eq!(t.strip_active, Color::Rgb(48, 48, 58));
        assert_eq!(t.selected_bg, Color::Rgb(35, 55, 70));
        assert_eq!(t.selection_bg, Color::Rgb(49, 62, 115));
    }

    #[test]
    fn default_preset_is_dark_and_cycle_round_trips() {
        assert_eq!(active_name(), "dark");
        assert_eq!(cycle(), "light");
        assert_eq!(active_name(), "light");
        assert_eq!(cycle(), "dark");
        assert_eq!(active_name(), "dark");
    }

    #[test]
    fn set_by_name_rejects_unknown_presets() {
        assert!(!set_by_name("solarized"));
        assert_eq!(active_name(), "dark");
        assert!(set_by_name("light"));
        assert_eq!(active_name(), "light");
        assert!(set_by_name("dark"));
    }

    #[test]
    fn light_preset_differs_from_dark() {
        let (d, l) = (dark(), light());
        assert_ne!(d.text, l.text);
        assert_ne!(d.accent, l.accent);
        assert_ne!(d.selected_bg, l.selected_bg);
        assert_ne!(d.md.surface.raised, l.md.surface.raised);
    }
}
