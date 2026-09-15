//! Shared rendering primitives for two-column tree pickers.
//!
//! Profile creation and agent resume both present a searchable tree with a
//! preview pane. Keeping the chrome and row styling here prevents the two
//! flows from drifting apart while allowing each picker to keep its own data.

use agentik_core::AgentProfile;
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
    let placeholder = format!(" search {noun}…");
    let line = if query.is_empty() {
        Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                placeholder,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("> ", Style::default().fg(accent)),
            Span::styled(query.to_string(), Style::default().fg(Color::White)),
        ])
    };

    Widget::render(Paragraph::new(line), area, buf);
}

pub fn render_separator(area: Rect, buf: &mut Buffer) {
    Widget::render(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
        area,
        buf,
    );
}

pub fn render_footer(area: Rect, buf: &mut Buffer, hint: &str) {
    Widget::render(
        Paragraph::new(hint).style(
            Style::default()
                .fg(Color::DarkGray)
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
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(Color::DarkGray))
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
    let indent = "  ".repeat(row.depth());
    let (icon, icon_color) = if row.is_leaf() {
        ("●", accent)
    } else if row.expanded() {
        ("▼", Color::Yellow)
    } else {
        ("▶", Color::Yellow)
    };

    let (icon_style, name_style) = if selected {
        (selected_style(accent), selected_style(accent))
    } else {
        (
            Style::default().fg(icon_color),
            if row.is_leaf() {
                Style::default().fg(Color::Gray)
            } else {
                Style::default()
                    .fg(Color::Yellow)
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
    let display_area = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: 1,
    };
    Widget::render(
        Paragraph::new(text).style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        display_area,
        buf,
    );
}

pub fn profile_preview_lines(
    profile: &AgentProfile,
    leading_fields: Vec<(&'static str, String)>,
) -> Vec<Line<'static>> {
    let mut lines = leading_fields
        .into_iter()
        .map(|(label, value)| field_line(label, value))
        .collect::<Vec<_>>();

    lines.push(field_line("Profile", profile.path.clone()));

    if let Some(parent) = profile.parent_path() {
        lines.push(field_line("Parent", parent.to_string()));
    }

    if !profile.description.is_empty() {
        lines.push(field_line("Desc", profile.description.clone()));
    }

    lines.push(field_line("Identity", profile.agent_identity.clone()));
    lines.push(field_line(
        "Model",
        profile
            .preferred_model
            .clone()
            .unwrap_or_else(|| "(global default)".to_string()),
    ));
    lines.push(field_line(
        "Memory",
        override_label(profile.runtime.use_memory),
    ));
    lines.push(field_line(
        "Memory gen",
        override_label(profile.runtime.generate_memory),
    ));
    lines.push(Line::from(""));
    lines.push(section_line("Capabilities"));

    for (name, enabled) in [
        ("bibliography", profile.enable_bibliography),
        ("writing", profile.enable_writing),
        ("opengwas", profile.enable_opengwas),
        ("opentargets", profile.enable_opentargets),
        ("gwascatalog", profile.enable_gwascatalog),
        ("chembl", profile.enable_chembl),
        ("rcsb", profile.enable_rcsb),
        ("kegg", profile.enable_kegg),
        ("dag-history", profile.enable_dag_history),
    ] {
        lines.push(flag_line(name, enabled));
    }

    lines
}

fn override_label(value: Option<bool>) -> String {
    match value {
        None => "inherit".to_string(),
        Some(true) => "on".to_string(),
        Some(false) => "off".to_string(),
    }
}

pub fn field_line(label: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {label:<11}"),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            value.into(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

pub fn section_line(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {title}:"),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ))
}

pub fn flag_line(name: &str, enabled: bool) -> Line<'static> {
    let (icon, color) = if enabled {
        ("●", Color::Green)
    } else {
        ("○", Color::DarkGray)
    };

    Line::from(vec![
        Span::styled("    ", Style::default()),
        Span::styled(icon, Style::default().fg(color)),
        Span::raw(" "),
        Span::styled(
            name.to_string(),
            Style::default().fg(if enabled {
                Color::Gray
            } else {
                Color::DarkGray
            }),
        ),
    ])
}

pub fn truncate_preview_lines(lines: Vec<Line<'static>>, height: usize) -> Vec<Line<'static>> {
    if lines.len() <= height {
        return lines;
    }

    let mut lines = lines;
    lines.truncate(height.saturating_sub(1));
    lines.push(Line::from(Span::styled(
        "    …",
        Style::default().fg(Color::DarkGray),
    )));
    lines
}

fn selected_style(accent: Color) -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(accent)
        .add_modifier(Modifier::BOLD)
}
