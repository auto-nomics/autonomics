//! Shared rendering primitives for two-column tree pickers.
//!
//! Profile creation and agent resume both present a searchable tree with a
//! preview pane. Keeping the chrome and row styling here prevents the two
//! flows from drifting apart while allowing each picker to keep its own data.

use agentik_core::AgentKind;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
};

use crate::widgets::popup::{Popup, PopupControls};

/// Vertical regions inside a picker popup.
#[derive(Clone, Copy, Debug)]
pub struct PickerLayout {
    pub search: Rect,
    pub separator: Rect,
    pub content: Rect,
    pub footer: Rect,
}

/// Horizontal regions in the picker content area.
#[derive(Clone, Copy, Debug)]
pub struct PickerContent {
    pub list: Rect,
    pub preview: Rect,
}

/// A row that can be rendered by the shared tree-list implementation.
pub trait PickerTreeRow {
    fn name(&self) -> &str;
    fn depth(&self) -> usize;
    fn is_leaf(&self) -> bool;
    fn expanded(&self) -> bool;
}

pub fn render_chrome(
    area: Rect,
    buf: &mut Buffer,
    title: &str,
    accent: Color,
    popup_width: u16,
) -> PickerLayout {
    let inner = Popup::new(title, PopupControls::default())
        .accent(accent)
        .width(popup_width)
        .render(area, buf);

    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    PickerLayout {
        search: regions[0],
        separator: regions[1],
        content: regions[2],
        footer: regions[3],
    }
}

pub fn split_content(content: Rect, list_width: u16) -> PickerContent {
    let regions = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(list_width), Constraint::Min(10)])
        .split(content);

    PickerContent {
        list: regions[0],
        preview: regions[1],
    }
}

pub fn render_search(area: Rect, buf: &mut Buffer, accent: Color, query: &str, noun: &str) {
    let theme = crate::theme::current();
    let placeholder = format!(" search {noun}…");
    let line = if query.is_empty() {
        Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.text_faint)),
            Span::styled(
                placeholder,
                Style::default()
                    .fg(theme.text_faint)
                    .add_modifier(Modifier::DIM),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("> ", Style::default().fg(accent)),
            Span::styled(query.to_string(), Style::default().fg(theme.text)),
        ])
    };

    Widget::render(Paragraph::new(line), area, buf);
}

pub fn render_separator(area: Rect, buf: &mut Buffer) {
    let theme = crate::theme::current();
    Widget::render(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(theme.text_faint)),
        area,
        buf,
    );
}

pub fn render_footer(area: Rect, buf: &mut Buffer, hint: &str) {
    let theme = crate::theme::current();
    Widget::render(
        Paragraph::new(hint).style(
            Style::default()
                .fg(theme.text_faint)
                .add_modifier(Modifier::DIM),
        ),
        area,
        buf,
    );
}

pub fn render_tree_list<T: PickerTreeRow>(
    area: Rect,
    buf: &mut Buffer,
    title: &str,
    empty_text: &str,
    rows: &[T],
    selected: usize,
    accent: Color,
    list_state: &mut ListState,
) {
    let theme = crate::theme::current();
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(theme.text_faint))
        .title(Span::styled(
            title,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    block.render(area, buf);

    if rows.is_empty() {
        render_preview_text(inner, buf, empty_text);
        return;
    }

    let items = rows
        .iter()
        .enumerate()
        .map(|(index, row)| ListItem::new(tree_line(row, index == selected, accent)))
        .collect::<Vec<_>>();

    let list = List::new(items).highlight_style(selected_style(accent));
    StatefulWidget::render(list, inner, buf, list_state);
}

fn tree_line<T: PickerTreeRow>(row: &T, selected: bool, accent: Color) -> Line<'static> {
    let theme = crate::theme::current();
    let indent = "  ".repeat(row.depth());
    let (icon, icon_color) = if row.is_leaf() {
        ("●", accent)
    } else if row.expanded() {
        ("▼", theme.warning)
    } else {
        ("▶", theme.warning)
    };

    let (icon_style, name_style) = if selected {
        (selected_style(accent), selected_style(accent))
    } else {
        (
            Style::default().fg(icon_color),
            if row.is_leaf() {
                Style::default().fg(theme.text_dim)
            } else {
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD)
            },
        )
    };

    Line::from(vec![
        Span::styled(indent, Style::default()),
        Span::styled(icon, icon_style),
        Span::raw(" "),
        Span::styled(row.name().to_string(), name_style),
    ])
}

pub fn render_preview_frame(area: Rect, buf: &mut Buffer, accent: Color) -> Rect {
    let block = Block::default().borders(Borders::NONE).title(Span::styled(
        " Preview ",
        Style::default().fg(accent).add_modifier(Modifier::BOLD),
    ));
    let inner = block.inner(area);
    block.render(area, buf);
    inner
}

pub fn render_preview_text(area: Rect, buf: &mut Buffer, text: &str) {
    let theme = crate::theme::current();
    let display_area = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: 1,
    };
    Widget::render(
        Paragraph::new(text).style(
            Style::default()
                .fg(theme.text_faint)
                .add_modifier(Modifier::DIM),
        ),
        display_area,
        buf,
    );
}

pub fn kind_preview_lines(
    kind: agentik_core::AgentKind,
    leading_fields: Vec<(&'static str, String)>,
) -> Vec<Line<'static>> {
    let mut lines = leading_fields
        .into_iter()
        .map(|(label, value)| field_line(label, value))
        .collect::<Vec<_>>();

    lines.push(field_line("Profile", kind.name().to_string()));
    lines.push(field_line("Desc", kind.description().to_string()));
    lines.push(field_line("Identity", kind.agent_identity().to_string()));
    lines.push(Line::from(""));
    lines.push(section_line("Capabilities"));

    for (name, enabled) in [
        ("bibliography", kind.enable_bibliography()),
        ("writing", kind.enable_writing()),
        ("opengwas", kind.enable_opengwas()),
        ("opentargets", kind.enable_opentargets()),
        ("gwascatalog", kind.enable_gwascatalog()),
        ("chembl", kind.enable_chembl()),
        ("rcsb", kind.enable_rcsb()),
        ("kegg", kind.enable_kegg()),
        ("dag-history", kind.enable_dag_history()),
        ("plugin-rsi", kind.enable_plugin_rsi()),
        ("wiki-read", kind.enable_wiki_read()),
        ("wiki-write", kind.enable_wiki_write()),
    ] {
        lines.push(flag_line(name, enabled));
    }

    lines
}

pub fn field_line(label: &str, value: impl Into<String>) -> Line<'static> {
    let theme = crate::theme::current();
    Line::from(vec![
        Span::styled(
            format!("  {label:<11}"),
            Style::default().fg(theme.text_faint),
        ),
        Span::styled(
            value.into(),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
    ])
}

pub fn section_line(title: &str) -> Line<'static> {
    let theme = crate::theme::current();
    Line::from(Span::styled(
        format!("  {title}:"),
        Style::default()
            .fg(theme.text_faint)
            .add_modifier(Modifier::BOLD),
    ))
}

pub fn flag_line(name: &str, enabled: bool) -> Line<'static> {
    let theme = crate::theme::current();
    let (icon, color) = if enabled {
        ("●", theme.success)
    } else {
        ("○", theme.text_faint)
    };

    Line::from(vec![
        Span::styled("    ", Style::default()),
        Span::styled(icon, Style::default().fg(color)),
        Span::raw(" "),
        Span::styled(
            name.to_string(),
            Style::default().fg(if enabled {
                theme.text_dim
            } else {
                theme.text_faint
            }),
        ),
    ])
}

pub fn truncate_preview_lines(lines: Vec<Line<'static>>, height: usize) -> Vec<Line<'static>> {
    let theme = crate::theme::current();
    if lines.len() <= height {
        return lines;
    }

    let mut lines = lines;
    lines.truncate(height.saturating_sub(1));
    lines.push(Line::from(Span::styled(
        "    …",
        Style::default().fg(theme.text_faint),
    )));
    lines
}

fn selected_style(accent: Color) -> Style {
    Style::default()
        .fg(crate::theme::current().on_accent)
        .bg(accent)
        .add_modifier(Modifier::BOLD)
}
