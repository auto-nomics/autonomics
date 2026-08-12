//! Spec form — JSON-schema-driven form for editing a node's `params`.
//!
//! Phase 2 ships a *JSON textarea* version: the user edits the raw JSON
//! directly. A schema-driven per-field form is on the Phase 2 backlog but
//! is non-essential for the editor to be usable.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

pub struct SpecFormWidget<'a> {
    pub buffer: &'a str,
}

impl<'a> Widget for SpecFormWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" params (JSON; Esc to commit) ");
        let inner = block.inner(area);
        block.render(area, buf);

        let lines: Vec<Line<'_>> = self
            .buffer
            .lines()
            .map(|l| Line::from(Span::raw(l.to_string())))
            .collect();
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(inner, buf);
        let _ = (Color::Yellow, Modifier::BOLD);
    }
}