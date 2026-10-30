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

use ratatui::{
    layout::Rect,
    prelude::{Buffer, Widget as _},
    widgets::StatefulWidgetRef as _,
};

use crate::state::{AgentTabState, DisplaySettings};
use crate::widgets::agent_leaf::AgentLeaf;

use empty_state::EmptyState;
pub(crate) use status::{display_name, status_color, status_icon};

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
}

impl AgentWorkspace<'_> {
    /// Render the active agent's leaf into the full workspace area.
    ///
    /// `tab_state` is the active agent's context state. When no agent is
    /// running, renders a placeholder prompting the user to spawn an agent.
    pub fn render(&self, area: Rect, buf: &mut Buffer, tab_state: &mut AgentTabState) {
        if self.agent_name.is_none() {
            EmptyState.render(area, buf);
            return;
        }

        let leaf = AgentLeaf {
            agent_name: self.agent_name,
            active_model: self.active_model,
            context_window: self.context_window,
            reasoning_effort: self.reasoning_effort,
            kms_activity: self.kms_activity,
            display: self.display,
        };
        leaf.render_ref(area, buf, tab_state);
    }
}
