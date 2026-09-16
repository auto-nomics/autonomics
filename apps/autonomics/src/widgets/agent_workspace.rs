//! Right-side workspace container with leaf tabs.
//!
//! Renders a horizontal tab bar (one tab per running agent) and the active
//! agent's leaf below it. The tab bar is purely visual — agent switching is
//! driven by the sidebar or keyboard shortcuts handled in `App`. Tabs that
//! do not fit the available width are collapsed into a `>` stub; the active
//! tab is never collapsed.
//!
//! # Layout
//!
//! ```text
//! ┌──────────────────────────────────────────┐
//! │ ● default │ ◐ literature │  gwas-analysis│  ← Tab bar
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

use unicode_width::UnicodeWidthStr;

use crate::state::{AgentStatus, AgentTabState, DisplaySettings};
use crate::widgets::agent_leaf::AgentLeaf;

/// Renders the workspace: leaf tab bar + active leaf.
pub struct AgentWorkspace<'a> {
    pub active_model: Option<&'a str>,
    /// Context window size of the active model (tokens), when known.
    pub context_window: Option<u64>,
    /// Sub-sessions for sidebar rendering.
    pub sessions: &'a [crate::widgets::session_list::SessionSummary],
    /// Global display settings (collapse toggles).
    pub display: &'a DisplaySettings,
}

/// One entry in the tab bar.
pub struct LeafTab {
    pub name: String,
    pub status: AgentStatus,
}

/// Result of fitting tabs into a fixed width: how many tabs render, whether
/// the rest were collapsed into a stub, and the display width the visible
/// part occupies (the stub, if any, spans `used_width..width`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TabBarFit {
    /// Number of tabs rendered before collapsing.
    pub visible_tabs: usize,
    /// `true` when the remaining tabs were collapsed into a `>` stub.
    pub collapsed: bool,
    /// Display width used by visible tabs + separators.
    pub used_width: usize,
}

/// Decide which tabs fit into `width` columns. Shared by the renderer and
/// the mouse handler (stub click detection) so both agree on the layout.
///
/// One column is reserved for the `>` stub so the collapse affordance is
/// always visible. The active tab is never collapsed away: if it lies past
/// the collapse point the visible range is extended to include it (any
/// overflow is clipped by the terminal edge).
pub(crate) fn fit_tabs(width: usize, tabs: &[LeafTab], active_idx: usize) -> TabBarFit {
    let budget = width.saturating_sub(1); // reserve the stub column
    let cost = |i: usize| -> usize {
        let tab = &tabs[i];
        let label_w = format!(" {} {} ", status_icon(&tab.status), tab.name).width();
        label_w + usize::from(i + 1 < tabs.len())
    };

    let mut used = 0usize;
    for i in 0..tabs.len() {
        if used + cost(i) > budget && i != active_idx {
            // Keep the active tab visible even past the collapse point.
            let visible = (active_idx + 1).min(tabs.len()).max(i);
            return TabBarFit {
                visible_tabs: visible,
                collapsed: visible < tabs.len(),
                used_width: (0..visible).map(cost).sum(),
            };
        }
        used += cost(i);
    }
    TabBarFit {
        visible_tabs: tabs.len(),
        collapsed: false,
        used_width: used,
    }
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
        Self::render_top_tab_bar(areas[0], buf, tabs, active_idx);

        // ── Active leaf ──
        let leaf = AgentLeaf {
            active_model: self.active_model,
            context_window: self.context_window,
            sessions: self.sessions,
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
                    Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
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

    /// Collapsed-tab indicator: a `>` centered in the remaining width.
    /// Returns an empty span when there is no room for it.
    fn render_tab_stub(width: usize) -> Span<'static> {
        if width <= 1 {
            return Span::raw("");
        }
        let left = (width - 1) / 2;
        Span::styled(
            format!("{}>{}", " ".repeat(left), " ".repeat(width - 1 - left)),
            Style::default().fg(Color::Reset),
        )
    }

    fn render_top_tab_bar(area: Rect, buf: &mut Buffer, tabs: &[LeafTab], active_idx: usize) {
        let fit = fit_tabs(area.width as usize, tabs, active_idx);
        let visible = &tabs[..fit.visible_tabs];
        let mut spans: Vec<Span> = Vec::new();

        for (i, tab) in visible.iter().enumerate() {
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
                spans.push(Span::styled(label, Style::default().fg(Color::DarkGray)));
            }

            // Separator between rendered tabs; when tabs were collapsed the
            // last visible tab keeps its separator before the stub.
            if i + 1 < visible.len() || fit.collapsed {
                spans.push(Span::raw("│"));
            }
        }

        if fit.collapsed {
            spans.push(Self::render_tab_stub(
                area.width as usize - fit.used_width.min(area.width as usize),
            ));
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

pub(crate) fn status_icon(status: &AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle | AgentStatus::Aborted => "●",
        AgentStatus::Requesting => "◐",
        AgentStatus::Streaming => "◑",
        AgentStatus::ToolRunning => "⚙",
        AgentStatus::Retrying => "↻",
        AgentStatus::Error => "✗",
        AgentStatus::Cancelled => "⊘",
        AgentStatus::Waiting => "☐",
        AgentStatus::Compacting => "⟳",
    }
}

pub(crate) fn status_color(status: &AgentStatus) -> Color {
    match status {
        AgentStatus::Idle | AgentStatus::Aborted => Color::Green,
        AgentStatus::Requesting | AgentStatus::Streaming => Color::Cyan,
        AgentStatus::ToolRunning => Color::Yellow,
        AgentStatus::Retrying => Color::Yellow,
        AgentStatus::Error => Color::Red,
        AgentStatus::Cancelled => Color::DarkGray,
        AgentStatus::Waiting => Color::Blue,
        AgentStatus::Compacting => Color::Magenta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(name: &str) -> LeafTab {
        LeafTab {
            name: name.to_string(),
            status: AgentStatus::Idle,
        }
    }

    #[test]
    fn fit_tabs_shows_everything_when_there_is_room() {
        let tabs = [tab("a"), tab("bb")];
        let fit = fit_tabs(40, &tabs, 0);
        assert!(!fit.collapsed);
        assert_eq!(fit.visible_tabs, 2);
        assert_eq!(fit.used_width, " ● a ".width() + 1 + " ● bb ".width());
    }

    #[test]
    fn fit_tabs_collapses_and_reports_stub_offset() {
        let tabs = [tab("aaaa"), tab("bbbb"), tab("cccc")];
        // Room for tab 0 + separator + the reserved stub column, no more.
        let first = " ● aaaa ".width();
        let fit = fit_tabs(first + 1 + 1, &tabs, 0);
        assert!(fit.collapsed);
        assert_eq!(fit.visible_tabs, 1);
        assert_eq!(fit.used_width, first + 1);
    }

    #[test]
    fn fit_tabs_keeps_active_tab_visible_even_past_collapse_point() {
        let tabs = [tab("aaaa"), tab("bbbb")];
        // Far too narrow for either tab, but tab 1 is active: the visible
        // range must extend to include it (clipped, not collapsed away).
        let fit = fit_tabs(3, &tabs, 1);
        assert!(!fit.collapsed);
        assert_eq!(fit.visible_tabs, 2);
    }

    #[test]
    fn fit_tabs_collapses_inactive_tabs_after_active() {
        let tabs = [tab("aa"), tab("bb"), tab("cc"), tab("dd")];
        // Active tab 0 fits; nothing else does.
        let fit = fit_tabs(" ● aa ".width() + 1 + 1, &tabs, 0);
        assert!(fit.collapsed);
        assert_eq!(fit.visible_tabs, 1);
    }
}
