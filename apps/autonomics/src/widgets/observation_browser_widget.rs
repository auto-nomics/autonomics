//! Two-column browser for the evidence recorded by skill evolution.

use gateway::proto::SkillObservationView;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget, Wrap},
};

use super::skill_browser_widget::{PanelFocus, format_created, truncate};

#[derive(Debug, Clone, Default)]
pub struct ObservationBrowserState {
    pub observations: Vec<SkillObservationView>,
    pub loaded: bool,
    pub error: Option<String>,
    pub selected: usize,
    pub focus: PanelFocus,
    list_state: ListState,
    detail_scroll: u16,
}

impl ObservationBrowserState {
    pub fn set_observations(&mut self, observations: Vec<SkillObservationView>) {
        let selected_id = self.selected_observation().map(|row| row.id.clone());
        self.selected = selected_id
            .as_ref()
            .and_then(|id| observations.iter().position(|row| &row.id == id))
            .unwrap_or(0);
        if observations.get(self.selected).map(|row| &row.id) != selected_id.as_ref() {
            self.detail_scroll = 0;
        }
        self.observations = observations;
        self.loaded = true;
        self.error = None;
    }

    pub fn selected_observation(&self) -> Option<&SkillObservationView> {
        self.observations.get(self.selected)
    }

    pub fn navigate(&mut self, down: bool) {
        if self.focus == PanelFocus::Detail {
            self.detail_scroll = if down {
                self.detail_scroll.saturating_add(1)
            } else {
                self.detail_scroll.saturating_sub(1)
            };
        } else if !self.observations.is_empty() {
            self.selected = if down {
                (self.selected + 1) % self.observations.len()
            } else if self.selected == 0 {
                self.observations.len() - 1
            } else {
                self.selected - 1
            };
            self.detail_scroll = 0;
        }
    }
}

pub struct ObservationBrowserWidget {
    pub accent: Color,
}

impl StatefulWidget for ObservationBrowserWidget {
    type State = ObservationBrowserState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let dim = Style::new().fg(Color::DarkGray);
        if let Some(error) = &state.error {
            Widget::render(
                Paragraph::new(format!(
                    "observations unavailable: {error}; press g to retry"
                ))
                .style(Style::new().fg(Color::Yellow))
                .wrap(Wrap { trim: false }),
                area,
                buf,
            );
            return;
        }
        if !state.loaded || state.observations.is_empty() {
            let message = if state.loaded {
                "no observations recorded"
            } else {
                "loading observations…"
            };
            Widget::render(Paragraph::new(message).style(dim), area, buf);
            return;
        }

        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length((area.width * 2 / 5).clamp(28, 56)),
                Constraint::Min(20),
            ])
            .split(area);
        let title_style = |focus| {
            if state.focus == focus {
                Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
            } else {
                dim
            }
        };
        let list_block = Block::default()
            .borders(Borders::RIGHT)
            .border_style(dim)
            .title(Span::styled(
                format!(" Observations ({}) ", state.observations.len()),
                title_style(PanelFocus::List),
            ));
        let list_area = list_block.inner(columns[0]);
        Widget::render(list_block, columns[0], buf);
        let rows: Vec<ListItem> = state
            .observations
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let selected = index == state.selected;
                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(
                            if selected { "▶ " } else { "  " },
                            Style::new().fg(self.accent),
                        ),
                        Span::styled(
                            truncate(&row.summary, list_area.width.saturating_sub(2) as usize),
                            if selected {
                                Style::new().add_modifier(Modifier::BOLD)
                            } else {
                                Style::new()
                            },
                        ),
                    ]),
                    Line::styled(
                        format!(
                            "  {} · {} · {}",
                            row.kind,
                            row.source,
                            format_created(row.created_at)
                        ),
                        dim,
                    ),
                ])
            })
            .collect();
        state.list_state.select(Some(state.selected));
        StatefulWidget::render(List::new(rows), list_area, buf, &mut state.list_state);

        let detail_block = Block::default().title(Span::styled(
            " Observation ",
            title_style(PanelFocus::Detail),
        ));
        let detail_area = detail_block.inner(columns[1]);
        Widget::render(detail_block, columns[1], buf);
        let Some(row) = state.selected_observation() else {
            return;
        };
        let field = |label: &str, value: &str| {
            Line::from(vec![
                Span::styled(format!("{label:<10}"), dim),
                Span::raw(value.to_string()),
            ])
        };
        let mut lines = vec![
            field("Summary", &row.summary),
            field("ID", &row.id),
            field("Kind", &row.kind),
            field("Source", &row.source),
            field("Created", &format_created(row.created_at)),
        ];
        if let Some(node) = &row.node_kind {
            lines.push(field("Node", node));
        }
        if let Some(error) = &row.error {
            lines.push(field("Error", error));
        }
        lines.push(Line::default());
        lines.push(Line::styled(
            "Evidence",
            Style::new().add_modifier(Modifier::BOLD),
        ));
        if row.body.trim().is_empty() {
            lines.push(Line::styled("(empty body)", dim));
        } else {
            lines.extend(row.body.lines().map(|line| Line::raw(line.to_string())));
        }
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        let max_scroll = paragraph
            .line_count(detail_area.width)
            .saturating_sub(detail_area.height as usize)
            .min(u16::MAX as usize) as u16;
        state.detail_scroll = state.detail_scroll.min(max_scroll);
        Widget::render(paragraph.scroll((state.detail_scroll, 0)), detail_area, buf);
    }
}
