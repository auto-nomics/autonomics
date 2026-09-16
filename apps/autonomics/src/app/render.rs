//! Workspace and overlay rendering.

use super::*;

impl App {
    /// Render the delete-agent confirmation popup.
    fn render_delete_confirm_popup(
        &self,
        area: ratatui::layout::Rect,
        buf: &mut ratatui::prelude::Buffer,
    ) {
        use ratatui::{
            layout::{Alignment, Rect},
            style::{Color, Modifier, Style},
            text::{Line, Span},
            widgets::{Clear, Paragraph, Widget},
        };

        let agent_name = self
            .state
            .sessions
            .get(self.state.active_agent_idx)
            .map(|s| s.name.as_str())
            .unwrap_or("(unknown)");

        // Fixed-size centered popup.
        let pw = 52u16.min(area.width);
        let ph = 5u16.min(area.height);
        let x = area.x + (area.width.saturating_sub(pw)) / 2;
        let y = area.y + (area.height.saturating_sub(ph)) / 3;
        let popup_area = Rect::new(x, y, pw, ph);

        Clear.render(popup_area, buf);

        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .title(Span::styled(
                " ⚠ Delete Agent ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(Color::Red));
        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let lines = vec![
            Line::from(vec![
                Span::styled(" Permanently delete ", Style::default().fg(Color::Gray)),
                Span::styled(
                    agent_name.to_string(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("?", Style::default().fg(Color::Gray)),
            ]),
            Line::from(Span::styled(
                " All conversation history will be erased.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled(" Press ", Style::default().fg(Color::Gray)),
                Span::styled(
                    "y",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" to confirm, ", Style::default().fg(Color::Gray)),
                Span::styled("n", Style::default().fg(Color::Green)),
                Span::styled(" or ", Style::default().fg(Color::Gray)),
                Span::styled("Esc", Style::default().fg(Color::Green)),
                Span::styled(" to cancel", Style::default().fg(Color::Gray)),
            ]),
        ];

        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .render(inner, buf);
    }

    pub(super) fn render(&mut self, frame: &mut Frame) {
        // Remember the frame area for mouse hit-testing (tab-bar stub).
        self.state.last_frame_area = frame.area();

        // ── Workspace (full screen) ──
        // Model info comes from the per-agent render cache (render must
        // never issue HTTP): filled on registration / model changes via
        // AppEvent::ModelInfoLoaded, falling back to the daemon's active
        // default model when no agent is active.
        let active_agent_name = self
            .state
            .sessions
            .get(self.state.active_agent_idx)
            .map(|s| s.name.clone());
        if let Some(name) = active_agent_name.clone() {
            self.refresh_agent_model_info(&name);
        }
        let model_info = active_agent_name
            .and_then(|name| self.agent_model_cache.get(&name).cloned())
            .or_else(|| self.state.active_model_display.clone());
        let (model_name, context_window) = match model_info {
            Some((name, ctx)) => (Some(name), Some(ctx)),
            None => (None, None),
        };

        // Collect tab data before mutably borrowing tab state.
        let workspace_tabs = self.workspace_tabs();
        let active_idx = self.state.active_agent_idx;

        // Build session summaries for the sidebar. Collect as owned data to
        // avoid holding an immutable borrow across the mutable
        // `active_tab_state_mut()` call below.
        let session_summaries: Vec<crate::widgets::session_list::SessionSummary> = self
            .state
            .sessions
            .get(active_idx)
            .map(|s| {
                s.sub_sessions
                    .iter()
                    .enumerate()
                    .map(|(i, sub)| crate::widgets::session_list::SessionSummary {
                        title: sub.title.clone(),
                        message_count: sub.tab_state.messages.len(),
                        is_active: i == s.active_sub_session_idx,
                        created_at: sub.created_at,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let display = self.state.display_settings.clone();
        let workspace = AgentWorkspace {
            active_model: model_name.as_deref(),
            context_window,
            sessions: &session_summaries,
            display: &display,
        };
        workspace.render(
            frame.area(),
            frame.buffer_mut(),
            &workspace_tabs,
            active_idx,
            self.state.active_tab_state_mut(),
        );

        // Position cursor only when there's an active agent leaf.
        if !self.state.sessions.is_empty() {
            if let Some((cx, cy)) = self.state.active_tab_state_mut().input.last_cursor_pos() {
                frame.set_cursor_position(ratatui::layout::Position { x: cx, y: cy });
            }
        }

        // ── Command palette overlay ──
        if self.state.command_palette.is_visible() {
            crate::widgets::command_palette::render_command_palette(
                frame.area(),
                frame.buffer_mut(),
                &mut self.state.command_palette,
            );
            if let Some((cx, cy)) = self.state.command_palette.cursor_pos {
                frame.set_cursor_position(ratatui::layout::Position { x: cx, y: cy });
            }
        }

        // ── Delete-agent confirmation popup ──
        if self.state.delete_agent_confirm {
            self.render_delete_confirm_popup(frame.area(), frame.buffer_mut());
        }

        // ── Profile picker popup ──
        if self.state.profile_picker.visible {
            use ratatui::widgets::StatefulWidget as _;
            crate::widgets::profile_picker::ProfilePicker::new()
                .popup_width((frame.area().width * 8 / 10).max(70))
                .list_width(28)
                .render(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.profile_picker,
                );
        }

        // ── Agent resume picker popup ──
        if self.state.agent_profile_picker.visible {
            use ratatui::widgets::StatefulWidget as _;
            crate::widgets::agent_profile_picker::AgentProfilePicker::new()
                .popup_width((frame.area().width * 8 / 10).max(70))
                .list_width(28)
                .render(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.agent_profile_picker,
                );
        }

        // ── Switch-agent picker popup ──
        if self.state.agent_picker.visible {
            use ratatui::widgets::StatefulWidget as _;
            crate::widgets::agent_picker::AgentPicker::new()
                .popup_width(50)
                .render(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.agent_picker,
                );
        }

        // ── Session picker popup ──
        if self.state.session_picker.visible {
            use ratatui::widgets::StatefulWidget as _;
            crate::widgets::session_picker::SessionPicker::new()
                .popup_width((frame.area().width * 85 / 100).max(80))
                .popup_height((frame.area().height * 80 / 100).max(24))
                .render(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.session_picker,
                );
        }

        // ── Message picker popup (top-most overlay) ──
        if self.state.message_picker.is_visible() {
            crate::widgets::message_picker::render_message_picker(
                frame.area(),
                frame.buffer_mut(),
                &mut self.state.message_picker,
            );
            // Position the hardware cursor inside the search input so the
            // user sees the caret where they are typing.
            if let Some((cx, cy)) = self.state.message_picker.cursor_pos {
                frame.set_cursor_position(ratatui::layout::Position { x: cx, y: cy });
            }
        }

        // ── Name input popup ──
        crate::widgets::name_input::render_name_input(
            frame.area(),
            frame.buffer_mut(),
            &mut self.state.name_input,
        );
        if let Some((x, y)) = self.state.name_input.cursor_pos {
            frame.set_cursor_position(ratatui::layout::Position { x, y });
        }

        // ── Model config popup ──
        if self.state.model_config_visible {
            use ratatui::widgets::StatefulWidgetRef as _;
            let popup_width = frame.area().width * 9 / 10;
            let popup_height = frame.area().height * 9 / 10;
            let popup = crate::widgets::popup::Popup::new(
                " Model Config ",
                crate::widgets::popup::PopupControls::default(),
            )
            .width(popup_width)
            .height(popup_height)
            .accent(ratatui::style::Color::Magenta);
            let inner = popup.render(frame.area(), frame.buffer_mut());
            let widget = crate::widgets::model_config_widget::ModelConfigWidget;
            widget.render_ref(
                inner,
                frame.buffer_mut(),
                &mut self.state.model_config_state,
            );
            if let Some((x, y)) = self.state.model_config_state.cursor_pos {
                frame.set_cursor_position(ratatui::layout::Position { x, y });
            }
        }

        // ── DAG view overlay ──
        if self.state.dag_view_visible {
            self.render_dag_view(frame.area(), frame.buffer_mut());
        }

        if self.state.agent_config.visible {
            crate::widgets::agent_config_widget::render_agent_config(
                frame.area(),
                frame.buffer_mut(),
                &mut self.state.agent_config,
            );
        }

        // ── Toast notifications (top-most overlay, bottom-right corner) ──
        self.state.toasts.tick();
        self.state.toasts.render(frame.area(), frame.buffer_mut());
    }
}
