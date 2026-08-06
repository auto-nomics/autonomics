//! Session picker — popup listing sub-sessions of the currently active agent.
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

/// Lightweight picker item — just id and title. The picker doesn't need the
/// full session tab state.
#[derive(Clone)]
pub struct PickerSession {
    pub id: uuid::Uuid,
    pub title: Option<String>,
}

/// State for the session picker popup.
pub struct SessionPickerState {
    pub visible: bool,
    pub agent_id: Option<uuid::Uuid>,
    pub agent_name: String,
    pub items: Vec<PickerSession>,
    pub active_id: Option<uuid::Uuid>,
    pub selected: usize,
    pub list_state: ListState,
}

impl Default for SessionPickerState {
    fn default() -> Self {
        Self {
            visible: false,
            agent_id: None,
            agent_name: String::new(),
            items: Vec::new(),
            active_id: None,
            selected: 0,
            list_state: ListState::default(),
        }
    }
}

impl SessionPickerState {
    /// Open the picker for the given agent. Caller must refresh `items`
    /// after the agent emits `SessionList`.
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
        // Default selection: active session if present, else first item.
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

/// Widget that renders the session picker as a centered popup.
pub struct SessionPicker {
    pub accent: Color,
    pub popup_width: u16,
    pub popup_height: u16,
}

impl SessionPicker {
    pub fn new() -> Self {
        Self {
            accent: Color::Magenta,
            popup_width: 50,
            popup_height: 14,
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
                Constraint::Length(1),       // Header (agent name)
                Constraint::Min(3),          // Session list
                Constraint::Length(1),       // Footer (hint)
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

        // ── Session list ──
        let block = Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray));
        let inner_list = block.inner(v_regions[1]);
        block.render(v_regions[1], buf);

        if state.items.is_empty() {
            let line = Line::from(Span::styled(
                "  (loading sessions…)",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
            Widget::render(Paragraph::new(line), inner_list, buf);
        } else {
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
                        Span::styled(
                            format!("{} ", marker),
                            Style::default().fg(self.accent),
                        ),
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
            StatefulWidget::render(list, inner_list, buf, &mut state.list_state);
        }

        // ── Footer ──
        let hint = " ↑↓ navigate  Enter switch  n new  d close  Esc cancel";
        let p = Paragraph::new(hint).style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        );
        Widget::render(p, v_regions[2], buf);
    }
}