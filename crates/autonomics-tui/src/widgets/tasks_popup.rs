//! Tasks popup (`<leader>T`): the agent's background tool tasks as a
//! floating list — the former sidebar tasks pane, promoted to a popup so
//! the chat pane keeps the full width. Rows carry the status glyph and the
//! delegation target (`#3 → worker`); Enter on a `delegate_to` row jumps
//! to that child agent's leaf, preserving the pane's old navigation.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};

use crate::state::ToolTaskInfo;

const ACCENT: Color = Color::Yellow;

/// Result of forwarding a key event to the tasks popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TasksPopupKeyOutcome {
    /// The key was consumed but no action is needed.
    Consumed,
    /// Jump to this delegated agent's leaf (an agent path).
    Jump(String),
    /// Close the popup.
    Close,
}

#[derive(Default)]
pub struct TasksPopupState {
    pub visible: bool,
    selected: usize,
}

impl TasksPopupState {
    pub fn open(&mut self) {
        self.selected = 0;
        self.visible = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    /// Esc closes; ↑↓/j/k/Home/End move the cursor; Enter jumps when the
    /// selected row is a delegation with a live target.
    pub fn handle_key(&mut self, key: KeyEvent, tasks: &[ToolTaskInfo]) -> TasksPopupKeyOutcome {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return TasksPopupKeyOutcome::Consumed;
        }
        let count = tasks.len();
        let selected = self.selected.min(count.saturating_sub(1));
        match key.code {
            KeyCode::Esc => TasksPopupKeyOutcome::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = selected.saturating_sub(1);
                TasksPopupKeyOutcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if count > 0 {
                    self.selected = (selected + 1).min(count - 1);
                }
                TasksPopupKeyOutcome::Consumed
            }
            KeyCode::Home => {
                self.selected = 0;
                TasksPopupKeyOutcome::Consumed
            }
            KeyCode::End => {
                self.selected = count.saturating_sub(1);
                TasksPopupKeyOutcome::Consumed
            }
            KeyCode::Enter => match tasks.get(selected) {
                Some(task) if task.name == "delegate_to" && task.target_agent.is_some() => {
                    TasksPopupKeyOutcome::Jump(task.target_agent.clone().expect("checked above"))
                }
                _ => TasksPopupKeyOutcome::Consumed,
            },
            _ => TasksPopupKeyOutcome::Consumed,
        }
    }
}

/// Render the tasks popup: a centred framed list of background tasks.
pub fn render_tasks_popup(
    frame_area: Rect,
    buf: &mut Buffer,
    state: &mut TasksPopupState,
    tasks: &[ToolTaskInfo],
) {
    if !state.visible {
        return;
    }

    let rows = tasks.len().max(1) as u16;
    let height = (rows + 2).min(frame_area.height.saturating_sub(2)).max(3);
    let width = (frame_area.width * 7 / 10).max(40).min(frame_area.width);
    let x = frame_area.x + (frame_area.width - width) / 2;
    let y = frame_area.y + (frame_area.height - height) / 3;
    let popup = Rect::new(x, y, width, height);

    Clear.render(popup, buf);
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .title(Span::styled(
            " Tasks ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(popup);
    block.render(popup, buf);

    if tasks.is_empty() {
        Paragraph::new("(no background tasks)")
            .style(Style::default().fg(Color::DarkGray))
            .centered()
            .render(inner, buf);
        return;
    }

    for (index, task) in tasks.iter().enumerate() {
        let line_y = inner.y + index as u16;
        if line_y >= inner.bottom() {
            break;
        }
        let selected = index == state.selected.min(tasks.len() - 1);
        let (icon, color) = match task.status {
            crate::state::ToolTaskStatus::Running => ("⏳", Color::Yellow),
            crate::state::ToolTaskStatus::Done { ok: true } => ("✓", Color::Green),
            crate::state::ToolTaskStatus::Done { ok: false } => ("✗", Color::Red),
        };
        let label = match &task.target_agent {
            Some(target) if task.name == "delegate_to" => format!("#{} → {}", task.seq, target),
            Some(target) => format!("#{} {} → {}", task.seq, task.name, target),
            None => format!("#{} {}", task.seq, task.name),
        };
        let text = truncate_str(&label, inner.width.saturating_sub(5) as usize);
        Paragraph::new(Line::from(vec![
            Span::raw(if selected { "› " } else { "  " }),
            Span::styled(format!("{icon} "), Style::default().fg(color)),
            Span::styled(
                text,
                Style::default()
                    .fg(Color::Rgb(200, 200, 200))
                    .add_modifier(Modifier::BOLD),
            ),
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
    use crate::state::ToolTaskStatus;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn task(seq: u64, name: &str, target: Option<&str>) -> ToolTaskInfo {
        ToolTaskInfo {
            seq,
            name: name.to_string(),
            status: ToolTaskStatus::Running,
            target_agent: target.map(str::to_string),
        }
    }

    #[test]
    fn enter_on_a_delegation_jumps_to_its_target() {
        let mut state = TasksPopupState::default();
        state.open();
        let tasks = vec![
            task(1, "run_dag", None),
            task(2, "delegate_to", Some("worker-a")),
        ];
        state.handle_key(key(KeyCode::Down), &tasks);
        assert_eq!(
            state.handle_key(key(KeyCode::Enter), &tasks),
            TasksPopupKeyOutcome::Jump("worker-a".into())
        );
        // Non-delegation rows consume Enter without jumping.
        state.handle_key(key(KeyCode::Up), &tasks);
        assert_eq!(
            state.handle_key(key(KeyCode::Enter), &tasks),
            TasksPopupKeyOutcome::Consumed
        );
    }

    #[test]
    fn esc_closes_and_empty_lists_clamp() {
        let mut state = TasksPopupState::default();
        state.open();
        assert_eq!(
            state.handle_key(key(KeyCode::Esc), &[]),
            TasksPopupKeyOutcome::Close
        );
        // Moving in an empty list is a harmless no-op.
        assert_eq!(
            state.handle_key(key(KeyCode::Down), &[]),
            TasksPopupKeyOutcome::Consumed
        );
    }
}
