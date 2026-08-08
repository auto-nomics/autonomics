//! Agent resume picker — standalone widget for listing and restoring
//! previously registered agents with their conversation history.
//!
//! Built on top of the generic [`Popup`](super::popup::Popup) container, but
//! self-contained: state + rendering + key handling all live here.

use agentik_core::storage::AgentRecord;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
};

use crate::widgets::popup::Popup;

/// One selectable agent record entry.
#[derive(Clone)]
pub struct AgentRecordItem {
    pub id: uuid::Uuid,
    pub name: String,
    pub last_active: i64,
    /// Serialized `AgentProfile` stored at registration time. Used to
    /// reconstruct the profile if the named profile no longer exists.
    pub config_json: serde_json::Value,
}

/// State for the agent resume picker.
#[derive(Default)]
pub struct AgentPickerState {
    pub visible: bool,
    /// Search query string.
    pub query: String,
    /// All registered agents (loaded from storage).
    items: Vec<AgentRecordItem>,
    /// Indices into `items` that match the current query.
    filtered: Vec<usize>,
    selected: usize,
    list_state: ListState,
    /// When `Some`, a delete confirmation is in progress for this agent ID.
    /// The user must type "yes" and press Enter to confirm.
    pub delete_confirm_id: Option<uuid::Uuid>,
    /// Text typed during delete confirmation.
    pub delete_confirm_input: String,
}

// impl Default for AgentPickerState {
//     fn default() -> Self {
//         Self {
//             visible: false,
//             query: String::new(),
//             items: Vec::new(),
//             filtered: Vec::new(),
//             selected: 0,
//             list_state: ListState::default(),
//             delete_confirm_id: None,
//             delete_confirm_input: String::new(),
//         }
//     }
// }

impl AgentPickerState {
    pub fn open(&mut self) {
        self.visible = true;
        self.query.clear();
        self.selected = 0;
        self.refilter();
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.delete_confirm_id = None;
        self.delete_confirm_input.clear();
    }

    /// Enter delete confirmation mode for the currently selected agent.
    pub fn start_delete_confirm(&mut self) {
        if let Some(item) = self.selected_item() {
            self.delete_confirm_id = Some(item.id);
            self.delete_confirm_input.clear();
        }
    }

    /// Cancel delete confirmation.
    pub fn cancel_delete(&mut self) {
        self.delete_confirm_id = None;
        self.delete_confirm_input.clear();
    }

    /// Check if the typed confirmation matches "yes". Returns the agent ID
    /// if confirmed, consuming the confirmation state.
    pub fn check_delete_confirm(&mut self) -> Option<uuid::Uuid> {
        if self.delete_confirm_input.trim().eq_ignore_ascii_case("yes") {
            let id = self.delete_confirm_id.take();
            self.delete_confirm_input.clear();
            id
        } else {
            None
        }
    }

    /// Remove an agent from the local list (after successful DB deletion).
    pub fn remove_by_id(&mut self, id: uuid::Uuid) {
        self.items.retain(|i| i.id != id);
        self.refilter();
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    pub fn pop_char(&mut self) {
        self.query.pop();
        self.refilter();
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
        self.sync_list_state();
    }

    pub fn move_down(&mut self) {
        let max = self.filtered.len().saturating_sub(1);
        if self.selected < max {
            self.selected += 1;
        }
        self.sync_list_state();
    }

    /// Returns a clone of the currently selected item, if any.
    pub fn selected_item(&self) -> Option<AgentRecordItem> {
        let &idx = self.filtered.get(self.selected)?;
        self.items.get(idx).cloned()
    }

    /// Number of items currently displayed (post-filter).
    pub fn filtered_len(&self) -> usize {
        self.filtered.len()
    }

    /// Populate the picker from agent records.
    pub fn set_records(&mut self, records: &[AgentRecord]) {
        self.items = records
            .iter()
            .map(|r| AgentRecordItem {
                id: r.id,
                name: r.name.clone(),
                last_active: r.last_active,
                config_json: r.config_json.clone(),
            })
            .collect();
        self.refilter();
    }

    fn sync_list_state(&mut self) {
        self.list_state.select(
            if self.filtered.is_empty() || self.selected >= self.filtered.len() {
                None
            } else {
                Some(self.selected)
            },
        );
    }

    fn refilter(&mut self) {
        let needle = self.query.trim().to_lowercase();
        if needle.is_empty() {
            self.filtered = (0..self.items.len()).collect();
        } else {
            self.filtered = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.name.to_lowercase().contains(&needle))
                .map(|(i, _)| i)
                .collect();
        }
        self.selected = 0;
        self.sync_list_state();
    }
}

/// Standalone widget that renders the agent resume picker as a centered popup
/// with two blocks: a searchable agent list (left) and a preview pane (right).
pub struct AgentPicker {
    pub accent: Color,
    /// Width of the popup (0 = auto, ~80% of frame).
    pub popup_width: u16,
    /// Width of the left list block.
    pub list_width: u16,
}

impl AgentPicker {
    pub fn new() -> Self {
        Self {
            accent: Color::Green,
            popup_width: 0,
            list_width: 28,
        }
    }

    pub fn accent(mut self, c: Color) -> Self {
        self.accent = c;
        self
    }

    pub fn popup_width(mut self, w: u16) -> Self {
        self.popup_width = w;
        self
    }

    pub fn list_width(mut self, w: u16) -> Self {
        self.list_width = w;
        self
    }
}

impl Default for AgentPicker {
    fn default() -> Self {
        Self::new()
    }
}

impl StatefulWidget for AgentPicker {
    type State = AgentPickerState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut AgentPickerState) {
        if !state.visible {
            return;
        }

        let popup = Popup::new(" Resume Agent ")
            .accent(self.accent)
            .width(self.popup_width);
        let inner = popup.render(area, buf);

        // Top-level vertical: search row (1) + content row (rest) + footer (1).
        let v_regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1), // seperate line
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(inner);

        // ── Search input row (spans both blocks) ──
        let input_line = if state.query.is_empty() {
            Line::from(vec![
                Span::styled("> ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    " search agents…",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled("> ", Style::default().fg(self.accent)),
                Span::styled(state.query.clone(), Style::default().fg(Color::White)),
            ])
        };
        Widget::render(Paragraph::new(input_line), v_regions[0], buf);

        Widget::render(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::DarkGray)),
            v_regions[1],
            buf,
        );

        // ── Content area: two horizontal blocks ──
        let h_regions = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(self.list_width), Constraint::Min(10)])
            .split(v_regions[2]);

        self.render_list_block(h_regions[0], buf, state);
        self.render_preview_block(h_regions[1], buf, state);

        // ── Footer ──
        let hint = if state.delete_confirm_id.is_some() {
            let item_name = state.selected_item().map(|i| i.name).unwrap_or_default();
            format!(
                " Type 'yes' to delete '{}' (case-insensitive)  Enter confirm  Esc cancel",
                item_name
            )
        } else {
            " Enter resume  Ctrl+D delete (yes)  ↑↓ navigate  Esc cancel".to_string()
        };
        let p = Paragraph::new(hint).style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        );
        Widget::render(p, v_regions[3], buf);

        // ── Delete confirmation overlay (small inline prompt at bottom of preview) ──
        if state.delete_confirm_id.is_some() {
            let confirm_area = Rect {
                x: h_regions[1].x,
                y: h_regions[1].y + h_regions[1].height.saturating_sub(2),
                width: h_regions[1].width,
                height: 2,
            };
            let block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::Red));
            let inner_confirm = block.inner(confirm_area);
            block.render(confirm_area, buf);

            let line = Line::from(vec![
                Span::styled("  Confirm delete? ", Style::default().fg(Color::Red)),
                Span::styled(
                    state.delete_confirm_input.clone(),
                    Style::default().fg(Color::White),
                ),
                Span::styled("▏", Style::default().fg(Color::Red)),
            ]);
            Widget::render(Paragraph::new(line), inner_confirm, buf);
        }
    }
}

impl AgentPicker {
    /// Render the left block: list of agents.
    fn render_list_block(&self, area: Rect, buf: &mut Buffer, state: &mut AgentPickerState) {
        // Block title.
        let block = Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " Agents ",
                Style::default()
                    .fg(self.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        block.render(area, buf);

        if state.filtered.is_empty() {
            let line = Line::from(Span::styled(
                "  No agents found.",
                Style::default().fg(Color::DarkGray),
            ));
            let area = Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: 1,
            };
            Widget::render(Paragraph::new(line), area, buf);
            return;
        }

        let items: Vec<ListItem> = state
            .filtered
            .iter()
            .enumerate()
            .map(|(sel_i, &item_i)| {
                let item = &state.items[item_i];
                let is_selected = sel_i == state.selected;
                let style = if is_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(self.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };

                Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled("● ", Style::default().fg(self.accent)),
                    Span::styled(item.name.clone(), style),
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

    /// Render the right block: preview of the currently selected agent.
    fn render_preview_block(&self, area: Rect, buf: &mut Buffer, state: &AgentPickerState) {
        let block = Block::default().borders(Borders::NONE).title(Span::styled(
            " Preview ",
            Style::default()
                .fg(self.accent)
                .add_modifier(Modifier::BOLD),
        ));
        let inner = block.inner(area);
        block.render(area, buf);

        let Some(item) = state.selected_item() else {
            let line = Line::from(Span::styled(
                "  No agent selected.",
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

        // Name.
        lines.push(Line::from(vec![
            Span::styled("  Name      ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                item.name.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        // ID.
        lines.push(Line::from(vec![
            Span::styled("  ID        ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", item.id), Style::default().fg(Color::Gray)),
        ]));

        // Last active.
        let last_active_str = chrono::DateTime::from_timestamp_millis(item.last_active)
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        lines.push(Line::from(vec![
            Span::styled("  Last      ", Style::default().fg(Color::DarkGray)),
            Span::styled(last_active_str, Style::default().fg(Color::Gray)),
        ]));

        // Spacer.
        lines.push(Line::from(""));

        // Config (pretty-printed JSON, capped by available height).
        lines.push(Line::from(Span::styled(
            "  Config:",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )));

        // Pretty-print config_json, one line per key.
        if let Some(obj) = item.config_json.as_object() {
            for (key, val) in obj {
                let val_str = match val {
                    serde_json::Value::String(s) => format!("\"{}\"", s),
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Null => "null".to_string(),
                    other => other.to_string(),
                };
                let line = Line::from(vec![
                    Span::styled("    ", Style::default()),
                    Span::styled(key.clone(), Style::default().fg(Color::Cyan)),
                    Span::styled(": ", Style::default().fg(Color::DarkGray)),
                    Span::styled(val_str, Style::default().fg(Color::Gray)),
                ]);
                lines.push(line);
            }
        } else {
            lines.push(Line::from(Span::styled(
                "    (non-object config)",
                Style::default().fg(Color::DarkGray),
            )));
        }

        // Truncate to fit available height.
        let max_lines = inner.height as usize;
        if lines.len() > max_lines {
            lines.truncate(max_lines.saturating_sub(1));
            lines.push(Line::from(Span::styled(
                "    …",
                Style::default().fg(Color::DarkGray),
            )));
        }

        let paragraph = Paragraph::new(lines);
        Widget::render(paragraph, inner, buf);
    }
}
