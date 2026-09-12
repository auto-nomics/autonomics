//! Text assembly for bibliography list and detail panes.

use bib_base::Article;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

pub(super) fn article_list_lines(article: &Article, accent: Color) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled(
                article
                    .year
                    .map_or_else(|| "----".to_owned(), |year| year.to_string()),
                Style::default().fg(accent),
            ),
            Span::raw("  "),
            Span::styled(article.title.clone(), Style::default().fg(Color::White)),
        ]),
        Line::styled(article.short_cite(), Style::default().fg(Color::DarkGray)),
    ]
}

pub(super) fn article_details_lines(
    article: &Article,
    heading: String,
    heading_color: Color,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled(
            heading,
            Style::default()
                .fg(heading_color)
                .add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            article.title.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            format!("ID: {}", article.id),
            Style::default().fg(Color::DarkGray),
        ),
    ];

    if !article.authors.is_empty() {
        let authors = article
            .authors
            .iter()
            .map(|author| author.display_name())
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(metadata_line("Authors", &authors));
    }
    if let Some(journal) = &article.journal {
        lines.push(metadata_line("Journal", journal));
    }
    if let Some(year) = article.year {
        lines.push(metadata_line("Year", &year.to_string()));
    }
    if let Some(doi) = article.doi() {
        lines.push(metadata_line("DOI", doi));
    }
    if !article.keywords.is_empty() {
        lines.push(metadata_line("Keywords", &article.keywords.join(", ")));
    }
    if let Some(abstract_text) = &article.abstract_text {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            "Abstract",
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::styled(
            abstract_text.clone(),
            Style::default().fg(Color::Gray),
        ));
    }

    lines
}

fn metadata_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), Style::default().fg(Color::Gray)),
        Span::raw(value.to_owned()),
    ])
}
