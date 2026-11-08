//! Central color theme for the TUI.
//!
//! Every widget draws through the semantic tokens in [`Theme`] instead of
//! hardcoding `ratatui::style::Color` values, so the whole surface can be
//! re-skinned at runtime. The project's own `dark` preset leads (and is
//! the default); `claude-code` carries its hand-mapped port from
//! cola-runner/awesome-tui-design, and the remaining fifteen themes from
//! that repository are registered via the shared [`Palette`] derivation.
//! The `dark` preset reproduces the palette that was previously hardcoded
//! across the widget modules — switching to it renders byte-identical to
//! the pre-theme code.
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

use std::sync::atomic::{AtomicU64, Ordering};
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
    /// Human-readable name shown in theme switching feedback.
    pub label: &'static str,
    pub theme: Theme,
}

/// The registry of built-in presets, in cycling order. The project's own
/// `dark` preset leads as the default; `claude-code` follows with its
/// hand-mapped tokens, then every theme ported from
/// cola-runner/awesome-tui-design.
pub fn presets() -> &'static [Preset] {
    use std::sync::LazyLock;
    static PRESETS: LazyLock<Vec<Preset>> = LazyLock::new(|| {
        let mut all = vec![
            Preset {
                name: "dark",
                label: "Default Dark",
                theme: dark(),
            },
            Preset {
                name: "claude-code",
                label: "Claude Code",
                theme: claude_code(),
            },
        ];
        for (name, palette) in PORTED.iter() {
            all.push(Preset {
                name,
                label: name,
                theme: palette.theme(),
            });
        }
        all
    });
    &PRESETS
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

/// Claude Code preset — ported from cola-runner/awesome-tui-design's
/// `designs/claude-code/DESIGN.md`. Accent and semantic values come
/// straight from that document's palette tables; the few structural
/// neutrals the token set needs beyond it (tab strip, selection,
/// scrollbar) are derived between its Background `#1a1a1a` and Surface
/// `#373737` and noted inline. Syntax highlighting keeps the dark
/// syntect scheme and renders on the documented warm tool-block surface.
pub fn claude_code() -> Theme {
    Theme {
        text: Color::Rgb(255, 255, 255),          // Foreground
        text_dim: Color::Rgb(153, 153, 153),      // #999999 Inactive
        text_faint: Color::Rgb(136, 136, 136),    // #888888 Muted
        highlight: Color::Rgb(255, 193, 7),       // #ffc107 amber
        active_marker: Color::Rgb(235, 159, 127), // #eb9f7f shimmer terracotta
        on_accent: Color::Rgb(26, 26, 26),        // #1a1a1a on terracotta
        selected_fg: Color::Rgb(255, 255, 255),
        overlay_bg: Color::Rgb(55, 55, 55), // #373737 Surface
        overlay_text: Color::Rgb(255, 255, 255),

        band_bg: Color::Rgb(26, 26, 26),      // #1a1a1a Background
        strip_bg: Color::Rgb(38, 38, 38),     // derived between bg & surface
        strip_active: Color::Rgb(55, 55, 55), // #373737 Surface
        selected_bg: Color::Rgb(68, 68, 68),  // derived neutral selection

        accent: Color::Rgb(215, 119, 87), // #d77757 Primary terracotta
        accent_alt: Color::Rgb(253, 93, 177), // #fd5db1 hot pink (tool borders)
        success: Color::Rgb(78, 186, 101), // #4eba65
        warning: Color::Rgb(255, 193, 7), // #ffc107
        error: Color::Rgb(255, 107, 128), // #ff6b80
        info: Color::Rgb(177, 185, 249),  // #b1b9f9 lavender (permission)

        selection_bg: Color::Rgb(68, 68, 68), // derived neutral
        selection_fg: Color::Rgb(255, 255, 255),
        scroll_track: Color::Rgb(47, 47, 47), // derived neutral
        scroll_thumb: Color::Rgb(68, 68, 68),

        md: MdTokens::claude_code(),
    }
}

// ── Ported palettes (cola-runner/awesome-tui-design) ──────────────────

/// The ten semantic roles every theme in cola-runner/awesome-tui-design
/// defines in its DESIGN.md "Semantic Roles" table. Themes expand into
/// full [`Theme`] token sets through [`Palette::theme`]; the hand-tuned
/// exceptions (`dark`, `claude-code`) keep their own constructors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Terminal background.
    pub bg: Color,
    /// Default text.
    pub fg: Color,
    /// Primary accent (brand / focus).
    pub primary: Color,
    /// Secondary accent (tool chrome, supporting).
    pub secondary: Color,
    /// Highlight accent (links, informational).
    pub accent: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    /// Dim text / borders.
    pub muted: Color,
    /// Raised surface.
    pub surface: Color,
}

/// Linear blend of two colors at `t` (0 = `a`, 1 = `b`). Non-RGB colors
/// pass through as `a` — every ported palette is RGB, so this only guards
/// hand-tuned tokens.
fn mix(a: Color, b: Color, t: f32) -> Color {
    let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (a, b) else {
        return a;
    };
    let blend = |c1: u8, c2: u8| (c1 as f32 + (c2 as f32 - c1 as f32) * t).round() as u8;
    Color::Rgb(blend(r1, r2), blend(g1, g2), blend(b1, b2))
}

/// Rec.601-ish luma, for contrast picks.
fn luma(c: Color) -> f32 {
    match c {
        Color::Rgb(r, g, b) => 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32,
        _ => 128.0,
    }
}

/// Whichever of `fg`/`bg` sits farther from `accent` in luma — readable
/// text on an accent-filled chip for both dark- and light-accent palettes.
fn contrast_on(accent: Color, fg: Color, bg: Color) -> Color {
    let la = luma(accent);
    if (luma(bg) - la).abs() >= (luma(fg) - la).abs() {
        bg
    } else {
        fg
    }
}

impl Palette {
    /// Expand the ten documented roles into the full token set.
    ///
    /// The mapping follows the claude-code hand port: primary drives the
    /// accent and heading h1, secondary drives tool chrome and h2, the
    /// palette's highlight accent drives `info` and links. Structural
    /// neutrals the documents don't define (tab strip, scrollbar,
    /// selection) blend between `bg`/`surface`/`primary` so every theme
    /// stays coherent without inventing new colors.
    pub fn theme(&self) -> Theme {
        let selected_bg = mix(self.surface, self.primary, 0.25);
        Theme {
            text: self.fg,
            text_dim: mix(self.fg, self.bg, 0.3),
            text_faint: self.muted,
            highlight: self.warning,
            active_marker: mix(self.primary, self.fg, 0.4),
            on_accent: contrast_on(self.primary, self.fg, self.bg),
            selected_fg: self.fg,
            overlay_bg: self.surface,
            overlay_text: self.fg,

            band_bg: self.bg,
            strip_bg: mix(self.bg, self.surface, 0.5),
            strip_active: self.surface,
            selected_bg,

            accent: self.primary,
            accent_alt: self.secondary,
            success: self.success,
            warning: self.warning,
            error: self.error,
            info: self.accent,

            selection_bg: selected_bg,
            selection_fg: self.fg,
            scroll_track: mix(self.bg, self.surface, 0.55),
            scroll_thumb: mix(self.surface, self.fg, 0.15),

            md: MdTokens::from_palette(self),
        }
    }
}

/// Themes ported from cola-runner/awesome-tui-design. Each palette's hex
/// values come verbatim from that theme's DESIGN.md "Semantic Roles"
/// table; `gemini-cli` is registered under the shorter name `gemini`.
static PORTED: std::sync::LazyLock<Vec<(&'static str, Palette)>> = std::sync::LazyLock::new(|| {
    vec![
        (
            "codex",
            Palette {
                bg: Color::Rgb(0, 0, 0),              // #000000
                fg: Color::Rgb(224, 224, 224),        // #e0e0e0
                primary: Color::Rgb(224, 224, 224),   // #e0e0e0 (monochrome)
                secondary: Color::Rgb(128, 128, 128), // #808080
                accent: Color::Rgb(34, 197, 94),      // #22c55e
                success: Color::Rgb(34, 197, 94),     // #22c55e
                warning: Color::Rgb(234, 179, 8),     // #eab308
                error: Color::Rgb(239, 68, 68),       // #ef4444
                muted: Color::Rgb(85, 85, 85),        // #555555
                surface: Color::Rgb(26, 26, 26),      // #1a1a1a
            },
        ),
        (
            "gemini",
            Palette {
                bg: Color::Rgb(26, 26, 46),           // #1a1a2e
                fg: Color::Rgb(224, 224, 224),        // #e0e0e0
                primary: Color::Rgb(135, 175, 255),   // #87afff
                secondary: Color::Rgb(215, 175, 255), // #d7afff
                accent: Color::Rgb(135, 215, 215),    // #87d7d7
                success: Color::Rgb(215, 255, 215),   // #d7ffd7
                warning: Color::Rgb(255, 255, 175),   // #ffffaf
                error: Color::Rgb(255, 135, 175),     // #ff87af
                muted: Color::Rgb(128, 128, 128),     // #808080
                surface: Color::Rgb(42, 42, 62),      // #2a2a3e
            },
        ),
        (
            "lazygit",
            Palette {
                bg: Color::Rgb(0, 0, 0),             // #000000
                fg: Color::Rgb(212, 212, 212),       // #d4d4d4
                primary: Color::Rgb(34, 197, 94),    // #22c55e
                secondary: Color::Rgb(59, 130, 246), // #3b82f6
                accent: Color::Rgb(6, 182, 212),     // #06b6d4
                success: Color::Rgb(34, 197, 94),    // #22c55e
                warning: Color::Rgb(234, 179, 8),    // #eab308
                error: Color::Rgb(239, 68, 68),      // #ef4444
                muted: Color::Rgb(107, 114, 128),    // #6b7280
                surface: Color::Rgb(30, 58, 95),     // #1e3a5f
            },
        ),
        (
            "k9s",
            Palette {
                bg: Color::Rgb(0, 0, 0),             // #000000
                fg: Color::Rgb(95, 158, 160),        // #5f9ea0
                primary: Color::Rgb(0, 255, 255),    // #00ffff
                secondary: Color::Rgb(30, 144, 255), // #1e90ff
                accent: Color::Rgb(255, 0, 255),     // #ff00ff
                success: Color::Rgb(173, 255, 47),   // #adff2f
                warning: Color::Rgb(255, 140, 0),    // #ff8c00
                error: Color::Rgb(255, 69, 0),       // #ff4500
                muted: Color::Rgb(128, 128, 128),    // #808080
                surface: Color::Rgb(26, 26, 46),     // #1a1a2e
            },
        ),
        (
            "btop",
            Palette {
                bg: Color::Rgb(0, 0, 0),              // #000000
                fg: Color::Rgb(204, 204, 204),        // #cccccc
                primary: Color::Rgb(238, 238, 238),   // #eeeeee
                secondary: Color::Rgb(119, 202, 155), // #77ca9b
                accent: Color::Rgb(220, 76, 76),      // #dc4c4c
                success: Color::Rgb(119, 202, 155),   // #77ca9b
                warning: Color::Rgb(203, 192, 108),   // #cbc06c
                error: Color::Rgb(220, 76, 76),       // #dc4c4c
                muted: Color::Rgb(85, 85, 85),        // #555555
                surface: Color::Rgb(17, 17, 17),      // #111111
            },
        ),
        (
            "dracula",
            Palette {
                bg: Color::Rgb(40, 42, 54),          // #282a36
                fg: Color::Rgb(248, 248, 242),       // #f8f8f2
                primary: Color::Rgb(189, 147, 249),  // #bd93f9
                secondary: Color::Rgb(98, 114, 164), // #6272a4
                accent: Color::Rgb(255, 121, 198),   // #ff79c6
                success: Color::Rgb(80, 250, 123),   // #50fa7b
                warning: Color::Rgb(241, 250, 140),  // #f1fa8c
                error: Color::Rgb(255, 85, 85),      // #ff5555
                muted: Color::Rgb(98, 114, 164),     // #6272a4
                surface: Color::Rgb(68, 71, 90),     // #44475a
            },
        ),
        (
            "catppuccin",
            Palette {
                bg: Color::Rgb(30, 30, 46),           // #1e1e2e
                fg: Color::Rgb(205, 214, 244),        // #cdd6f4
                primary: Color::Rgb(203, 166, 247),   // #cba6f7
                secondary: Color::Rgb(137, 180, 250), // #89b4fa
                accent: Color::Rgb(245, 194, 231),    // #f5c2e7
                success: Color::Rgb(166, 227, 161),   // #a6e3a1
                warning: Color::Rgb(249, 226, 175),   // #f9e2af
                error: Color::Rgb(243, 139, 168),     // #f38ba8
                muted: Color::Rgb(88, 91, 112),       // #585b70
                surface: Color::Rgb(49, 50, 68),      // #313244
            },
        ),
        (
            "nord",
            Palette {
                bg: Color::Rgb(46, 52, 64),           // #2e3440
                fg: Color::Rgb(236, 239, 244),        // #eceff4
                primary: Color::Rgb(136, 192, 208),   // #88c0d0
                secondary: Color::Rgb(129, 161, 193), // #81a1c1
                accent: Color::Rgb(94, 129, 172),     // #5e81ac
                success: Color::Rgb(163, 190, 140),   // #a3be8c
                warning: Color::Rgb(235, 203, 139),   // #ebcb8b
                error: Color::Rgb(191, 97, 106),      // #bf616a
                muted: Color::Rgb(76, 86, 106),       // #4c566a
                surface: Color::Rgb(59, 66, 82),      // #3b4252
            },
        ),
        (
            "gruvbox",
            Palette {
                bg: Color::Rgb(40, 40, 40),           // #282828
                fg: Color::Rgb(235, 219, 178),        // #ebdbb2
                primary: Color::Rgb(254, 128, 25),    // #fe8019
                secondary: Color::Rgb(211, 134, 155), // #d3869b
                accent: Color::Rgb(250, 189, 47),     // #fabd2f
                success: Color::Rgb(184, 187, 38),    // #b8bb26
                warning: Color::Rgb(250, 189, 47),    // #fabd2f
                error: Color::Rgb(251, 73, 52),       // #fb4934
                muted: Color::Rgb(102, 92, 84),       // #665c54
                surface: Color::Rgb(60, 56, 54),      // #3c3836
            },
        ),
        (
            "tokyo-night",
            Palette {
                bg: Color::Rgb(36, 40, 59),           // #24283b
                fg: Color::Rgb(192, 202, 245),        // #c0caf5
                primary: Color::Rgb(122, 162, 247),   // #7aa2f7
                secondary: Color::Rgb(187, 154, 247), // #bb9af7
                accent: Color::Rgb(125, 207, 255),    // #7dcfff
                success: Color::Rgb(158, 206, 106),   // #9ece6a
                warning: Color::Rgb(224, 175, 104),   // #e0af68
                error: Color::Rgb(247, 118, 142),     // #f7768e
                muted: Color::Rgb(86, 95, 137),       // #565f89
                surface: Color::Rgb(26, 27, 38),      // #1a1b26
            },
        ),
        (
            "rose-pine",
            Palette {
                bg: Color::Rgb(25, 23, 36),           // #191724
                fg: Color::Rgb(224, 222, 244),        // #e0def4
                primary: Color::Rgb(235, 188, 186),   // #ebbcba
                secondary: Color::Rgb(196, 167, 231), // #c4a7e7
                accent: Color::Rgb(246, 193, 119),    // #f6c177
                success: Color::Rgb(49, 116, 143),    // #31748f
                warning: Color::Rgb(246, 193, 119),   // #f6c177
                error: Color::Rgb(235, 111, 146),     // #eb6f92
                muted: Color::Rgb(110, 106, 134),     // #6e6a86
                surface: Color::Rgb(31, 29, 46),      // #1f1d2e
            },
        ),
        (
            "minimal",
            Palette {
                bg: Color::Rgb(10, 10, 10),           // #0a0a0a
                fg: Color::Rgb(237, 237, 237),        // #ededed
                primary: Color::Rgb(255, 255, 255),   // #ffffff
                secondary: Color::Rgb(136, 136, 136), // #888888
                accent: Color::Rgb(0, 112, 243),      // #0070f3
                success: Color::Rgb(0, 200, 83),      // #00c853
                warning: Color::Rgb(245, 166, 35),    // #f5a623
                error: Color::Rgb(238, 0, 0),         // #ee0000
                muted: Color::Rgb(85, 85, 85),        // #555555
                surface: Color::Rgb(26, 26, 26),      // #1a1a1a
            },
        ),
        (
            "cyberpunk",
            Palette {
                bg: Color::Rgb(13, 13, 26),         // #0d0d1a
                fg: Color::Rgb(224, 224, 255),      // #e0e0ff
                primary: Color::Rgb(0, 255, 255),   // #00ffff
                secondary: Color::Rgb(255, 0, 255), // #ff00ff
                accent: Color::Rgb(255, 255, 0),    // #ffff00
                success: Color::Rgb(0, 255, 136),   // #00ff88
                warning: Color::Rgb(255, 170, 0),   // #ffaa00
                error: Color::Rgb(255, 0, 68),      // #ff0044
                muted: Color::Rgb(74, 74, 106),     // #4a4a6a
                surface: Color::Rgb(26, 26, 46),    // #1a1a2e
            },
        ),
        (
            "ocean",
            Palette {
                bg: Color::Rgb(11, 25, 41),          // #0b1929
                fg: Color::Rgb(200, 214, 229),       // #c8d6e5
                primary: Color::Rgb(72, 219, 251),   // #48dbfb
                secondary: Color::Rgb(10, 189, 227), // #0abde3
                accent: Color::Rgb(85, 239, 196),    // #55efc4
                success: Color::Rgb(0, 210, 211),    // #00d2d3
                warning: Color::Rgb(254, 202, 87),   // #feca57
                error: Color::Rgb(255, 107, 107),    // #ff6b6b
                muted: Color::Rgb(87, 101, 116),     // #576574
                surface: Color::Rgb(21, 45, 69),     // #152d45
            },
        ),
        (
            "retro",
            Palette {
                bg: Color::Rgb(26, 18, 0),          // #1a1200
                fg: Color::Rgb(255, 176, 0),        // #ffb000
                primary: Color::Rgb(255, 204, 0),   // #ffcc00
                secondary: Color::Rgb(204, 136, 0), // #cc8800
                accent: Color::Rgb(255, 102, 0),    // #ff6600
                success: Color::Rgb(51, 255, 0),    // #33ff00
                warning: Color::Rgb(255, 204, 0),   // #ffcc00
                error: Color::Rgb(255, 0, 51),      // #ff0033
                muted: Color::Rgb(102, 85, 0),      // #665500
                surface: Color::Rgb(42, 32, 0),     // #2a2000
            },
        ),
    ]
});

// ── Global slot ────────────────────────────────────────────────────────

struct Active {
    name: &'static str,
    theme: Theme,
}

fn slot() -> &'static RwLock<Active> {
    static SLOT: OnceLock<RwLock<Active>> = OnceLock::new();
    SLOT.get_or_init(|| {
        RwLock::new(Active {
            name: presets()[0].name,
            theme: presets()[0].theme,
        })
    })
}

/// Monotonic counter of preset switches in this process.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Generation of the active preset: bumped on every successful switch.
///
/// Downstream caches (the per-message transcript cache, notably) bake
/// colors into `Line`s at render time; they compare the generation they
/// were built at against this value to drop entries styled under an
/// older preset.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

fn bump_generation() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Snapshot of the active theme. Widgets call this at render time and
/// read token fields off the copy.
pub fn current() -> Theme {
    let mut theme = slot()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .theme;
    theme.md = MdTokens::from_theme(&theme);
    theme
}

/// Name of the active preset.
pub fn active_name() -> &'static str {
    slot()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .name
}

/// Human-readable label of the active preset for status messages and menus.
pub fn active_label() -> &'static str {
    presets()
        .iter()
        .find(|preset| preset.name == active_name())
        .map(|preset| preset.label)
        .unwrap_or(active_name())
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
    bump_generation();
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
    bump_generation();
    active.name
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialize tests that touch the process-global preset slot: parallel
    /// mutators race each other, and whichever runs last decides what every
    /// later test module reads. Returns the guard; callers must keep it for
    /// the whole mutation section.
    fn locked() -> std::sync::MutexGuard<'static, ()> {
        static PRESET_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        PRESET_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Put the slot back on the default preset so tests outside this
    /// module always see the process-default tokens.
    fn restore_default() {
        assert!(set_by_name(presets()[0].name));
        assert_eq!(active_name(), presets()[0].name);
    }

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

    /// Pins the Claude Code palette ported from
    /// cola-runner/awesome-tui-design's `designs/claude-code/DESIGN.md` —
    /// every asserted value must stay traceable to that document.
    #[test]
    fn claude_code_matches_the_ported_palette() {
        let t = claude_code();
        assert_eq!(t.accent, Color::Rgb(0xd7, 0x77, 0x57)); // Primary terracotta
        assert_eq!(t.accent_alt, Color::Rgb(0xfd, 0x5d, 0xb1)); // hot pink tool borders
        assert_eq!(t.info, Color::Rgb(0xb1, 0xb9, 0xf9)); // lavender permission
        assert_eq!(t.success, Color::Rgb(0x4e, 0xba, 0x65));
        assert_eq!(t.warning, Color::Rgb(0xff, 0xc1, 0x07));
        assert_eq!(t.error, Color::Rgb(0xff, 0x6b, 0x80));
        assert_eq!(t.text, Color::Rgb(0xff, 0xff, 0xff)); // Foreground
        assert_eq!(t.text_faint, Color::Rgb(0x88, 0x88, 0x88)); // Muted
        assert_eq!(t.band_bg, Color::Rgb(0x1a, 0x1a, 0x1a)); // Background
        assert_eq!(t.overlay_bg, Color::Rgb(0x37, 0x37, 0x37)); // Surface
        assert_eq!(t.md.surface.raised, Color::Rgb(65, 60, 65)); // bash output bg
        assert_eq!(t.md.accent.link, Color::Rgb(0xb1, 0xb9, 0xf9));
    }

    #[test]
    fn default_preset_is_dark_and_full_cycle_returns() {
        let _guard = locked();
        restore_default();
        assert_eq!(active_name(), "dark");
        assert_eq!(cycle(), "claude-code");
        // A full lap over every registered preset lands back on the default.
        for _ in 1..presets().len() {
            cycle();
        }
        assert_eq!(active_name(), "dark");
    }

    /// The awesome-tui-design themes resolve by name and map their palette
    /// roles through the shared derivation.
    #[test]
    fn every_ported_theme_resolves_and_maps_roles() {
        let _guard = locked();
        restore_default();
        let dark = dark();
        for (name, palette) in PORTED.iter() {
            assert!(set_by_name(name), "preset {name} must resolve");
            let t = current();
            assert_ne!(
                t.accent, dark.accent,
                "{name} accent must differ from the dark preset"
            );
            assert_eq!(t.accent, palette.primary, "{name}: accent maps to primary");
            assert_eq!(
                t.info, palette.accent,
                "{name}: info maps to highlight accent"
            );
            assert_eq!(
                t.md.surface.raised, palette.surface,
                "{name}: md code surface maps to surface"
            );
            assert_eq!(t.error, palette.error, "{name}: error is verbatim");
        }
        restore_default();
    }

    #[test]
    fn preset_names_are_unique_and_cover_the_ported_set() {
        let names: std::collections::HashSet<_> = presets().iter().map(|p| p.name).collect();
        assert_eq!(names.len(), presets().len(), "no duplicate preset names");
        for expected in [
            "dark",
            "claude-code",
            "codex",
            "gemini",
            "lazygit",
            "k9s",
            "btop",
            "dracula",
            "catppuccin",
            "nord",
            "gruvbox",
            "tokyo-night",
            "rose-pine",
            "minimal",
            "cyberpunk",
            "ocean",
            "retro",
        ] {
            assert!(names.contains(&expected), "missing preset {expected}");
        }
    }

    #[test]
    fn set_by_name_rejects_unknown_presets() {
        let _guard = locked();
        restore_default();
        assert!(!set_by_name("solarized"));
        // The old light preset was removed in the theme-port work.
        assert!(!set_by_name("light"));
        assert_eq!(active_name(), "dark");
        assert!(set_by_name("claude-code"));
        assert_eq!(active_name(), "claude-code");
        assert!(set_by_name("dracula"));
        assert_eq!(active_name(), "dracula");
        assert!(set_by_name("dark"));
    }

    /// Every successful preset switch must bump the generation so caches
    /// holding colors baked under the previous preset can detect staleness.
    #[test]
    fn switching_presets_bumps_generation() {
        let _guard = locked();
        restore_default();
        let before = generation();
        cycle();
        assert!(generation() > before, "cycle() must bump the generation");
        let before = generation();
        assert!(set_by_name(active_name()));
        assert!(
            generation() > before,
            "set_by_name() must bump the generation"
        );
        restore_default();
    }

    #[test]
    fn claude_code_differs_from_dark() {
        let (d, c) = (dark(), claude_code());
        assert_ne!(d.text, c.text);
        assert_ne!(d.accent, c.accent);
        assert_ne!(d.selected_bg, c.selected_bg);
        assert_ne!(d.md.surface.raised, c.md.surface.raised);
    }
}
