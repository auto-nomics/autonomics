//! Effort quick-picker for the docked agents panel (`E`, bulk path).
//!
//! A compact popup listing the reasoning-effort stops: `inherit (模型默认)`
//! plus `minimal … ultra`. Choosing one targets **every agent passing the
//! panel filter**; the shared `y/n` bulk confirmation popup runs before
//! anything is sent. Non-thinking models cannot take an override — the
//! apply path skips and counts them in the summary toast.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};

use agentik_types::ReasoningEffort;

use crate::widgets::popup::{Popup, PopupControls};

const ACCENT: Color = Color::Yellow;

/// One cyclable stop; `None` = inherit (model default).
pub type EffortStop = Option<ReasoningEffort>;

/// Label for a stop: `inherit ·模型默认` or the effort name.
pub fn effort_stop_label(stop: EffortStop) -> String {
    match stop {
        None => "inherit ·模型默认".into(),
        Some(effort) => effort.as_str().into(),
    }
}

/// The cyclable stops, in cycle order: inherit, then every effort from the
/// lightest to the heaviest. The apply path skips non-thinking models.
pub const EFFORT_STOPS: [EffortStop; 8] = [
    None,
    Some(ReasoningEffort::Minimal),
    Some(ReasoningEffort::Low),
    Some(ReasoningEffort::Medium),
    Some(ReasoningEffort::High),
    Some(ReasoningEffort::Xhigh),
    Some(ReasoningEffort::Max),
    Some(ReasoningEffort::Ultra),
];

/// The next stop after `current` in [`EFFORT_STOPS`], wrapping around.
pub fn next_effort_stop(current: EffortStop) -> EffortStop {
    let pos = EFFORT_STOPS
        .iter()
        .position(|stop| *stop == current)
        .unwrap_or(0);
    EFFORT_STOPS[(pos + 1) % EFFORT_STOPS.len()]
}

// ═══════════════════════════════════════════════════════════════════════
// Key outcome
// ═══════════════════════════════════════════════════════════════════════

/// Result of forwarding a key event to the effort picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEffortPickerKeyOutcome {
    /// The key was consumed but no action is needed.
    Consumed,
    /// The user pressed Enter with a selection — apply `stop` to the bulk
    /// set (the caller runs the shared confirmation popup first).
    Confirm { stop: EffortStop },
    /// The user pressed Esc — close the picker.
    Close,
}

// ═══════════════════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════════════════

/// State for the agents-panel effort quick-picker popup.
#[derive(Default)]
pub struct AgentEffortPickerState {
    pub visible: bool,
    pub selected: usize,
}

impl AgentEffortPickerState {
    pub fn open(&mut self) {
        self.selected = 0;
        self.visible = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    /// Esc → close. Enter → confirm. Up/Down (or j/k) move the cursor.
    pub fn handle_key(&mut self, key: KeyEvent) -> AgentEffortPickerKeyOutcome {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return AgentEffortPickerKeyOutcome::Consumed;
        }
        match key.code {
            KeyCode::Esc => AgentEffortPickerKeyOutcome::Close,
            KeyCode::Enter => AgentEffortPickerKeyOutcome::Confirm {
                stop: EFFORT_STOPS[self.selected.min(EFFORT_STOPS.len() - 1)],
            },
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                AgentEffortPickerKeyOutcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(EFFORT_STOPS.len() - 1);
                AgentEffortPickerKeyOutcome::Consumed
            }
            _ => AgentEffortPickerKeyOutcome::Consumed,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Render
// ═══════════════════════════════════════════════════════════════════════

/// Render the effort quick-picker: a compact centred list of stops.
pub fn render_agent_effort_picker(
    frame_area: Rect,
    buf: &mut Buffer,
    state: &mut AgentEffortPickerState,
) {
    if !state.visible {
        return;
    }

    let height = EFFORT_STOPS.len() as u16 + 2;
    let popup = Popup::new(" Set Effort (bulk) ", PopupControls::default())
        .accent(ACCENT)
        .width(36.min(frame_area.width))
        .height(height.min(frame_area.height));
    let inner = popup.render(frame_area, buf);
    Clear.render(inner, buf);

    for (index, stop) in EFFORT_STOPS.iter().enumerate() {
        let y = inner.y + index as u16;
        if y >= inner.bottom() {
            break;
        }
        let selected = index == state.selected;
        let line = Line::from(vec![
            Span::styled(
                if selected { "› " } else { "  " },
                Style::default().fg(ACCENT),
            ),
            Span::styled(
                effort_stop_label(*stop),
                if selected {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
        ]);
        Paragraph::new(line)
            .style(if selected {
                Style::default().bg(Color::Rgb(35, 55, 70))
            } else {
                Style::default()
            })
            .render(
                Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: 1,
                },
                buf,
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_wraps_through_every_stop() {
        let mut seen = Vec::new();
        let mut current: EffortStop = None;
        for _ in 0..EFFORT_STOPS.len() {
            current = next_effort_stop(current);
            seen.push(current);
        }
        // A full cycle from inherit returns to inherit having visited all
        // other stops.
        assert_eq!(current, None);
        assert_eq!(seen.len(), EFFORT_STOPS.len() - 1 + 1);
        assert!(seen.contains(&Some(ReasoningEffort::High)));
        assert!(seen.contains(&Some(ReasoningEffort::Ultra)));
    }
}
