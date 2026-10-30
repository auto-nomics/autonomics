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

    /// Render the shared y/n confirmation popup for bulk model/effort
    /// fan-out (same shape as the delete-agent confirmation).
    fn render_bulk_confirm_popup(
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
        let Some(confirm) = &self.state.bulk_confirm else {
            return;
        };

        let pw = 56u16.min(area.width);
        let ph = 5u16.min(area.height);
        let x = area.x + (area.width.saturating_sub(pw)) / 2;
        let y = area.y + (area.height.saturating_sub(ph)) / 3;
        let popup_area = Rect::new(x, y, pw, ph);

        Clear.render(popup_area, buf);

        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .title(Span::styled(
                format!(" {} ", confirm.title),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(Color::Yellow));
        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let lines = vec![
            Line::from(Span::styled(
                format!(" {}", confirm.detail),
                Style::default().fg(Color::White),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled(" Press ", Style::default().fg(Color::Gray)),
                Span::styled(
                    "y",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" to apply, ", Style::default().fg(Color::Gray)),
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

    /// The which-key panel: a bordered floating list anchored at the
    /// bottom-right of the frame, showing the current prefix's bindings —
    /// leaves first, then `+count group` rows — with an Esc / back footer
    /// (LazyVim's which-key layout).
    fn render_leader_panel(
        &self,
        area: ratatui::layout::Rect,
        buf: &mut ratatui::prelude::Buffer,
        prefix: &[char],
    ) {
        use ratatui::{
            layout::Rect,
            style::{Color, Modifier, Style},
            text::{Line, Span},
            widgets::{Block, Borders, Clear, Paragraph, Widget},
        };
        use unicode_width::UnicodeWidthStr;

        let rows = super::leader::whichkey_rows(prefix);
        let at_root = prefix.is_empty();

        // Panel width fits the widest row (` key → label`) and the footer.
        let key_col = 1usize;
        let arrow_w = " → ".width();
        let label_w = rows
            .iter()
            .map(|row| {
                if row.is_group {
                    format!("+{} {}", row.children, row.label).width()
                } else {
                    row.label.width()
                }
            })
            .max()
            .unwrap_or(10);
        let footer_w = if at_root {
            "Esc close".width()
        } else {
            "Esc close · ⌫ back".width()
        };
        let inner_w = (key_col + arrow_w + label_w).max(footer_w + 2);
        let width = (inner_w as u16 + 2)
            .clamp(20, 44)
            .min(area.width.saturating_sub(2));
        let height = (rows.len() as u16 + 1 + 2) // rows + footer + borders
            .min(area.height.saturating_sub(2))
            .max(5);
        if width == 0 || height == 0 {
            return;
        }

        // Anchor at the bottom-right with a one-cell margin.
        let x = area.right().saturating_sub(width).saturating_sub(1);
        let y = area.bottom().saturating_sub(height).saturating_sub(1);
        let panel = Rect::new(x, y, width, height);

        Clear.render(panel, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
                " leader ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(panel);
        block.render(panel, buf);

        let key_style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let arrow_style = Style::default().fg(Color::DarkGray);
        let group_style = Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD);
        let leaf_style = Style::default().fg(Color::Gray);

        // Rows stop one row above the footer; overflow is clipped (the
        // binding trees are small enough to fit at any sane terminal size).
        let list_bottom = inner.bottom().saturating_sub(1);
        for (offset, row) in rows.iter().enumerate() {
            let row_y = inner.y + offset as u16;
            if row_y >= list_bottom {
                break;
            }
            let key_text = match row.key {
                ' ' => "\u{2423}".to_string(),
                k => k.to_string(),
            };
            let label = if row.is_group {
                format!("+{} {}", row.children, row.label)
            } else {
                row.label.to_string()
            };
            let line = Line::from(vec![
                Span::styled(format!(" {key_text}"), key_style),
                Span::styled(" → ", arrow_style),
                Span::styled(
                    label,
                    if row.is_group {
                        group_style
                    } else {
                        leaf_style
                    },
                ),
            ]);
            Paragraph::new(line).render(Rect::new(inner.x, row_y, inner.width, 1), buf);
        }

        // Footer: close always; away from the root, the back key.
        let mut footer = vec![Span::styled(
            "Esc close",
            Style::default().fg(Color::DarkGray),
        )];
        if !at_root {
            footer.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
            footer.push(Span::styled("⌫ back", Style::default().fg(Color::Yellow)));
        }
        Paragraph::new(Line::from(footer)).render(
            Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
            buf,
        );
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
            .as_deref()
            .and_then(|name| self.agent_model_cache.get(name).cloned())
            .or_else(|| self.state.active_model_display.clone());
        let (model_name, context_window, reasoning_effort) = match model_info {
            Some(display) => (
                Some(display.spec),
                Some(display.context_length),
                display.reasoning_effort,
            ),
            None => (None, None, None),
        };

        let display = self.state.display_settings.clone();
        let kms_running = self
            .state
            .kms_workers
            .iter()
            .filter(|worker| worker.running)
            .count();
        let kms_activity = (!self.state.kms_workers.is_empty())
            .then(|| format!("{kms_running}/{} running", self.state.kms_workers.len()));
        let workspace = AgentWorkspace {
            agent_name: active_agent_name.as_deref(),
            active_model: model_name.as_deref(),
            context_window,
            reasoning_effort: reasoning_effort.as_deref(),
            kms_activity: kms_activity.as_deref(),
            display: &display,
        };
        workspace.render(
            frame.area(),
            frame.buffer_mut(),
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

        // ── Agents popup (Ctrl+T) ──
        if self.state.agents_popup.is_visible() {
            crate::widgets::agents_popup::render_agents_popup(
                frame.area(),
                frame.buffer_mut(),
                &mut self.state.agents_popup,
            );
            if let Some((cx, cy)) = self.state.agents_popup.cursor_pos {
                frame.set_cursor_position(ratatui::layout::Position { x: cx, y: cy });
            }
        }

        // ── Plan / Tasks popups (<leader>p / <leader>T) ──
        if self.state.plan_popup.visible {
            let plan = self.state.active_tab_state().plan.clone();
            crate::widgets::plan_popup::render_plan_popup(
                frame.area(),
                frame.buffer_mut(),
                &mut self.state.plan_popup,
                &plan,
            );
        }
        if self.state.tasks_popup.visible {
            let tasks = self.state.active_tab_state().tool_tasks.clone();
            crate::widgets::tasks_popup::render_tasks_popup(
                frame.area(),
                frame.buffer_mut(),
                &mut self.state.tasks_popup,
                &tasks,
            );
        }

        // ── Agents-popup model / effort quick-pickers ──
        if self.state.agent_model_picker.is_visible() {
            crate::widgets::agent_model_picker::render_agent_model_picker(
                frame.area(),
                frame.buffer_mut(),
                &mut self.state.agent_model_picker,
            );
            if let Some((cx, cy)) = self.state.agent_model_picker.cursor_pos {
                frame.set_cursor_position(ratatui::layout::Position { x: cx, y: cy });
            }
        }
        if self.state.agent_effort_picker.visible {
            crate::widgets::agent_effort_picker::render_agent_effort_picker(
                frame.area(),
                frame.buffer_mut(),
                &mut self.state.agent_effort_picker,
            );
        }

        // ── Bulk apply confirmation popup (y / n) ──
        if self.state.bulk_confirm.is_some() {
            self.render_bulk_confirm_popup(frame.area(), frame.buffer_mut());
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

        // ── Embedding config popup ──
        if self.state.embedding_config_visible {
            use ratatui::widgets::StatefulWidgetRef as _;
            let popup_width = frame.area().width * 7 / 10;
            let popup_height = frame.area().height * 8 / 10;
            let popup = crate::widgets::popup::Popup::new(
                " Embedding Model ",
                crate::widgets::popup::PopupControls::default(),
            )
            .width(popup_width)
            .height(popup_height)
            .accent(ratatui::style::Color::Magenta);
            let inner = popup.render(frame.area(), frame.buffer_mut());
            crate::widgets::embedding_config_widget::EmbeddingConfigWidget.render_ref(
                inner,
                frame.buffer_mut(),
                &mut self.state.embedding_config_state,
            );
            if let Some((x, y)) = self.state.embedding_config_state.cursor_pos {
                frame.set_cursor_position(ratatui::layout::Position { x, y });
            }
        }

        // ── DAG view overlay ──
        if self.state.dag_view_visible {
            self.render_dag_view(frame.area(), frame.buffer_mut());
        }

        if self.state.bibliography_visible {
            self.render_bibliography(frame.area(), frame.buffer_mut());
        }

        if self.state.agent_config.visible {
            crate::widgets::agent_config_widget::render_agent_config(
                frame.area(),
                frame.buffer_mut(),
                &mut self.state.agent_config,
            );
        }

        // ── Skill-evolution dashboard ──
        if self.state.skill_evolution.visible {
            use ratatui::widgets::StatefulWidget as _;
            crate::widgets::evolution_widget::SkillEvolutionWidget::new()
                .popup_width(frame.area().width * 9 / 10)
                .popup_height((frame.area().height * 3 / 4).max(12))
                .render(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.skill_evolution,
                );
        }

        // ── Which-key panel (leader prefix active) ──
        if let Some(prefix) = &self.leader_pending {
            self.render_leader_panel(frame.area(), frame.buffer_mut(), prefix);
        }

        // ── Toast notifications (top-most overlay, bottom-right corner) ──
        self.state.toasts.tick();
        self.state.toasts.render(frame.area(), frame.buffer_mut());
    }
}
