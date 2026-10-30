//! Two-column browser for the evidence recorded by skill evolution.

use gateway::proto::{ClusterView, ObservationView};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget, Wrap},
};

use super::skill_browser::{PanelFocus, format_created, truncate};

/// Where the cluster-mode selection sits: on a cluster header, or on
/// one of the selected cluster's member rows. Only the selected
/// cluster is expanded — moving onto a header opens it, moving off
/// collapses it, so the list stays one-screen-navigable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusterCursor {
    pub cluster: usize,
    /// `None` = the cluster header itself.
    pub member: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct ObservationBrowserState {
    pub observations: Vec<ObservationView>,
    /// Surfaced clusters from the gateway, largest first. Empty when
    /// the pass has not produced any pattern yet.
    pub clusters: Vec<ClusterView>,
    /// Clustered (`c`) vs flat display of the same observations.
    pub clustered: bool,
    pub loaded: bool,
    pub error: Option<String>,
    pub selected: usize,
    pub cluster_cursor: Option<ClusterCursor>,
    pub focus: PanelFocus,
    list_state: ListState,
    detail_scroll: u16,
}

impl ObservationBrowserState {
    pub fn set_observations(&mut self, observations: Vec<ObservationView>) {
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

    pub fn set_clusters(&mut self, clusters: Vec<ClusterView>) {
        self.clusters = clusters;
        // Clamp the cursor against the new set; a cursor left past
        // the end would index panics in render/navigation.
        if let Some(cursor) = self.cluster_cursor {
            let Some(cluster) = self.clusters.get(cursor.cluster) else {
                self.cluster_cursor = None;
                self.detail_scroll = 0;
                return;
            };
            self.cluster_cursor = Some(ClusterCursor {
                cluster: cursor.cluster,
                member: cursor
                    .member
                    .filter(|m| *m < cluster.members.len())
                    .or((!cluster.members.is_empty()).then_some(0)),
            });
        }
    }

    /// Toggle between flat and clustered display; the flat selection
    /// and the cluster cursor are kept independently, so switching
    /// back and forth restores where the user was.
    pub fn toggle_clustered(&mut self) {
        self.clustered = !self.clustered;
        if self.clustered && self.cluster_cursor.is_none() && !self.clusters.is_empty() {
            self.cluster_cursor = Some(ClusterCursor {
                cluster: 0,
                member: None,
            });
        }
        self.detail_scroll = 0;
    }

    pub fn selected_observation(&self) -> Option<&ObservationView> {
        if self.clustered {
            let cursor = self.cluster_cursor?;
            let member = self
                .clusters
                .get(cursor.cluster)?
                .members
                .get(cursor.member?)?;
            // Member detail comes from the observation ledger; the
            // cluster view only carries list fields.
            self.observations.iter().find(|o| o.id == member.id)
        } else {
            self.observations.get(self.selected)
        }
    }

    pub fn navigate(&mut self, down: bool) {
        if self.focus == PanelFocus::Detail {
            self.detail_scroll = if down {
                self.detail_scroll.saturating_add(1)
            } else {
                self.detail_scroll.saturating_sub(1)
            };
        } else if self.clustered {
            self.navigate_clusters(down);
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

    /// Move through the flattened cluster view: cluster header, then
    /// its members (only the selected cluster is expanded), then the
    /// next header — wrapping at both ends.
    fn navigate_clusters(&mut self, down: bool) {
        let Some(mut cursor) = self.cluster_cursor else {
            if !self.clusters.is_empty() {
                self.cluster_cursor = Some(ClusterCursor {
                    cluster: 0,
                    member: None,
                });
            }
            return;
        };
        let len = self.clusters.len();
        cursor = if down {
            let members = self.clusters[cursor.cluster].members.len();
            match cursor.member {
                None if members > 0 => ClusterCursor {
                    cluster: cursor.cluster,
                    member: Some(0),
                },
                Some(m) if m + 1 < members => ClusterCursor {
                    cluster: cursor.cluster,
                    member: Some(m + 1),
                },
                _ => ClusterCursor {
                    cluster: (cursor.cluster + 1) % len,
                    member: None,
                },
            }
        } else {
            match cursor.member {
                // First member up to its own header — the exact
                // inverse of header-down-to-first-member.
                Some(0) => ClusterCursor {
                    cluster: cursor.cluster,
                    member: None,
                },
                // Header up to the previous cluster's last member
                // (which re-expands that cluster).
                None => {
                    let prev = if cursor.cluster == 0 {
                        len - 1
                    } else {
                        cursor.cluster - 1
                    };
                    let members = self.clusters[prev].members.len();
                    ClusterCursor {
                        cluster: prev,
                        member: (members > 0).then_some(members.saturating_sub(1)),
                    }
                }
                Some(m) => ClusterCursor {
                    cluster: cursor.cluster,
                    member: Some(m - 1),
                },
            }
        };
        if Some(cursor) != self.cluster_cursor {
            self.cluster_cursor = Some(cursor);
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
        if state.clustered {
            self.render_clustered(area, buf, state);
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
        let lines = observation_detail_lines(row, dim);
        render_scrolled_detail(lines, detail_area, buf, state);
    }
}

impl ObservationBrowserWidget {
    /// Clustered display: the left column lists surfaced patterns as
    /// `kind · node · (n)` headers with the selected cluster's
    /// members indented beneath it; the right column shows cluster
    /// meta on a header, or the member's full observation detail.
    fn render_clustered(self, area: Rect, buf: &mut Buffer, state: &mut ObservationBrowserState) {
        let dim = Style::new().fg(Color::DarkGray);
        if state.clusters.is_empty() {
            let message = if state.loaded {
                "no clusters surfaced yet (need ≥3 similar anchored observations)"
            } else {
                "loading clusters…"
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
                format!(" Clusters ({}) ", state.clusters.len()),
                title_style(PanelFocus::List),
            ));
        let list_area = list_block.inner(columns[0]);
        Widget::render(list_block, columns[0], buf);

        let cursor = state.cluster_cursor.unwrap_or(ClusterCursor {
            cluster: 0,
            member: None,
        });
        let mut rows: Vec<ListItem> = Vec::new();
        let mut selected_row = 0;
        for (cluster_index, cluster) in state.clusters.iter().enumerate() {
            let expanded = cluster_index == cursor.cluster;
            let header_selected = expanded && cursor.member.is_none();
            if header_selected {
                selected_row = rows.len();
            }
            // Header: `▶ kind · node  (n)` — the count is budgeted
            // into the row so a long partition label truncates
            // instead of pushing the count under the pane border.
            let count = format!("  ({})", cluster.members.len());
            let label = truncate(
                &format!("{} · {}", cluster.kind, cluster.node_kind),
                list_area
                    .width
                    .saturating_sub(2 + count.chars().count() as u16) as usize,
            );
            rows.push(ListItem::new(vec![
                Line::from(vec![
                    Span::styled(
                        if header_selected { "▶ " } else { "  " },
                        Style::new().fg(self.accent),
                    ),
                    Span::styled(
                        label,
                        if header_selected {
                            Style::new().add_modifier(Modifier::BOLD)
                        } else {
                            Style::new()
                        },
                    ),
                    Span::styled(
                        count,
                        Style::new().fg(if expanded {
                            self.accent
                        } else {
                            Color::DarkGray
                        }),
                    ),
                ]),
                Line::styled(
                    format!(
                        "  {} · {}",
                        truncate(
                            cluster
                                .members
                                .last()
                                .map(|m| m.summary.as_str())
                                .unwrap_or(""),
                            list_area.width.saturating_sub(4) as usize,
                        ),
                        format_created(
                            cluster
                                .members
                                .last()
                                .map(|m| m.created_at)
                                .unwrap_or_default(),
                        ),
                    ),
                    dim,
                ),
            ]));
            if expanded {
                for (member_index, member) in cluster.members.iter().enumerate() {
                    if cursor.member == Some(member_index) {
                        selected_row = rows.len();
                    }
                    let member_selected = cursor.member == Some(member_index);
                    rows.push(ListItem::new(vec![
                        Line::from(vec![
                            Span::styled(
                                if member_selected { "▶ " } else { "  " },
                                Style::new().fg(self.accent),
                            ),
                            Span::styled("  ", Style::new()),
                            Span::styled(
                                truncate(
                                    &member.summary,
                                    list_area.width.saturating_sub(4) as usize,
                                ),
                                if member_selected {
                                    Style::new().add_modifier(Modifier::BOLD)
                                } else {
                                    Style::new()
                                },
                            ),
                        ]),
                        Line::styled(format!("    {}", format_created(member.created_at)), dim),
                    ]));
                }
            }
        }
        state.list_state.select(Some(selected_row));
        StatefulWidget::render(List::new(rows), list_area, buf, &mut state.list_state);

        let detail_block = Block::default().title(Span::styled(
            if cursor.member.is_some() {
                " Member "
            } else {
                " Cluster "
            },
            title_style(PanelFocus::Detail),
        ));
        let detail_area = detail_block.inner(columns[1]);
        Widget::render(detail_block, columns[1], buf);
        let Some(cluster) = state.clusters.get(cursor.cluster) else {
            return;
        };
        let lines = match cursor.member {
            // Member row: full observation detail from the ledger,
            // falling back to the summary-only view when the flat
            // list does not carry that observation (e.g. trimmed).
            Some(member_index) => match cluster.members.get(member_index) {
                Some(member) => match state.observations.iter().find(|o| o.id == member.id) {
                    Some(row) => observation_detail_lines(row, dim),
                    None => vec![
                        Line::styled(
                            member.summary.clone(),
                            Style::new().add_modifier(Modifier::BOLD),
                        ),
                        Line::styled(
                            format!("created {}", format_created(member.created_at)),
                            dim,
                        ),
                        Line::default(),
                        Line::styled("(full body unavailable in this snapshot)", dim),
                    ],
                },
                None => vec![Line::styled("(no member selected)", dim)],
            },
            // Header: cluster meta + every member's summary.
            None => {
                let field = |label: &str, value: String| {
                    Line::from(vec![
                        Span::styled(format!("{label:<10}"), dim),
                        Span::raw(value),
                    ])
                };
                let mut lines = vec![
                    field("Kind", cluster.kind.clone()),
                    field("Node", cluster.node_kind.clone()),
                    field("Members", cluster.members.len().to_string()),
                    field("Model", cluster.model_signature.clone()),
                    Line::default(),
                    Line::styled("Members", Style::new().add_modifier(Modifier::BOLD)),
                ];
                lines.extend(cluster.members.iter().map(|member| {
                    Line::from(vec![
                        Span::styled(format!("{}  ", format_created(member.created_at)), dim),
                        Span::raw(member.summary.clone()),
                    ])
                }));
                lines
            }
        };
        render_scrolled_detail(lines, detail_area, buf, state);
    }
}

/// The flat observation detail document shared by both modes.
fn observation_detail_lines(row: &ObservationView, dim: Style) -> Vec<Line<'static>> {
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
    lines
}

/// Wrap-and-scroll the detail document, clamping the scroll offset
/// to the rendered height so a stale offset cannot blank the pane.
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
    use gateway::proto::{ClusterMemberView, ClusterView};

    fn member(id: &str, summary: &str, created_at: i64) -> ClusterMemberView {
        ClusterMemberView {
            id: id.into(),
            created_at,
            summary: summary.into(),
        }
    }

    fn cluster(id: &str, kind: &str, node: &str, members: Vec<ClusterMemberView>) -> ClusterView {
        ClusterView {
            id: id.into(),
            kind: kind.into(),
            node_kind: node.into(),
            model_signature: "hashing-test".into(),
            members,
        }
    }

    fn observation(id: &str, summary: &str, body: &str) -> ObservationView {
        ObservationView {
            id: id.into(),
            created_at: 1700000000,
            kind: "failure".into(),
            source: "agent".into(),
            summary: summary.into(),
            body: body.into(),
            node_kind: Some("sql".into()),
            error: Some("bad type".into()),
        }
    }

    /// Render and return each terminal row as one string, so tests
    /// (and humans via --nocapture) read what the screen shows.
    fn draw(state: &mut ObservationBrowserState) -> Vec<String> {
        use ratatui::backend::TestBackend;
        let width = 80u16;
        let height = 16u16;
        let mut terminal = ratatui::Terminal::new(TestBackend::new(width, height)).unwrap();
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
                    .skip(y as usize * width as usize)
                    .take(width as usize)
                    .map(|cell| cell.symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn demo_state() -> ObservationBrowserState {
        let mut state = ObservationBrowserState {
            loaded: true,
            ..Default::default()
        };
        state.set_observations(vec![
            observation("o1", "quote mixed-case column names", "recipe body a"),
            observation("o2", "identifiers are case-sensitive", "recipe body b"),
            observation("o3", "use quoted identifiers", "recipe body c"),
            observation("o4", "column 3 has bad type", "cast to f64"),
        ]);
        state.set_clusters(vec![
            cluster(
                "c1",
                "recipe",
                "file_to_dataframe",
                vec![
                    member("o1", "quote mixed-case column names", 1700000100),
                    member("o2", "identifiers are case-sensitive", 1700000200),
                    member("o3", "use quoted identifiers", 1700000300),
                ],
            ),
            cluster(
                "c2",
                "failure",
                "sql",
                vec![
                    member("o4", "column 3 has bad type", 1700000400),
                    member("o5", "column 7 has bad type", 1700000500),
                    member("o6", "column 9 has bad type", 1700000600),
                ],
            ),
        ]);
        state
    }

    /// Visual check (run with --nocapture): header row shows the
    /// partition and count; the selected cluster expands its members.
    #[test]
    fn clustered_view_renders_tree() {
        let mut state = demo_state();
        state.toggle_clustered();
        let screen = draw(&mut state);
        println!("{}", screen.join("\n"));
        // Row 0 is the pane title; the tree starts at row 1.
        assert!(screen[0].contains("Clusters (2)"));
        assert!(screen[1].contains("recipe · file_to_dataf"));
        assert!(screen[1].contains("(3)"));
        // Row 2 previews the cluster's newest member.
        assert!(screen[2].contains("use quoted identifiers"));
        // Expanded members under the first header.
        assert!(screen[3].contains("quote mixed-case column na"));
        assert!(screen[5].contains("identifiers are case-sensi"));
        // Second header collapsed — its members are absent.
        assert!(screen[9].contains("failure · sql"));
        assert!(!screen.iter().any(|l| l.contains("column 7")));
    }

    /// Header detail: partition meta plus every member summary.
    #[test]
    fn header_detail_shows_cluster_meta() {
        let mut state = demo_state();
        state.toggle_clustered();
        let screen = draw(&mut state);
        let detail = screen.join("\n");
        assert!(detail.contains("Model"));
        assert!(detail.contains("hashing-test"));
        assert!(detail.contains("quote mixed-case column names"));
    }

    /// Member rows resolve to full observation detail from the flat
    /// list, and to a summary fallback when the ledger lacks the id.
    #[test]
    fn member_detail_resolves_and_falls_back() {
        let mut state = demo_state();
        state.toggle_clustered();
        // Onto the first member (o1) — full body from the ledger.
        state.navigate(true);
        let screen = draw(&mut state);
        let text = screen.join("\n");
        assert!(text.contains("recipe body a"), "full body renders");
        assert!(text.contains("Summary"));

        // Third member is o3; hydrate a cluster whose member has no
        // flat observation (id unknown) → summary fallback.
        state.set_clusters(vec![cluster(
            "c9",
            "recipe",
            "sql",
            vec![
                member("x1", "summary only one", 1),
                member("x2", "summary only two", 2),
                member("x3", "summary only three", 3),
            ],
        )]);
        let screen = draw(&mut state);
        let text = screen.join("\n");
        assert!(text.contains("summary only two"));
        assert!(text.contains("full body unavailable"));
    }

    /// Navigation walks header → members → next header, wrapping.
    #[test]
    fn navigation_walks_the_flattened_tree() {
        let mut state = demo_state();
        state.toggle_clustered();
        // header 0 → m0 → m1 → m2 → header 1 (collapsed members of
        // cluster 0 gone) → wrap.
        let expect = [
            Some((0, None)),
            Some((0, Some(0))),
            Some((0, Some(1))),
            Some((0, Some(2))),
            Some((1, None)),
            Some((1, Some(0))),
            Some((1, Some(1))),
            Some((1, Some(2))),
            Some((0, None)),
        ];
        for want in expect.map(|t| t.map(ClusterCursor::from)) {
            assert_eq!(state.cluster_cursor, want, "at {want:?}");
            state.navigate(true);
        }
        // The loop's trailing navigate(true) sits at (0, Some(0));
        // back up through the header to the last member of cluster 1.
        state.navigate(false);
        state.navigate(false);
        assert_eq!(state.cluster_cursor, Some((1, Some(2)).into()));
        // Up from a first member lands on its own header.
        state.navigate(false);
        state.navigate(false);
        state.navigate(false);
        assert_eq!(state.cluster_cursor, Some((1, None).into()));
    }

    impl From<(usize, Option<usize>)> for ClusterCursor {
        fn from(value: (usize, Option<usize>)) -> Self {
            ClusterCursor {
                cluster: value.0,
                member: value.1,
            }
        }
    }
}
