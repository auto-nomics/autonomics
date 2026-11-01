//! Middle pane: compact article rows and full-text state.

use std::collections::HashMap;

use bib_base::{Article, FullText};
use ratatui::{
    layout::Rect,
    prelude::{Buffer, StatefulWidget, Widget},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

pub(super) struct ArticleList<'a> {
    pub articles: &'a [Article],
    pub visible: Vec<&'a str>,
    pub selected: Option<&'a str>,
    pub fulltexts: &'a HashMap<String, FullText>,
    pub focused: bool,
    pub accent: Color,
}

impl StatefulWidget for ArticleList<'_> {
    type State = ListState;

    fn render(self, area: Rect, buf: &mut Buffer, list_state: &mut Self::State) {
        let border = Style::new().fg(if self.focused {
            self.accent
        } else {
            Color::DarkGray
        });
        let block = Block::default()
            .title(Span::styled(
                format!(" Articles ({}) ", self.visible.len()),
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

        let items: Vec<_> = self
            .visible
            .iter()
            .filter_map(|id| self.articles.iter().find(|article| article.id == *id))
            .map(|article| self.row(article, inner.width))
            .collect();
        StatefulWidget::render(List::new(items), inner, buf, list_state);
    }
}

impl ArticleList<'_> {
    fn row(&self, article: &Article, width: u16) -> ListItem<'static> {
        let selected = self.selected == Some(article.id.as_str());
        let year = article
            .year
            .map(|y| y.to_string())
            .unwrap_or_else(|| "----".into());
        let marker = if selected { "▸ " } else { "  " };
        let fulltext = match self.fulltexts.get(&article.id) {
            Some(ft) => match ft.extract_status {
                Some(bib_base::ExtractStatus::Done) => "✓",
                Some(bib_base::ExtractStatus::Failed) => "✗",
                Some(bib_base::ExtractStatus::Running) => "◐",
                _ => "·",
            },
            None => " ",
        };
        let mut spans = vec![
            Span::styled(
                marker,
                if selected {
                    Style::new().fg(Color::LightYellow)
                } else {
                    Style::new()
                },
            ),
            Span::styled(year, Style::new().fg(Color::DarkGray)),
            Span::raw(" "),
            Span::styled(fulltext, Style::new().fg(Color::Green)),
            Span::raw(" "),
            Span::styled(
                article.title.clone(),
                if selected {
                    Style::new().add_modifier(Modifier::BOLD)
                } else {
                    Style::new()
                },
            ),
        ];
        // The dimmed author helps identify rows, but only when the whole
        // suffix fits — a half-clipped "· Smi" reads worse than no author.
        if let Some(author) = article.authors.first() {
            let suffix = format!(" · {}", author.display_name());
            let used: usize = spans.iter().map(|s| s.content.len()).sum();
            if used + suffix.len() <= width as usize {
                spans.push(Span::styled(suffix, Style::new().fg(Color::DarkGray)));
            }
        }
        ListItem::new(Line::from(spans))
    }
}
