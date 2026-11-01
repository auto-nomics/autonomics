//! Agent naming flow — the name-input popup that follows profile selection.

use super::*;

impl App {
    /// Key handling while the name input popup is open. The popup collects
    /// a name for a freshly selected agent profile; sessions no longer have
    /// names (each agent owns one persistent context).
    pub(super) fn handle_name_input_key(&mut self, key: &KeyEvent) {
        use crate::widgets::name_input::NameInputKeyOutcome;

        match self.state.name_input.handle_key(*key) {
            NameInputKeyOutcome::Cancel => {
                self.state.name_input.close();
                self.state.pending_profile = None;
            }
            NameInputKeyOutcome::Submit => {
                let name = self.state.name_input.value().to_string();
                self.state.name_input.close();

                // ── Agent naming mode ──
                let kind = self.state.pending_profile.take();
                if let Some(kind) = kind {
                    if name.is_empty() {
                        tracing::warn!("agent name cannot be empty");
                        return;
                    }
                    self.spawn_agent_from_profile(kind, &name);
                }
            }
            NameInputKeyOutcome::Handled => {}
        }
    }
}
