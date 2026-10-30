//! Sidebar showing the agent's task plan and background tasks. Frameless
//! and untitled — no outer top or bottom border rows and no section
//! headers — so the lists keep every row: the first box carries only side
//! rules, and the second tops with a single bare divider line. Populated
//! sections identify themselves through their rows. Each box's rules light
//! up (cyan) only when its own pane holds focus, mirroring the chat pane's
//! left-rule indicator.

use ratatui::{
    layout::{Alignment, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
};

use crate::state::{PlanState, ToolTaskInfo, ToolTaskStatus, WorkspaceFocus};

#[derive(Default)]
pub struct SidebarState {
    pub plan: ListState,
    pub tasks: ListState,
    selected_task_seq: Option<u64>,
}

impl SidebarState {
    /// Keep selection attached to the same task when completed rows disappear.
    pub fn sync(&mut self, plan_len: usize, tasks: &[ToolTaskInfo]) {
        self.plan
            .select((plan_len > 0).then(|| self.plan.selected().unwrap_or(0).min(plan_len - 1)));
        let selected = self
            .selected_task_seq
            .and_then(|seq| tasks.iter().position(|task| task.seq == seq))
            .or_else(|| {
                (!tasks.is_empty()).then(|| self.tasks.selected().unwrap_or(0).min(tasks.len() - 1))
            });
        self.select_task(selected, tasks);
    }

    pub fn select_task(&mut self, selected: Option<usize>, tasks: &[ToolTaskInfo]) {
        let selected = selected.filter(|&index| index < tasks.len());
        self.tasks.select(selected);
        self.selected_task_seq = selected.map(|index| tasks[index].seq);
    }
}

/// Data bundle the sidebar needs from the agent tab state.
pub struct SidebarData<'a> {
    pub plan: &'a PlanState,
    pub tool_tasks: &'a [ToolTaskInfo],
}

/// Fit populated sections to their content. When space is scarce, keep the
/// focused list usable first, then share remaining rows between visible
/// lists. `rows` excludes the inter-section divider.
fn sidebar_areas(area: Rect, focus: WorkspaceFocus, rows: [usize; 2]) -> [Rect; 2] {
    let focused = match focus {
        WorkspaceFocus::Plan => 0,
        _ => 1,
    };
    let mut priority = vec![focused];
    for index in [1, 0] {
        if !priority.contains(&index) {
            priority.push(index);
        }
    }
    let mut heights = [0u16; 2];
    let mut remaining = area.height;
    let mut visible = 0;
    for &index in &priority {
        if rows[index] == 0 || remaining == 0 {
            continue;
        }
        let divider = u16::from(visible > 0);
        if remaining >= 1 + divider {
            heights[index] = 1;
            remaining -= 1 + divider;
            visible += 1;
        } else if visible == 0 {
            // Extremely short windows still expose the focused controls.
            heights[index] = remaining;
            remaining = 0;
            visible += 1;
        }
    }
    while remaining > 0 {
        let mut grew = false;
        for &index in &priority {
            if heights[index] > 0 && usize::from(heights[index]) < rows[index] {
                heights[index] += 1;
                remaining -= 1;
                grew = true;
                if remaining == 0 {
                    break;
                }
            }
        }
        if !grew {
            break;
        }
    }
    let mut y = area.y;
    let mut first = true;
    heights.map(|height| {
        let divider = u16::from(!first && height > 0);
        let section = Rect::new(area.x, y, area.width, height + divider);
        y += section.height;
        first &= height == 0;
        section
    })
}

/// Right-hand sidebar widget showing the plan and background tasks.
pub struct SideBar<'a> {
    pub data: SidebarData<'a>,
    pub focus: WorkspaceFocus,
}

impl StatefulWidget for SideBar<'_> {
    type State = SidebarState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        state.sync(self.data.plan.steps.len(), self.data.tool_tasks);
        let plan_focused = self.focus == WorkspaceFocus::Plan;
        let tasks_focused = self.focus == WorkspaceFocus::Tasks;

        let [plan_area, tasks_area] = sidebar_areas(
            area,
            self.focus,
            [self.data.plan.steps.len(), self.data.tool_tasks.len()],
        );
        let borders = |section: Rect| {
            Borders::LEFT
                | Borders::RIGHT
                | if section.y > area.y {
                    Borders::TOP
                } else {
                    Borders::NONE
                }
        };

        // ── Box 1: plan (side rules only; content starts on the first row) ──
        let plan_block =
            Block::default()
                .borders(borders(plan_area))
                .border_style(Style::default().fg(if plan_focused {
                    Color::Cyan
                } else {
                    Color::DarkGray
                }));
        let plan_inner = plan_block.inner(plan_area);
        plan_block.render(plan_area, buf);
        if plan_inner.height > 0 {
            render_plan_section(
                plan_inner,
                buf,
                self.data.plan,
                &mut state.plan,
                plan_focused,
            );
        }

        // ── Box 2: tasks (bare top divider; open bottom) ──
        let tasks_block =
            Block::default()
                .borders(borders(tasks_area))
                .border_style(Style::default().fg(if tasks_focused {
                    Color::Cyan
                } else {
                    Color::DarkGray
                }));
        let tasks_inner = tasks_block.inner(tasks_area);
        tasks_block.render(tasks_area, buf);
        if tasks_inner.height > 0 {
            render_task_section(
                tasks_inner,
                buf,
                self.data.tool_tasks,
                &mut state.tasks,
                tasks_focused,
            );
        }
    }
}

// ── Section renderers ──────────────────────────────────

/// Render the plan steps list, or a placeholder if empty.
fn render_plan_section(
    area: Rect,
    buf: &mut Buffer,
    plan: &PlanState,
    state: &mut ListState,
    focused: bool,
) {
    if area.height == 0 {
        return;
    }
    if plan.is_empty() {
        render_placeholder(area, buf, "(no plan)");
        return;
    }
    let rows = plan
        .steps
        .iter()
        .map(|step| ListItem::new(plan_step_line(step, area.width.saturating_sub(2))));
    StatefulWidget::render(
        List::new(rows)
            .highlight_symbol(if focused { "› " } else { "  " })
            .highlight_style(selection_style(focused)),
        area,
        buf,
        state,
    );
}

/// Render the background tasks list, or a placeholder if empty.
fn render_task_section(
    area: Rect,
    buf: &mut Buffer,
    tasks: &[ToolTaskInfo],
    state: &mut ListState,
    focused: bool,
) {
    if area.height == 0 {
        return;
    }
    if tasks.is_empty() {
        render_placeholder(area, buf, "(no tasks)");
        return;
    }
    let rows = tasks
        .iter()
        .map(|task| ListItem::new(task_line(task, area.width.saturating_sub(2))));
    StatefulWidget::render(
        List::new(rows)
            .highlight_symbol(if focused { "› " } else { "  " })
            .highlight_style(selection_style(focused)),
        area,
        buf,
        state,
    );
}

fn selection_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .bg(Color::Rgb(35, 55, 70))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

// ── Row renderers ──────────────────────────────────────

/// Render a single plan step row.
fn plan_step_line(step: &agentik_types::PlanStep, width: u16) -> Line<'static> {
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

    let icon_width = 2;
    let max_text_len = width.saturating_sub(icon_width) as usize;
    let text = truncate_str(&step.step, max_text_len);

    Line::from(vec![Span::styled(icon, style), Span::styled(text, style)])
}

/// Render a single background task row.
fn task_line(task: &ToolTaskInfo, width: u16) -> Line<'static> {
    let (icon, color) = match &task.status {
        ToolTaskStatus::Running => ("⏳", Color::Yellow),
        ToolTaskStatus::Done { ok: true } => ("✓", Color::Green),
        ToolTaskStatus::Done { ok: false } => ("✗", Color::Red),
    };

    let icon_width = 3; // "⏳ " = 3 cells (hourglass renders double-width)
    let max_text_len = width.saturating_sub(icon_width) as usize;
    let label = match &task.target_agent {
        Some(target) if task.name == "delegate_to" => format!("#{} → {}", task.seq, target),
        Some(target) => format!("#{} {} → {}", task.seq, task.name, target),
        None => format!("#{} {}", task.seq, task.name),
    };
    let text = truncate_str(&label, max_text_len);

    Line::from(vec![
        Span::styled(format!("{icon} "), Style::default().fg(color)),
        Span::styled(
            text,
            Style::default()
                .fg(Color::Rgb(200, 200, 200))
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Render a muted placeholder centered in the section body.
fn render_placeholder(area: Rect, buf: &mut Buffer, text: &str) {
    let line = Line::from(Span::styled(
        text.to_string(),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM | Modifier::ITALIC),
    ));
    Paragraph::new(line)
        .alignment(Alignment::Center)
        .render(area, buf);
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

    fn task(seq: u64) -> ToolTaskInfo {
        ToolTaskInfo {
            seq,
            name: "delegate_to".into(),
            status: ToolTaskStatus::Running,
            target_agent: Some(format!("worker-{seq}")),
        }
    }

    #[test]
    fn task_rows_name_delegation_targets() {
        let line = task_line(&task(7), 40);
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.to_string())
            .collect();
        assert!(text.contains("#7 → worker-7"), "got: {text}");
    }

    #[test]
    fn task_selection_survives_completed_rows_disappearing() {
        let mut state = SidebarState::default();
        let tasks = vec![task(1), task(2), task(3)];
        state.sync(0, &tasks);
        state.select_task(Some(2), &tasks);
        // The selected task completes and drains away; selection reattaches.
        let remaining = vec![task(1)];
        state.sync(0, &remaining);
        assert_eq!(state.tasks.selected(), Some(0));
    }
}
