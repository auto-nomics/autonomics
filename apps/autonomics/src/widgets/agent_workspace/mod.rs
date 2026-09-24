//! Right-side workspace container composed from child widgets.
//!
//! Renders either an empty-state placeholder or a horizontal tab bar followed
//! by the active agent's leaf. The tab bar is purely visual: agent switching
//! is driven by the sidebar or keyboard shortcuts handled in `App`.
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

mod empty_state;
mod logo;
mod tab_bar;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::{Buffer, Widget},
    widgets::StatefulWidgetRef,
};

use crate::state::{AgentTabState, DisplaySettings};
use crate::widgets::agent_leaf::AgentLeaf;

use empty_state::EmptyState;
pub use tab_bar::LeafTab;
use tab_bar::TabBar;
pub(crate) use tab_bar::{TabBarFit, fit_tabs, status_color, status_icon};

/// Composes the workspace from its empty state or tab-bar + active-leaf parts.
pub struct AgentWorkspace<'a> {
    pub active_model: Option<&'a str>,
    /// Context window size of the active model (tokens), when known.
    pub context_window: Option<u64>,
    /// Sub-sessions for sidebar rendering.
    pub sessions: &'a [crate::widgets::session_list::SessionSummary],
    /// Global display settings (collapse toggles).
    pub display: &'a DisplaySettings,
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
        if tabs.is_empty() {
            EmptyState.render(area, buf);
            return;
        }

        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // Tab bar (1 row tabs + 1 row border)
                Constraint::Min(3),    // Active leaf
            ])
            .split(area);

        TabBar { tabs, active_idx }.render(areas[0], buf);

        let leaf = AgentLeaf {
            active_model: self.active_model,
            context_window: self.context_window,
            sessions: self.sessions,
            display: self.display,
        };
        leaf.render_ref(areas[1], buf, tab_state);
    }
}
