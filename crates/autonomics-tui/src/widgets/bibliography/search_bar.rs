//! Search input and result summary.

use ratatui::{
    layout::Rect,
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

pub(super) struct SearchBar<'a> {
    pub query: &'a str,
    pub focused: bool,
    pub result_count: usize,
    pub accent: Color,
}

impl Widget for SearchBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let dim = Style::new().fg(Color::DarkGray);
        let block = Block::default()
            .title(Span::styled(
                " Search ",
                if self.focused {
                    Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
                } else {
                    dim
                },
            ))
            .borders(Borders::BOTTOM)
            .border_type(BorderType::Plain)
            .border_style(if self.focused {
                Style::new().fg(self.accent)
            } else {
                dim
            });
        let inner = block.inner(area);
        block.render(area, buf);

        let prompt = if self.query.is_empty() { "/" } else { "" };
        let mut left_len = prompt.chars().count() + self.query.chars().count();
        let mut spans = vec![Span::styled(prompt, dim), Span::raw(self.query)];
        if self.focused {
            spans.push(Span::styled("▌", Style::new().fg(self.accent)));
            left_len += 1;
        }

        let summary = format!("{} results ", self.result_count);
        let pad = inner
            .width
            .saturating_sub(left_len as u16 + summary.chars().count() as u16)
            .max(1);
        spans.push(Span::raw(" ".repeat(pad as usize)));
        spans.push(Span::styled(summary, dim));
        Paragraph::new(Line::from(spans)).render(inner, buf);
    }
}
