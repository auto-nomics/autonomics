//! Reusable centered popup container.
//!
//! Provides the geometry (centered box with border + title) and a `Clear`
//! backdrop. Callers render their own content inside the returned inner area.
//!
//! # Usage
//!
//! ```ignore
//! let popup = Popup::new(" Select Profile ");
//! let inner = popup.compute_inner(frame.area());
//! popup.render_frame(frame.area(), buf);
//! // Render your widget inside `inner`…
//! ```

use ratatui::{
    layout::Rect,
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Clear, Widget},
};

/// A centered popup with a titled border.
pub struct Popup<'a> {
    pub title: &'a str,
    /// Fixed width; 0 = auto (60% of frame, clamped 40–80).
    pub width: u16,
    /// Fixed height; 0 = auto (content-driven, clamped to frame).
    pub height: u16,
    /// Accent color for the title / border.
    pub accent: Color,
}

impl<'a> Popup<'a> {
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            width: 0,
            height: 0,
            accent: Color::Cyan,
        }
    }

    pub fn width(mut self, w: u16) -> Self {
        self.width = w;
        self
    }

    pub fn height(mut self, h: u16) -> Self {
        self.height = h;
        self
    }

    pub fn accent(mut self, c: Color) -> Self {
        self.accent = c;
        self
    }

    /// Compute the outer rect for the popup within `frame_area`.
    pub fn outer_rect(&self, frame_area: Rect) -> Rect {
        let pw = if self.width > 0 {
            self.width.min(frame_area.width)
        } else {
            (frame_area.width * 6 / 10).clamp(40, 80)
        };
        let ph = if self.height > 0 {
            self.height.min(frame_area.height)
        } else {
            (frame_area.height * 5 / 10).clamp(8, frame_area.height)
        };
        let x = frame_area.x + (frame_area.width.saturating_sub(pw)) / 2;
        let y = frame_area.y + (frame_area.height.saturating_sub(ph)) / 3;
        Rect::new(x, y, pw, ph)
    }

    /// Compute the inner content area (inside the border).
    pub fn inner_rect(&self, frame_area: Rect) -> Rect {
        let outer = self.outer_rect(frame_area);
        // Borders take 1 row/col on each side.
        Rect::new(
            outer.x + 1,
            outer.y + 1,
            outer.width.saturating_sub(2),
            outer.height.saturating_sub(2),
        )
    }

    /// Render the popup backdrop + bordered frame. Returns the inner area
    /// for content rendering.
    pub fn render(&self, frame_area: Rect, buf: &mut Buffer) -> Rect {
        let outer = self.outer_rect(frame_area);
        let inner = self.inner_rect(frame_area);

        // Backdrop clears underlying content.
        Clear.render(outer, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
                self.title,
                Style::default().fg(self.accent).add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(Color::DarkGray));
        block.render(outer, buf);

        inner
    }
}
