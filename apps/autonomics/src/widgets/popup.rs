//! Reusable centered popup container.
//!
//! Provides the geometry (centered box with border + title) and a `Clear`
//! backdrop. Callers render their own content inside the returned inner area.
//!
//! # Usage
//!
//! ```ignore
//! let popup = Popup::new(" Select Profile ", PopupControls::default());
//! let inner = popup.render(frame.area(), buf);
//! // Render your widget inside `inner`…
//! ```

use ratatui::{
    layout::Rect,
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Clear, Widget},
};

/// A centered popup container whose title and border are configurable.
pub struct Popup<'a> {
    pub title: &'a str,
    /// Fixed width; 0 = auto (60% of frame, clamped 40–80).
    pub width: u16,
    /// Fixed height; 0 = auto (content-driven, clamped to frame).
    pub height: u16,
    /// Accent color for the title / border.
    pub accent: Color,
    /// Center vertically in the frame instead of using the default
    /// upper-third placement preferred by larger pickers.
    pub vertically_centered: bool,
    /// Whether the title and border are rendered.
    pub controls: PopupControls,
}

/// Chrome controls for [`Popup`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupControls {
    /// Render the title text.
    pub show_title: bool,
    /// Render the border.
    pub show_border: bool,
}

impl PopupControls {
    pub const fn new(show_title: bool, show_border: bool) -> Self {
        Self {
            show_title,
            show_border,
        }
    }
}

impl Default for PopupControls {
    fn default() -> Self {
        Self::new(true, true)
    }
}

impl<'a> Popup<'a> {
    pub fn new(title: &'a str, controls: PopupControls) -> Self {
        Self {
            title,
            width: 0,
            height: 0,
            accent: Color::Cyan,
            vertically_centered: false,
            controls,
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

    pub fn vertically_centered(mut self, enabled: bool) -> Self {
        self.vertically_centered = enabled;
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
        let divisor = if self.vertically_centered { 2 } else { 3 };
        let y = frame_area.y + (frame_area.height.saturating_sub(ph)) / divisor;
        Rect::new(x, y, pw, ph)
    }

    /// Compute the inner content area (inside the border).
    pub fn inner_rect(&self, frame_area: Rect) -> Rect {
        let outer = self.outer_rect(frame_area);
        // A border takes 1 row/col on each side. Without a border, still
        // reserve the title row when the caller asks for one.
        let (top, sides, bottom) = if self.controls.show_border {
            (1, 1, 1)
        } else if self.controls.show_title {
            (1, 0, 0)
        } else {
            (0, 0, 0)
        };
        Rect::new(
            outer.x + sides,
            outer.y + top,
            outer.width.saturating_sub(sides * 2),
            outer.height.saturating_sub(top + bottom),
        )
    }

    /// Render the popup backdrop + bordered frame. Returns the inner area
    /// for content rendering.
    pub fn render(&self, frame_area: Rect, buf: &mut Buffer) -> Rect {
        let outer = self.outer_rect(frame_area);
        let inner = self.inner_rect(frame_area);

        // Backdrop clears underlying content.
        Clear.render(outer, buf);

        let mut block = Block::default();
        if self.controls.show_border {
            block = block
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray));
        }
        if self.controls.show_title {
            block = block.title(Span::styled(
                self.title,
                Style::default()
                    .fg(self.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        block.render(outer, buf);

        inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inner_rect_reserves_border_chrome() {
        let popup = Popup::new("Test", PopupControls::default())
            .width(70)
            .height(10);
        let area = Rect::new(0, 0, 70, 10);

        assert_eq!(popup.inner_rect(area), Rect::new(1, 1, 68, 8));
    }

    #[test]
    fn inner_rect_reserves_title_without_border() {
        let popup = Popup::new("Test", PopupControls::new(true, false))
            .width(70)
            .height(10);
        let area = Rect::new(0, 0, 70, 10);

        assert_eq!(popup.inner_rect(area), Rect::new(0, 1, 70, 9));
    }

    #[test]
    fn vertically_centered_popups_use_the_frame_middle() {
        let popup = Popup::new("Test", PopupControls::default())
            .width(20)
            .height(6)
            .vertically_centered(true);
        let frame = Rect::new(10, 20, 100, 30);

        assert_eq!(popup.outer_rect(frame), Rect::new(50, 32, 20, 6));
    }
}
