//! Command palette dispatch and shortcut actions.

use super::history::{compute_search_matches, load_selected_history_match};
use super::*;

impl App {
    // ── Command palette ─────────────────────────────────

    /// Key handling while the command palette is open. Returns control to
    /// `handle_key`'s caller; the caller is responsible for closing on Esc
    /// (via [`CommandPaletteState::close`]).
    pub(super) fn handle_command_palette_key(&mut self, key: &KeyEvent) {
        use crate::widgets::command_palette::CommandPaletteKeyOutcome;

        match self.state.command_palette.handle_key(*key) {
            CommandPaletteKeyOutcome::Consumed => {}
            CommandPaletteKeyOutcome::Execute(action) => {
                self.state.command_palette.close();
                self.run_command_action(action);
            }
            CommandPaletteKeyOutcome::Close => self.state.command_palette.close(),
        }
    }

    /// Execute a command selected from the palette. Each action mirrors an
    /// existing key binding — the palette is just a discoverable shortcut to
    /// the same operations.
    pub(super) fn run_command_action(
        &mut self,
        action: crate::widgets::command_palette::CommandAction,
    ) {
        use crate::widgets::command_palette::CommandAction;

        match action {
            CommandAction::Quit => {
                self.should_quit = true;
            }
            CommandAction::CancelAgent => {
                if self.state.active_status().is_active() {
                    let agent_name = self
                        .state
                        .sessions
                        .get(self.state.active_agent_idx)
                        .map(|s| s.name.clone());
                    if let Some(an) = agent_name {
                        if let Some(host) = self.host.as_ref() {
                            host.control().cancel_agent(&an);
                        }
                    }
                    self.cancel_requested_at = Some(Instant::now());
                    self.state.active_tab_state_mut().cancel_pending = true;
                }
            }
            CommandAction::EnterInput => {
                if true {
                    self.state.active_tab_state_mut().input_mode = InputMode::Input;
                }
            }
            CommandAction::ToggleAutoScroll => {
                if true {
                    let ts = self.state.active_tab_state_mut();
                    if ts.auto_scroll {
                        ts.auto_scroll = false;
                    } else {
                        ts.scroll_to_bottom();
                    }
                }
            }
            CommandAction::ScrollToBottom => {
                if true {
                    self.state.active_tab_state_mut().scroll_to_bottom();
                }
            }
            CommandAction::ScrollToTop => {
                if true {
                    let ts = self.state.active_tab_state_mut();
                    ts.scroll_offset = 0;
                    ts.auto_scroll = false;
                }
            }
            CommandAction::HistorySearch => {
                // Trigger the same flow as Ctrl+R: only meaningful in the
                // Agent tab when there is history and the agent is idle.
                let is_agent_tab = true;
                let ts = self.state.active_tab_state_mut();
                let can = is_agent_tab && !ts.status.is_active() && !ts.input_history.is_empty();
                if can {
                    ts.input_mode = InputMode::Input;
                    ts.in_history_search = true;
                    ts.history_search_query.clear();
                    ts.history_search_draft = Some(ts.input.value());
                    ts.history_search_matches = compute_search_matches(&ts.input_history, "");
                    ts.history_search_selected = 0;
                    load_selected_history_match(ts);
                }
            }
            CommandAction::ClearTranscript => {
                let ts = self.state.active_tab_state_mut();
                ts.messages.clear();
                ts.msg_versions.clear();
                ts.cached_msg_lines.clear();
                ts.cached_msg_versions.clear();
                ts.scroll_offset = 0;
                ts.scroll_to_bottom();
            }
            CommandAction::ReloadConfig => {
                Self::load_model_config(&self.conn, &mut self.state.model_config_state);
            }
            CommandAction::SpawnAgent(profile_name) => {
                if let Some(profile) = self
                    .state
                    .profiles
                    .iter()
                    .find(|p| p.path == profile_name)
                    .cloned()
                {
                    // Stash the profile; name input prompts for the agent name.
                    self.state.pending_profile = Some(profile.clone());
                    self.state
                        .name_input
                        .open(format!(" New Agent ({}) ", profile.name()), profile.name());
                }
            }
            CommandAction::NewAgent => {
                self.state.profile_picker.open();
            }
            CommandAction::ResumeAgent => {
                self.open_agent_picker();
            }
            CommandAction::CloseAgent => {
                if !self.state.sessions.is_empty() {
                    self.close_active_agent();
                }
            }
            CommandAction::DeleteAgent => {
                if !self.state.sessions.is_empty() {
                    self.state.delete_agent_confirm = true;
                }
            }
            CommandAction::ModelConfig => {
                self.state.model_config_visible = true;
            }
            CommandAction::OpenSessions => {
                self.open_session_picker();
            }
            CommandAction::NewSession => {
                // Prompt for a session name, then create a real backend
                // session via host.control().create_session(...). This flows
                // through handle_name_input_key → pending_session_name.
                if self.state.sessions.is_empty() {
                    tracing::warn!("no active agent — cannot create session");
                    return;
                }
                self.state.pending_session_name = true;
                self.state.name_input.open(" New Session ", "New session");
            }
            CommandAction::ToggleCollapseThinking => {
                self.state.display_settings.toggle_thinking();
                self.persist_display_setting(
                    "collapse_thinking",
                    self.state.display_settings.collapse_thinking,
                );
            }
            CommandAction::ToggleCollapseToolCalls => {
                self.state.display_settings.toggle_tool_calls();
                self.persist_display_setting(
                    "collapse_tool_calls",
                    self.state.display_settings.collapse_tool_calls,
                );
            }
            CommandAction::ToggleCollapseToolResults => {
                self.state.display_settings.toggle_tool_results();
                self.persist_display_setting(
                    "collapse_tool_results",
                    self.state.display_settings.collapse_tool_results,
                );
            }
            CommandAction::CopyMessage => {
                self.open_message_picker();
            }
            CommandAction::Compact => {
                let agent_name = self
                    .state
                    .sessions
                    .get(self.state.active_agent_idx)
                    .map(|s| s.name.clone());
                if let Some(an) = agent_name {
                    if let Some(host) = self.host.as_ref() {
                        host.control().compact_agent(&an);
                    }
                }
            }
        }
    }
}
