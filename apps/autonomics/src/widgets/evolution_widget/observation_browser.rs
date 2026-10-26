//! Two-column browser for durable KMS evidence.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget, Wrap},
};

use super::skill_browser::{PanelFocus, format_created, truncate};

/// Presentation row projected from durable KMS evidence.
#[derive(Debug, Clone)]
pub struct EvidenceRow {
    pub id: String,
    pub created_at: i64,
    pub kind: String,
    pub source: String,
    pub summary: String,
    pub body: String,
    pub node_kind: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ObservationBrowserState {
    pub observations: Vec<EvidenceRow>,
    pub loaded: bool,
    pub error: Option<String>,
    pub selected: usize,
    pub focus: PanelFocus,
    list_state: ListState,
    detail_scroll: u16,
}

impl ObservationBrowserState {
    pub fn set_observations(&mut self, observations: Vec<EvidenceRow>) {
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

    pub fn selected_observation(&self) -> Option<&EvidenceRow> {
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
                    "KMS evidence unavailable: {error}; press g to retry"
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
                "no KMS evidence recorded"
            } else {
                "loading KMS evidence..."
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
                format!(" Evidence ({}) ", state.observations.len()),
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

        let detail_block =
            Block::default().title(Span::styled(" Evidence ", title_style(PanelFocus::Detail)));
        let detail_area = detail_block.inner(columns[1]);
        Widget::render(detail_block, columns[1], buf);
        let Some(row) = state.selected_observation() else {
            return;
        };
        let lines = observation_detail_lines(row, dim);
        render_scrolled_detail(lines, detail_area, buf, state);
    }
}

fn observation_detail_lines(row: &EvidenceRow, dim: Style) -> Vec<Line<'static>> {
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
        field("Seen", &format_created(row.created_at)),
    ];
    if let Some(anchor) = &row.node_kind {
        lines.push(field("Anchor", anchor));
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
    lines
}

fn render_scrolled_detail(
    lines: Vec<Line<'static>>,
    area: Rect,
    buf: &mut Buffer,
    state: &mut ObservationBrowserState,
) {
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let max_scroll = paragraph
        .line_count(area.width)
        .saturating_sub(area.height as usize)
        .min(u16::MAX as usize) as u16;
    state.detail_scroll = state.detail_scroll.min(max_scroll);
    Widget::render(paragraph.scroll((state.detail_scroll, 0)), area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(id: &str, summary: &str, body: &str) -> EvidenceRow {
        EvidenceRow {
            id: id.into(),
            created_at: 1_700_000_000,
            kind: "pending · eval_failure".into(),
            source: "skill_failure · eval-1".into(),
            summary: summary.into(),
            body: body.into(),
            node_kind: Some("/procedures/echo-flow".into()),
            error: Some("rows_min mismatch".into()),
        }
    }

    fn draw(state: &mut ObservationBrowserState) -> Vec<String> {
        use ratatui::backend::TestBackend;
        let width = 80_usize;
        let height = 16;
        let mut terminal = ratatui::Terminal::new(TestBackend::new(width as u16, height)).unwrap();
        terminal
            .draw(|frame| {
                ObservationBrowserWidget {
                    accent: Color::Cyan,
                }
                .render(frame.area(), frame.buffer_mut(), state)
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                buffer
                    .content()
                    .iter()
                    .skip(y as usize * width)
                    .take(width)
                    .map(|cell| cell.symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn navigation_wraps_and_renders_kms_evidence() {
        let mut state = ObservationBrowserState {
            loaded: true,
            ..Default::default()
        };
        state.set_observations(vec![
            observation("e1", "eval smoke fails", "workflow emitted one row"),
            observation("e2", "eval boundary fails", "workflow emitted zero rows"),
        ]);

        let screen = draw(&mut state);
        let text = screen.join("\n");
        assert!(text.contains("Evidence (2)"));
        assert!(text.contains("eval smoke fails"));
        assert!(text.contains("pending · eval_failure"));
        assert!(text.contains("/procedures/echo-flow"));
        assert!(text.contains("workflow emitted one row"));

        state.navigate(true);
        assert_eq!(state.selected, 1);
        state.navigate(true);
        assert_eq!(state.selected, 0);
        state.navigate(false);
        assert_eq!(state.selected, 1);
    }
}
