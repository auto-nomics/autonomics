//! Right-side workspace container composed from child widgets.
//!
//! Renders either an empty-state placeholder or the active agent's leaf.
//! Agent switching is driven by the docked agents panel or keyboard
//! shortcuts handled in `App`; the active agent's identity is shown at the
//! front of the status line inside the composer row instead of a dedicated
//! tab strip, so the conversation gets the full height.
//!
//! # Layout
//!
//! ```text
//! ┌──────────────────────────────────────────┐
//! │                                          │
//! │  Active AgentLeaf                        │
//! │  (chat + sidebar + composer + footer)    │
//! │                                          │
//! │  ○ name │ i: … o: …      glm-5.3 ⋅ high │
//! │  ↑ composer row; shows the status line  │
//! │    while the input buffer is empty       │
//! └──────────────────────────────────────────┘
//! ```

mod empty_state;
mod logo;
mod status;
mod tab_bar;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::{Buffer, Widget as _},
    widgets::StatefulWidgetRef as _,
};

use crate::state::{AgentTabState, DisplaySettings};
use crate::widgets::agent_leaf::AgentLeaf;
use crate::widgets::sidebar::RunningAgent;

use empty_state::EmptyState;
pub(crate) use status::{display_name, status_color, status_icon};
pub use tab_bar::LeafTab;
use tab_bar::TabBar;
pub(crate) use tab_bar::{TabBarFit, fit_tabs};

/// Composes the workspace from its empty state or active-leaf parts.
pub struct AgentWorkspace<'a> {
    /// Name of the active agent; `None` when no agent is running, which
    /// renders the empty-state placeholder instead of a leaf.
    pub agent_name: Option<&'a str>,
    pub active_model: Option<&'a str>,
    /// Context window size of the active model (tokens), when known.
    pub context_window: Option<u64>,
    /// The reasoning effort the active agent's next request would send.
    pub reasoning_effort: Option<&'a str>,
    pub kms_activity: Option<&'a str>,
    /// Global display settings (collapse toggles).
    pub display: &'a DisplaySettings,
    /// All running agents for the sidebar's docked switch-agent panel.
    pub agents: &'a [RunningAgent],
}

impl AgentWorkspace<'_> {
    /// Render the workspace: tab bar (2 rows) + active leaf (remaining
    /// space).
    ///
    /// `tabs` is the full list of running agents; `active_idx` determines
    /// which tab is highlighted and which leaf's `tab_state` is rendered.
    /// When `tabs` is empty, renders a placeholder prompting the user to
    /// spawn an agent.
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
            agent_name: self.agent_name,
            active_model: self.active_model,
            context_window: self.context_window,
            reasoning_effort: self.reasoning_effort,
            kms_activity: self.kms_activity,
            display: self.display,
            agents: self.agents,
        };
        leaf.render_ref(areas[1], buf, tab_state);
    }
}
