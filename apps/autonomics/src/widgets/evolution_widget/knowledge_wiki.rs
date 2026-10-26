//! Two-column browser for the durable KMS knowledge wiki.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget, Wrap},
};

use super::skill_browser::{PanelFocus, format_created, truncate};

#[derive(Debug, Clone, Default)]
pub struct KnowledgeWikiState {
    pub nodes: Vec<gateway::proto::KmsKnowledgeNodeView>,
    pub loaded: bool,
    pub error: Option<String>,
    pub selected: usize,
    pub focus: PanelFocus,
    list_state: ListState,
    detail_scroll: u16,
}

impl KnowledgeWikiState {
    pub fn set_nodes(&mut self, mut nodes: Vec<gateway::proto::KmsKnowledgeNodeView>) {
        nodes.sort_by(|a, b| path_segments(&a.path).cmp(&path_segments(&b.path)));
        let selected_id = self.selected_node().map(|node| node.id);
        self.selected = selected_id
            .and_then(|id| nodes.iter().position(|node| node.id == id))
            .unwrap_or(0);
        if nodes.get(self.selected).map(|node| node.id) != selected_id {
            self.detail_scroll = 0;
        }
        self.nodes = nodes;
        self.loaded = true;
        self.error = None;
    }

    pub fn selected_node(&self) -> Option<&gateway::proto::KmsKnowledgeNodeView> {
        self.nodes.get(self.selected)
    }

    pub fn navigate(&mut self, down: bool) {
        if self.focus == PanelFocus::Detail {
            self.detail_scroll = if down {
                self.detail_scroll.saturating_add(1)
            } else {
                self.detail_scroll.saturating_sub(1)
            };
        } else if !self.nodes.is_empty() {
            self.selected = if down {
                (self.selected + 1) % self.nodes.len()
            } else if self.selected == 0 {
                self.nodes.len() - 1
            } else {
                self.selected - 1
            };
            self.detail_scroll = 0;
        }
    }
}

fn path_segments(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .collect()
}

pub struct KnowledgeWikiWidget {
    pub accent: Color,
}

impl StatefulWidget for KnowledgeWikiWidget {
    type State = KnowledgeWikiState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let dim = Style::new().fg(Color::DarkGray);
        if let Some(error) = &state.error {
            Widget::render(
                Paragraph::new(format!(
                    "KMS knowledge unavailable: {error}; press g to retry"
                ))
                .style(Style::new().fg(Color::Yellow))
                .wrap(Wrap { trim: false }),
                area,
                buf,
            );
            return;
        }
        if !state.loaded || state.nodes.is_empty() {
            let message = if state.loaded {
                "no KMS wiki nodes recorded"
            } else {
                "loading KMS knowledge..."
            };
            Widget::render(Paragraph::new(message).style(dim), area, buf);
            return;
        }

        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length((area.width * 2 / 5).clamp(28, 60)),
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
                format!(" Knowledge ({}) ", state.nodes.len()),
                title_style(PanelFocus::List),
            ));
        let list_area = list_block.inner(columns[0]);
        Widget::render(list_block, columns[0], buf);

        let rows: Vec<ListItem> = state
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let selected = index == state.selected;
                let depth = node.path.matches('/').count().saturating_sub(1);
                let indent = "  ".repeat(depth.min(7));
                let title = truncate(
                    &format!("{indent}{}", node.title),
                    list_area.width.saturating_sub(2) as usize,
                );
                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(
                            if selected { "> " } else { "  " },
                            Style::new().fg(self.accent),
                        ),
                        Span::styled(
                            title,
                            if selected {
                                Style::new().add_modifier(Modifier::BOLD)
                            } else if node.structural {
                                Style::new().add_modifier(Modifier::DIM)
                            } else {
                                Style::new()
                            },
                        ),
                    ]),
                    Line::styled(format!("  {indent}{}", node.path), dim),
                ])
            })
            .collect();
        state.list_state.select(Some(state.selected));
        StatefulWidget::render(List::new(rows), list_area, buf, &mut state.list_state);

        let detail_block =
            Block::default().title(Span::styled(" Wiki ", title_style(PanelFocus::Detail)));
        let detail_area = detail_block.inner(columns[1]);
        Widget::render(detail_block, columns[1], buf);
        let Some(node) = state.selected_node() else {
            return;
        };
        let lines = wiki_detail_lines(node, dim);
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        let max_scroll = paragraph
            .line_count(detail_area.width)
            .saturating_sub(detail_area.height as usize)
            .min(u16::MAX as usize) as u16;
        state.detail_scroll = state.detail_scroll.min(max_scroll);
        Widget::render(paragraph.scroll((state.detail_scroll, 0)), detail_area, buf);
    }
}

fn wiki_detail_lines(
    node: &gateway::proto::KmsKnowledgeNodeView,
    dim: Style,
) -> Vec<Line<'static>> {
    let field = |label: &str, value: String| {
        Line::from(vec![
            Span::styled(format!("{label:<10}"), dim),
            Span::raw(value),
        ])
    };
    let kind = if node.structural {
        "section"
    } else {
        "article"
    };
    let mut lines = vec![
        Line::styled(
            node.title.clone(),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        field("Path", node.path.clone()),
        field("Kind", kind.to_string()),
        field("Status", node.status.clone()),
        field("Confidence", format!("{:.2}", node.confidence)),
        field("Created", format_created(node.created_at)),
        field("Updated", format_created(node.updated_at)),
    ];
    if !node.source_hash.is_empty() {
        lines.push(field("Source", node.source_hash.clone()));
    }
    lines.push(Line::default());
    lines.push(Line::styled(
        "Content",
        Style::new().add_modifier(Modifier::BOLD),
    ));
    if node.body_md.trim().is_empty() {
        lines.push(Line::styled("(empty body)", dim));
    } else {
        lines.extend(node.body_md.lines().map(|line| Line::raw(line.to_string())));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(path: &str, title: &str, body: &str) -> gateway::proto::KmsKnowledgeNodeView {
        gateway::proto::KmsKnowledgeNodeView {
            id: uuid::Uuid::new_v4(),
            parent_id: None,
            path: path.into(),
            title: title.into(),
            body_md: body.into(),
            status: "active".into(),
            confidence: 0.9,
            source_hash: "hash-1".into(),
            structural: path.ends_with("engineering"),
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
        }
    }

    fn draw(state: &mut KnowledgeWikiState) -> Vec<String> {
        use ratatui::backend::TestBackend;
        let width = 100_usize;
        let height = 18;
        let mut terminal =
            ratatui::Terminal::new(TestBackend::new(width as u16, height as u16)).unwrap();
        terminal
            .draw(|frame| {
                KnowledgeWikiWidget {
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
                    .skip(y * width)
                    .take(width)
                    .map(|cell| cell.symbol().to_string())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn renders_sorted_tree_and_selected_article() {
        let article = node(
            "/engineering/testing/release",
            "Release",
            "Run checks first.",
        );
        let section = node("/engineering/testing", "Testing", "");
        let engineering = node("/engineering", "Engineering", "");
        let mut state = KnowledgeWikiState {
            nodes: vec![article.clone(), section, engineering],
            loaded: true,
            focus: PanelFocus::List,
            ..Default::default()
        };

        let screen = draw(&mut state);
        let text = screen.concat();
        assert!(text.contains("Engineering"));
        assert!(text.contains("/engineering"));
        assert!(text.contains("  Testing"));
        assert!(text.contains("    Release"));
        assert!(text.contains("Run checks first."));
    }

    #[test]
    fn loading_and_error_states_are_user_readable() {
        let mut loading = KnowledgeWikiState::default();
        let screen = draw(&mut loading).concat();
        assert!(screen.contains("loading KMS knowledge"));

        let mut unavailable = KnowledgeWikiState {
            error: Some("daemon offline".into()),
            ..Default::default()
        };
        let screen = draw(&mut unavailable).concat();
        assert!(screen.contains("KMS knowledge unavailable"));
        assert!(screen.contains("daemon offline"));
    }

    #[test]
    fn refresh_preserves_selected_node_by_id() {
        let first = node("/product/design", "Design", "old");
        let id = first.id;
        let mut refreshed_design = first.clone();
        let mut state = KnowledgeWikiState {
            nodes: vec![node("/product/api", "API", "api"), first],
            loaded: true,
            selected: 1,
            ..Default::default()
        };

        refreshed_design.body_md = "new".into();
        let mut refreshed = vec![node("/product/api", "API", ""), refreshed_design];
        refreshed.sort_by(|a, b| b.path.cmp(&a.path));
        state.set_nodes(refreshed);

        assert_eq!(state.selected_node().map(|node| node.id), Some(id));
    }
}
