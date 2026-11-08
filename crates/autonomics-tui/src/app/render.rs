//! Workspace and overlay rendering.

use super::overlays::Overlay;
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
            style::{Modifier, Style},
            text::{Line, Span},
            widgets::{Clear, Paragraph, Widget},
        };

        let theme = crate::theme::current();

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
                " ! Delete Agent ",
                Style::default()
                    .fg(theme.error)
                    .add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(theme.error));
        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let lines = vec![
            Line::from(vec![
                Span::styled(" Permanently delete ", Style::default().fg(theme.text_dim)),
                Span::styled(
                    agent_name.to_string(),
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ),
                Span::styled("?", Style::default().fg(theme.text_dim)),
            ]),
            Line::from(Span::styled(
                " All conversation history will be erased.",
                Style::default().fg(theme.text_faint),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled(" Press ", Style::default().fg(theme.text_dim)),
                Span::styled(
                    "y",
                    Style::default()
                        .fg(theme.error)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" to confirm, ", Style::default().fg(theme.text_dim)),
                Span::styled("n", Style::default().fg(theme.success)),
                Span::styled(" or ", Style::default().fg(theme.text_dim)),
                Span::styled("Esc", Style::default().fg(theme.success)),
                Span::styled(" to cancel", Style::default().fg(theme.text_dim)),
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
            style::{Modifier, Style},
            text::{Line, Span},
            widgets::{Clear, Paragraph, Widget},
        };

        let theme = crate::theme::current();
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
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(theme.warning));
        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let lines = vec![
            Line::from(Span::styled(
                format!(" {}", confirm.detail),
                Style::default().fg(theme.text),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled(" Press ", Style::default().fg(theme.text_dim)),
                Span::styled(
                    "y",
                    Style::default()
                        .fg(theme.warning)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" to apply, ", Style::default().fg(theme.text_dim)),
                Span::styled("n", Style::default().fg(theme.success)),
                Span::styled(" or ", Style::default().fg(theme.text_dim)),
                Span::styled("Esc", Style::default().fg(theme.success)),
                Span::styled(" to cancel", Style::default().fg(theme.text_dim)),
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
            style::{Modifier, Style},
            text::{Line, Span},
            widgets::{Block, Borders, Clear, Paragraph, Widget},
        };
        use unicode_width::UnicodeWidthStr;

        let theme = crate::theme::current();

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
                    .fg(theme.on_accent)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(theme.accent));
        let inner = block.inner(panel);
        block.render(panel, buf);

        let key_style = Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD);
        let arrow_style = Style::default().fg(theme.text_faint);
        let group_style = Style::default()
            .fg(theme.success)
            .add_modifier(Modifier::BOLD);
        let leaf_style = Style::default().fg(theme.text_dim);

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
            Style::default().fg(theme.text_faint),
        )];
        if !at_root {
            footer.push(Span::styled(" · ", Style::default().fg(theme.text_faint)));
            footer.push(Span::styled("⌫ back", Style::default().fg(theme.warning)));
        }
        Paragraph::new(Line::from(footer)).render(
            Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
            buf,
        );
    }

    /// Render one overlay by variant, called bottom→top over
    /// [`AppState::overlay_stack`]. The variant set and their stacking
    /// order are defined in `app::overlays` — this match only maps each
    /// variant to its drawing code.
    fn render_overlay(&mut self, frame: &mut Frame, overlay: Overlay) {
        match overlay {
            Overlay::CommandPalette => {
                crate::widgets::command_palette::render_command_palette(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.command_palette,
                );
                if let Some((cx, cy)) = self.state.command_palette.cursor_pos {
                    frame.set_cursor_position(ratatui::layout::Position { x: cx, y: cy });
                }
            }
            Overlay::DeleteConfirm => {
                self.render_delete_confirm_popup(frame.area(), frame.buffer_mut());
            }
            Overlay::AgentsPopup => {
                crate::widgets::agents_popup::render_agents_popup(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.agents_popup,
                );
                if let Some((cx, cy)) = self.state.agents_popup.cursor_pos {
                    frame.set_cursor_position(ratatui::layout::Position { x: cx, y: cy });
                }
            }
            Overlay::PlanPopup => {
                let plan = self.state.active_tab_state().plan.clone();
                crate::widgets::plan_popup::render_plan_popup(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.plan_popup,
                    &plan,
                );
            }
            Overlay::TasksPopup => {
                let tasks = self.state.active_tab_state().tool_tasks.clone();
                crate::widgets::tasks_popup::render_tasks_popup(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.tasks_popup,
                    &tasks,
                );
            }
            Overlay::AgentModelPicker => {
                crate::widgets::agent_model_picker::render_agent_model_picker(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.agent_model_picker,
                );
                if let Some((cx, cy)) = self.state.agent_model_picker.cursor_pos {
                    frame.set_cursor_position(ratatui::layout::Position { x: cx, y: cy });
                }
            }
            Overlay::AgentEffortPicker => {
                crate::widgets::agent_effort_picker::render_agent_effort_picker(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.agent_effort_picker,
                );
            }
            Overlay::BulkConfirm => {
                self.render_bulk_confirm_popup(frame.area(), frame.buffer_mut());
            }
            Overlay::MessagePicker => {
                crate::widgets::message_picker::render_message_picker(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.message_picker,
                );
                // Position the hardware cursor inside the search input so
                // the user sees the caret where they are typing.
                if let Some((cx, cy)) = self.state.message_picker.cursor_pos {
                    frame.set_cursor_position(ratatui::layout::Position { x: cx, y: cy });
                }
            }
            Overlay::NameInput => {
                crate::widgets::name_input::render_name_input(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.name_input,
                );
                if let Some((x, y)) = self.state.name_input.cursor_pos {
                    frame.set_cursor_position(ratatui::layout::Position { x, y });
                }
            }
            Overlay::VfsBrowser => {
                crate::widgets::vfs_browser::render_vfs_browser(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.vfs_browser,
                );
            }
            Overlay::HpcPanel => {
                use ratatui::widgets::StatefulWidgetRef as _;
                let popup_width = frame.area().width * 8 / 10;
                let popup_height = frame.area().height * 9 / 10;
                let popup = crate::widgets::popup::Popup::new(
                    " HPC Site ",
                    crate::widgets::popup::PopupControls::default(),
                )
                .width(popup_width)
                .height(popup_height)
                .accent(crate::theme::current().accent_alt);
                let inner = popup.render(frame.area(), frame.buffer_mut());
                let widget = crate::widgets::hpc_panel_widget::HpcPanelWidget;
                widget.render_ref(inner, frame.buffer_mut(), &mut self.state.hpc_panel);
            }
            Overlay::ModelConfig => {
                use ratatui::widgets::StatefulWidgetRef as _;
                let popup_width = frame.area().width * 9 / 10;
                let popup_height = frame.area().height * 9 / 10;
                let popup = crate::widgets::popup::Popup::new(
                    " Model Config ",
                    crate::widgets::popup::PopupControls::default(),
                )
                .width(popup_width)
                .height(popup_height)
                .accent(crate::theme::current().accent_alt);
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
            Overlay::EmbeddingConfig => {
                use ratatui::widgets::StatefulWidgetRef as _;
                let popup_width = frame.area().width * 7 / 10;
                let popup_height = frame.area().height * 8 / 10;
                let popup = crate::widgets::popup::Popup::new(
                    " Embedding Model ",
                    crate::widgets::popup::PopupControls::default(),
                )
                .width(popup_width)
                .height(popup_height)
                .accent(crate::theme::current().accent_alt);
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
            Overlay::DagView => {
                self.render_dag_view(frame.area(), frame.buffer_mut());
            }
            Overlay::FlowView => {
                self.render_flow_view(frame.area(), frame.buffer_mut());
            }
            Overlay::Bibliography => {
                self.render_bibliography(frame.area(), frame.buffer_mut());
            }
            Overlay::AgentConfig => {
                crate::widgets::agent_config_widget::render_agent_config(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.agent_config,
                );
            }
            Overlay::SkillLibrary => {
                use ratatui::widgets::StatefulWidget as _;
                crate::widgets::skill_library::SkillLibraryWidget::new()
                    .popup_width(frame.area().width * 9 / 10)
                    .popup_height((frame.area().height * 3 / 4).max(12))
                    .render(
                        frame.area(),
                        frame.buffer_mut(),
                        &mut self.state.skill_library,
                    );
            }
        }
    }

    pub(super) fn render(&mut self, frame: &mut Frame) {
        // Remember the frame area for mouse hit-testing (tab strip).
        self.state.last_frame_area = frame.area();

        // ── Tab strip (view-only) ──
        // One row listing the open agent leaves. Tabs are windows, not
        // lifecycle: opening/closing/switching never touches the agent.
        let mut workspace_area = frame.area();
        if !self.state.sessions.is_empty() {
            let [bar_area, rest] = ratatui::layout::Layout::vertical([
                ratatui::layout::Constraint::Length(1),
                ratatui::layout::Constraint::Min(0),
            ])
            .areas(frame.area());
            crate::widgets::tab_bar::TabBar {
                sessions: &self.state.sessions,
                active_idx: self.state.active_agent_idx,
            }
            .render(bar_area, frame.buffer_mut());
            workspace_area = rest;
        }

        // ── Workspace (full screen) ──
        // Model info comes from the per-agent render cache (render must
        // never issue HTTP): filled on registration / model changes via
        // AppEvent::ModelInfoLoaded, falling back to the daemon's active
        // default model when no agent is active.
        // `s.name` holds the agent's hierarchical AgentPath, e.g.
        // `/root/researcher`; it keys the model cache and labels the leaf.
        let active_agent_path = self
            .state
            .sessions
            .get(self.state.active_agent_idx)
            .map(|s| s.name.clone());
        if let Some(path) = active_agent_path.clone() {
            self.refresh_agent_model_info(&path);
        }
        let model_info = active_agent_path
            .as_deref()
            .and_then(|path| self.agent_model_cache.get(path).cloned())
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
        let workspace = AgentWorkspace {
            agent_path: active_agent_path.as_deref(),
            active_model: model_name.as_deref(),
            context_window,
            reasoning_effort: reasoning_effort.as_deref(),
            display: &display,
        };
        let split = self.split_leaf.clone().and_then(|(first, second, vertical)| {
            let a = self.state.sessions.iter().position(|s| s.name == first)?;
            let b = self.state.sessions.iter().position(|s| s.name == second)?;
            (a != b && (self.state.active_agent_idx == a || self.state.active_agent_idx == b)).then_some((a, b, vertical))
        });
        if let Some((a, b, vertical)) = split {
            let layout = if vertical { Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]) } else { Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]) };
            let [first, second] = layout.areas(workspace_area);
            for (index, area) in [(a, first), (b, second)] {
                let session = &mut self.state.sessions[index];
                let info = self.agent_model_cache.get(&session.name).cloned().or_else(|| self.state.active_model_display.clone());
                let pane = AgentWorkspace { agent_path: Some(&session.name), active_model: info.as_ref().map(|i| i.spec.as_str()), context_window: info.as_ref().map(|i| i.context_length), reasoning_effort: info.as_ref().and_then(|i| i.reasoning_effort.as_deref()), display: &display };
                let state = match session.context.as_mut() { Some(context) => &mut context.tab_state, None => &mut session.pending_tab_state };
                pane.render(area, frame.buffer_mut(), state);
            }
        } else {
            workspace.render(workspace_area, frame.buffer_mut(), self.state.active_tab_state_mut());
        }

        // Position cursor only when there's an active agent leaf.
        if !self.state.sessions.is_empty() {
            if let Some((cx, cy)) = self.state.active_tab_state_mut().input.last_cursor_pos() {
                frame.set_cursor_position(ratatui::layout::Position { x: cx, y: cy });
            }
        }

        // ── Overlays, bottom→top ──
        // One z-order for the whole app: [`AppState::overlay_stack`] is the
        // same ordered list the key dispatcher consults (app::overlays), so
        // what is painted on top is exactly what captures input.
        for overlay in self.state.overlay_stack() {
            self.render_overlay(frame, overlay);
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
