//! Bottom status bar — mode + last status + keymap hint.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::tui::mode::Mode;

/// Bottom bar showing mode, status, and key hints.
pub struct StatusBarWidget<'a> {
    /// Current editor mode.
    pub mode: Mode,
    /// Latest application status message.
    pub status: &'a str,
    /// Key hints for the current mode.
    pub hint: &'a str,
}

impl<'a> Widget for StatusBarWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let bg = Style::default().bg(Color::Rgb(40, 40, 50));
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf[(x, y)].set_style(bg);
            }
        }
        let line = Line::from(vec![
            Span::styled(
                format!(" {:<10} ", self.mode.label()),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {} ", self.status),
                Style::default()
                    .fg(Color::White)
                    .bg(bg.bg.unwrap_or(Color::Reset)),
            ),
            Span::styled(
                " ".repeat(
                    (area.width as usize)
                        .saturating_sub(self.mode.label().len() + 3 + self.status.len() + 3),
                ),
                bg,
            ),
            Span::styled(
                format!(" {} ", self.hint),
                Style::default()
                    .fg(Color::DarkGray)
                    .bg(bg.bg.unwrap_or(Color::Reset)),
            ),
        ]);
        line.render(area, buf);
    }
}
