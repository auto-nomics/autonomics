//! Global terminal-input routing and top-level key dispatch.

use super::*;

impl App {
    pub(super) fn handle_event(&mut self, event: &Event) -> i32 {
        match event {
            Event::Key(key) if key.kind == crossterm::event::KeyEventKind::Press => {
                self.handle_key(key);
                0
            }
            Event::Resize(_, _) | Event::FocusGained | Event::FocusLost => 0,
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            Event::Paste(s) => {
                if self.state.model_config_visible {
                    self.state.model_config_state.insert_paste(s);
                    return 0;
                }

                if self.state.name_input.visible {
                    self.state.name_input.paste(s);
                    return 0;
                }

                // Insert paste into the agent chat input area when in input mode.
                if true {
                    let ts = self.state.active_tab_state_mut();
                    if ts.input_mode == InputMode::Input {
                        ts.input.insert_str(s);
                    }
                }
                // Insert paste into the focused textarea when in Config mode.
                if false {
                    use crate::widgets::model_config_widget::{ConfigField, ProviderPanelState};
                    if let ProviderPanelState::Config {
                        api_key,
                        base_url,
                        focused_field,
                        ..
                    } = &mut self.state.model_config_state.provider_panel_state
                    {
                        match *focused_field {
                            ConfigField::ApiKey => api_key.insert_str(s),
                            ConfigField::BaseUrl => base_url.insert_str(s),
                        }
                    }
                }
                0
            }
            _ => 0,
        }
    }

    /// Reset the cancel-request timestamp (called when the agent returns to Idle).
    pub(super) fn clear_cancel_pending(&mut self) {
        self.cancel_requested_at = None;
        self.state.active_tab_state_mut().cancel_pending = false;
    }

    /// Handle mouse events: scroll wheel scrolls the chat in Agent tab.
    /// Returns the scroll delta to be batched with other scroll events.
    fn handle_mouse(&mut self, mouse: &MouseEvent) -> i32 {
        // Mouse input is not routed by screen area yet. While an overlay is
        // visible, ignore wheel events instead of moving the hidden chat.
        let state = &self.state;
        if state.delete_agent_confirm
            || state.command_palette.is_visible()
            || state.profile_picker.visible
            || state.agent_picker.visible
            || state.name_input.visible
            || state.session_picker.visible
            || state.message_picker.is_visible()
            || state.dag_view_visible
            || state.model_config_visible
        {
            return 0;
        }

        let lines_per_tick: i32 = 3;

        match mouse.kind {
            MouseEventKind::ScrollDown => {
                let ts = self.state.active_tab_state_mut();
                ts.auto_scroll = false;
                lines_per_tick
            }
            MouseEventKind::ScrollUp => {
                let ts = self.state.active_tab_state_mut();
                ts.auto_scroll = false;
                -lines_per_tick
            }
            _ => 0,
        }
    }

    /// Apply a batched scroll delta to the agent tab.
    pub(super) fn apply_scroll_delta(&mut self, delta: i32) {
        let ts = self.state.active_tab_state_mut();
        if delta > 0 {
            ts.scroll_offset = ts.scroll_offset.saturating_add(delta as usize);
        } else {
            ts.scroll_offset = ts.scroll_offset.saturating_sub((-delta) as usize);
        }
    }

    pub(super) fn handle_key(&mut self, key: &KeyEvent) {
        // Delete-agent confirmation popup captures keys when visible.
        if self.state.delete_agent_confirm {
            self.handle_delete_confirm_key(key);
            return;
        }

        // Ctrl+P: toggle the command palette. Handled globally so it works
        // from any tab / input mode. When opening, takes precedence over all
        // other handlers; when closing, behaves identically to Esc.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
            self.state.command_palette.toggle();
            return;
        }

        // Ctrl+O opens a read-only view of the active agent's data DAG. This
        // is intentionally global: the overlay is useful while an agent is
        // composing or executing pipeline changes.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o') {
            self.open_dag_view();
            return;
        }

        // While the command palette is open it captures all remaining keys
        // (navigation, filtering, execution, dismissal) — tab handlers never
        // see them.
        if self.state.command_palette.is_visible() {
            self.handle_command_palette_key(key);
            return;
        }

        // Ctrl+W: close the active agent leaf (and terminate its background
        // process). Disabled when no agent is running.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('w') {
            if self.state.sessions.is_empty() {
                return;
            }
            self.close_active_agent();
            return;
        }

        // Profile picker popup captures keys when visible.
        if self.state.profile_picker.visible {
            self.handle_profile_picker_key(key);
            return;
        }

        // Agent resume picker popup captures keys when visible.
        if self.state.agent_picker.visible {
            self.handle_agent_picker_key(key);
            return;
        }

        // Name input popup captures keys when visible.
        if self.state.name_input.visible {
            self.handle_name_input_key(key);
            return;
        }

        // Session picker popup captures keys when visible.
        if self.state.session_picker.visible {
            self.handle_session_picker_key(key);
            return;
        }

        // Message picker popup captures keys when visible. Sits below the
        // session/profile/agent pickers so they remain authoritative when
        // a higher-level picker is also open.
        if self.state.message_picker.is_visible() {
            self.handle_message_picker_key(key);
            return;
        }

        // The DAG overlay captures keys after all high-level pickers so it can
        // be closed from beneath them, but before the agent composer sees
        // navigation keys as text.
        if self.state.dag_view_visible {
            self.handle_dag_view_key(key);
            return;
        }

        // Model config popup captures keys when visible.
        if self.state.model_config_visible {
            self.handle_model_config_key(key);
            return;
        }

        // Ctrl+C: cancel running agent first, then quit on second press.
        // If the agent is blocked and doesn't transition to Idle after the
        // first cancel, a second Ctrl+C within FORCE_QUIT_WINDOW force-quits.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.should_quit {
                // Already quitting — no-op.
                return;
            }
            let active_status = self.state.active_status();
            if !active_status.is_active() && active_status != AgentStatus::Waiting {
                // Idle, Error, Cancelled, or Aborted — quit immediately.
                self.should_quit = true;
                return;
            }
            // Agent is running — check for force-quit (double Ctrl+C).
            // Only treat as a double-press if the active tab is already
            // showing "cancel pending" (i.e., the first Ctrl+C targeted
            // *this* agent, not a different tab).
            if self.state.active_tab_state().cancel_pending {
                if let Some(ts) = self.cancel_requested_at {
                    if ts.elapsed() < FORCE_QUIT_WINDOW {
                        tracing::info!("force-quit: second Ctrl+C within {:?}", FORCE_QUIT_WINDOW);
                        let agent_name = self
                            .state
                            .sessions
                            .get(self.state.active_agent_idx)
                            .map(|s| s.name.clone());
                        if let Some(an) = agent_name {
                            self.shutdown_agent_remotely(&an);
                        }
                        self.should_quit = true;
                        return;
                    }
                }
            }
            // First Ctrl+C: cooperative cancel.
            let agent_name = self
                .state
                .sessions
                .get(self.state.active_agent_idx)
                .map(|s| s.name.clone());
            if let Some(an) = agent_name {
                let client = self.client.clone();
                self.spawn_client_task("cancel_agent", move || async move {
                    if let Err(e) = client.cancel_agent(&an).await {
                        tracing::warn!(agent = %an, error = %e, "cancel request failed");
                    }
                });
            }
            self.cancel_requested_at = Some(Instant::now());
            // Mark the active agent's tab as "cancel pending" so the UI
            // can show a "cancelling…" indicator.
            self.state.active_tab_state_mut().cancel_pending = true;
            return;
        }

        // Ctrl+G: toggle the auto-scroll-to-bottom lock on the Agent tab.
        // When locked (following the tail), the first press releases the lock
        // so the user can scroll freely; a second press re-pins to the bottom.
        // Scroll-producing keys/mouse also release the lock (see handle_mouse
        // and handle_browse_key); Ctrl+G is the dedicated toggle.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('g') {
            let ts = self.state.active_tab_state_mut();
            if ts.auto_scroll {
                ts.auto_scroll = false;
            } else {
                ts.scroll_to_bottom();
            }
            return;
        }

        // All remaining keys go to the agent workspace.
        self.handle_agent_key(key);
    }

    fn handle_agent_key(&mut self, key: &KeyEvent) {
        // Alt+1..9: switch to leaf by index.
        if key.modifiers.contains(KeyModifiers::ALT) {
            if let KeyCode::Char(c) = key.code {
                if let Some(digit) = c.to_digit(10) {
                    let idx = (digit as usize).saturating_sub(1);
                    if idx < self.state.sessions.len() {
                        self.state.active_agent_idx = idx;
                    }
                    return;
                }
            }
        }

        let input_mode = self.state.active_tab_state().input_mode;
        match input_mode {
            InputMode::Browse => self.handle_browse_key(key),
            InputMode::Input => self.handle_input_key(key),
        }
    }
}
