//! Right-side workspace container with leaf tabs.
//!
//! Renders a horizontal tab bar (one tab per running agent) and the active
//! agent's leaf below it. The tab bar is purely visual — agent switching is
//! driven by the sidebar or keyboard shortcuts handled in `App`.
//!
//! # Layout
//!
//! ```text
//! ┌──────────────────────────────────────────┐
//! │ ● default │ ◐ literature │   gwas-analysis│  ← Tab bar
//! ├──────────────────────────────────────────┤
//! │                                          │
//! │  Active AgentLeaf                        │
//! │  (status bar + chat + input + footer)    │
//! │                                          │
//! └──────────────────────────────────────────┘
//! ```

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::{Buffer, Widget},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, StatefulWidgetRef},
};

use crate::state::{AgentStatus, AgentTabState, DisplaySettings};
use crate::widgets::agent_leaf::AgentLeaf;

/// Renders the workspace: leaf tab bar + active leaf.
pub struct AgentWorkspace<'a> {
    pub active_model: Option<&'a str>,
    /// Context window size of the active model (tokens), when known.
    pub context_window: Option<u64>,
    /// Active session title.
    pub session_title: Option<&'a str>,
    /// 1-based index of the active session.
    pub session_index: usize,
    /// Total number of sessions.
    pub session_count: usize,
    /// Global display settings (collapse toggles).
    pub display: &'a DisplaySettings,
}

/// One entry in the tab bar.
pub struct LeafTab {
    pub name: String,
    pub status: AgentStatus,
}

impl AgentWorkspace<'_> {
    /// Render the workspace: tab bar (1 row) + active leaf (remaining space).
    ///
    /// `tabs` is the full list of running agents; `active_idx` determines
    /// which tab is highlighted and which leaf's `tab_state` is rendered.
    /// When `tabs` is empty, renders a placeholder prompting the user to
    /// spawn an agent from the sidebar.
    pub fn render(
        &self,
        area: Rect,
        buf: &mut Buffer,
        tabs: &[LeafTab],
        active_idx: usize,
        tab_state: &mut AgentTabState,
    ) {
        // ── Empty state: no agents running ──
        if tabs.is_empty() {
            Self::render_empty(area, buf, self.active_model);
            return;
        }

        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // Tab bar (1 row tabs + 1 row border)
                Constraint::Min(3),    // Active leaf
            ])
            .split(area);

        // ── Tab bar ──
        Self::render_tab_bar(areas[0], buf, tabs, active_idx);

        // ── Active leaf ──
        let agent_name = tabs.get(active_idx).map(|t| t.name.as_str());
        let leaf = AgentLeaf {
            active_model: self.active_model,
            agent_name,
            context_window: self.context_window,
            session_title: self.session_title,
            session_index: self.session_index,
            session_count: self.session_count,
            display: self.display,
        };
        leaf.render_ref(areas[1], buf, tab_state);
    }

    /// Render a placeholder when no agents are running.
    fn render_empty(area: Rect, buf: &mut Buffer, _active_model: Option<&str>) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " No Agent ",
                Style::default().fg(Color::DarkGray),
            ));
        let inner = block.inner(area);
        block.render(area, buf);

        let msg = if inner.height > 5 {
            vec![
                Line::raw(""),
                Line::raw(""),
                Line::from(Span::styled(
                    "  No agents running.",
                    Style::default()
                        .fg(Color::Gray)
                        .add_modifier(Modifier::DIM),
                )),
                Line::raw(""),
                Line::from(Span::styled(
                    "  Ctrl+P → \"New agent\" or \"Resume agent\" to start.",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                )),
            ]
        } else {
            vec![Line::from(Span::styled(
                "  No agents — spawn one from the sidebar",
                Style::default().fg(Color::DarkGray),
            ))]
        };

        ratatui::widgets::Paragraph::new(msg).render(inner, buf);
    }

    fn render_tab_bar(area: Rect, buf: &mut Buffer, tabs: &[LeafTab], active_idx: usize) {
        let mut spans: Vec<Span> = Vec::new();

        for (i, tab) in tabs.iter().enumerate() {
            let is_active = i == active_idx;
            let icon = status_icon(&tab.status);
            let color = status_color(&tab.status);

            let label = format!(" {} {} ", icon, tab.name);

            if is_active {
                spans.push(Span::styled(
                    label,
                    Style::default()
                        .fg(Color::White)
                        .bg(color)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(
                    label,
                    Style::default().fg(Color::DarkGray),
                ));
            }

            // Separator
            if i < tabs.len() - 1 {
                spans.push(Span::raw("│"));
            }
        }

        // Render tab labels on the FIRST row only (height 1 sub-area).
        let text_area = Rect { height: 1, ..area };
        let line = Line::from(spans);
        ratatui::widgets::Paragraph::new(line).render(text_area, buf);

        // Bottom border on the SECOND row.
        if area.height >= 2 && area.width > 0 {
            let border_y = area.y + 1;
            let border_style = Style::default().fg(Color::DarkGray);
            for x in area.x..area.x + area.width {
                buf.set_string(x, border_y, "─", border_style);
            }
        }
    }
}

fn status_icon(status: &AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle => "●",
        AgentStatus::Requesting => "◐",
        AgentStatus::Streaming => "◑",
        AgentStatus::Retrying => "↻",
        AgentStatus::Error => "✗",
    }
}

fn status_color(status: &AgentStatus) -> Color {
    match status {
        AgentStatus::Idle => Color::Green,
        AgentStatus::Requesting | AgentStatus::Streaming => Color::Cyan,
        AgentStatus::Retrying => Color::Yellow,
        AgentStatus::Error => Color::Red,
    }
}
