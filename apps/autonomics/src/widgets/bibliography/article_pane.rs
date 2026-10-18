//! Right pane: selected article metadata, extracted text, and annotations.

use bib_base::{Annotation, Article, FullText};
use ratatui::{
    layout::Rect,
    prelude::{Buffer, StatefulWidget},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Widget},
};

use super::DetailsTab;

pub(super) struct ArticlePane<'a> {
    pub article: Option<&'a Article>,
    pub fulltext: Option<&'a FullText>,
    pub annotations: Option<&'a [Annotation]>,
    pub tab: DetailsTab,
    pub focused: bool,
    pub scroll: u16,
    pub accent: Color,
}

impl Widget for ArticlePane<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default().title(self.tab_bar());
        let inner = block.inner(area);
        block.render(area, buf);

        let lines = self.lines();
        let max_scroll = lines.len().saturating_sub(inner.height as usize) as u16;
        let scroll = self.scroll.min(max_scroll);
        let total_lines = lines.len();
        Paragraph::new(lines).scroll((scroll, 0)).render(inner, buf);

        // Start below the tab bar so the thumb never overwrites tab labels.
        let scrollbar_area = Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(1),
            ..area
        };
        let mut scrollbar_state = ScrollbarState::new(total_lines).position(scroll as usize);
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::new().fg(Color::DarkGray))
            .render(scrollbar_area, buf, &mut scrollbar_state);
    }
}

impl ArticlePane<'_> {
    /// Title line listing every tab; the active one is accented so the
    /// `h`/`l` switching affordance is discoverable without a manual.
    fn tab_bar(&self) -> Line<'static> {
        let mut spans = Vec::new();
        for (index, tab) in DetailsTab::TABS.into_iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled("│", Style::new().fg(Color::DarkGray)));
            }
            let label = format!(" {} ", tab.label());
            let style = if tab == self.tab {
                if self.focused {
                    Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD)
                }
            } else {
                Style::new().fg(Color::DarkGray)
            };
            spans.push(Span::styled(label, style));
        }
        Line::from(spans)
    }

    fn lines(&self) -> Vec<Line<'static>> {
        let Some(article) = self.article else {
            return vec![Line::styled(
                "No article selected",
                Style::new().fg(Color::DarkGray),
            )];
        };
        match self.tab {
            DetailsTab::Metadata => self.metadata(article),
            DetailsTab::FullText => self.fulltext(article),
            DetailsTab::Notes => self.notes(article),
        }
    }

    fn metadata(&self, article: &Article) -> Vec<Line<'static>> {
        let mut lines = vec![
            field("Title", article.title.clone()),
            field(
                "Authors",
                article
                    .authors
                    .iter()
                    .map(|a| a.display_name())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            field(
                "Year",
                article.year.map(|y| y.to_string()).unwrap_or_default(),
            ),
            field("Journal", article.journal.clone().unwrap_or_default()),
            field("DOI", article.doi().unwrap_or_default().to_string()),
            field("PMID", article.pmid().unwrap_or_default().to_string()),
            field("Keywords", article.keywords.join(", ")),
            Line::raw(""),
            Line::styled(
                "Abstract",
                Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(text) = &article.abstract_text {
            lines.extend(text.lines().map(|line| Line::raw(line.to_string())));
        } else {
            lines.push(Line::styled(
                "Not available",
                Style::new().fg(Color::DarkGray),
            ));
        }
        lines
    }

    fn fulltext(&self, article: &Article) -> Vec<Line<'static>> {
        match self.fulltext {
            Some(ft) if ft.text_content.is_some() => ft
                .text_content
                .as_deref()
                .unwrap_or_default()
                .lines()
                .map(|line| Line::raw(line.to_string()))
                .collect(),
            Some(ft) => vec![
                Line::styled(
                    "Full text uploaded; extraction pending",
                    Style::new().fg(Color::Yellow),
                ),
                field("File", &ft.file_path),
                field("Extractor", ft.extracted_by.clone().unwrap_or_default()),
            ],
            None => vec![
                Line::styled("No full text stored", Style::new().fg(Color::DarkGray)),
                Line::styled(
                    format!("Article ID: {}", article.id),
                    Style::new().fg(Color::DarkGray),
                ),
            ],
        }
    }

    fn notes(&self, article: &Article) -> Vec<Line<'static>> {
        let empty = vec![Line::styled(
            "No notes yet",
            Style::new().fg(Color::DarkGray),
        )];
        let Some(notes) = self.annotations else {
            return empty;
        };
        if notes.is_empty() {
            return empty;
        }
        let mut lines = Vec::new();
        for note in notes {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("[{}] ", note.kind.as_str()),
                    Style::new().fg(self.accent),
                ),
                Span::raw(note.content.clone()),
            ]));
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!("Article ID: {}", article.id),
            Style::new().fg(Color::DarkGray),
        ));
        lines
    }
}

fn field(name: &str, value: impl Into<String>) -> Line<'static> {
    let value = value.into();
    if value.is_empty() {
        return Line::from(vec![
            Span::styled(format!("{name}: "), Style::new().fg(Color::DarkGray)),
            Span::styled("—", Style::new().fg(Color::DarkGray)),
        ]);
    }
    Line::from(vec![
        Span::styled(format!("{name}: "), Style::new().fg(Color::Gray)),
        Span::raw(value),
    ])
}
