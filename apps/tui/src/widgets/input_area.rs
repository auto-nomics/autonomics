use crate::xai_textarea::{TextArea, TextAreaState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::StatefulWidgetRef;
use ratatui::{
    layout::Rect,
    prelude::{StatefulWidget, Widget},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::Block,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

// ── Multi-line textarea input (used by the agent chat) ─────────

/// Wrapper around `xai_textarea::TextArea` for the chat input area.
///
/// Wraps the xAI TextArea which provides undo/redo, mouse selection,
/// scrollbar, and full Emacs keybindings out of the box.
#[derive(Debug)]
pub struct InputArea {
    pub textarea: TextArea,
    pub textarea_state: TextAreaState,
    /// Placeholder text stored locally (xAI textarea has no built-in placeholder).
    placeholder: String,
    /// Maximum grapheme clusters accepted. `None` = no cap. Defaults to
    /// [`InputArea::DEFAULT_MAX_LENGTH`] so an accidental 1 MB paste
    /// cannot produce a 1 MB outbound message.
    max_length: Option<usize>,
    /// On-screen cursor position computed during the most recent render, or
    /// `None` when the input is disabled. The host reads this after drawing
    /// and calls `Frame::set_cursor_position` inside its draw closure so
    /// ratatui emits the hardware cursor without a hide→show cycle.
    last_cursor_pos: Option<(u16, u16)>,
}

impl InputArea {
    /// Default length cap on the chat input — 16 KiB of user
    /// content is generous for a chat message and prevents runaway
    /// pastes without affecting normal use.
    pub const DEFAULT_MAX_LENGTH: usize = 16_384;

    pub fn new() -> Self {
        // No placeholder set — the InputWidget renders it directly when empty.
        // Wrapping is built-in via textwrap (no WrapMode needed).
        Self {
            textarea: TextArea::new(),
            textarea_state: TextAreaState::default(),
            max_length: Some(Self::DEFAULT_MAX_LENGTH),
            placeholder: String::new(),
            last_cursor_pos: None,
        }
    }

    /// Maximum number of input rows the layout will allocate before the
    /// textarea scrolls internally instead of growing further. Keeps a
    /// long paste from consuming the whole chat viewport.
    pub const MAX_INPUT_ROWS: u16 = 10;

    /// Number of terminal rows the current buffer occupies when wrapped to
    /// `width` columns. Used by the parent layout to size the input area
    /// dynamically: the box grows as the user types and caps at
    /// [`MAX_INPUT_ROWS`]. `width` is the *text* width (after any prompt /
    /// gutter), so the caller subtracts the prompt gutter first.
    pub fn display_height(&self, width: u16) -> u16 {
        self.textarea
            .desired_height(width)
            .clamp(1, Self::MAX_INPUT_ROWS)
    }

    /// Insert a newline (Shift+Enter / Alt+Enter). Respects `max_length`.
    pub fn insert_newline(&mut self) {
        if let Some(cap) = self.max_length {
            if self.grapheme_len() >= cap {
                return;
            }
        }
        self.textarea.insert_str("\n");
    }

    /// Get the current text content as a single string.
    pub fn value(&self) -> String {
        self.textarea.text().to_string()
    }

    /// Check if the textarea is empty.
    pub fn is_empty(&self) -> bool {
        self.textarea.is_empty()
    }

    /// Clear all content.
    pub fn clear(&mut self) {
        self.textarea.set_text("");
    }

    /// Set or replace the maximum-length cap (grapheme clusters,
    /// not bytes). `None` disables the cap.
    // Live code goes through `new()`'s `DEFAULT_MAX_LENGTH`; only the
    // unit tests override the cap via this setter.
    #[allow(dead_code)]
    pub fn set_max_length(&mut self, cap: Option<usize>) {
        self.max_length = cap;
    }

    /// Total grapheme clusters across all lines of the textarea.
    /// Computed lazily; `O(n)` over the buffer length.
    pub fn grapheme_len(&self) -> usize {
        self.value().graphemes(true).count()
    }

    /// Handle a key event. Returns true if the text content was modified.
    /// Enter and Escape are intercepted (not forwarded to the textarea) since
    /// the caller handles them for message sending / mode switching.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Reject keystrokes whose combined effect would breach the cap.
        // We don't know grapheme count for a keystroke a priori, so we
        // pre-flight by checking the cap (cheap; we already track
        // grapheme_len on demand).
        if let Some(cap) = self.max_length {
            if matches!(
                key.code,
                KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
            ) && self.grapheme_len() >= cap
            {
                // Already at the cap for character-affecting keys.
                // Backspace/Delete are still allowed so the user can
                // shrink before retyping.
                if matches!(key.code, KeyCode::Char(_)) {
                    return false;
                }
            }
        }
        match key.code {
            KeyCode::Enter | KeyCode::Esc => false,
            _ => {
                self.textarea.input(key);
                true
            }
        }
    }

    /// Insert a paste string at the cursor position, truncated to
    /// honour `max_length`. Returns `true` if anything was inserted.
    pub fn insert_str(&mut self, s: &str) -> bool {
        if s.is_empty() {
            return false;
        }
        let to_insert: String = match self.max_length {
            None => s.to_string(),
            Some(cap) => {
                let current = self.grapheme_len();
                let budget = cap.saturating_sub(current);
                if budget == 0 {
                    return false;
                }
                s.graphemes(true).take(budget).collect()
            }
        };
        if to_insert.is_empty() {
            return false;
        }
        self.textarea.insert_str(&to_insert);
        true
    }

    /// Set placeholder text (used for disabled / running state).
    pub fn set_placeholder(&mut self, text: &str) {
        self.placeholder = text.to_string();
    }

    /// On-screen cursor position from the most recent render. The caller
    /// should call `Frame::set_cursor_position` with this (when `Some`)
    /// inside its draw closure, so ratatui emits the hardware cursor
    /// without a hide→show cycle that resets the blink timer.
    pub fn last_cursor_pos(&self) -> Option<(u16, u16)> {
        self.last_cursor_pos
    }
}

impl Default for InputArea {
    fn default() -> Self {
        Self::new()
    }
}

/// State for [`InputWidget`].
pub struct InputWidgetState<'a> {
    pub input: &'a mut InputArea,
}

/// Width of the borderless prompt gutter ("› " = prompt glyph + space).
pub const PROMPT_GUTTER: u16 = 2;

/// Composite widget: a boxed prompt backed by the xAI `TextArea`, styled to
/// match the Grok TUI composer.
///
/// Renders a rounded border box (`╭─╮│╰─╯`) with a `❯` prefix on the first
/// content row, followed by the textarea to the right of a 2-column gutter.
/// The parent layout passes a height of `display_height(width) + 2` so the
/// border rows are accounted for.
pub struct InputWidget<'a> {
    pub disabled: bool,
    /// Whether the composer is actively editable (Input mode, agent idle).
    /// Drives hardware-cursor visibility: the caret only shows when the user
    /// can type. Browse mode and running both hide it.
    pub editable: bool,
    pub title: &'a str,
    pub placeholder: &'a str,
}

impl<'a> StatefulWidget for InputWidget<'a> {
    type State = InputWidgetState<'a>;

    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer, state: &mut Self::State) {
        if area.width < 2 || area.height < 2 {
            return;
        }

        // ── Chrome: rounded border box ──
        // Active (idle) → cyan border; disabled (agent running) → dim gray.
        let border_color = if self.disabled {
            Color::DarkGray
        } else {
            Color::Cyan
        };
        // Title (status label) colour: red on error, yellow while the agent is
        // active, dim gray when idle.
        let title_color = if self.title == "error" {
            Color::Red
        } else if self.disabled {
            Color::Yellow
        } else {
            Color::DarkGray
        };
        let block = Block::bordered()
            .border_set(ratatui::symbols::border::ROUNDED)
            .border_style(Style::default().fg(border_color))
            .title_top(
                Line::from(format!(" {} ", self.title)).style(Style::default().fg(
                    if self.title.is_empty() {
                        border_color
                    } else {
                        title_color
                    },
                )),
            );
        let content = block.inner(area);
        block.render(area, buf);

        if content.width == 0 || content.height == 0 {
            return;
        }

        // ── Prefix: ❯ (U+276F) + space, 2 cols, in accent color ──
        let prompt_style = if self.disabled {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        };
        buf.set_string(content.x, content.y, "❯", prompt_style);

        // Textarea renders into the area right of the prefix gutter.
        let inner = if content.width > PROMPT_GUTTER {
            Rect {
                x: content.x + PROMPT_GUTTER,
                width: content.width - PROMPT_GUTTER,
                ..content
            }
        } else {
            content
        };

        state.input.set_placeholder(self.placeholder);

        // The hardware caret only shows when the composer is actively editable
        // (Input mode + agent idle). Browse mode and running both hide it.
        if !self.editable {
            state.input.last_cursor_pos = None;
        }

        // Render placeholder directly when textarea is empty (xAI textarea has
        // no built-in placeholder support). The cursor sits at the start of the
        // editable row so the hardware caret appears over the first cell.
        if state.input.is_empty() && !state.input.placeholder.is_empty() {
            let placeholder_style = Style::default()
                .fg(Color::DarkGray)
                .add_modifier(ratatui::style::Modifier::ITALIC);
            let truncated = truncate_to_width(&state.input.placeholder, inner.width as usize);
            buf.set_string(inner.x, inner.y, &truncated, placeholder_style);
            if self.editable {
                state.input.last_cursor_pos = Some((inner.x, inner.y));
            }
        } else {
            // StatefulWidgetRef is implemented for &TextArea, not TextArea.
            // Split-borrow the two pub fields and call render_ref on the reference.
            let textarea: &TextArea = &state.input.textarea;
            let ta_state: &mut TextAreaState = &mut state.input.textarea_state;
            textarea.render_ref(inner, buf, ta_state);
            if self.editable {
                // TextAreaState is Copy — query the cursor against the post-render state.
                state.input.last_cursor_pos =
                    textarea.cursor_pos_with_state(inner, state.input.textarea_state);
            }
        }
    }
}

/// Truncate `s` so that its rendered display width does not exceed
/// `max_width` columns. Avoids splitting mid-grapheme.
fn truncate_to_width(s: &str, max_width: usize) -> String {
    let mut out = String::new();
    let mut width = 0;
    for grapheme in s.graphemes(true) {
        let w = unicode_width::UnicodeWidthStr::width(grapheme);
        if width + w > max_width {
            break;
        }
        out.push_str(grapheme);
        width += w;
    }
    out
}

// ── Input-history helpers (chat input Up/Down recall) ───────────

/// Append a freshly-submitted prompt to `history`, evicting the
/// oldest entry when capacity is reached. Whitespace-only entries
/// are skipped.
pub(crate) fn history_push(
    history: &mut std::collections::VecDeque<String>,
    text: String,
    capacity: usize,
) {
    if text.trim().is_empty() {
        return;
    }
    if history.len() >= capacity {
        history.pop_front();
    }
    history.push_back(text);
}

/// Handle `Up` on the chat input. Returns `true` if the key was
/// consumed (i.e. it changed the visible buffer or harmlessly
/// failed against an empty history).
///
/// State machine:
///
/// - `recall == None` and history non-empty → save current buffer
///   as `draft`, replace buffer with `history[last]`,
///   `recall = Some(last)`.
/// - `recall == Some(idx)` and `idx + 1 < len` → `recall = Some(idx+1)`,
///   replace buffer with `history[idx+1]`.
/// - `recall == Some(idx)` and `idx + 1 == len` → no-op (already at
///   the newest entry).
pub(crate) fn history_up(
    input: &mut InputArea,
    history: &std::collections::VecDeque<String>,
    draft: &mut Option<String>,
    recall: &mut Option<usize>,
) -> bool {
    if history.is_empty() {
        return false;
    }
    if let Some(idx) = *recall {
        if idx + 1 < history.len() {
            let new_idx = idx + 1;
            input.clear();
            input.insert_str(&history[new_idx]);
            *recall = Some(new_idx);
            return true;
        }
        return false;
    }
    // First Up: snapshot draft and load the newest history entry.
    *draft = Some(input.value());
    input.clear();
    let last = history.len() - 1;
    input.insert_str(&history[last]);
    *recall = Some(last);
    true
}

/// Handle `Down` on the chat input. Returns `true` if the key was
/// consumed.
///
/// State machine:
///
/// - `recall == None` → no-op (already editing the draft).
/// - `recall == Some(0)` → restore `draft`, clear recall. If the
///   draft was empty, the buffer ends up empty too.
/// - `recall == Some(idx)` with `idx > 0` → `recall = Some(idx-1)`,
///   replace buffer with `history[idx-1]`.
pub(crate) fn history_down(
    input: &mut InputArea,
    history: &std::collections::VecDeque<String>,
    draft: &mut Option<String>,
    recall: &mut Option<usize>,
) -> bool {
    let Some(idx) = *recall else {
        return false;
    };
    if idx == 0 {
        // Restore the draft buffer.
        let restored = draft.take().unwrap_or_default();
        input.clear();
        input.insert_str(&restored);
        *recall = None;
        return true;
    }
    let new_idx = idx - 1;
    input.clear();
    input.insert_str(&history[new_idx]);
    *recall = Some(new_idx);
    true
}

/// Drop any active recall state. Called when the user submits a
/// message or starts editing the buffer (any non-`Up`/`Down` key
/// after recall implies they're done browsing history).
pub(crate) fn history_clear_recall(draft: &mut Option<String>, recall: &mut Option<usize>) {
    draft.take();
    *recall = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// `history_push` skips empty inputs and evicts oldest when full.
    #[test]
    fn history_push_skips_empty_and_caps() {
        let mut h: VecDeque<String> = VecDeque::new();
        history_push(&mut h, "first".into(), 3);
        history_push(&mut h, "   ".into(), 3); // skipped (trim is empty)
        history_push(&mut h, "second".into(), 3);
        history_push(&mut h, "third".into(), 3);
        history_push(&mut h, "fourth".into(), 3); // evicts "first"
        let collected: Vec<&str> = h.iter().map(String::as_str).collect();
        assert_eq!(collected, vec!["second", "third", "fourth"]);
    }

    /// End-to-end Up/Down state machine against a populated history.
    /// Verifies the readline convention:
    ///   - First Up snapshots the current draft and shows the newest entry.
    ///   - Subsequent Up walks toward older entries; at the oldest,
    ///     Up is a no-op.
    ///   - Down walks toward newer entries; only when already at the
    ///     oldest (`idx == 0`) does Down restore the draft and exit
    ///     recall.
    #[test]
    fn history_up_down_state_machine() {
        let mut h: VecDeque<String> = VecDeque::new();
        history_push(&mut h, "alpha".into(), 10);
        history_push(&mut h, "beta".into(), 10);
        history_push(&mut h, "gamma".into(), 10);

        let mut input = InputArea::new();
        input.insert_str("draft-msg");
        let mut draft: Option<String> = None;
        let mut recall: Option<usize> = None;

        // First Up: snapshot the working draft, show "gamma" (newest).
        assert!(history_up(&mut input, &h, &mut draft, &mut recall));
        assert_eq!(input.value(), "gamma");
        assert_eq!(recall, Some(2));
        assert_eq!(draft.as_deref(), Some("draft-msg"));

        // Up at idx == len-1 is a no-op (already at newest).
        assert!(!history_up(&mut input, &h, &mut draft, &mut recall));
        assert_eq!(input.value(), "gamma");

        // Down walks toward older: Some(2) -> Some(1) = "beta".
        assert!(history_down(&mut input, &h, &mut draft, &mut recall));
        assert_eq!(input.value(), "beta");
        assert_eq!(recall, Some(1));

        // Down again: Some(1) -> Some(0) = "alpha".
        assert!(history_down(&mut input, &h, &mut draft, &mut recall));
        assert_eq!(input.value(), "alpha");
        assert_eq!(recall, Some(0));

        // Down from Some(0) restores the draft, clears recall.
        assert!(history_down(&mut input, &h, &mut draft, &mut recall));
        assert_eq!(input.value(), "draft-msg");
        assert_eq!(recall, None);
        assert!(draft.is_none(), "draft consumed on restoration");

        // From the restored draft, Up again goes to the newest history
        // entry (we don't remember the lower idx).
        assert!(history_up(&mut input, &h, &mut draft, &mut recall));
        assert_eq!(input.value(), "gamma");
        assert_eq!(recall, Some(2));

        // Out-of-bounds up at the start: history_up on a fresh input
        // with empty history must return false and not touch state.
        let empty_h: VecDeque<String> = VecDeque::new();
        let mut input3 = InputArea::new();
        let mut draft3 = None;
        let mut recall3 = None;
        assert!(!history_up(
            &mut input3,
            &empty_h,
            &mut draft3,
            &mut recall3
        ));
        assert_eq!(recall3, None);
    }

    /// `history_down` with no recall in progress is a no-op (we're
    /// editing the draft already).
    #[test]
    fn history_down_no_recall_is_noop() {
        let mut input = InputArea::new();
        let mut h: VecDeque<String> = VecDeque::new();
        history_push(&mut h, "x".into(), 10);
        let mut draft = None;
        let mut recall = None;
        assert!(!history_down(&mut input, &h, &mut draft, &mut recall));
        assert_eq!(input.value(), "");
    }

    /// `insert_str` on `InputArea` must honour the chat-box
    /// `max_length` cap (default 16 384 chars per
    /// [`InputArea::DEFAULT_MAX_LENGTH`]).
    #[test]
    fn input_area_paste_truncates_at_cap() {
        let mut input = InputArea::new();
        // Override the default cap so the test is fast.
        input.set_max_length(Some(8));
        assert!(input.insert_str("abcdefghij"));
        // Only "abcdefgh" should fit.
        assert_eq!(input.value(), "abcdefgh");
        assert_eq!(input.grapheme_len(), 8);
        // Further pastes are silently dropped.
        assert!(!input.insert_str("xyz"));
        assert_eq!(input.value(), "abcdefgh");
    }

    // ── display_height test ───────────────

    /// `display_height` grows with the buffer and is capped at
    /// [`InputArea::MAX_INPUT_ROWS`]. A short single-line buffer is one row;
    /// a buffer wider than the wrap width wraps onto extra rows.
    #[test]
    fn display_height_grows_and_caps() {
        let mut input = InputArea::new();

        // Empty buffer → one row.
        assert_eq!(input.display_height(40), 1);

        // "hello world" is 11 columns; at width 40 it fits on one row.
        input.insert_str("hello world");
        assert_eq!(input.display_height(40), 1);

        // Force a wrap: "hello world" (11 cols) wraps at word boundaries.
        // xAI's textwrap puts "hello" (5 cols) on row 1 and "world" on row 2.
        assert_eq!(input.display_height(5), 2);

        // An explicit newline always adds a row.
        input.insert_newline();
        assert_eq!(input.display_height(40), 2);

        // Many lines cap at MAX_INPUT_ROWS even when the raw count is larger.
        for _ in 0..50 {
            input.insert_newline();
        }
        assert_eq!(input.display_height(40), InputArea::MAX_INPUT_ROWS);
    }
}
