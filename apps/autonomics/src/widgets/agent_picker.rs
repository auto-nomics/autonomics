//! Switch-agent picker — popup listing all **running** agents.
//!
//! Complements the workspace tab bar ([`crate::widgets::agent_workspace`]):
//! when tabs overflow the bar they collapse into a `>` stub, hiding agents.
//! This picker (Ctrl+T, or clicking the stub) shows every running agent
//! with the same status icon/color language as the tab bar and switches
//! the active leaf on Enter.
//!
//! ```text
//! ┌─ Switch Agent ───────────────────────┐
//! │ > search agents…                     │
//! │ ──────────────────────────────────── │
//! │  ● default              ● active      │
//! │  ◐ literature                         │
//! │  ⚙ gwas-analysis                      │
//! ├──────────────────────────────────────┤
//! │  Enter switch  ↑↓ navigate  Esc close │
//! └──────────────────────────────────────┘
//! ```
//!
//! Distinct from [`crate::widgets::agent_profile_picker`], which resumes
//! **stored** agents from the daemon's storage.

use ratatui::{
    layout::Rect,
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, StatefulWidget, Widget},
};

use crate::state::AgentStatus;
use crate::widgets::agent_workspace::{status_color, status_icon};
use crate::widgets::tree_picker::{render_chrome, render_footer, render_search, render_separator};

// ═══════════════════════════════════════════════════════════════════════
// Data types
// ═══════════════════════════════════════════════════════════════════════

/// One running agent entry, supplied by the app on refresh.
#[derive(Clone, Debug)]
pub struct RunningAgent {
    pub name: String,
    pub status: AgentStatus,
    /// `true` for the currently active leaf.
    pub is_active: bool,
}

/// One visible (search-filtered) row.
#[derive(Clone, Debug)]
struct Row {
    agent_idx: usize,
    name: String,
    status: AgentStatus,
    is_active: bool,
}

// ═══════════════════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════════════════

/// State for the switch-agent picker.
#[derive(Default)]
pub struct AgentPickerState {
    pub visible: bool,
    /// Search query string.
    pub query: String,
    /// All running agents (refreshed from `sessions` while open).
    agents: Vec<RunningAgent>,
    /// Search-filtered rows in display order.
    rows: Vec<Row>,
    selected: usize,
    list_state: ListState,
}

impl AgentPickerState {
    pub fn open(&mut self) {
        self.visible = true;
        self.query.clear();
        // Default to the active agent so Enter is a no-op confirmation.
        self.selected = self.rows.iter().position(|r| r.is_active).unwrap_or(0);
        self.sync_list_state();
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    /// Replace the agent list (called on open and before each key event,
    /// so agents spawned/closed while the picker is up stay in sync).
    /// Preserves the query and reselects the same agent by name.
    pub fn set_agents(&mut self, agents: Vec<RunningAgent>) {
        let selected_name = self.rows.get(self.selected).map(|r| r.name.clone());
        self.agents = agents;
        self.refilter();
        if let Some(name) = selected_name {
            if let Some(i) = self.rows.iter().position(|r| r.name == name) {
                self.selected = i;
            }
        }
        self.sync_list_state();
    }

    // ── Search ──

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    pub fn pop_char(&mut self) {
        self.query.pop();
        self.refilter();
    }

    // ── Navigation ──

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.sync_list_state();
    }

    pub fn move_down(&mut self) {
        let max = self.rows.len().saturating_sub(1);
        if self.selected < max {
            self.selected += 1;
        }
        self.sync_list_state();
    }

    /// Returns the selected running agent, if any.
    pub fn selected_agent(&self) -> Option<&RunningAgent> {
        let row = self.rows.get(self.selected)?;
        self.agents.get(row.agent_idx)
    }

    // ── Internal ──

    fn refilter(&mut self) {
        let needle = self.query.trim().to_lowercase();
        self.rows = self
            .agents
            .iter()
            .enumerate()
            .filter(|(_, a)| needle.is_empty() || a.name.to_lowercase().contains(&needle))
            .map(|(i, a)| Row {
                agent_idx: i,
                name: a.name.clone(),
                status: a.status,
                is_active: a.is_active,
            })
            .collect();
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
        self.sync_list_state();
    }

    fn sync_list_state(&mut self) {
        self.list_state.select(
            if self.rows.is_empty() || self.selected >= self.rows.len() {
                None
            } else {
                Some(self.selected)
            },
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Widget
// ═══════════════════════════════════════════════════════════════════════

/// Standalone widget that renders the switch-agent picker as a centered
/// popup with a single searchable agent list.
pub struct AgentPicker {
    pub accent: Color,
    /// Width of the popup (0 = auto, ~60% of frame).
    pub popup_width: u16,
}

impl AgentPicker {
    pub fn new() -> Self {
        Self {
            accent: Color::Cyan,
            popup_width: 0,
        }
    }

    pub fn popup_width(mut self, w: u16) -> Self {
        self.popup_width = w;
        self
    }
}

impl Default for AgentPicker {
    fn default() -> Self {
        Self::new()
    }
}

impl StatefulWidget for AgentPicker {
    type State = AgentPickerState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut AgentPickerState) {
        if !state.visible {
            return;
        }

        let layout = render_chrome(area, buf, " Switch Agent ", self.accent, self.popup_width);
        render_search(layout.search, buf, self.accent, &state.query, "agents");
        render_separator(layout.separator, buf);

        let block = Block::default().title(Span::styled(
            " Running Agents ",
            Style::default()
                .fg(self.accent)
                .add_modifier(Modifier::BOLD),
        ));
        let inner = block.inner(layout.content);
        Widget::render(block, layout.content, buf);

        if state.rows.is_empty() {
            let empty = if state.agents.is_empty() {
                "  No running agents."
            } else {
                "  No matching agents."
            };
            let line = Line::from(Span::styled(empty, Style::default().fg(Color::DarkGray)));
            // First content row is the block title; show text one row below.
            let text_area = Rect {
                x: layout.content.x,
                y: layout.content.y + 1,
                width: layout.content.width,
                height: 1,
            };
            Widget::render(ratatui::widgets::Paragraph::new(line), text_area, buf);
        } else {
            let items: Vec<ListItem> = state
                .rows
                .iter()
                .enumerate()
                .map(|(i, row)| ListItem::new(agent_line(row, i == state.selected, self.accent)))
                .collect();

            let list = List::new(items).highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(self.accent)
                    .add_modifier(Modifier::BOLD),
            );
            StatefulWidget::render(list, inner, buf, &mut state.list_state);
        }

        render_footer(layout.footer, buf, " Enter switch  ↑↓ navigate  Esc close");
    }
}

/// One list row: status icon (same language as the tab bar) + agent name,
/// with an `● active` marker on the current leaf.
fn agent_line(row: &Row, selected: bool, accent: Color) -> Line<'static> {
    let icon = status_icon(&row.status);
    let color = status_color(&row.status);

    let (icon_style, name_style) = if selected {
        (
            Style::default()
                .fg(Color::Black)
                .bg(accent)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(Color::Black)
                .bg(accent)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(color),
            if row.is_active {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        )
    };

    let mut spans = vec![
        Span::raw("  "),
        Span::styled(icon, icon_style),
        Span::raw(" "),
        Span::styled(row.name.clone(), name_style),
    ];
    if row.is_active {
        spans.push(Span::styled(
            "  ● active",
            if selected {
                Style::default().fg(Color::Black).bg(accent)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(name: &str, active: bool) -> RunningAgent {
        RunningAgent {
            name: name.to_string(),
            status: AgentStatus::Idle,
            is_active: active,
        }
    }

    #[test]
    fn open_defaults_to_active_agent() {
        let mut s = AgentPickerState::default();
        s.set_agents(vec![agent("alpha", false), agent("beta", true)]);
        s.open();
        assert_eq!(s.selected_agent().unwrap().name, "beta");
    }

    #[test]
    fn query_filters_rows() {
        let mut s = AgentPickerState::default();
        s.set_agents(vec![agent("alpha", false), agent("beta", true)]);
        s.open();
        s.push_char('l'); // "alpha" only — "beta" has no 'l'
        assert_eq!(s.selected_agent().unwrap().name, "alpha");
        s.pop_char();
        s.push_char('b'); // "beta" only
        assert_eq!(s.selected_agent().unwrap().name, "beta");
    }

    #[test]
    fn refresh_preserves_selection_by_name() {
        let mut s = AgentPickerState::default();
        s.set_agents(vec![agent("alpha", false), agent("beta", true)]);
        s.open();
        s.move_up(); // select "alpha"
        assert_eq!(s.selected_agent().unwrap().name, "alpha");
        // Agents reordered underneath the open picker.
        s.set_agents(vec![agent("beta", true), agent("alpha", false)]);
        assert_eq!(s.selected_agent().unwrap().name, "alpha");
    }

    #[test]
    fn refresh_drops_closed_agent_selection() {
        let mut s = AgentPickerState::default();
        s.set_agents(vec![agent("alpha", false), agent("beta", true)]);
        s.open();
        s.move_up(); // "alpha"
        s.set_agents(vec![agent("beta", true)]); // alpha closed
        assert_eq!(s.selected_agent().unwrap().name, "beta");
        assert_eq!(s.rows.len(), 1);
    }
}
