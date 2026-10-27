//! Left navigation pane: virtual all-articles row plus collections.

use bib_base::Collection;
use ratatui::{
    layout::Rect,
    prelude::{Buffer, StatefulWidget},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Widget},
};

pub(super) struct CollectionPane<'a> {
    pub rows: &'a [(&'a Collection, usize)],
    pub expanded: &'a std::collections::HashSet<String>,
    pub recursive: bool,
    pub selected: Option<&'a str>,
    pub focused: bool,
    pub accent: Color,
}

impl Widget for CollectionPane<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let border = Style::new().fg(if self.focused {
            self.accent
        } else {
            Color::DarkGray
        });
        let block = Block::default()
            .title(Span::styled(
                if self.recursive {
                    " Collections (+ descendants) "
                } else {
                    " Collections "
                },
                if self.focused {
                    Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(Color::DarkGray)
                },
            ))
            .borders(Borders::RIGHT)
            .border_style(border);
        let inner = block.inner(area);
        block.render(area, buf);

        let mut items = Vec::with_capacity(self.rows.len() + 1);
        let mut state = ratatui::widgets::ListState::default();
        if self.selected.is_none() {
            state.select(Some(0));
        }
        items.push(row("All articles", self.selected.is_none(), inner.width));
        for (index, (collection, depth)) in self.rows.iter().enumerate() {
            if self.selected == Some(collection.id.as_str()) {
                state.select(Some(index + 1));
            }
            let label = format!(
                "{}{} {} ({})",
                "  ".repeat(*depth),
                if self.expanded.contains(&collection.id) {
                    "▾"
                } else {
                    "▸"
                },
                collection.name,
                collection.article_ids.len()
            );
            items.push(row(
                &label,
                self.selected == Some(collection.id.as_str()),
                inner.width,
            ));
        }
        ratatui::prelude::StatefulWidget::render(List::new(items), inner, buf, &mut state);
    }
}

fn row(label: &str, selected: bool, width: u16) -> ListItem<'static> {
    let marker = if selected { "▸ " } else { "  " };
    let budget = width.saturating_sub(2) as usize;
    let label = if label.chars().count() > budget {
        let kept: String = label.chars().take(budget.saturating_sub(1)).collect();
        format!("{kept}…")
    } else {
        label.to_string()
    };
    ListItem::new(Line::from(vec![
        Span::styled(
            marker,
            if selected {
                Style::new().fg(Color::LightYellow)
            } else {
                Style::new()
            },
        ),
        Span::styled(
            label,
            if selected {
                Style::new().add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            },
        ),
    ]))
}
