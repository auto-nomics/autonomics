//! The overlay registry — the single source of truth for modal surfaces.
//!
//! Three consumers used to hand-maintain parallel lists of every modal
//! surface: the render z-order ([`super::render`]), the key-capture chain
//! ([`super::keyboard::App::handle_key`]), and the mouse-wheel guard
//! ([`super::keyboard::App::handle_mouse`]). The lists drifted:
//!
//! - the delete-agent confirmation captured keys while rendering
//!   *beneath* the agents popup that opened it (invisible y/n prompt);
//! - the command palette opened over the DAG/bibliography overlays via
//!   the global `Ctrl+P` chord but painted beneath them (invisible
//!   palette that swallowed every key);
//! - the skill library dashboard was in the key-capture chain and in
//!   `any_overlay_open` but missing from the mouse guard, so wheel
//!   events scrolled the hidden chat underneath it.
//!
//! [`AppState::overlay_stack`] is now the one ordered list and all three
//! consumers derive from it.
//!
//! # Ordering rule
//!
//! The stack is listed bottom→top in *interaction priority*: the surface
//! that captures keys first is painted last. Concretely — confirmation
//! dialogs on top (they outrank even the global chords), then the
//! command palette, then the pickers/popups in the order `handle_key`
//! consults them, then the full-screen overlays (DAG, bibliography,
//! dashboards, config forms).
//!
//! Two surfaces deliberately stay outside the stack:
//!
//! - the leader which-key panel — a transient prefix hint that cannot
//!   coexist with any overlay (`<Space>` only arms in browse mode with
//!   the stack empty) and is drawn above everything while pending;
//! - toasts — display-only notifications that never capture input.

use crate::state::AppState;

/// A modal surface that captures keys and paints above the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Overlay {
    // ── Full-screen overlays and config forms (bottom of the stack) ──
    /// Per-agent runtime config form.
    AgentConfig,
    /// Model provider/catalog config form.
    ModelConfig,
    /// HPC (Slurm) site config + job monitor panel.
    HpcPanel,
    /// Embedding model config form.
    EmbeddingConfig,
    /// Skill library dashboard.
    SkillLibrary,
    /// Bibliography browser.
    Bibliography,
    /// VFS browser (files, mounts, container outputs).
    VfsBrowser,
    /// Interactive DAG view.
    DagView,
    FlowView,
    // ── Pickers and popups ──
    /// "Copy message" picker.
    MessagePicker,
    /// Agents-panel bulk effort quick-picker (`E`).
    AgentEffortPicker,
    /// Agents-panel model quick-picker (`m` / `M`).
    AgentModelPicker,
    /// Background tool tasks popup (`<leader>T`).
    TasksPopup,
    /// Task plan popup (`<leader>p`).
    PlanPopup,
    /// Agents popup (`Ctrl+T`), the agent switcher.
    AgentsPopup,
    /// New-agent name input.
    NameInput,
    /// Profile picker (spawn-agent kind list).
    /// Command palette (`Ctrl+P`).
    CommandPalette,
    // ── Confirmations (top of the stack) ──
    /// Bulk model/effort apply confirmation (y/n).
    BulkConfirm,
    /// Delete-agent confirmation (y/n).
    DeleteConfirm,
}

impl AppState {
    /// Visible overlays, bottom→top.
    ///
    /// The one ordered list backing the render z-order, the key-capture
    /// dispatch, and the mouse guard. Allocated per call — hot paths call
    /// it once per event/frame; an 18-slot `Vec` is noise next to a frame
    /// draw or a key handler.
    pub(super) fn overlay_stack(&self) -> Vec<Overlay> {
        use Overlay::*;
        [
            (AgentConfig, self.agent_config.visible),
            (ModelConfig, self.model_config_visible),
            (HpcPanel, self.hpc_panel.visible),
            (EmbeddingConfig, self.embedding_config_visible),
            (SkillLibrary, self.skill_library.visible),
            (Bibliography, self.bibliography_visible),
            (VfsBrowser, self.vfs_browser.visible),
            (DagView, self.dag_view_visible),
            (FlowView, self.flow_view_visible),
            (MessagePicker, self.message_picker.is_visible()),
            (TasksPopup, self.tasks_popup.visible),
            (PlanPopup, self.plan_popup.visible),
            (AgentsPopup, self.agents_popup.is_visible()),
            // The agents popup's own quick-pickers (m/M/e/E) spawn on top
            // of it and must paint above it — and capture keys — or they
            // open invisibly beneath it and the popup swallows every key.
            (AgentEffortPicker, self.agent_effort_picker.visible),
            (AgentModelPicker, self.agent_model_picker.is_visible()),
            (NameInput, self.name_input.visible),
            (CommandPalette, self.command_palette.is_visible()),
            (BulkConfirm, self.bulk_confirm.is_some()),
            (DeleteConfirm, self.delete_agent_confirm),
        ]
        .into_iter()
        .filter_map(|(overlay, visible)| visible.then_some(overlay))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flip every overlay's visibility flag on.
    fn all_visible(state: &mut AppState) {
        state.agent_config.visible = true;
        state.model_config_visible = true;
        state.hpc_panel.visible = true;
        state.embedding_config_visible = true;
        state.skill_library.visible = true;
        state.bibliography_visible = true;
        state.dag_view_visible = true;
        state.message_picker.picker.visible = true;
        state.agent_effort_picker.visible = true;
        state.agent_model_picker.picker.visible = true;
        state.tasks_popup.open();
        state.plan_popup.open();
        state.agents_popup.picker.visible = true;
        state.name_input.visible = true;
        state.command_palette.open();
        state.bulk_confirm = Some(crate::state::BulkConfirm {
            title: "t".into(),
            detail: "d".into(),
            action: crate::state::BulkAction::SetModels {
                agents: vec![],
                spec: "m".into(),
            },
        });
        state.delete_agent_confirm = true;
    }

    #[test]
    fn empty_state_has_no_overlays() {
        let state = AppState::default();
        assert!(state.overlay_stack().is_empty());
    }

    #[test]
    fn all_overlays_appear_exactly_once() {
        let mut state = AppState::default();
        all_visible(&mut state);
        let stack = state.overlay_stack();
        assert_eq!(stack.len(), 17, "every overlay variant must be listed");
        let unique: std::collections::HashSet<_> = stack.iter().collect();
        assert_eq!(unique.len(), stack.len(), "no duplicates in the stack");
    }

    #[test]
    fn confirmations_paint_above_everything() {
        let mut state = AppState::default();
        state.dag_view_visible = true;
        state.agents_popup.picker.visible = true;
        state.delete_agent_confirm = true;
        state.bulk_confirm = Some(crate::state::BulkConfirm {
            title: "t".into(),
            detail: "d".into(),
            action: crate::state::BulkAction::SetModels {
                agents: vec![],
                spec: "m".into(),
            },
        });
        let stack = state.overlay_stack();
        assert_eq!(stack.last(), Some(&Overlay::DeleteConfirm));
        assert_eq!(stack[stack.len() - 2], Overlay::BulkConfirm);
    }

    #[test]
    fn palette_paints_above_overlays_it_opens_over() {
        // Ctrl+P pierces the DAG/bibliography overlays, so the palette
        // must render on top of them (it used to paint beneath and
        // invisibly swallow every key).
        let mut state = AppState::default();
        state.dag_view_visible = true;
        state.bibliography_visible = true;
        state.command_palette.open();
        let stack = state.overlay_stack();
        let pos = |o| stack.iter().position(|&x| x == o).unwrap();
        assert!(pos(Overlay::CommandPalette) > pos(Overlay::DagView));
        assert!(pos(Overlay::CommandPalette) > pos(Overlay::Bibliography));
    }

    #[test]
    fn agents_popup_paints_above_the_confirm_it_spawns() {
        // Inverse of the confirmations test at the pair level: the
        // agents popup sits below both confirmations, so the y/n prompt
        // spawned from inside it is visible.
        let mut state = AppState::default();
        state.agents_popup.picker.visible = true;
        state.delete_agent_confirm = true;
        let stack = state.overlay_stack();
        assert!(
            stack.iter().position(|&x| x == Overlay::DeleteConfirm)
                > stack.iter().position(|&x| x == Overlay::AgentsPopup)
        );
    }

    #[test]
    fn quick_pickers_paint_above_the_agents_popup_that_spawns_them() {
        // Regression: m/M/e/E inside the agents popup open the model /
        // effort pickers. They must sit above the popup in the z-order —
        // below it they opened invisibly and the popup kept every key,
        // so the shortcuts appeared dead.
        let mut state = AppState::default();
        state.agents_popup.picker.visible = true;
        state.agent_model_picker.picker.visible = true;
        state.agent_effort_picker.visible = true;
        let stack = state.overlay_stack();
        let pos = |o: Overlay| stack.iter().position(|&x| x == o).unwrap();
        assert!(pos(Overlay::AgentModelPicker) > pos(Overlay::AgentsPopup));
        assert!(pos(Overlay::AgentEffortPicker) > pos(Overlay::AgentsPopup));
    }

    #[test]
    fn order_is_canonical_regardless_of_flag_order() {
        // The stack is a fixed z-order table, not open-order tracking:
        // whichever flags are set, relative order must be the same.
        let mut a = AppState::default();
        a.dag_view_visible = true;
        a.message_picker.picker.visible = true;
        a.command_palette.open();
        let mut b = AppState::default();
        b.command_palette.open();
        b.dag_view_visible = true;
        b.message_picker.picker.visible = true;
        assert_eq!(a.overlay_stack(), b.overlay_stack());
    }
}
