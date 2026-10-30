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
                if self.state.embedding_config_visible {
                    self.state.embedding_config_state.insert_paste(s);
                    return 0;
                }

                if self.state.bibliography_visible {
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

    /// Handle mouse events: scroll wheel scrolls the chat in Agent tab;
    /// clicking the tab bar's collapse stub opens the switch-agent picker.
    /// Returns the scroll delta to be batched with other scroll events.
    fn handle_mouse(&mut self, mouse: &MouseEvent) -> i32 {
        // Mouse input is not routed by screen area yet. While an overlay is
        // visible, ignore wheel events instead of moving the hidden chat.
        let state = &self.state;
        if state.delete_agent_confirm
            || state.bulk_confirm.is_some()
            || state.command_palette.is_visible()
            || state.profile_picker.visible
            || state.agent_profile_picker.visible
            || state.agents_popup.is_visible()
            || state.plan_popup.visible
            || state.tasks_popup.visible
            || state.agent_model_picker.is_visible()
            || state.agent_effort_picker.visible
            || state.name_input.visible
            || state.message_picker.is_visible()
            || state.dag_view_visible
            || state.bibliography_visible
            || state.model_config_visible
            || state.embedding_config_visible
            || state.agent_config.visible
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

    /// True while any modal surface owns the screen (leader keys are a
    /// browse-mode affordance and must not fight an open popup).
    fn any_overlay_open(&self) -> bool {
        let state = &self.state;
        state.delete_agent_confirm
            || state.bulk_confirm.is_some()
            || state.command_palette.is_visible()
            || state.profile_picker.visible
            || state.agent_profile_picker.visible
            || state.agents_popup.is_visible()
            || state.plan_popup.visible
            || state.tasks_popup.visible
            || state.agent_model_picker.is_visible()
            || state.agent_effort_picker.visible
            || state.name_input.visible
            || state.message_picker.is_visible()
            || state.dag_view_visible
            || state.bibliography_visible
            || state.model_config_visible
            || state.embedding_config_visible
            || state.agent_config.visible
            || state.skill_evolution.visible
    }

    /// Feed one key to the pending leader prefix. Returns `true` when the
    /// key was consumed (the caller must not forward it). `Esc` cancels;
    /// any unmapped key cancels and is swallowed, LazyVim-style.
    fn handle_leader_key(&mut self, key: &KeyEvent) -> bool {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return true;
        }
        if key.code == KeyCode::Esc {
            self.leader_pending = None;
            return true;
        }
        // Backspace climbs one level (the panel's 返回 hint); at the root
        // it closes, like LazyVim's which-key back.
        if key.code == KeyCode::Backspace {
            let mut prefix = self.leader_pending.take().unwrap_or_default();
            if !prefix.is_empty() {
                prefix.pop();
                self.leader_pending = Some(prefix);
            }
            return true;
        }
        let Some(ch) = key.code.as_char() else {
            // Chords (Ctrl+…, arrows, Enter) cancel the prefix and are
            // swallowed; re-press the chord if it was meant to act.
            self.leader_pending = None;
            return true;
        };
        let prefix = self.leader_pending.take().unwrap_or_default();
        match super::leader::resolve(&prefix, ch) {
            super::leader::LeaderResolve::Deeper(_) => {
                let mut deeper = prefix;
                deeper.push(ch);
                self.leader_pending = Some(deeper);
            }
            super::leader::LeaderResolve::Hit(action) => {
                self.run_command_action(action);
            }
            super::leader::LeaderResolve::Miss => {
                self.state.toasts.info(
                    "Leader",
                    Some(format!("␣{ch} 未绑定;Esc 取消,␣␣ 打开命令面板")),
                );
            }
        }
        true
    }

    pub(super) fn handle_key(&mut self, key: &KeyEvent) {
        // Delete-agent confirmation popup captures keys when visible.
        if self.state.delete_agent_confirm {
            self.handle_delete_confirm_key(key);
            return;
        }

        // Bulk model/effort apply confirmation captures keys when visible
        // (same y/n pattern as the delete confirmation above it).
        if self.state.bulk_confirm.is_some() {
            self.handle_bulk_confirm_key(key);
            return;
        }

        // An active leader prefix consumes the next key (which-key hint is
        // rendered above the composer).
        if self.leader_pending.is_some() {
            self.handle_leader_key(key);
            return;
        }

        // Ctrl+P: toggle the command palette. Handled globally so it works
        // from any tab / input mode. When opening, takes precedence over all
        // other handlers; when closing, behaves identically to Esc.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
            self.state.command_palette.toggle();
            return;
        }

        // Ctrl+T: toggle the agents popup — the picker-style switcher over
        // every running agent (search, status tabs, model/effort quick
        // switching).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('t') {
            if !self.state.sessions.is_empty() {
                self.toggle_agents_popup();
            }
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
        // process). Inside the agents popup it closes the selected leaf.
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
        if self.state.agent_profile_picker.visible {
            self.handle_agent_profile_picker_key(key);
            return;
        }

        // Name input popup captures keys when visible.
        if self.state.name_input.visible {
            self.handle_name_input_key(key);
            return;
        }

        // Agents popup (Ctrl+T) captures keys when visible; its quick
        // pickers sit just below it, then the plan/tasks popups.
        if self.state.agents_popup.is_visible() {
            self.handle_agents_popup_key(key);
            return;
        }
        if self.state.plan_popup.visible {
            self.handle_plan_popup_key(key);
            return;
        }
        if self.state.tasks_popup.visible {
            self.handle_tasks_popup_key(key);
            return;
        }

        // Agents-popup model quick-picker (m / M) captures keys when
        // visible; the effort picker (E) sits just below it.
        if self.state.agent_model_picker.is_visible() {
            self.handle_agent_model_picker_key(key);
            return;
        }
        if self.state.agent_effort_picker.visible {
            self.handle_agent_effort_picker_key(key);
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

        if self.state.bibliography_visible {
            self.handle_bibliography_key(key);
            return;
        }

        // Skill-evolution dashboard captures keys when visible.
        if self.state.skill_evolution.visible {
            self.handle_skill_evolution_key(key);
            return;
        }

        if self.state.embedding_config_visible {
            self.handle_embedding_config_key(key);
            return;
        }

        // Model config popup captures keys when visible.
        if self.state.model_config_visible {
            self.handle_model_config_key(key);
            return;
        }

        if self.state.agent_config.visible {
            self.handle_agent_config_key(key);
            return;
        }

        // <Space> enters the leader prefix (LazyVim-style), available in
        // browse mode whenever no popup owns the screen. The composer is
        // never active in browse mode, so the space is free.
        if key.code == KeyCode::Char(' ')
            && key.modifiers.is_empty()
            && !self.any_overlay_open()
            && self.state.active_tab_state().input_mode == InputMode::Browse
        {
            // Empty prefix = the root menu.
            self.leader_pending = Some(Vec::new());
            return;
        }

        // Ctrl+C is deliberately NOT bound: accidental presses repeatedly
        // interrupted running agents mid-task. Cancelling and quitting are
        // reachable only through the command palette (Ctrl+P → "Cancel
        // agent" / "Quit"), which requires an explicit selection. When no
        // popup captures the key, Ctrl+C falls through to the workspace
        // handler, which ignores chords with CONTROL held.

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
