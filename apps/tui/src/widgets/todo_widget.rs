//! Widget that renders the agent's task plan as a compact checkbox list.
//!
//! Sits between the status bar and the chat area in the agent leaf. When no
//! plan is active (all steps empty or no steps), the widget collapses to zero
//! height and renders nothing.
//!
//! ```text
//! ┌ Task Plan ─────────────────────────── 2/5 ─┐
//! │ ✔ Parse input data                          │
//! │ ▶ Run regression analysis                   │
//! │ ○ Generate report                           │
//! │ ○ Review results                            │
//! │ ○ Clean up                                  │
//! └─────────────────────────────────────────────┘
//! ```

use ratatui::{
    layout::Rect,
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};

use crate::state::PlanState;

/// Widget that renders a [`PlanState`] as a bordered checkbox list.
///
/// Currently superseded by the sidebar's inline plan section, but kept as a
/// standalone widget for potential reuse.
#[allow(dead_code)]
pub struct TodoWidget<'a> {
    pub plan: &'a PlanState,
}

impl Widget for TodoWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.plan.is_empty() {
            return;
        }

        let (done, total) = self.plan.progress().unwrap_or((0, self.plan.steps.len()));

        // ── Build the title with progress ──
        let title = format!(" Task Plan ── {done}/{total} ");

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));

        let inner = block.inner(area);
        block.render(area, buf);

        // If there's not enough room for even one line, bail.
        if inner.height == 0 {
            return;
        }

        // ── Render each step ──
        for (i, step) in self.plan.steps.iter().enumerate() {
            let y = inner.y + i as u16;
            if y >= inner.y + inner.height {
                break;
            }

            let (icon, style) = match step.status {
                agentik_types::StepStatus::Completed => (
                    "✔ ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::DIM | Modifier::CROSSED_OUT),
                ),
                agentik_types::StepStatus::InProgress => (
                    "▶ ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                agentik_types::StepStatus::Pending => ("○ ", Style::default().fg(Color::DarkGray)),
            };

            // Truncate step text to fit within the inner width.
            let icon_width = 2; // "✔ " = 2 chars
            let max_text_len = inner.width.saturating_sub(icon_width) as usize;
            let text = truncate_str(&step.step, max_text_len);

            let line = Line::from(vec![Span::styled(icon, style), Span::styled(text, style)]);
            line.render(
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
}

/// Truncate a string to `max_len` characters, appending `…` if truncated.
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
    use agentik_types::{PlanStep, StepStatus};

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        assert_eq!(truncate_str("hello world", 5), "hell…");
    }

    #[test]
    fn truncate_exact_fit() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn plan_state_progress() {
        let plan = PlanState {
            steps: vec![
                PlanStep {
                    step: "a".into(),
                    status: StepStatus::Completed,
                },
                PlanStep {
                    step: "b".into(),
                    status: StepStatus::InProgress,
                },
                PlanStep {
                    step: "c".into(),
                    status: StepStatus::Pending,
                },
            ],
            explanation: None,
            revision: 1,
        };
        assert_eq!(plan.progress(), Some((1, 3)));
        assert!(!plan.is_empty());
    }

    #[test]
    fn empty_plan_state() {
        let plan = PlanState::default();
        assert!(plan.is_empty());
        assert_eq!(plan.progress(), None);
    }
}
