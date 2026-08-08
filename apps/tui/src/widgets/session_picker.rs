//! Session picker — popup listing sub-sessions of the currently active agent.
//!
//! Two-block layout: left = searchable session list, right = preview of the
//! selected session (metadata + message count + first/last message snippet).
//!
//! Triggered from the command palette ("Sessions" command). The picker is
//! populated lazily from `AgentEvent::SessionList`, so the caller must
//! request a fresh listing via `AgentHandle::list_sessions()` after opening.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
};

use crate::widgets::popup::Popup;

/// Lightweight picker item — just id, title and basic metadata.
#[derive(Clone)]
pub struct PickerSession {
    pub id: uuid::Uuid,
    pub title: Option<String>,
    pub message_count: usize,
    pub last_active: i64,
}

/// State for the session picker popup.
#[derive(Default)]
pub struct SessionPickerState {
    pub visible: bool,
    pub agent_id: Option<uuid::Uuid>,
    pub agent_name: String,
    pub items: Vec<PickerSession>,
    pub active_id: Option<uuid::Uuid>,
    pub selected: usize,
    pub list_state: ListState,
}


impl SessionPickerState {
    /// Open the picker for the given agent.
    pub fn open(&mut self, agent_id: uuid::Uuid, agent_name: String) {
        self.visible = true;
        self.agent_id = Some(agent_id);
        self.agent_name = agent_name;
        self.selected = 0;
        self.sync_list_state();
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.agent_id = None;
        self.agent_name.clear();
        self.items.clear();
        self.active_id = None;
        self.selected = 0;
    }

    /// Replace the local session list (called from the SessionList event).
    pub fn set_sessions(&mut self, items: Vec<PickerSession>, active_id: Option<uuid::Uuid>) {
        self.items = items;
        self.active_id = active_id;
        if active_id.is_some() {
            if let Some(idx) = self.items.iter().position(|s| Some(s.id) == active_id) {
                self.selected = idx;
            }
        }
        if self.selected >= self.items.len() && !self.items.is_empty() {
            self.selected = self.items.len() - 1;
        }
        self.sync_list_state();
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
        self.sync_list_state();
    }

    pub fn move_down(&mut self) {
        let max = self.items.len().saturating_sub(1);
        if self.selected < max {
            self.selected += 1;
        }
        self.sync_list_state();
    }

    /// Returns the currently selected session ID (if any).
    pub fn selected_id(&self) -> Option<uuid::Uuid> {
        self.items.get(self.selected).map(|s| s.id)
    }

    fn sync_list_state(&mut self) {
        self.list_state.select(
            if self.items.is_empty() || self.selected >= self.items.len() {
                None
            } else {
                Some(self.selected)
            },
        );
    }
}

/// Widget that renders the session picker as a centered popup with two blocks.
pub struct SessionPicker {
    pub accent: Color,
    pub popup_width: u16,
    pub popup_height: u16,
    pub list_width: u16,
}

impl SessionPicker {
    pub fn new() -> Self {
        Self {
            accent: Color::Magenta,
            popup_width: 70,
            popup_height: 18,
            list_width: 28,
        }
    }

    pub fn accent(mut self, c: Color) -> Self {
        self.accent = c;
        self
    }
}

impl Default for SessionPicker {
    fn default() -> Self {
        Self::new()
    }
}

impl StatefulWidget for SessionPicker {
    type State = SessionPickerState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut SessionPickerState) {
        if !state.visible {
            return;
        }

        let popup = Popup::new(" Sessions ")
            .accent(self.accent)
            .width(self.popup_width)
            .height(self.popup_height);
        let inner = popup.render(area, buf);

        let v_regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Header (agent name)
                Constraint::Min(3),   // Content (list + preview)
                Constraint::Length(1), // Footer (hint)
            ])
            .split(inner);

        // ── Header: which agent's sessions ──
        let header = Line::from(vec![
            Span::styled("  Agent: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                state.agent_name.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        Widget::render(Paragraph::new(header), v_regions[0], buf);

        // ── Content: two horizontal blocks ──
        let h_regions = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(self.list_width),
                Constraint::Min(10),
            ])
            .split(v_regions[1]);

        self.render_list_block(h_regions[0], buf, state);
        self.render_preview_block(h_regions[1], buf, state);

        // ── Footer ──
        let hint = " ↑↓ navigate  Enter switch  n new  r rename  d close  Esc cancel";
        let p = Paragraph::new(hint).style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        );
        Widget::render(p, v_regions[2], buf);
    }
}

impl SessionPicker {
    /// Render the left block: session list.
    fn render_list_block(&self, area: Rect, buf: &mut Buffer, state: &mut SessionPickerState) {
        let block = Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " Sessions ",
                Style::default()
                    .fg(self.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        block.render(area, buf);

        if state.items.is_empty() {
            let line = Line::from(Span::styled(
                "  (loading…)",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
            Widget::render(Paragraph::new(line), inner, buf);
            return;
        }

        let items: Vec<ListItem> = state
            .items
            .iter()
            .enumerate()
            .map(|(idx, session)| {
                let is_active = state.active_id == Some(session.id);
                let is_selected = idx == state.selected;
                let marker = if is_active { "●" } else { "○" };
                let title = session.title.as_deref().unwrap_or("(untitled)");
                let style = if is_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(self.accent)
                        .add_modifier(Modifier::BOLD)
                } else if is_active {
                    Style::default()
                        .fg(self.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };
                Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(format!("{} ", marker), Style::default().fg(self.accent)),
                    Span::styled(title.to_string(), style),
                ])
            })
            .map(ListItem::new)
            .collect();

        let list = List::new(items).highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(self.accent)
                .add_modifier(Modifier::BOLD),
        );
        StatefulWidget::render(list, inner, buf, &mut state.list_state);
    }

    /// Render the right block: preview of the selected session.
    fn render_preview_block(&self, area: Rect, buf: &mut Buffer, state: &SessionPickerState) {
        let block = Block::default().borders(Borders::NONE).title(Span::styled(
            " Preview ",
            Style::default()
                .fg(self.accent)
                .add_modifier(Modifier::BOLD),
        ));
        let inner = block.inner(area);
        block.render(area, buf);

        let Some(session) = state.items.get(state.selected) else {
            let line = Line::from(Span::styled(
                "  No session selected.",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
            let area = Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: 1,
            };
            Widget::render(Paragraph::new(line), area, buf);
            return;
        };

        let mut lines: Vec<Line> = Vec::new();

        // Title
        lines.push(Line::from(vec![
            Span::styled("  Title     ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                session.title.clone().unwrap_or_else(|| "(untitled)".into()),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        // Session ID (truncated)
        let id_short = &session.id.to_string()[..8];
        lines.push(Line::from(vec![
            Span::styled("  ID       ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}…", id_short), Style::default().fg(Color::Gray)),
        ]));

        // Status
        let is_active = state.active_id == Some(session.id);
        let status_text = if is_active { "● active" } else { "○ inactive" };
        let status_color = if is_active { self.accent } else { Color::DarkGray };
        lines.push(Line::from(vec![
            Span::styled("  Status   ", Style::default().fg(Color::DarkGray)),
            Span::styled(status_text, Style::default().fg(status_color)),
        ]));

        // Message count
        lines.push(Line::from(vec![
            Span::styled("  Messages ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", session.message_count),
                Style::default().fg(Color::Cyan),
            ),
        ]));

        // Last active
        let last_active_str = chrono::DateTime::from_timestamp_millis(session.last_active)
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        lines.push(Line::from(vec![
            Span::styled("  Last     ", Style::default().fg(Color::DarkGray)),
            Span::styled(last_active_str, Style::default().fg(Color::Gray)),
        ]));

        // Spacer
        lines.push(Line::from(""));

        // Mini bar chart of message count vs largest session
        let max_msgs = state.items.iter().map(|s| s.message_count).max().unwrap_or(1);
        let bar_len = if max_msgs > 0 {
            ((session.message_count * 20) / max_msgs).min(20)
        } else {
            0
        };
        lines.push(Line::from(vec![
            Span::styled("  Volume   ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{} ({})", "█".repeat(bar_len), session.message_count),
                Style::default().fg(self.accent),
            ),
        ]));

        // Truncate to fit available height.
        let max_lines = inner.height as usize;
        if lines.len() > max_lines {
            lines.truncate(max_lines.saturating_sub(1));
            lines.push(Line::from(Span::styled(
                "  …",
                Style::default().fg(Color::DarkGray),
            )));
        }

        Widget::render(Paragraph::new(lines), inner, buf);
    }
}
