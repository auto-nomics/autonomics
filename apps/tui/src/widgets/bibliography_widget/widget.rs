//! Public stateful widget implementation.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    prelude::Widget,
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, StatefulWidget},
};

use super::render::{render_body, render_footer, render_search};
use super::state::BibliographyState;

/// Stateful bibliography workspace widget.
pub struct BibliographyWidget {
    accent: Color,
}

impl BibliographyWidget {
    pub fn new() -> Self {
        Self {
            accent: Color::Cyan,
        }
    }

    pub fn accent(mut self, color: Color) -> Self {
        self.accent = color;
        self
    }
}

impl Default for BibliographyWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl StatefulWidget for BibliographyWidget {
    type State = BibliographyState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut BibliographyState) {
        if area.width < 8 || area.height < 5 {
            return;
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
                " Bibliography ",
                Style::default()
                    .fg(self.accent)
                    .add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        block.render(area, buf);

        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(1),
            ])
            .split(inner);

        render_search(areas[0], buf, state, self.accent);
        render_body(areas[1], buf, state, self.accent);
        render_footer(areas[2], buf, state, self.accent);
    }
}
