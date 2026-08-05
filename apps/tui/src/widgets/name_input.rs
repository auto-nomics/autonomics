//! Single-line text input popup for naming a new agent.
//!
//! Tiny popup that prompts the user for a name. The profile picker (or any
//! other spawn flow) opens this popup with a pre-filled default; the user
//! edits and presses Enter to confirm, or Esc to cancel.

use ratatui::{
    layout::{Alignment, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use crate::widgets::popup::Popup;

/// State for the name input popup.
#[derive(Default)]
pub struct NameInputState {
    pub visible: bool,
    pub title: String,
    /// Text being edited.
    pub input: String,
    /// Default value (pre-filled when opened).
    pub default: String,
}

impl NameInputState {
    pub fn open(&mut self, title: impl Into<String>, default: impl Into<String>) {
        self.title = title.into();
        self.default = default.into();
        self.input = self.default.clone();
        self.visible = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn push_char(&mut self, c: char) {
        self.input.push(c);
    }

    pub fn pop_char(&mut self) {
        self.input.pop();
    }

    /// Returns the trimmed input as the final name.
    pub fn value(&self) -> &str {
        self.input.trim()
    }
}

/// Render the name input popup.
pub fn render_name_input(area: Rect, buf: &mut Buffer, state: &NameInputState) {
    if !state.visible {
        return;
    }

    let popup = Popup::new(&state.title).accent(Color::Yellow);
    let inner = popup.render(area, buf);

    // Input line: "  Name: <text><cursor>"
    let line = Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled("Name: ", Style::default().fg(Color::DarkGray)),
        Span::styled(state.input.clone(), Style::default().fg(Color::White)),
        Span::styled(
            "▏",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::SLOW_BLINK),
        ),
    ]);
    let p = Paragraph::new(line);
    Widget::render(p, inner, buf);

    // Footer hint
    if inner.height > 2 {
        let hint_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        let hint = Paragraph::new(Line::from(Span::styled(
            " Enter create  Esc cancel",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )))
        .alignment(Alignment::Center);
        Widget::render(hint, hint_area, buf);
    }
}
