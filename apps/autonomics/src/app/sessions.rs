//! Backend session switching, creation, renaming, and closing.

use super::*;

impl App {
    /// Key handling while the name input popup is open.
    pub(super) fn handle_name_input_key(&mut self, key: &KeyEvent) {
        use crate::widgets::name_input::NameInputKeyOutcome;

        match self.state.name_input.handle_key(*key) {
            NameInputKeyOutcome::Cancel => {
                self.state.name_input.close();
                self.state.pending_profile = None;
                self.state.pending_session_name = false;
                self.state.pending_session_rename_id = None;
            }
            NameInputKeyOutcome::Submit => {
                let name = self.state.name_input.value().to_string();
                self.state.name_input.close();

                if let Some(session_id) = self.state.pending_session_rename_id.take() {
                    // ── Session rename mode ──
                    let title = if name.is_empty() {
                        "Untitled".to_string()
                    } else {
                        name
                    };
                    let agent_name = self
                        .state
                        .sessions
                        .get(self.state.active_agent_idx)
                        .map(|s| s.name.clone());
                    if let Some(an) = agent_name {
                        self.fire_session_command("rename_session", move |client| async move {
                            let _ = client.rename_session(&an, session_id, title).await;
                        });
                    }
                } else if self.state.pending_session_name {
                    // ── Session naming mode (new session) ──
                    self.state.pending_session_name = false;
                    let title = if name.is_empty() {
                        "New session".to_string()
                    } else {
                        name
                    };
                    if let Some(an) = self
                        .state
                        .sessions
                        .get(self.state.active_agent_idx)
                        .map(|s| s.name.clone())
                    {
                        self.fire_session_command("create_session", move |client| async move {
                            let _ = client.create_session(&an, Some(title), None).await;
                        });
                    }
                } else {
                    // ── Agent naming mode ──
                    let profile = self.state.pending_profile.take();
                    if let Some(p) = profile {
                        if name.is_empty() {
                            tracing::warn!("agent name cannot be empty");
                            return;
                        }
                        self.spawn_agent_from_profile(&p, &name);
                    }
                }
            }
            NameInputKeyOutcome::Handled => {}
        }
    }

    /// Key handling while the session picker popup is open.
    ///
    /// Matches the agent picker pattern: plain characters feed the search
    /// filter; `Ctrl+`-modified keys trigger actions (new, close, rename).
    pub(super) fn handle_session_picker_key(&mut self, key: &KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.state.session_picker.close();
            }
            KeyCode::Up => self.state.session_picker.move_up(),
            KeyCode::Down => self.state.session_picker.move_down(),
            KeyCode::Backspace => self.state.session_picker.pop_char(),
            // Ctrl+N: new session
            KeyCode::Char('n') if ctrl => {
                self.state.session_picker.close();
                self.state.pending_session_name = true;
                self.state.name_input.open(" New Session ", "New session");
            }
            // Ctrl+D: close the selected session
            KeyCode::Char('d') if ctrl => {
                if let Some(id) = self.state.session_picker.selected_id() {
                    let agent_name = self
                        .state
                        .sessions
                        .get(self.state.active_agent_idx)
                        .map(|s| s.name.clone());
                    if let Some(an) = agent_name {
                        self.fire_session_command("close_session", move |client| async move {
                            let _ = client.close_session(&an, id).await;
                        });
                    }
                }
                self.state.session_picker.close();
            }
            // Ctrl+R: rename the selected session
            KeyCode::Char('r') if ctrl => {
                let selected_id = self.state.session_picker.selected_id();
                if let Some(id) = selected_id {
                    // Pre-fill with current title.
                    let current_title = self
                        .state
                        .session_picker
                        .selected_id()
                        .and_then(|sid| {
                            self.state
                                .session_picker
                                .items
                                .iter()
                                .find(|s| s.id == sid)
                                .and_then(|s| s.title.clone())
                        })
                        .unwrap_or_default();
                    self.state.pending_session_rename_id = Some(id);
                    self.state.session_picker.close();
                    self.state
                        .name_input
                        .open(" Rename Session ", current_title);
                }
            }
            // Regular characters → search filter
            KeyCode::Char(c) if !ctrl => {
                self.state.session_picker.push_char(c);
            }
            KeyCode::Enter => {
                // Switch to the selected session.
                if let Some(id) = self.state.session_picker.selected_id() {
                    let agent_name = self
                        .state
                        .sessions
                        .get(self.state.active_agent_idx)
                        .map(|s| s.name.clone());
                    if let Some(an) = agent_name {
                        self.fire_session_command("switch_session", move |client| async move {
                            let _ = client.switch_session(&an, id).await;
                        });
                    }
                }
                self.state.session_picker.close();
            }
            _ => {}
        }
    }
    /// Open the session picker for the currently active agent.
    pub(super) fn open_session_picker(&mut self) {
        let Some(agent_session) = self.state.sessions.get(self.state.active_agent_idx) else {
            tracing::warn!("no active agent to open session picker");
            return;
        };
        let agent_id = agent_session.agent_id;
        let agent_name = agent_session.name.clone();
        // Seed picker with whatever sub_sessions are already known; the
        // SessionList event will refresh the list shortly.
        let initial_items: Vec<crate::widgets::session_picker::PickerSession> = agent_session
            .sub_sessions
            .iter()
            .map(|s| {
                let stats =
                    crate::widgets::session_picker::compute_session_stats(&s.tab_state.messages);
                crate::widgets::session_picker::PickerSession {
                    id: s.id,
                    title: s.title.clone(),
                    last_active: s.last_active,
                    created_at: 0,
                    user_message_count: stats.user_message_count,
                    assistant_message_count: stats.assistant_message_count,
                    tool_call_count: s.telemetry.total_tool_use as usize,
                    input_tokens: s.telemetry.input_tokens,
                    output_tokens: s.telemetry.output_tokens,
                    total_tokens: s.telemetry.total_tokens,
                    total_tool_use: s.telemetry.total_tool_use,
                    time_consume_ms: s.telemetry.time_consume_ms,
                    first_user_message: stats.first_user_message,
                    last_assistant_message: stats.last_assistant_message,
                }
            })
            .collect();
        let active_id = agent_session
            .sub_sessions
            .get(agent_session.active_sub_session_idx)
            .map(|s| s.id);
        self.state.session_picker.open(agent_id, agent_name.clone());
        self.state
            .session_picker
            .set_sessions(initial_items, active_id);
        // Ask the daemon for a fresh list (will arrive via SessionList
        // event frames).
        let agent_for_task = agent_name.clone();
        self.fire_session_command("list_sessions", move |client| async move {
            let _ = client.list_sessions(&agent_for_task).await;
        });
        tracing::info!(agent = %agent_name, "session picker opened");
    }
}

impl App {
    /// Fire a per-agent session command at the daemon. All session
    /// commands are fire-and-forget (202): results flow back as agent
    /// event frames, exactly as with the in-process host.
    pub(super) fn fire_session_command<F, Fut>(&self, name: &'static str, make: F)
    where
        F: FnOnce(gateway::GatewayClient) -> Fut,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let client = self.client.clone();
        let task = make(client);
        agentik_core::supervise::spawn_safe_on_drop(&self.runtime_handle, name, task);
    }
}
