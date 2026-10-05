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

use crate::widgets::popup::{Popup, PopupControls};

const PREVIEW_MAX_CHARS: usize = 1024;

// ═══════════════════════════════════════════════════════════════════════
// Data types
// ═══════════════════════════════════════════════════════════════════════

/// Enriched session data for the picker — includes message statistics and
/// preview snippets computed from the TUI's `ChatLine` history.
#[derive(Clone)]
pub struct PickerSession {
    pub id: uuid::Uuid,
    pub title: Option<String>,
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
    /// Total raw token units reported by the runtime.
    pub total_tokens: u64,
    /// Tool-use blocks requested by the model.
    pub total_tool_use: u64,
    /// Active agent work time, excluding background-task waits.
    pub time_consume_ms: u64,
    /// First user message text, capped at 1024 characters.
    pub first_user_message: Option<String>,
    /// Last assistant message text, capped at 1024 characters.
    pub last_assistant_message: Option<String>,
}

/// Which sub-view is rendered in the right-hand panel of the session picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InfoTab {
    /// Title, status, IDs, timestamps, and first/last message snippets.
    #[default]
    Preview,
    /// Per-session message-count bar chart and token/time telemetry.
    Telemetry,
}

impl InfoTab {
    fn label(self) -> &'static str {
        match self {
            Self::Preview => "Preview",
            Self::Telemetry => "Telemetry",
        }
    }

    /// Cycle to the next tab (wraps around).
    fn next(self) -> Self {
        match self {
            Self::Preview => Self::Telemetry,
            Self::Telemetry => Self::Preview,
        }
    }
}

/// Which column of the session picker currently holds focus.
///
/// Focus is purely a visual indicator here — keyboard shortcuts continue to
/// apply globally (search filter, list navigation, info-tab cycling) so the
/// user can interleave typing and tabbing without mode switching. The
/// accent colour follows the focused panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelFocus {
    /// Left column = searchable session list.
    #[default]
    Left,
    /// Right column = info tabs (Preview / Telemetry).
    Right,
}

impl PanelFocus {
    fn toggle(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

/// Statistics extracted from a slice of `ChatLine` messages.
pub struct SessionStats {
    pub user_message_count: usize,
    pub assistant_message_count: usize,
    pub first_user_message: Option<String>,
    pub last_assistant_message: Option<String>,
}

/// Compute session statistics from TUI chat lines.
pub fn compute_session_stats(messages: &[crate::state::ChatLine]) -> SessionStats {
    let mut user_count = 0usize;
    let mut assistant_count = 0usize;
    let mut first_user: Option<String> = None;
    let mut last_assistant: Option<String> = None;

    for msg in messages {
        match msg {
            crate::state::ChatLine::User(text) => {
                user_count += 1;
                if first_user.is_none() {
                    first_user = Some(truncate_for_preview(text, PREVIEW_MAX_CHARS));
                }
            }
            crate::state::ChatLine::Assistant { text, .. } => {
                assistant_count += 1;
                if !text.trim().is_empty() {
                    last_assistant = Some(truncate_for_preview(text, PREVIEW_MAX_CHARS));
                }
            }
            crate::state::ChatLine::ToolCall { .. } => {}
            _ => {}
        }
    }

    SessionStats {
        user_message_count: user_count,
        assistant_message_count: assistant_count,
        first_user_message: first_user,
        last_assistant_message: last_assistant,
    }
}

/// Truncate a string for preview display, capping at `max_chars`.
fn truncate_for_preview(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let normalized = s.trim().replace("\r\n", "\n").replace('\r', "\n");
    if normalized.chars().count() <= max_chars {
        normalized
    } else {
        let mut truncated: String = normalized.chars().take(max_chars - 1).collect();
        truncated.push('…');
        truncated
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
    /// Which sub-view is currently shown in the right panel.
    pub info_tab: InfoTab,
    /// Which column (left list / right info) currently holds focus.
    pub focus: PanelFocus,
}

impl SessionPickerState {
    /// Open the picker for the given agent.
    pub fn open(&mut self, agent_id: uuid::Uuid, agent_name: String) {
        self.visible = true;
        self.agent_id = Some(agent_id);
        self.agent_name = agent_name;
        self.query.clear();
        self.selected = 0;
        self.info_tab = InfoTab::default();
        self.focus = PanelFocus::default();
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
            if let Some(idx) = self.filtered_items().iter().position(|s| s.id == active) {
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

    /// Cycle the right-panel info tab (Preview ↔ Telemetry).
    pub fn cycle_tab(&mut self) {
        self.info_tab = self.info_tab.next();
    }

    /// Toggle keyboard focus between the left list and right info panel.
    pub fn cycle_focus(&mut self) {
        self.focus = self.focus.toggle();
    }

    /// Returns the currently selected session ID (if any).
    pub fn selected_id(&self) -> Option<uuid::Uuid> {
        self.filtered_items().get(self.selected).map(|s| s.id)
    }

    fn sync_list_state(&mut self) {
        let len = self.filtered_items().len();
        self.list_state.select(if len == 0 || self.selected >= len {
            None
        } else {
            Some(self.selected)
        });
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
            popup_width: 0,
            popup_height: 0,
            list_width: 34,
        }
    }

    pub fn popup_width(mut self, w: u16) -> Self {
        self.popup_width = w;
        self
    }

    pub fn popup_height(mut self, h: u16) -> Self {
        self.popup_height = h;
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

        let popup = Popup::new(" Sessions ", PopupControls::default())
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
                Constraint::Min(3),    // Content (list + preview)
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
                Span::styled(state.query.clone(), Style::default().fg(Color::White)),
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
            .constraints([Constraint::Length(self.list_width), Constraint::Min(10)])
            .split(v_regions[2]);

        self.render_list_block(h_regions[0], buf, state);
        self.render_info_block(h_regions[1], buf, state);

        // ── Footer ──
        let hint = " ↑↓ nav  Tab tabs  Ctrl+Tab focus  ⏎ select  ^N new  ^R rename  ^D close  Esc cancel";
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
        let focused = state.focus == PanelFocus::Left;
        let accent_color = if focused { self.accent } else { Color::DarkGray };
        let block = Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(accent_color))
            .title(Span::styled(
                " Sessions ",
                Style::default()
                    .fg(accent_color)
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
                ListItem::new(session_list_lines(
                    session,
                    is_active,
                    is_selected,
                    self.accent,
                ))
            })
            .collect();

        let list = List::new(items).highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(self.accent)
                .add_modifier(Modifier::BOLD),
        );
        StatefulWidget::render(list, inner, buf, &mut state.list_state);
    }

    /// Render the right block: title + tab bar + body.
    fn render_info_block(&self, area: Rect, buf: &mut Buffer, state: &mut SessionPickerState) {
        let regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Title row (active view)
                Constraint::Length(1), // Tab bar
                Constraint::Min(0),     // Body
            ])
            .split(area);

        let right_focused = state.focus == PanelFocus::Right;
        let border_color = if right_focused {
            self.accent
        } else {
            Color::DarkGray
        };

        // ── Title row: section title reflecting the active info tab ──
        let title_line = Line::from(vec![
            Span::styled("── ", Style::default().fg(border_color)),
            Span::styled(
                state.info_tab.label().to_string(),
                Style::default()
                    .fg(border_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ──", Style::default().fg(border_color)),
        ]);
        Widget::render(
            Paragraph::new(title_line).alignment(ratatui::layout::Alignment::Center),
            regions[0],
            buf,
        );

        // ── Tab bar with bottom border ──
        let tab_block = Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(border_color));
        let tab_inner = tab_block.inner(regions[1]);
        tab_block.render(regions[1], buf);

        let tabs = [InfoTab::Preview, InfoTab::Telemetry];
        let mut spans: Vec<Span> = vec![Span::raw(" ")];
        for (i, tab) in tabs.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(
                    "   │   ",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            let is_active = state.info_tab == *tab;
            if is_active {
                spans.push(Span::styled("▸ ", Style::default().fg(self.accent)));
                spans.push(Span::styled(
                    tab.label().to_string(),
                    Style::default()
                        .fg(self.accent)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(
                    tab.label().to_string(),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }
        Widget::render(Paragraph::new(Line::from(spans)), tab_inner, buf);

        // ── Body ──
        let body_area = regions[2];
        let filtered = state.filtered_items();
        let label_style = Style::default().fg(Color::DarkGray);
        let Some(session) = filtered.get(state.selected) else {
            let line = Line::from(Span::styled(
                "  No session selected.",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
            let y = body_area.y + body_area.height.saturating_sub(1) / 2;
            Widget::render(
                Paragraph::new(line),
                Rect {
                    x: body_area.x,
                    y,
                    width: body_area.width,
                    height: 1,
                },
                buf,
            );
            return;
        };
        let is_active = state.active_id == Some(session.id);
        match state.info_tab {
            InfoTab::Preview => {
                self.render_preview_body(body_area, buf, session, is_active, label_style);
            }
            InfoTab::Telemetry => {
                self.render_telemetry_body(body_area, buf, session, label_style);
            }
        }
    }

    /// Body content for the Telemetry tab: message-count bar chart and token/time stats.
    fn render_telemetry_body(
        &self,
        area: Rect,
        buf: &mut Buffer,
        session: &PickerSession,
        label_style: Style,
    ) {
        let mut lines: Vec<Line> = Vec::new();
        let total =
            session.user_message_count + session.assistant_message_count + session.tool_call_count;

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

        let bar_width = (area.width - 3) as usize;

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
        if session.input_tokens > 0
            || session.output_tokens > 0
            || session.total_tokens > 0
            || session.total_tool_use > 0
            || session.time_consume_ms > 0
        {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  ── Telemetry ──",
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
            lines.push(Line::from(vec![
                Span::styled("  Total    ", label_style),
                Span::styled(
                    format_tokens(session.total_tokens),
                    Style::default().fg(Color::Yellow),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Tools    ", label_style),
                Span::styled(
                    session.total_tool_use.to_string(),
                    Style::default().fg(Color::Magenta),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Time     ", label_style),
                Span::styled(
                    format_duration(session.time_consume_ms),
                    Style::default().fg(Color::Cyan),
                ),
            ]));
        }

        Widget::render(Paragraph::new(lines), area, buf);
    }

    /// Body content for the Preview tab: metadata + first/last message snippets.
    fn render_preview_body(
        &self,
        area: Rect,
        buf: &mut Buffer,
        session: &PickerSession,
        is_active: bool,
        label_style: Style,
    ) {
        let mut lines: Vec<Line> = Vec::new();

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

            let preview_width = (area.width as usize).saturating_sub(6).max(1);
            if let Some(ref first_msg) = session.first_user_message {
                lines.push(Line::from(vec![Span::styled(
                    "  ❯ ",
                    Style::default().fg(Color::Cyan),
                )]));
                push_wrapped_preview(
                    &mut lines,
                    first_msg,
                    preview_width,
                    Style::default().fg(Color::Gray),
                );
            }

            if let Some(ref last_msg) = session.last_assistant_message {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![Span::styled(
                    "  🤖 ",
                    Style::default().fg(Color::Green),
                )]));
                push_wrapped_preview(
                    &mut lines,
                    last_msg,
                    preview_width,
                    Style::default().fg(Color::Gray),
                );
            }
        }

        // Truncate to fit available height.
        let max_lines = area.height as usize;
        if lines.len() > max_lines {
            lines.truncate(max_lines.saturating_sub(1));
            lines.push(Line::from(Span::styled(
                "  …",
                Style::default().fg(Color::DarkGray),
            )));
        }

        Widget::render(Paragraph::new(lines), area, buf);
    }
}

fn session_list_lines(
    session: &PickerSession,
    is_active: bool,
    is_selected: bool,
    accent: Color,
) -> Vec<Line<'static>> {
    let marker = if is_active { "●" } else { "○" };
    let title = session.title.as_deref().unwrap_or("(untitled)");
    let title_style = if is_selected {
        Style::default()
            .fg(Color::Black)
            .bg(accent)
            .add_modifier(Modifier::BOLD)
    } else if is_active {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    let last_style = if is_selected {
        title_style
    } else {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM)
    };

    vec![
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(format!("{marker} "), Style::default().fg(accent)),
            Span::styled(title.to_string(), title_style),
        ]),
        Line::from(vec![
            Span::styled("    Last ", last_style),
            Span::styled(format_timestamp(session.last_active), last_style),
        ]),
    ]
}

// ── Formatting helpers ──────────────────────────────────

/// Append preview text as wrapped terminal lines, preserving source newlines.
fn push_wrapped_preview(lines: &mut Vec<Line<'static>>, text: &str, width: usize, style: Style) {
    for source_line in text.split('\n') {
        for wrapped_line in textwrap::wrap(source_line, width) {
            lines.push(Line::from(vec![
                Span::styled("      ", style),
                Span::styled(wrapped_line.to_string(), style),
            ]));
        }
    }
}

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

fn format_duration(millis: u64) -> String {
    if millis < 1_000 {
        format!("{millis}ms")
    } else if millis < 60_000 {
        format!("{:.1}s", millis as f64 / 1_000.0)
    } else {
        let minutes = millis / 60_000;
        let seconds = (millis % 60_000) / 1_000;
        if minutes < 60 {
            format!("{minutes}m {seconds}s")
        } else {
            let hours = minutes / 60;
            format!("{hours}h {}m", minutes % 60)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_message_previews_are_exactly_one_k_characters() {
        let preview = truncate_for_preview(&"x".repeat(2048), PREVIEW_MAX_CHARS);

        assert_eq!(preview.chars().count(), PREVIEW_MAX_CHARS);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn compute_session_stats_keeps_the_fixed_preview_budget() {
        let stats = compute_session_stats(&[crate::state::ChatLine::User("y".repeat(5000))]);

        assert_eq!(
            stats
                .first_user_message
                .expect("user preview")
                .chars()
                .count(),
            PREVIEW_MAX_CHARS
        );
    }

    #[test]
    fn preview_wrapping_and_newlines_produce_multiple_rows() {
        let text = truncate_for_preview("alpha beta gamma\nshort line", PREVIEW_MAX_CHARS);
        let mut lines = Vec::new();

        push_wrapped_preview(&mut lines, &text, 10, Style::default());

        assert!(lines.len() > 2);
        assert_eq!(lines[2].spans[1].content.to_string(), "short line");
    }

    #[test]
    fn session_list_rows_include_last_active_time() {
        let session = PickerSession {
            id: uuid::Uuid::new_v4(),
            title: Some("results".into()),
            last_active: 0,
            created_at: 0,
            user_message_count: 1,
            assistant_message_count: 1,
            tool_call_count: 0,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            total_tool_use: 0,
            time_consume_ms: 0,
            first_user_message: None,
            last_assistant_message: None,
        };

        let lines = session_list_lines(&session, false, false, Color::Cyan);
        let last_line = lines[1]
            .spans
            .iter()
            .map(|span| span.content.to_string())
            .collect::<String>();

        assert_eq!(lines.len(), 2);
        assert!(last_line.starts_with("    Last "));
    }

    #[test]
    fn cycle_focus_toggles_between_panels() {
        let mut state = SessionPickerState::default();
        assert_eq!(state.focus, PanelFocus::Left);

        state.cycle_focus();
        assert_eq!(state.focus, PanelFocus::Right);

        state.cycle_focus();
        assert_eq!(state.focus, PanelFocus::Left);
    }

    #[test]
    fn cycle_tab_and_cycle_focus_are_independent() {
        let mut state = SessionPickerState::default();
        assert_eq!(state.info_tab, InfoTab::Preview);
        assert_eq!(state.focus, PanelFocus::Left);

        state.cycle_tab();
        assert_eq!(state.info_tab, InfoTab::Telemetry);
        assert_eq!(state.focus, PanelFocus::Left, "cycle_tab must not move focus");

        state.cycle_focus();
        assert_eq!(state.focus, PanelFocus::Right);
        assert_eq!(
            state.info_tab,
            InfoTab::Telemetry,
            "cycle_focus must not move info tab"
        );
    }

    #[test]
    fn open_resets_focus_and_tab() {
        let mut state = SessionPickerState::default();
        state.focus = PanelFocus::Right;
        state.info_tab = InfoTab::Telemetry;

        state.open(uuid::Uuid::new_v4(), "agent".into());

        assert_eq!(state.focus, PanelFocus::Left);
        assert_eq!(state.info_tab, InfoTab::Preview);
    }
}

#[allow(clippy::items_after_test_module)]
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
