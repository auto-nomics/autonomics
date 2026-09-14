//! Single-line text input popup for naming a new agent.
//!
//! Tiny popup that prompts the user for a name. The profile picker (or any
//! other spawn flow) opens this popup with a pre-filled default; the user
//! edits and presses Enter to confirm, or Esc to cancel.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, StatefulWidgetRef, Widget},
};

use crate::xai_textarea::{TextArea, TextAreaState};

use crate::widgets::popup::{Popup, PopupControls};

/// Compact popup geometry: large enough for a one-line editor and hint,
/// but visibly lighter than the full-sized pickers.
const POPUP_WIDTH: u16 = 56;
const POPUP_HEIGHT: u16 = 7;
const INPUT_WIDTH: u16 = 44;

/// Result of routing a key to the name editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameInputKeyOutcome {
    /// Confirm the current value.
    Submit,
    /// Cancel and close the popup.
    Cancel,
    /// The key was consumed by the editor or intentionally ignored.
    Handled,
}

/// State for the name input popup.
pub struct NameInputState {
    pub visible: bool,
    pub title: String,
    /// Text being edited.
    pub textarea: TextArea,
    /// Render state for the embedded single-line editor.
    pub textarea_state: TextAreaState,
    /// Default value (pre-filled when opened).
    pub default: String,
    /// On-screen cursor position computed during rendering.
    pub cursor_pos: Option<(u16, u16)>,
}

impl NameInputState {
    fn new() -> Self {
        let mut textarea = TextArea::new();
        textarea.show_scrollbar = false;
        Self {
            visible: false,
            title: String::new(),
            textarea,
            textarea_state: TextAreaState::default(),
            default: String::new(),
            cursor_pos: None,
        }
    }

    pub fn open(&mut self, title: impl Into<String>, default: impl Into<String>) {
        self.title = title.into();
        self.default = default.into();
        self.textarea.set_text(&self.default);
        self.textarea.set_cursor(self.default.len());
        self.textarea.clear_history();
        self.textarea_state = TextAreaState::default();
        self.cursor_pos = None;
        self.visible = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.cursor_pos = None;
    }

    /// Insert terminal paste while keeping the name field single-line.
    pub fn paste(&mut self, text: &str) {
        let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
        self.textarea.insert_str(&normalized);
    }

    /// Route keys through [`TextArea`] while keeping the field single-line.
    pub fn handle_key(&mut self, key: KeyEvent) -> NameInputKeyOutcome {
        match key.code {
            KeyCode::Esc => NameInputKeyOutcome::Cancel,
            KeyCode::Enter => NameInputKeyOutcome::Submit,
            // These are Enter aliases in TextArea; keep the popup modal
            // single-line and submit only on the regular Enter key.
            KeyCode::Char('j' | 'm') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                NameInputKeyOutcome::Handled
            }
            // Vertical movement is meaningless in a single-line field.
            KeyCode::Up | KeyCode::Down => NameInputKeyOutcome::Handled,
            _ => {
                self.textarea.input(key);
                NameInputKeyOutcome::Handled
            }
        }
    }

    /// Returns the trimmed input as the final name.
    pub fn value(&self) -> &str {
        self.textarea.text().trim()
    }
}

impl Default for NameInputState {
    fn default() -> Self {
        Self::new()
    }
}

/// Render the name input popup.
pub fn render_name_input(area: Rect, buf: &mut Buffer, state: &mut NameInputState) {
    if !state.visible {
        return;
    }

    let popup = Popup::new(&state.title, PopupControls::default())
        .accent(Color::Yellow)
        .width(POPUP_WIDTH)
        .height(POPUP_HEIGHT)
        .vertically_centered(true);
    let inner = popup.render(area, buf);

    let [label_area, editor_row, _spacer, hint_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(inner);

    Paragraph::new(Line::from(Span::styled(
        "Name",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center)
    .render(label_area, buf);

    let input_width = INPUT_WIDTH.min(editor_row.width);
    let editor_area = Rect {
        x: editor_row.x + editor_row.width.saturating_sub(input_width) / 2,
        y: editor_row.y,
        width: input_width,
        height: 1,
        ..editor_row
    };

    if state.textarea.is_empty() {
        let placeholder: String = "Enter a name…"
            .chars()
            .take(editor_area.width as usize)
            .collect();
        buf.set_string(
            editor_area.x,
            editor_area.y,
            placeholder,
            Style::default().fg(Color::DarkGray),
        );
        state.cursor_pos = Some((editor_area.x, editor_area.y));
    } else {
        let textarea: &TextArea = &state.textarea;
        let textarea_state: &mut TextAreaState = &mut state.textarea_state;
        textarea.render_ref(editor_area, buf, textarea_state);
        state.cursor_pos = state
            .textarea
            .cursor_pos_with_state(editor_area, state.textarea_state);
    }

    // Footer hint
    Paragraph::new(Line::from(Span::styled(
        " Enter create  Esc cancel",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    )))
    .alignment(Alignment::Center)
    .render(hint_area, buf);
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn textarea_backed_input_supports_editing_and_trim() {
        let mut state = NameInputState::default();
        state.open(" New Agent ", "worker");
        state.handle_key(key(KeyCode::Left, KeyModifiers::NONE));
        state.handle_key(key(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(state.value(), "workexr");
        state.handle_key(key(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(state.value(), "worker");
    }

    #[test]
    fn enter_and_vertical_motion_do_not_create_lines() {
        let mut state = NameInputState::default();
        state.open(" New Agent ", "agent");
        state.handle_key(key(KeyCode::Enter, KeyModifiers::NONE));
        state.handle_key(key(KeyCode::Char('j'), KeyModifiers::CONTROL));
        state.handle_key(key(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(state.value(), "agent");
    }

    #[test]
    fn paste_is_flattened_to_one_line() {
        let mut state = NameInputState::default();
        state.open(" New Agent ", "");
        state.paste("first\r\nsecond");

        assert_eq!(state.value(), "first second");
    }

    #[test]
    fn renders_centered_editor_and_exposes_cursor() {
        let mut state = NameInputState::default();
        state.open(" New Agent ", "worker");
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

        terminal
            .draw(|frame| {
                render_name_input(frame.area(), frame.buffer_mut(), &mut state);
            })
            .unwrap();

        let cursor = state.cursor_pos.expect("cursor position after render");
        assert_eq!(cursor, (24, 10));
    }
}
