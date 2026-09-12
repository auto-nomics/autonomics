//! Ratatui layout and rendering helpers.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, StatefulWidget, Widget, Wrap},
};

use super::state::{BibliographyFocus, BibliographyState};
use super::text::{article_details_lines, article_list_lines};

pub(super) fn render_search(
    area: Rect,
    buf: &mut Buffer,
    state: &mut BibliographyState,
    accent: Color,
) {
    let focused = state.focus == BibliographyFocus::Search;
    let color = if focused { accent } else { Color::DarkGray };
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .title(Span::styled(" Search ", Style::default().fg(color)))
        .border_style(Style::default().fg(color));
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.width < 3 {
        return;
    }

    let cursor = if focused { "_" } else { "" };
    let line = if state.query().is_empty() {
        Line::from(vec![
            Span::styled("/", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(" search{cursor}"),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("/", Style::default().fg(accent)),
            Span::styled(
                format!("{}{cursor}", state.query()),
                Style::default().fg(Color::White),
            ),
        ])
    };
    Widget::render(Paragraph::new(line), inner, buf);
}

pub(super) fn render_body(
    area: Rect,
    buf: &mut Buffer,
    state: &mut BibliographyState,
    accent: Color,
) {
    let areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(area);

    render_results(areas[0], buf, state, accent);
    render_details(areas[1], buf, state, accent);
}

fn render_results(area: Rect, buf: &mut Buffer, state: &mut BibliographyState, accent: Color) {
    let focused = state.focus == BibliographyFocus::Results;
    let color = if focused { accent } else { Color::DarkGray };
    let block = Block::default()
        .borders(Borders::RIGHT)
        .title(Span::styled(
            format!(
                " Articles {}/{} ",
                state.match_count(),
                state.article_count()
            ),
            Style::default().fg(color),
        ))
        .border_style(Style::default().fg(color));
    let inner = block.inner(area);
    block.render(area, buf);

    if state.match_count() == 0 {
        let message = if state.articles().is_empty() {
            "No articles in library"
        } else {
            "No matching articles"
        };
        Widget::render(
            Paragraph::new(message).style(
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
            inner,
            buf,
        );
        return;
    }

    let items: Vec<ListItem<'_>> = state
        .matched_articles()
        .map(|article| ListItem::new(article_list_lines(article, accent)))
        .collect();
    let list = List::new(items)
        .highlight_symbol("> ")
        .highlight_style(Style::default().fg(accent).add_modifier(Modifier::BOLD));
    StatefulWidget::render(list, inner, buf, state.list_state_mut());
}

fn render_details(area: Rect, buf: &mut Buffer, state: &mut BibliographyState, accent: Color) {
    let focused = state.focus == BibliographyFocus::Details;
    let heading = state
        .selected_article()
        .map(|article| article.short_cite())
        .unwrap_or_else(|| "Details".to_owned());

    let Some(article) = state.selected_article() else {
        Widget::render(
            Paragraph::new("Select an article to inspect metadata").style(
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
            area,
            buf,
        );
        return;
    };

    let heading_color = if focused { accent } else { Color::Gray };
    let lines = article_details_lines(article, heading, heading_color);
    Widget::render(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((state.details_scroll(), 0)),
        area,
        buf,
    );
}

pub(super) fn render_footer(
    area: Rect,
    buf: &mut Buffer,
    state: &BibliographyState,
    accent: Color,
) {
    let line = Line::from(vec![
        Span::styled(
            format!("[{}]", state.focus().label()),
            Style::default().fg(accent),
        ),
        Span::styled(
            "  Tab focus  / search  j/k navigate  Enter open  Esc back",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("  {}", state.status_message().unwrap_or("")),
            Style::default(),
        ),
    ]);
    Widget::render(Paragraph::new(line), area, buf);
}
