//! Message picker — fuzzy-filterable list of **text** chat messages
//! (User + Assistant only) from the active session. Triggered via the
//! command palette's "Copy message" action; selecting an entry copies
//! that message's full text to the system clipboard via [`arboard`].
//!
//! The search input is backed by [`TextArea`](crate::xai_textarea::TextArea)
//! for a richer editing experience: cursor navigation (Left/Right/Home/End),
//! word deletion (Ctrl+W, Alt+Backspace), undo/redo (Ctrl+Z / Ctrl+Shift+Z),
//! yank (Ctrl+Y), and paste (Ctrl+V) — all within a single-line search field.
//!
//! Uses [`PickerState`] for filtering / navigation and ships a custom
//! renderer for the list area (colored role badges, multi-line preview).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, List, ListItem, Paragraph, StatefulWidget, StatefulWidgetRef, Widget,
    },
};

use crate::state::ChatLine;
use crate::widgets::popup::Popup;
use crate::widgets::searchable_picker::{PickerItem, PickerState};
use crate::xai_textarea::{TextArea, TextAreaState};

// ═══════════════════════════════════════════════════════════════════════
// Item
// ═══════════════════════════════════════════════════════════════════════

/// Role of a text message — determines badge label + color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
}

impl MessageRole {
    /// Short uppercase badge label (3 chars).
    pub fn badge(self) -> &'static str {
        match self {
            Self::User => "YOU",
            Self::Assistant => "AI",
        }
    }

    /// Color used for the badge background and the message title.
    pub fn color(self) -> Color {
        match self {
            Self::User => Color::Cyan,
            Self::Assistant => Color::Green,
        }
    }

    /// Lowercase tag for keywords matching (e.g. searching "user" or
    /// "assistant" narrows by role).
    pub fn tag(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// One selectable text message from the active session's transcript.
#[derive(Debug, Clone)]
pub struct MessageItem {
    /// Short single-line preview used as the list title.
    pub preview: String,
    /// Full text payload to copy to the clipboard.
    pub full_text: String,
    /// Role badge (User or Assistant).
    pub role: MessageRole,
    /// 1-indexed position in the source transcript (for orientation).
    pub index: usize,
}

impl MessageItem {
    /// Build a [`MessageItem`] from a [`ChatLine`] at the given
    /// transcript `index`. Returns `None` for anything other than
    /// non-empty **User** or **Assistant** text — tool calls, thinking
    /// blocks, errors, separators, etc. are all filtered out.
    pub fn from_chat_line(line: &ChatLine, index: usize) -> Option<Self> {
        let (text, role) = match line {
            ChatLine::User(s) => (s.clone(), MessageRole::User),
            ChatLine::Assistant { text, .. } => (text.clone(), MessageRole::Assistant),
            // Everything else is intentionally excluded — the picker is
            // for copying conversational text, not internal events.
            _ => return None,
        };
        if text.trim().is_empty() {
            return None;
        }
        Some(Self {
            preview: one_line_preview(&text),
            full_text: text,
            role,
            index,
        })
    }
}

impl PickerItem for MessageItem {
    fn title(&self) -> &str {
        &self.preview
    }
    fn keywords(&self) -> &str {
        &self.full_text
    }
    fn category(&self) -> Option<&str> {
        Some(self.role.tag())
    }
}

/// First non-empty line of `text`, collapsed to single spaces, capped at
/// 100 characters with a trailing ellipsis when truncated.
fn one_line_preview(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let collapsed: String = first.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX: usize = 100;
    if collapsed.chars().count() <= MAX {
        collapsed
    } else {
        let mut s: String = collapsed.chars().take(MAX).collect();
        s.push('…');
        s
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Key outcome
// ═══════════════════════════════════════════════════════════════════════

/// Result of forwarding a key event to the message picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessagePickerKeyOutcome {
    /// The key was consumed but no action is needed.
    Consumed,
    /// The user pressed Enter with a selection — copy the selected item.
    CopySelected,
    /// The user pressed Esc — close the picker.
    Close,
}

// ═══════════════════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════════════════

/// State for the message-copy popup.
///
/// Wraps a [`PickerState<MessageItem>`] for item storage / filtering /
/// navigation, plus a [`TextArea`] that powers the search input with
/// full cursor navigation, word deletion, undo/redo, and paste.
pub struct MessagePickerState {
    /// Item list + filtering state.
    pub picker: PickerState<MessageItem>,
    /// Search input editor (single-line).
    pub textarea: TextArea,
    /// Render state for the textarea.
    pub textarea_state: TextAreaState,
    /// On-screen cursor position computed during the most recent render.
    /// The host reads this after drawing and calls
    /// `Frame::set_cursor_position` so ratatui emits the hardware cursor.
    pub cursor_pos: Option<(u16, u16)>,
}

impl Default for MessagePickerState {
    fn default() -> Self {
        let mut textarea = TextArea::new();
        // Single-line search input — disable the scrollbar.
        textarea.show_scrollbar = false;
        Self {
            picker: PickerState::new_empty(),
            textarea,
            textarea_state: TextAreaState::default(),
            cursor_pos: None,
        }
    }
}

impl MessagePickerState {
    /// Whether the picker popup is currently visible.
    pub fn is_visible(&self) -> bool {
        self.picker.visible
    }

    /// Close the picker and clear the search input.
    pub fn close(&mut self) {
        self.picker.close();
        self.textarea.set_text("");
        self.textarea.clear_history();
        self.cursor_pos = None;
    }

    /// Open the picker with the supplied messages, clearing the search
    /// input and resetting the selection.
    pub fn open_with(&mut self, messages: Vec<MessageItem>) {
        self.textarea.set_text("");
        self.textarea.clear_history();
        self.picker.set_items(messages);
        self.picker.open();
    }

    /// Move the list selection up by one row.
    pub fn move_up(&mut self) {
        self.picker.move_up();
    }

    /// Move the list selection down by one row.
    pub fn move_down(&mut self) {
        self.picker.move_down();
    }

    /// Clone of the currently selected item, if any.
    pub fn selected_item(&self) -> Option<MessageItem> {
        self.picker.selected_item()
    }

    // ── Key handling ──

    /// Handle a raw key event.
    ///
    /// Esc → [`MessagePickerKeyOutcome::Close`].
    /// Enter → [`MessagePickerKeyOutcome::CopySelected`] (if a message is
    /// selected).
    /// Up / Down → list navigation (not forwarded to the textarea so the
    /// selection cursor stays in the single-line input).
    /// Everything else → forwarded to [`TextArea::input`], then the
    /// textarea text is synced into the picker query to re-run the filter.
    pub fn handle_key(&mut self, key: KeyEvent) -> MessagePickerKeyOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            // Esc — close.
            KeyCode::Esc => return MessagePickerKeyOutcome::Close,

            // Enter — copy the selected message.
            KeyCode::Enter => return MessagePickerKeyOutcome::CopySelected,

            // Up / Down — navigate the filtered list. NOT forwarded to
            // the textarea (whose own Up/Down move the cursor between
            // lines, which is meaningless in a single-line input).
            KeyCode::Up => {
                self.picker.move_up();
                return MessagePickerKeyOutcome::Consumed;
            }
            KeyCode::Down => {
                self.picker.move_down();
                return MessagePickerKeyOutcome::Consumed;
            }

            // Ctrl+U — clear the entire search input (a common readline
            // shortcut that xai_textarea doesn't handle).
            KeyCode::Char('u') if ctrl => {
                self.textarea.set_text("");
                self.sync_query();
                return MessagePickerKeyOutcome::Consumed;
            }

            // All other keys go to the TextArea for rich editing.
            _ => {}
        }

        // Forward to the textarea. The TextArea handles:
        //   - Char insertion / Backspace / Delete
        //   - Left / Right / Home / End / Ctrl+A / Ctrl+E
        //   - Ctrl+W (delete word), Alt+Backspace
        //   - Ctrl+Z / Ctrl+Shift+Z (undo/redo)
        //   - Ctrl+Y (yank), Ctrl+V (paste)
        //   - Ctrl+K (kill to end of line)
        self.textarea.input(key);
        self.sync_query();
        MessagePickerKeyOutcome::Consumed
    }

    /// Sync the textarea content into the picker query and re-filter.
    fn sync_query(&mut self) {
        let text = self.textarea.text().to_string();
        self.picker.set_query(&text);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Builder helper
// ═══════════════════════════════════════════════════════════════════════

/// Walk a transcript and emit [`MessageItem`]s for every **User** or
/// **Assistant** text message, skipping everything else. The returned
/// `Vec` is in **reverse** order (newest first) so the most recent
/// messages appear at the top of the picker list.
pub fn collect_text_messages(messages: &[ChatLine]) -> Vec<MessageItem> {
    messages
        .iter()
        .enumerate()
        .rev()
        .filter_map(|(i, line)| MessageItem::from_chat_line(line, i + 1))
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════
// Render — two-column layout (list + preview)
// ═══════════════════════════════════════════════════════════════════════

const ACCENT: Color = Color::Magenta;
/// Prompt gutter width for the search input: `❯ ` = 2 cols.
const PROMPT_GUTTER: u16 = 2;

/// Render the message picker popup with a two-column layout:
///
/// ```text
/// ┌─ Copy Message ───────────────────────────────────────────────┐
/// │ ❯ filter messages…                                            │  ← search
/// ├──────────────────────────────────────────────────────────────┤  ← separator
/// │  Messages         │  Preview                                  │
/// │ ┌────────────────┐ │ ┌──────────────────────────────────────┐ │
/// │ │ YOU hello world│ │ │  Role    YOU                         │ │
/// │ │ AI  hi! how…   │ │ │  Index   #1                          │ │
/// │ │ AI  The answer │ │ │  Length  42 chars                     │ │
/// │ │                │ │ │                                        │ │
/// │ │                │ │ │  hello world                           │ │
/// │ │                │ │ │  ...                                   │ │
/// │ └────────────────┘ │ └──────────────────────────────────────┘ │
/// ├──────────────────────────────────────────────────────────────┤
/// │ Enter copy  ↑↓ navigate  Esc cancel                           │  ← footer
/// └──────────────────────────────────────────────────────────────┘
/// ```
pub fn render_message_picker(frame_area: Rect, buf: &mut Buffer, state: &mut MessagePickerState) {
    if !state.picker.visible {
        return;
    }

    // Wide popup: 80% of frame width, auto height.
    let popup_w = (frame_area.width * 8 / 10).max(60);
    let popup = Popup::new(" Copy Message ").accent(ACCENT).width(popup_w);
    let inner = popup.render(frame_area, buf);

    // Vertical: search input (1) + separator (1) + content (rest) + footer (1).
    let v_regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Search input (textarea)
            Constraint::Length(1), // Separator line
            Constraint::Min(5),    // Two-column content
            Constraint::Length(1), // Footer
        ])
        .split(inner);

    // ── Search input row ──
    render_search_input(v_regions[0], buf, state);

    // ── Separator line ──
    Widget::render(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
        v_regions[1],
        buf,
    );

    // ── Two-column content area ──
    // Left: message list (~40% of popup). Right: preview (rest).
    let list_w = (inner.width * 2 / 5).clamp(24, 48);
    let h_regions = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(list_w), Constraint::Min(10)])
        .split(v_regions[2]);

    render_list_block(h_regions[0], buf, state);
    render_preview_block(h_regions[1], buf, state);

    // ── Footer ──
    let footer = Line::from(vec![
        Span::styled(
            " Enter",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" copy  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "↑↓",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "Esc",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
    ]);
    Widget::render(Paragraph::new(footer), v_regions[3], buf);
}

// ── Search input ──────────────────────────────────────────────

/// Render the TextArea-backed search input in the top row.
fn render_search_input(area: Rect, buf: &mut Buffer, state: &mut MessagePickerState) {
    let prompt_style = if state.textarea.is_empty() {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    };
    buf.set_string(area.x, area.y, "❯", prompt_style);

    let ta_area = if area.width > PROMPT_GUTTER {
        Rect {
            x: area.x + PROMPT_GUTTER,
            width: area.width - PROMPT_GUTTER,
            ..area
        }
    } else {
        area
    };

    if state.textarea.is_empty() {
        let placeholder_style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM);
        let placeholder = "filter messages…";
        let truncated = if placeholder.len() > ta_area.width as usize {
            &placeholder[..ta_area.width as usize]
        } else {
            placeholder
        };
        buf.set_string(ta_area.x, ta_area.y, truncated, placeholder_style);
        state.cursor_pos = Some((ta_area.x, ta_area.y));
    } else {
        let textarea: &TextArea = &state.textarea;
        let ta_state: &mut TextAreaState = &mut state.textarea_state;
        textarea.render_ref(ta_area, buf, ta_state);
        state.cursor_pos = state
            .textarea
            .cursor_pos_with_state(ta_area, state.textarea_state);
    }
}

// ── Left block: message list ──────────────────────────────────

/// Render the left column — a compact list of messages with role badges.
fn render_list_block(area: Rect, buf: &mut Buffer, state: &mut MessagePickerState) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            format!(" Messages ({}) ", state.picker.filtered.len()),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    block.render(area, buf);

    if state.picker.filtered.is_empty() {
        let line = Line::from(Span::styled(
            "  No messages found.",
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

    let selected_idx = state.picker.selected;
    let list_items: Vec<ListItem> = state
        .picker
        .filtered
        .iter()
        .enumerate()
        .map(|(sel_i, &item_i)| {
            let item = &state.picker.items[item_i];
            let is_selected = sel_i == selected_idx;

            let title_style = if is_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(item.role.color())
            };

            let badge_style = if is_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::Black)
                    .bg(item.role.color())
                    .add_modifier(Modifier::BOLD)
            };

            // Truncate the preview to fit the list column width (minus
            // badge + padding).  The right-side preview pane shows the
            // full text, so the list item just needs to be scannable.
            let avail = inner.width as usize;
            let badge_w = item.role.badge().len() + 4; // " badge " + spaces
            let max_preview = avail.saturating_sub(badge_w + 6).max(10);
            let preview_trunc = truncate_str(&item.preview, max_preview);

            ListItem::new(Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled(format!(" {} ", item.role.badge()), badge_style),
                Span::styled(" ", Style::default()),
                Span::styled(preview_trunc, title_style),
            ]))
        })
        .collect();

    let list = List::new(list_items).highlight_style(
        Style::default()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD),
    );

    StatefulWidget::render(list, inner, buf, &mut state.picker.list_state);
}

// ── Right block: full preview ─────────────────────────────────

/// Render the right column — a detailed preview of the selected message
/// showing metadata (role, index, character count) followed by the full
/// text wrapped to the available width.
fn render_preview_block(area: Rect, buf: &mut Buffer, state: &MessagePickerState) {
    let block = Block::default().borders(Borders::NONE).title(Span::styled(
        " Preview ",
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    ));
    let inner = block.inner(area);
    block.render(area, buf);

    let Some(item) = state.picker.selected_item() else {
        let line = Line::from(Span::styled(
            "  Select a message to preview.",
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

    let char_count = item.full_text.chars().count();
    let line_count = item.full_text.lines().count();

    let mut lines: Vec<Line> = Vec::new();

    // ── Metadata header ──
    let badge_bg = item.role.color();
    lines.push(Line::from(vec![
        Span::styled("  Role     ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!(" {} ", item.role.badge()), {
            Style::default()
                .fg(Color::Black)
                .bg(badge_bg)
                .add_modifier(Modifier::BOLD)
        }),
        Span::styled(
            format!("  ({})", item.role.tag()),
            Style::default().fg(Color::DarkGray),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled("  Index    ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("#{}", item.index),
            Style::default().fg(Color::White),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled("  Length   ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{char_count} chars, {line_count} lines"),
            Style::default().fg(Color::Gray),
        ),
    ]));

    // Separator between metadata and content.
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Content:",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    // ── Full text with wrapping ──
    let content_indent = "  ";
    for raw_line in item.full_text.lines() {
        let trimmed = raw_line.trim_end();
        if trimmed.is_empty() {
            lines.push(Line::from(""));
            continue;
        }
        // Wrap long lines to fit the preview width.
        let max_w = (inner.width as usize).saturating_sub(content_indent.len());
        if max_w < 4 {
            // Not enough width — just show raw line.
            lines.push(Line::from(Span::styled(
                format!("{content_indent}{trimmed}"),
                Style::default().fg(Color::Gray),
            )));
            continue;
        }
        let wrapped = wrap_text(trimmed, max_w);
        for wl in &wrapped {
            lines.push(Line::from(Span::styled(
                format!("{content_indent}{wl}"),
                Style::default().fg(Color::Gray),
            )));
        }
    }

    // If the text doesn't end with a newline, the loop above already
    // handled the last line via `.lines()`. But if the text IS empty
    // (just whitespace), show a placeholder.
    if item.full_text.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            format!("{content_indent}(empty message)"),
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Truncate to available height with ellipsis indicator.
    let max_lines = inner.height as usize;
    if lines.len() > max_lines {
        lines.truncate(max_lines.saturating_sub(1));
        lines.push(Line::from(Span::styled(
            format!("{content_indent}…"),
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines);
    Widget::render(paragraph, inner, buf);
}

// ── Helpers ───────────────────────────────────────────────────

/// Truncate a string to `max` chars, appending `…` if truncated.
fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// Greedy word-wrap: break `text` into lines no wider than `max_width`
/// (in display columns). Preserves long unbreakable tokens (truncates
/// them with `…` rather than splitting mid-word).
fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut current_w = 0usize;

    for word in text.split_whitespace() {
        let word_w = word.chars().count();
        if current_w == 0 {
            // First word on the line.
            if word_w > max_width {
                // Truncate very long unbreakable tokens.
                current = word.chars().take(max_width.saturating_sub(1)).collect();
                current.push('…');
                current_w = max_width;
            } else {
                current = word.to_string();
                current_w = word_w;
            }
        } else if current_w + 1 + word_w <= max_width {
            current.push(' ');
            current.push_str(word);
            current_w += 1 + word_w;
        } else {
            // Wrap.
            result.push(std::mem::take(&mut current));
            if word_w > max_width {
                current = word.chars().take(max_width.saturating_sub(1)).collect();
                current.push('…');
                current_w = max_width;
            } else {
                current = word.to_string();
                current_w = word_w;
            }
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}

// ═══════════════════════════════════════════════════════════════════════
// Clipboard — multi-strategy (OSC 52 → arboard → xclip/wl-copy)
// ═══════════════════════════════════════════════════════════════════════

/// Copy `text` to the system clipboard.
///
/// Tries three strategies in order so the copy works across all
/// environments the TUI might run in:
///
/// 1. **OSC 52** escape sequence — the terminal emulator itself sets the
///    clipboard. Works over SSH, tmux, screen, and on headless servers
///    without X11/Wayland. Supported by xterm, iTerm2, Alacritty,
///    Kitty, WezTerm, Windows Terminal, and others.
/// 2. **`arboard`** — direct X11/Wayland/macOS/Windows clipboard API.
///    Works when a display server is available (`$DISPLAY` or
///    `$WAYLAND_DISPLAY` is set).
/// 3. **CLI tools** (`xclip`, `xsel`, `wl-copy`, `pbcopy`) — piped
///    subprocess fallback for environments where neither OSC 52 nor
///    arboard are available.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    // 1. OSC 52 — always attempt; harmless if the terminal ignores it.
    if let Ok(()) = osc52_copy(text) {
        return Ok(());
    }

    // 2. arboard — works on macOS, Windows, and Linux with a display.
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if clipboard.set_text(text.to_string()).is_ok() {
            return Ok(());
        }
    }

    // 3. CLI tools — pipe text to the first available clipboard helper.
    cli_copy(text)
}

/// Send an OSC 52 escape sequence to set the clipboard via the terminal
/// emulator.
///
/// Tries `/dev/tty` first (so the sequence reaches the terminal even
/// when stdout is buffered by ratatui's rendering loop). If `/dev/tty`
/// is unavailable (e.g. headless container without a controlling
/// terminal), falls back to `stdout` — which still works because the
/// TUI's stdout is connected to the terminal emulator.
///
/// When running inside tmux (`$TMUX` is set), the sequence is wrapped
/// in a DCS passthrough envelope so tmux forwards it to the outer
/// terminal.
fn osc52_copy(text: &str) -> Result<(), String> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::io::Write;

    const OSC52_MAX_BYTES: usize = 100_000;
    if text.len() > OSC52_MAX_BYTES {
        return Err(format!(
            "OSC 52 payload too large ({} bytes; max {OSC52_MAX_BYTES})",
            text.len()
        ));
    }

    let encoded = STANDARD.encode(text.as_bytes());
    let in_tmux = std::env::var_os("TMUX").is_some();
    let seq = if in_tmux {
        // DCS passthrough: ESC P tmux ; ESC ESC ] 52 ; c ; <data> BEL ESC \
        format!("\x1bPtmux;\x1b\x1b]52;c;{encoded}\x07\x1b\\")
    } else {
        format!("\x1b]52;c;{encoded}\x07")
    };

    // Strategy 1: /dev/tty (preferred — independent of stdout buffering).
    #[cfg(unix)]
    {
        match std::fs::OpenOptions::new().write(true).open("/dev/tty") {
            Ok(mut tty) => {
                if let Err(e) = tty.write_all(seq.as_bytes()) {
                    tracing::debug!("OSC 52 /dev/tty write failed: {e}; trying stdout");
                } else if let Err(e) = tty.flush() {
                    tracing::debug!("OSC 52 /dev/tty flush failed: {e}; trying stdout");
                } else {
                    return Ok(());
                }
            }
            Err(e) => {
                tracing::debug!("OSC 52 /dev/tty open failed: {e}; trying stdout");
            }
        }
    }

    // Strategy 2: stdout fallback (when /dev/tty is unavailable).
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(seq.as_bytes())
        .map_err(|e| format!("OSC 52 stdout write failed: {e}"))?;
    stdout
        .flush()
        .map_err(|e| format!("OSC 52 stdout flush failed: {e}"))
}

/// Pipe `text` to the first available clipboard CLI tool.
fn cli_copy(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // (program, args)
    let candidates: &[(&str, Vec<&str>)] = &[
        ("xclip", vec!["-selection", "clipboard"]),
        ("xsel", vec!["--clipboard", "--input"]),
        ("wl-copy", vec![]),
        ("pbcopy", vec![]),
    ];

    for (prog, args) in candidates {
        let result = Command::new(prog)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();

        let mut child = match result {
            Ok(c) => c,
            Err(_) => continue, // not installed — try next
        };

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
            let _ = stdin.flush();
        }
        // Wait for the process to finish so the clipboard content is
        // committed before we return.
        let _ = child.wait();
        return Ok(());
    }

    Err(
        "no clipboard backend available (OSC 52, arboard, xclip/xsel/wl-copy/pbcopy all failed)"
            .to_string(),
    )
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn make_messages() -> Vec<ChatLine> {
        vec![
            ChatLine::User("hello world".into()),
            ChatLine::Assistant {
                text: "hi! how can I help?".into(),
                usage: None,
            },
            ChatLine::Thinking("let me think...".into()),
            ChatLine::ToolCall {
                name: "search".into(),
                input: "{\"q\":\"rust\"}".into(),
            },
            ChatLine::ToolResult {
                ok: true,
                content: "result body".into(),
            },
            ChatLine::Error("oops".into()),
            ChatLine::Separator,
            ChatLine::Cancelled,
            ChatLine::User("   \n  ".into()),
            ChatLine::ToolBackground {
                seq: 1,
                name: "bg".into(),
            },
        ]
    }

    #[test]
    fn collect_text_messages_keeps_only_user_and_assistant() {
        let msgs = make_messages();
        let items = collect_text_messages(&msgs);
        assert_eq!(items.len(), 2);
        // Reverse order: newest (assistant, index 2) first.
        assert_eq!(items[0].role, MessageRole::Assistant);
        assert_eq!(items[0].index, 2);
        assert_eq!(items[0].preview, "hi! how can I help?");
        assert_eq!(items[1].role, MessageRole::User);
        assert_eq!(items[1].index, 1);
        assert_eq!(items[1].preview, "hello world");
    }

    #[test]
    fn role_badge_and_color_are_stable() {
        assert_eq!(MessageRole::User.badge(), "YOU");
        assert_eq!(MessageRole::Assistant.badge(), "AI");
        assert_eq!(MessageRole::User.color(), Color::Cyan);
        assert_eq!(MessageRole::Assistant.color(), Color::Green);
        assert_eq!(MessageRole::User.tag(), "user");
        assert_eq!(MessageRole::Assistant.tag(), "assistant");
    }

    #[test]
    fn preview_collapses_whitespace_and_truncates() {
        let mut long_line = String::from("  alpha   beta  ");
        long_line.push_str(&"x".repeat(300));
        let preview = one_line_preview(&long_line);
        assert!(preview.starts_with("alpha beta"));
        assert!(preview.ends_with('…'));
        assert!(preview.chars().count() <= 101);
        assert!(!preview.contains('\n'));
    }

    #[test]
    fn preview_only_takes_first_line_of_multiline_input() {
        let input = "first line\nsecond line with more text\nthird";
        assert_eq!(one_line_preview(input), "first line");
    }

    #[test]
    fn preview_uses_full_text_when_single_line_fits() {
        assert_eq!(one_line_preview("hello"), "hello");
        assert_eq!(one_line_preview(""), "");
    }

    #[test]
    fn keywords_match_against_full_text() {
        let item = MessageItem::from_chat_line(
            &ChatLine::Assistant {
                text: "line one\nline two with keyword".into(),
                usage: None,
            },
            1,
        )
        .unwrap();
        assert_eq!(item.preview, "line one");
        assert!(item.keywords().contains("keyword"));
    }

    #[test]
    fn from_chat_line_rejects_non_text_variants() {
        assert!(MessageItem::from_chat_line(&ChatLine::Thinking("hmm".into()), 1).is_none());
        assert!(
            MessageItem::from_chat_line(
                &ChatLine::ToolCall {
                    name: "x".into(),
                    input: "{}".into()
                },
                1
            )
            .is_none()
        );
        assert!(
            MessageItem::from_chat_line(
                &ChatLine::ToolResult {
                    ok: true,
                    content: "x".into()
                },
                1
            )
            .is_none()
        );
        assert!(MessageItem::from_chat_line(&ChatLine::Error("x".into()), 1).is_none());
        assert!(MessageItem::from_chat_line(&ChatLine::Separator, 1).is_none());
    }

    // ── TextArea-backed key handling ──

    #[test]
    fn handle_key_esc_closes() {
        let mut s = MessagePickerState::default();
        s.open_with(collect_text_messages(&make_messages()));
        let outcome = s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(outcome, MessagePickerKeyOutcome::Close);
    }

    #[test]
    fn handle_key_enter_returns_copy_selected() {
        let mut s = MessagePickerState::default();
        s.open_with(collect_text_messages(&make_messages()));
        let outcome = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(outcome, MessagePickerKeyOutcome::CopySelected);
    }

    #[test]
    fn handle_key_up_down_navigates() {
        let mut s = MessagePickerState::default();
        s.open_with(collect_text_messages(&make_messages()));
        assert_eq!(s.picker.selected, 0);
        s.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(s.picker.selected, 1);
        s.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(s.picker.selected, 0);
    }

    #[test]
    fn handle_key_char_types_into_textarea_and_filters() {
        let mut s = MessagePickerState::default();
        s.open_with(collect_text_messages(&make_messages()));
        // Type "hello" — should narrow to the first user message.
        for c in "hello".chars() {
            s.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(s.textarea.text(), "hello");
        assert_eq!(s.picker.query, "hello");
        // Only "hello world" matches.
        assert_eq!(s.picker.filtered.len(), 1);
    }

    #[test]
    fn handle_key_backspace_deletes_from_textarea() {
        let mut s = MessagePickerState::default();
        s.open_with(collect_text_messages(&make_messages()));
        for c in "abc".chars() {
            s.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        s.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(s.textarea.text(), "ab");
        assert_eq!(s.picker.query, "ab");
    }

    #[test]
    fn handle_key_ctrl_u_clears_input() {
        let mut s = MessagePickerState::default();
        s.open_with(collect_text_messages(&make_messages()));
        for c in "hello".chars() {
            s.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        s.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(s.textarea.is_empty());
        assert!(s.picker.query.is_empty());
        // Filter resets to all items.
        assert_eq!(s.picker.filtered.len(), 2);
    }

    #[test]
    fn close_clears_textarea() {
        let mut s = MessagePickerState::default();
        s.open_with(collect_text_messages(&make_messages()));
        for c in "hello".chars() {
            s.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert!(!s.textarea.is_empty());
        s.close();
        assert!(s.textarea.is_empty());
        assert!(!s.picker.visible);
    }

    // ── Two-column render helpers ──

    #[test]
    fn truncate_str_short_returns_unchanged() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("", 5), "");
    }

    #[test]
    fn truncate_str_long_appends_ellipsis() {
        let result = truncate_str("abcdef", 4);
        assert_eq!(result, "abc…");
        assert_eq!(result.chars().count(), 4);
    }

    #[test]
    fn wrap_text_fits_single_line() {
        let lines = wrap_text("hello world", 20);
        assert_eq!(lines, vec!["hello world"]);
    }

    #[test]
    fn wrap_text_breaks_at_word_boundary() {
        let lines = wrap_text("alpha beta gamma delta", 12);
        assert_eq!(lines, vec!["alpha beta", "gamma delta"]);
    }

    #[test]
    fn wrap_text_truncates_unbreakable_token() {
        let lines = wrap_text("abcdefghijklmnopqrstuvwxyz", 5);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "abcd…");
    }

    #[test]
    fn wrap_text_empty_returns_single_empty_line() {
        let lines = wrap_text("", 10);
        assert_eq!(lines, vec![""]);
    }

    #[test]
    fn wrap_text_preserves_multiple_short_words() {
        let lines = wrap_text("a b c d e", 5);
        assert_eq!(lines, vec!["a b c", "d e"]);
    }
}
