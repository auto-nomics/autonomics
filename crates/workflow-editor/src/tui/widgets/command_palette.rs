//! Command palette — modal popup for `:w`, `:q`, `:r`, ...

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Widget, Wrap};

pub struct CommandPaletteWidget<'a> {
    pub buffer: &'a str,
    pub suggestions: Vec<(&'a str, &'a str)>,
}

impl<'a> Widget for CommandPaletteWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Clear background
        Clear.render(area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" command ");
        let inner = block.inner(area);
        block.render(area, buf);

        // Top: typed buffer
        let prompt = format!(":{:<width$}", self.buffer, width = inner.width as usize - 2);
        let prompt_line = Line::from(Span::styled(prompt, Style::default().fg(Color::White)));
        prompt_line.render(
            Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 1,
            },
            buf,
        );

        // Below: matching suggestions
        let q = self.buffer.to_lowercase();
        let items: Vec<ListItem> = self
            .suggestions
            .iter()
            .filter(|(name, _)| q.is_empty() || name.contains(&q.as_str()))
            .map(|(name, desc)| {
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{:<14}", name), Style::default().fg(Color::Cyan)),
                    Span::styled(*desc, Style::default().fg(Color::Gray)),
                ]))
            })
            .collect();

        let list_area = Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: inner.height.saturating_sub(1),
        };
        List::new(items)
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .render(list_area, buf);
        let _ = Wrap { trim: false };
    }
}
