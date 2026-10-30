//! Plan popup (`<leader>p`): the agent's current task plan as a floating
//! list — the former sidebar plan pane, promoted to a popup so the chat
//! pane keeps the full width. Steps render with their status glyph
//! (✔ completed / ▶ in progress / ○ pending); navigation is j/k or
//! arrows; Esc closes.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};

use crate::state::PlanState;

const ACCENT: Color = Color::Cyan;

/// Result of forwarding a key event to the plan popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanPopupKeyOutcome {
    /// The key was consumed but no action is needed.
    Consumed,
    /// Close the popup.
    Close,
}

#[derive(Default)]
pub struct PlanPopupState {
    pub visible: bool,
    selected: usize,
}

impl PlanPopupState {
    pub fn open(&mut self) {
        self.selected = 0;
        self.visible = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    /// Esc closes; ↑↓/j/k/Home/End move the cursor (clamped to the live
    /// step count, so a plan updated mid-flight cannot strand it).
    pub fn handle_key(&mut self, key: KeyEvent, steps: usize) -> PlanPopupKeyOutcome {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return PlanPopupKeyOutcome::Consumed;
        }
        match key.code {
            KeyCode::Esc => PlanPopupKeyOutcome::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.min(steps.saturating_sub(1)).saturating_sub(1);
                PlanPopupKeyOutcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if steps > 0 {
                    self.selected = (self.selected + 1).min(steps - 1);
                }
                PlanPopupKeyOutcome::Consumed
            }
            KeyCode::Home => {
                self.selected = 0;
                PlanPopupKeyOutcome::Consumed
            }
            KeyCode::End => {
                self.selected = steps.saturating_sub(1);
                PlanPopupKeyOutcome::Consumed
            }
            _ => PlanPopupKeyOutcome::Consumed,
        }
    }
}

/// Render the plan popup: a centred framed list of plan steps with the
/// revision in the title.
pub fn render_plan_popup(
    frame_area: Rect,
    buf: &mut Buffer,
    state: &mut PlanPopupState,
    plan: &PlanState,
) {
    if !state.visible {
        return;
    }

    let rows = plan.steps.len().max(1) as u16;
    let height = (rows + 2).min(frame_area.height.saturating_sub(2)).max(3);
    let width = (frame_area.width * 7 / 10).max(40).min(frame_area.width);
    let x = frame_area.x + (frame_area.width - width) / 2;
    let y = frame_area.y + (frame_area.height - height) / 3;
    let popup = Rect::new(x, y, width, height);

    Clear.render(popup, buf);
    let title = if plan.revision > 0 {
        format!(" Plan · r{} ", plan.revision)
    } else {
        " Plan ".to_string()
    };
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .title(Span::styled(
            title,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(popup);
    block.render(popup, buf);

    if plan.is_empty() {
        Paragraph::new("(no active plan)")
            .style(Style::default().fg(Color::DarkGray))
            .centered()
            .render(inner, buf);
        return;
    }

    for (index, step) in plan.steps.iter().enumerate() {
        let line_y = inner.y + index as u16;
        if line_y >= inner.bottom() {
            break;
        }
        let selected = index == state.selected.min(plan.steps.len() - 1);
        let (icon, style) = step_style(step);
        let text = truncate_str(&step.step, inner.width.saturating_sub(4) as usize);
        Paragraph::new(Line::from(vec![
            Span::raw(if selected { "› " } else { "  " }),
            Span::styled(icon, style),
            Span::raw(" "),
            Span::styled(text, style),
        ]))
        .style(if selected {
            Style::default().bg(Color::Rgb(35, 55, 70))
        } else {
            Style::default()
        })
        .render(
            Rect {
                x: inner.x,
                y: line_y,
                width: inner.width,
                height: 1,
            },
            buf,
        );
    }
}

fn step_style(step: &agentik_types::PlanStep) -> (&'static str, Style) {
    match step.status {
        agentik_types::StepStatus::Completed => (
            "✔",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::DIM | Modifier::CROSSED_OUT),
        ),
        agentik_types::StepStatus::InProgress => (
            "▶",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        agentik_types::StepStatus::Pending => ("○", Style::default().fg(Color::DarkGray)),
    }
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else if max_len == 0 {
        String::new()
    } else {
        let truncated: String = s.chars().take(max_len - 1).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn navigation_clamps_to_the_live_step_count() {
        let mut state = PlanPopupState::default();
        state.open();
        // Plan shrank to 2 steps while the popup was open; End clamps, Up
        // still moves within bounds.
        assert_eq!(
            state.handle_key(key(KeyCode::End), 2),
            PlanPopupKeyOutcome::Consumed
        );
        assert_eq!(state.selected, 1);
        state.handle_key(key(KeyCode::Up), 2);
        assert_eq!(state.selected, 0);
        // Down cannot leave a single-step plan.
        state.handle_key(key(KeyCode::Down), 1);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn esc_closes_and_chords_are_consumed() {
        let mut state = PlanPopupState::default();
        state.open();
        assert_eq!(
            state.handle_key(key(KeyCode::Esc), 0),
            PlanPopupKeyOutcome::Close
        );
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL), 0),
            PlanPopupKeyOutcome::Consumed
        );
    }
}
