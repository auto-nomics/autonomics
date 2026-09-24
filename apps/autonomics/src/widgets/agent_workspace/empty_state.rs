use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::{Buffer, Widget},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use super::logo::Logo;

/// Placeholder shown when no agents are running.
pub(super) struct EmptyState;

impl Widget for EmptyState {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " No Agent ",
                Style::default().fg(Color::DarkGray),
            ));
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.is_empty() {
            return;
        }

        if Logo::fits(inner) {
            self.render_full(inner, buf);
        } else {
            self.render_compact(inner, buf);
        }
    }
}

impl EmptyState {
    fn render_full(&self, area: Rect, buf: &mut Buffer) {
        let (_, logo_height) = Logo::dimensions();
        let content_height = logo_height.saturating_add(4);
        if area.height < content_height {
            self.render_compact(area, buf);
            return;
        }

        let middle = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(logo_height),
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(0),
            ])
            .split(area);

        Logo.render(middle[1], buf);

        let hints = vec![
            Line::from(Span::styled(
                "No agents running.",
                Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
            )),
            Line::raw(""),
            Line::from(Span::styled(
                "Ctrl+P → \"New agent\" or \"Resume agent\" to start.",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )),
        ];
        Paragraph::new(hints)
            .alignment(Alignment::Center)
            .render(middle[3], buf);
    }

    fn render_compact(&self, area: Rect, buf: &mut Buffer) {
        let message = if area.height > 5 {
            vec![
                Line::from(Span::styled(
                    "AUTONOMICS",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::raw(""),
                Line::from(Span::styled(
                    "No agents running.",
                    Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                )),
                Line::raw(""),
                Line::from(Span::styled(
                    "Ctrl+P → \"New agent\" or \"Resume agent\" to start.",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                )),
            ]
        } else {
            vec![Line::from(Span::styled(
                "No agents — press Ctrl+P to start",
                Style::default().fg(Color::DarkGray),
            ))]
        };

        Paragraph::new(message)
            .alignment(Alignment::Center)
            .render(area, buf);
    }
}
