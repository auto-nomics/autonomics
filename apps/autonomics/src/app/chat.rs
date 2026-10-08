//! Chat navigation, composer handling, history search, and copy tools.

use super::history::{
    compute_search_matches, end_history_search, load_selected_history_match,
    recompute_history_search,
};
use super::*;

impl App {
    pub(super) fn handle_browse_key(&mut self, key: &KeyEvent) {
        // Shift+H/L: switch to previous/next agent leaf tab.
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::Char('H') | KeyCode::Char('h') => {
                    if self.state.active_agent_idx > 0 {
                        self.state.active_agent_idx -= 1;
                    }
                    return;
                }
                KeyCode::Char('L') | KeyCode::Char('l') => {
                    let max = self.state.sessions.len().saturating_sub(1);
                    if self.state.active_agent_idx < max {
                        self.state.active_agent_idx += 1;
                    }
                    return;
                }
                _ => {}
            }
        }

        let ts = self.state.active_tab_state_mut();

        match key.code {
            // Down / PageDown: scroll down (show later content)
            KeyCode::Down => {
                ts.scroll_offset = ts.scroll_offset.saturating_add(1);
                ts.auto_scroll = false;
            }
            // Up: scroll up (show earlier content)
            KeyCode::Up => {
                ts.scroll_offset = ts.scroll_offset.saturating_sub(1);
                ts.auto_scroll = false;
            }
            // End: jump to bottom, re-enable auto-scroll
            KeyCode::End => {
                ts.auto_scroll = true;
            }
            // Home: jump to top
            KeyCode::Home => {
                ts.scroll_offset = 0;
                ts.auto_scroll = false;
            }
            // PageDown: half-page down
            KeyCode::PageDown => {
                ts.scroll_offset = ts.scroll_offset.saturating_add(HALF_PAGE);
                ts.auto_scroll = false;
            }
            // PageUp: half-page up
            KeyCode::PageUp => {
                ts.scroll_offset = ts.scroll_offset.saturating_sub(HALF_PAGE);
                ts.auto_scroll = false;
            }
            // Enter: enter the composer in input mode.
            KeyCode::Enter => {
                ts.input_mode = InputMode::Input;
            }
            _ => {}
        }
    }

    /// Key handling in input mode: typing goes to input, Enter sends, Esc exits.
    ///
    /// When the agent is busy, Enter sends immediately into the runtime's
    /// internal queue. The Session commits it at the next safe iteration
    /// boundary, and the TUI keeps an unacknowledged preview until then.
    pub(super) fn handle_input_key(&mut self, key: &KeyEvent) {
        use crate::widgets::input_area::{history_clear_recall, history_down, history_up};

        // While an incremental Ctrl+R search is active, every keystroke
        // drives the search instead of editing the buffer.
        if self.state.active_tab_state_mut().in_history_search {
            self.handle_history_search_key(key);
            return;
        }

        let active_idx = self.state.active_agent_idx;
        let ts = self.state.active_tab_state_mut();

        // Ctrl+R: enter incremental history search (codex-style).
        // Available in both idle and running states.
        if !ts.input_history.is_empty()
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('r')
        {
            ts.in_history_search = true;
            ts.history_search_query.clear();
            ts.history_search_draft = Some(ts.input.value());
            ts.history_search_matches = compute_search_matches(&ts.input_history, "");
            ts.history_search_selected = 0;
            load_selected_history_match(ts);
            return;
        }

        // Every submitted message is delivered after the `ts` borrow ends. For
        // busy agents the pending queue tracks acknowledgement/rendering only.
        let mut send_text: Option<String> = None;

        match key.code {
            // Esc: leave input mode, return to browse. Any in-progress
            // Up/Down history recall is collapsed first.
            KeyCode::Esc => {
                history_clear_recall(&mut ts.input_draft, &mut ts.input_recall);
                ts.input_mode = InputMode::Browse;
            }
            // Enter: Shift/Alt+Enter inserts a newline (multiline compose);
            // a plain Enter sends the message and returns to browse mode.
            KeyCode::Enter => {
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT)
                {
                    // Alt is a fallback for terminals that don't report Shift on Enter.
                    ts.input.insert_newline();
                    return;
                }
                if ts.can_send() {
                    // Agent idle — deliver immediately.
                    let text = ts.take_input();
                    crate::widgets::input_area::history_push(
                        &mut ts.input_history,
                        text.clone(),
                        ts.input_history_capacity,
                    );
                    history_clear_recall(&mut ts.input_draft, &mut ts.input_recall);

                    ts.push_user_message(text.clone());
                    ts.enqueue_pending(text.clone(), true);
                    send_text = Some(text);
                    ts.scroll_to_bottom();
                } else if ts.can_enqueue() {
                    // Agent busy — deliver now, but render only after the
                    // Session commits it to conversation memory.
                    let text = ts.take_input();
                    crate::widgets::input_area::history_push(
                        &mut ts.input_history,
                        text.clone(),
                        ts.input_history_capacity,
                    );
                    history_clear_recall(&mut ts.input_draft, &mut ts.input_recall);

                    ts.enqueue_pending(text.clone(), false);
                    send_text = Some(text);
                }
                ts.input_mode = InputMode::Browse;
            }
            // Up/Down: recall history (Up) / advance towards draft (Down)
            KeyCode::Up => {
                let _ = history_up(
                    &mut ts.input,
                    &ts.input_history,
                    &mut ts.input_draft,
                    &mut ts.input_recall,
                );
            }
            KeyCode::Down => {
                let _ = history_down(
                    &mut ts.input,
                    &ts.input_history,
                    &mut ts.input_draft,
                    &mut ts.input_recall,
                );
            }
            // Any other key: collapse in-progress recall so subsequent
            // edits are treated as user-driven (not as a recalled entry
            // we'd accidentally re-push when sent).
            _ => {
                if ts.input_recall.is_some() {
                    history_clear_recall(&mut ts.input_draft, &mut ts.input_recall);
                }
                ts.input.handle_key(*key);
            }
        }

        // Dispatch deliver_message outside the `ts` borrow.
        if let Some(text) = send_text {
            let name = self.state.sessions.get(active_idx).map(|s| s.name.clone());
            if let Some(name) = name {
                let client = self.client.clone();
                self.spawn_client_task("deliver_message", move || async move {
                    if let Err(e) = client.deliver_message(&name, text).await {
                        tracing::error!(agent = %name, error = %e, "message delivery failed");
                    }
                });
            }
        }
    }

    /// Key handling while a Ctrl+R incremental history search is active.
    pub(super) fn handle_history_search_key(&mut self, key: &KeyEvent) {
        let ts = self.state.active_tab_state_mut();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            // Esc: cancel the search, restore the original draft buffer.
            KeyCode::Esc => {
                let draft = ts.history_search_draft.take().unwrap_or_default();
                ts.input.clear();
                ts.input.insert_str(&draft);
                end_history_search(ts);
            }
            // Enter: accept the currently-previewed match into the buffer
            // and resume normal input editing.
            KeyCode::Enter => {
                end_history_search(ts);
            }
            // Up: move to the next-older match.
            KeyCode::Up => {
                if !ts.history_search_matches.is_empty() {
                    ts.history_search_selected =
                        (ts.history_search_selected + 1).min(ts.history_search_matches.len() - 1);
                    load_selected_history_match(ts);
                }
            }
            // Down: move toward the newest match.
            KeyCode::Down => {
                if !ts.history_search_matches.is_empty() {
                    ts.history_search_selected = ts.history_search_selected.saturating_sub(1);
                    load_selected_history_match(ts);
                }
            }
            // Backspace: drop the last query character and refilter.
            KeyCode::Backspace => {
                ts.history_search_query.pop();
                recompute_history_search(ts);
            }
            // Type into the query (plain chars only).
            KeyCode::Char(c) if !ctrl => {
                ts.history_search_query.push(c);
                recompute_history_search(ts);
            }
            _ => {}
        }
    }
    /// Open the message-copy picker. Snapshots every text-bearing chat
    /// line from the active session so the picker survives mid-stream
    /// mutations of the underlying `Vec`. No-op when there are no
    /// text messages to copy.
    pub(super) fn open_message_picker(&mut self) {
        let messages = self.state.active_tab_state().messages.clone();
        let items = crate::widgets::message_picker::collect_text_messages(&messages);
        if items.is_empty() {
            tracing::info!("message picker: no text messages in active session");
            return;
        }
        tracing::info!(
            count = items.len(),
            "opening message picker for clipboard copy"
        );
        self.state.message_picker.open_with(items);
    }

    /// Key handling while the message-copy picker is open.
    ///
    /// All keys are delegated to `MessagePickerState::handle_key`, which
    /// forwards text-editing keys to the embedded `TextArea` and
    /// intercepts Esc / Enter / Up / Down for popup control. The
    /// `TextArea` provides cursor navigation (Left/Right/Home/End),
    /// word deletion (Ctrl+W), undo/redo (Ctrl+Z), yank (Ctrl+Y), and
    /// paste (Ctrl+V) within the search input.
    pub(super) fn handle_message_picker_key(&mut self, key: &KeyEvent) {
        use crate::widgets::message_picker::MessagePickerKeyOutcome;
        let outcome = self.state.message_picker.handle_key(*key);
        match outcome {
            MessagePickerKeyOutcome::Close => {
                self.state.message_picker.close();
            }
            MessagePickerKeyOutcome::CopySelected => {
                if let Some(item) = self.state.message_picker.selected_item() {
                    let preview = item.preview.clone();
                    let role_tag = item.role.tag().to_string();
                    let char_count = item.full_text.chars().count();
                    self.state.message_picker.close();
                    match crate::widgets::message_picker::copy_to_clipboard(&item.full_text) {
                        Ok(lease) => {
                            self.clipboard_lease = lease;
                            tracing::info!(
                                role = role_tag,
                                chars = char_count,
                                preview = %preview,
                                "copied message to clipboard"
                            );
                            self.state.toasts.success(
                                "Copied to clipboard",
                                Some(format!("{char_count} chars • {preview}")),
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                role = role_tag,
                                error = %e,
                                "failed to copy message to clipboard"
                            );
                            self.state.toasts.error("Clipboard failed", Some(e));
                        }
                    }
                }
            }
            MessagePickerKeyOutcome::Consumed => {}
        }
    }
}
