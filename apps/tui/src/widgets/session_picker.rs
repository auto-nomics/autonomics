//! Session picker — popup listing sub-sessions of the currently active agent.
//!
//! Two-block layout matching the agent picker: left = searchable session list,
//! right = rich preview (metadata + message statistics + message snippets).
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

// ═══════════════════════════════════════════════════════════════════════
// Data types
// ═══════════════════════════════════════════════════════════════════════

/// Enriched session data for the picker — includes message statistics and
/// preview snippets computed from the TUI's `ChatLine` history.
#[derive(Clone)]
pub struct PickerSession {
    pub id: uuid::Uuid,
    pub title: Option<String>,
    pub message_count: usize,
    pub last_active: i64,
    // ── Enriched fields ──
    /// Epoch-millis creation timestamp (from `SessionInfo::created_at` or
    /// first message). 0 if unknown.
    pub created_at: i64,
    /// Number of user turns.
    pub user_message_count: usize,
    /// Number of assistant turns.
    pub assistant_message_count: usize,
    /// Number of tool calls.
    pub tool_call_count: usize,
    /// Cumulative input tokens across all assistant turns.
    pub input_tokens: u64,
    /// Cumulative output tokens across all assistant turns.
    pub output_tokens: u64,
    /// First user message text (truncated), for topic preview.
    pub first_user_message: Option<String>,
    /// Last assistant message text (truncated), for recent context preview.
    pub last_assistant_message: Option<String>,
}

/// Statistics extracted from a slice of `ChatLine` messages.
pub struct SessionStats {
    pub user_message_count: usize,
    pub assistant_message_count: usize,
    pub tool_call_count: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub first_user_message: Option<String>,
    pub last_assistant_message: Option<String>,
}

/// Compute session statistics from TUI chat lines.
pub fn compute_session_stats(
    messages: &[crate::state::ChatLine],
) -> SessionStats {
    let mut user_count = 0usize;
    let mut assistant_count = 0usize;
    let mut tool_call_count = 0usize;
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut first_user: Option<String> = None;
    let mut last_assistant: Option<String> = None;

    for msg in messages {
        match msg {
            crate::state::ChatLine::User(text) => {
                user_count += 1;
                if first_user.is_none() {
                    first_user = Some(truncate_for_preview(text, 120));
                }
            }
            crate::state::ChatLine::Assistant { text, usage } => {
                assistant_count += 1;
                if let Some(u) = usage {
                    if let Some(inp) = u.input_tokens {
                        input_tokens += inp;
                    }
                    output_tokens += u.output_tokens;
                }
                if !text.trim().is_empty() {
                    last_assistant = Some(truncate_for_preview(text, 120));
                }
            }
            crate::state::ChatLine::ToolCall { .. } => {
                tool_call_count += 1;
            }
            _ => {}
        }
    }

    SessionStats {
        user_message_count: user_count,
        assistant_message_count: assistant_count,
        tool_call_count,
        input_tokens,
        output_tokens,
        first_user_message: first_user,
        last_assistant_message: last_assistant,
    }
}

/// Truncate a string for preview display, capping at `max_chars`.
fn truncate_for_preview(s: &str, max_chars: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max_chars {
        s.replace('\n', " ")
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}…", truncated.replace('\n', " "))
    }
}

// ═══════════════════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════════════════

/// State for the session picker popup.
#[derive(Default)]
pub struct SessionPickerState {
    pub visible: bool,
    pub agent_id: Option<uuid::Uuid>,
    pub agent_name: String,
    /// All session items (unfiltered).
    pub items: Vec<PickerSession>,
    /// Search query for filtering sessions by title.
    pub query: String,
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
        self.query.clear();
        self.selected = 0;
        self.sync_list_state();
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.agent_id = None;
        self.agent_name.clear();
        self.items.clear();
        self.query.clear();
        self.active_id = None;
        self.selected = 0;
    }

    /// Replace the local session list (called from the SessionList event).
    pub fn set_sessions(&mut self, items: Vec<PickerSession>, active_id: Option<uuid::Uuid>) {
        self.items = items;
        self.active_id = active_id;
        self.sync_selection_to_active();
    }

    // ── Search ──

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.clamp_selection();
    }

    pub fn pop_char(&mut self) {
        self.query.pop();
        self.clamp_selection();
    }

    /// Return items matching the current search query.
    fn filtered_items(&self) -> Vec<&PickerSession> {
        let needle = self.query.trim().to_lowercase();
        if needle.is_empty() {
            self.items.iter().collect()
        } else {
            self.items
                .iter()
                .filter(|s| {
                    s.title
                        .as_deref()
                        .unwrap_or("(untitled)")
                        .to_lowercase()
                        .contains(&needle)
                })
                .collect()
        }
    }

    /// Clamp selection after filter changes and resync to active session.
    fn clamp_selection(&mut self) {
        let max = self.filtered_items().len().saturating_sub(1);
        if self.selected > max {
            self.selected = max;
        }
        self.sync_list_state();
    }

    fn sync_selection_to_active(&mut self) {
        if let Some(active) = self.active_id {
            if let Some(idx) = self
                .filtered_items()
                .iter()
                .position(|s| s.id == active)
            {
                self.selected = idx;
            }
        }
        if self.selected >= self.items.len() && !self.items.is_empty() {
            self.selected = self.items.len() - 1;
        }
        self.sync_list_state();
    }

    // ── Navigation ──

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
        self.sync_list_state();
    }

    pub fn move_down(&mut self) {
        let max = self.filtered_items().len().saturating_sub(1);
        if self.selected < max {
            self.selected += 1;
        }
        self.sync_list_state();
    }

    /// Returns the currently selected session ID (if any).
    pub fn selected_id(&self) -> Option<uuid::Uuid> {
        self.filtered_items()
            .get(self.selected)
            .map(|s| s.id)
    }

    fn sync_list_state(&mut self) {
        let len = self.filtered_items().len();
        self.list_state.select(
            if len == 0 || self.selected >= len {
                None
            } else {
                Some(self.selected)
            },
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Widget
// ═══════════════════════════════════════════════════════════════════════

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
            popup_height: 22,
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

        // ── Layout: search (1) + separator (1) + content (rest) + footer (1) ──
        let v_regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Search bar
                Constraint::Length(1), // Separator
                Constraint::Min(3),   // Content (list + preview)
                Constraint::Length(1), // Footer
            ])
            .split(inner);

        // ── Search bar ──
        let search_line = if state.query.is_empty() {
            Line::from(vec![
                Span::styled("> ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    " search sessions…",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled("> ", Style::default().fg(self.accent)),
                Span::styled(
                    state.query.clone(),
                    Style::default().fg(Color::White),
                ),
            ])
        };
        Widget::render(Paragraph::new(search_line), v_regions[0], buf);

        // ── Separator ──
        Widget::render(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::DarkGray)),
            v_regions[1],
            buf,
        );

        // ── Content: two horizontal blocks ──
        let h_regions = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(self.list_width),
                Constraint::Min(10),
            ])
            .split(v_regions[2]);

        self.render_list_block(h_regions[0], buf, state);
        self.render_preview_block(h_regions[1], buf, state);

        // ── Footer ──
        let hint = " ↑↓ navigate  Enter switch  Ctrl+N new  Ctrl+R rename  Ctrl+D close  Esc cancel";
        let p = Paragraph::new(hint).style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        );
        Widget::render(p, v_regions[3], buf);
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

        let filtered = state.filtered_items();

        if filtered.is_empty() {
            let text = if state.items.is_empty() {
                "  (loading…)"
            } else {
                "  (no match)"
            };
            let line = Line::from(Span::styled(
                text,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
            Widget::render(Paragraph::new(line), inner, buf);
            return;
        }

        let items: Vec<ListItem> = filtered
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

    /// Render the right block: rich preview of the selected session.
    fn render_preview_block(&self, area: Rect, buf: &mut Buffer, state: &SessionPickerState) {
        let block = Block::default().borders(Borders::NONE).title(Span::styled(
            " Preview ",
            Style::default()
                .fg(self.accent)
                .add_modifier(Modifier::BOLD),
        ));
        let inner = block.inner(area);
        block.render(area, buf);

        let filtered = state.filtered_items();
        let Some(session) = filtered.get(state.selected) else {
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
        let label_style = Style::default().fg(Color::DarkGray);

        // ── Metadata section ──
        let is_active = state.active_id == Some(session.id);

        // Title
        lines.push(Line::from(vec![
            Span::styled("  Title    ", label_style),
            Span::styled(
                session.title.clone().unwrap_or_else(|| "(untitled)".into()),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        // Status
        let (status_text, status_color) = if is_active {
            ("● active", self.accent)
        } else {
            ("○ inactive", Color::DarkGray)
        };
        lines.push(Line::from(vec![
            Span::styled("  Status  ", label_style),
            Span::styled(status_text, Style::default().fg(status_color)),
        ]));

        // Session ID (truncated)
        let id_short = &session.id.to_string()[..8];
        lines.push(Line::from(vec![
            Span::styled("  ID      ", label_style),
            Span::styled(format!("{}…", id_short), Style::default().fg(Color::Gray)),
        ]));

        // Created
        if session.created_at > 0 {
            let created_str = format_timestamp(session.created_at);
            lines.push(Line::from(vec![
                Span::styled("  Created ", label_style),
                Span::styled(created_str, Style::default().fg(Color::Gray)),
            ]));
        }

        // Last active
        let last_active_str = format_timestamp(session.last_active);
        lines.push(Line::from(vec![
            Span::styled("  Last    ", label_style),
            Span::styled(last_active_str, Style::default().fg(Color::Gray)),
        ]));

        // ── Separator ──
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  ── Messages ──",
            label_style.add_modifier(Modifier::BOLD),
        )));

        // ── Message statistics ──
        let total = session.user_message_count
            + session.assistant_message_count
            + session.tool_call_count;

        // Mini bar chart comparing message type counts
        let max_cat = [
            session.user_message_count,
            session.assistant_message_count,
            session.tool_call_count,
        ]
        .into_iter()
        .max()
        .unwrap_or(1)
        .max(1);

        let bar_width = 12usize;

        // User messages
        let user_bar_len = (session.user_message_count * bar_width) / max_cat;
        lines.push(Line::from(vec![
            Span::styled("  User      ", label_style),
            Span::styled(
                format!("{:>4}", session.user_message_count),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(
                format!("  {}", "█".repeat(user_bar_len)),
                Style::default().fg(Color::Cyan),
            ),
        ]));

        // Assistant messages
        let asst_bar_len = (session.assistant_message_count * bar_width) / max_cat;
        lines.push(Line::from(vec![
            Span::styled("  Assistant ", label_style),
            Span::styled(
                format!("{:>4}", session.assistant_message_count),
                Style::default().fg(Color::Green),
            ),
            Span::styled(
                format!("  {}", "█".repeat(asst_bar_len)),
                Style::default().fg(Color::Green),
            ),
        ]));

        // Tool calls
        let tool_bar_len = (session.tool_call_count * bar_width) / max_cat;
        lines.push(Line::from(vec![
            Span::styled("  Tools     ", label_style),
            Span::styled(
                format!("{:>4}", session.tool_call_count),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(
                format!("  {}", "█".repeat(tool_bar_len)),
                Style::default().fg(Color::Yellow),
            ),
        ]));

        // Total
        lines.push(Line::from(vec![
            Span::styled("  Total     ", label_style),
            Span::styled(
                format!("{:>4}", total),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        // ── Token usage ──
        if session.input_tokens > 0 || session.output_tokens > 0 {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  ── Tokens ──",
                label_style.add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(vec![
                Span::styled("  Input    ", label_style),
                Span::styled(
                    format_tokens(session.input_tokens),
                    Style::default().fg(Color::Cyan),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Output   ", label_style),
                Span::styled(
                    format_tokens(session.output_tokens),
                    Style::default().fg(Color::Green),
                ),
            ]));
        }

        // ── Message previews ──
        let has_preview = session
            .first_user_message
            .as_ref()
            .is_some_and(|s| !s.is_empty())
            || session
                .last_assistant_message
                .as_ref()
                .is_some_and(|s| !s.is_empty());

        if has_preview {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  ── Preview ──",
                label_style.add_modifier(Modifier::BOLD),
            )));

            if let Some(ref first_msg) = session.first_user_message {
                let preview = truncate_for_preview(first_msg, (inner.width as usize).saturating_sub(6));
                lines.push(Line::from(vec![
                    Span::styled("  ❯ ", Style::default().fg(Color::Cyan)),
                    Span::styled(preview, Style::default().fg(Color::Gray)),
                ]));
            }

            if let Some(ref last_msg) = session.last_assistant_message {
                let preview = truncate_for_preview(last_msg, (inner.width as usize).saturating_sub(6));
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled("  🤖 ", Style::default().fg(Color::Green)),
                    Span::styled(preview, Style::default().fg(Color::Gray)),
                ]));
            }
        }

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

// ── Formatting helpers ──────────────────────────────────

/// Format an epoch-millis timestamp as a human-readable string.
fn format_timestamp(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .map(|d| {
            let now = chrono::Utc::now();
            let delta = now.timestamp_millis() - millis;
            if delta < 60_000 {
                "just now".to_string()
            } else if delta < 3_600_000 {
                let mins = delta / 60_000;
                format!("{mins}m ago")
            } else if delta < 86_400_000 {
                let hours = delta / 3_600_000;
                format!("{hours}h ago")
            } else if delta < 7 * 86_400_000 {
                let days = delta / 86_400_000;
                format!("{days}d ago")
            } else {
                d.format("%Y-%m-%d %H:%M").to_string()
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Format a token count with thousands separators (e.g. "12,345").
fn format_tokens(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut result = String::new();
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            result.push(',');
        }
        result.push(b as char);
    }
    result
}
