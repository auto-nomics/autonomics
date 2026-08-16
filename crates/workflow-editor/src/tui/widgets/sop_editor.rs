//! SOP editor — single-line-of-multi-line text buffer for editing SOP text.
//!
//! Renders a title + the buffer (truncated to fit). Multi-line editing is
//! not exposed in Phase 2; the buffer is editable one line at a time via
//! the keymap.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

/// Text widget for editing a workflow SOP.
pub struct SopEditorWidget<'a> {
    /// Current SOP text buffer.
    pub buffer: &'a str,
}

impl<'a> Widget for SopEditorWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" SOP editor (Esc to commit) ");
        let inner = block.inner(area);
        block.render(area, buf);

        let lines: Vec<Line<'_>> = self
            .buffer
            .lines()
            .map(|l| Line::from(Span::raw(l.to_string())))
            .chain(std::iter::once(Line::from(Span::styled(
                "▌".to_string(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::SLOW_BLINK),
            ))))
            .collect();

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(inner, buf);
    }
}
