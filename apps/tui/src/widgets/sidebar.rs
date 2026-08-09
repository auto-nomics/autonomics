//! Right-hand sidebar for the agent leaf — consolidates the task plan,
//! session info, and background tool tasks into one persistent column.
//!
//! The sidebar is always divided into **three equal-height sections**:
//! Task Plan, Sessions, and Tasks. Each section gets exactly ⅓ of the inner
//! height, regardless of content. Empty sections show a muted placeholder.
//!
//! ```text
//! ┌ Sidebar ────────────┐
//! │ TASK PLAN    2/5    │
//! │ ✔ Parse data        │
//! │ ▶ Regression        │
//! │ ○ Report            │
//! │ ○ Review            │
//! ├─────────────────────┤
//! │ SESSIONS      [2/3] │
//! │ 💬 GWAS analysis    │
//! │                     │
//! │                     │
//! ├─────────────────────┤
//! │ TASKS               │
//! │ ⏳ run_dag          │
//! │                     │
//! └─────────────────────┘
//! ```

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::state::{PlanState, ToolTaskInfo, ToolTaskStatus};
// Re-export SessionSummary so callers (agent_leaf, agent_workspace, app) can
// construct SidebarData without a separate import from session_list.
use crate::widgets::session_list::SessionList;
pub use crate::widgets::session_list::SessionSummary;

/// Data bundle the sidebar needs from the agent tab state.
pub struct SidebarData<'a> {
    pub plan: &'a PlanState,
    pub tool_tasks: &'a [ToolTaskInfo],
    pub sessions: &'a [SessionSummary],
}

/// Right-hand sidebar widget showing plan, sessions, and tasks.
///
/// Always renders three sections of equal height (⅓ each), separated by
/// divider lines. Empty sections show a muted "(none)" placeholder.
pub struct SideBar<'a> {
    pub data: SidebarData<'a>,
}

impl Widget for SideBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Outer border with a title.
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " Sidebar ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width < 8 || inner.height < 6 {
            return;
        }

        // ── Three equal-height sections with dividers between them ──
        //
        // Layout: [ section_0 | divider(1) | section_1 | divider(1) | section_2 ]
        // Each section gets (inner.height - 2) / 3 rows.
        let dividers = 2u16;
        let section_h = inner.height.saturating_sub(dividers) / 3;

        let rects = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(section_h), // Plan
                Constraint::Length(1),         // Divider
                Constraint::Length(section_h), // Sessions
                Constraint::Length(1),         // Divider
                Constraint::Min(0),            // Tasks (gets remainder)
            ])
            .split(inner);

        let plan_area = rects[0];
        let div0 = rects[1];
        let session_area = rects[2];
        let div1 = rects[3];
        let task_area = rects[4];

        // ── Section 1: Task Plan ──
        render_section_header(plan_area, buf, "TASK PLAN", self.data.plan.progress());
        let plan_body = Rect {
            y: plan_area.y + 1,
            height: plan_area.height.saturating_sub(1),
            ..plan_area
        };
        render_plan_section(plan_body, buf, self.data.plan);

        // ── Divider ──
        render_divider(div0, buf);

        // ── Section 2: Sessions ──
        render_section_header(session_area, buf, "SESSIONS", None);
        let session_body = Rect {
            y: session_area.y + 1,
            height: session_area.height.saturating_sub(1),
            ..session_area
        };
        SessionList {
            sessions: self.data.sessions,
        }
        .render(session_body, buf);

        // ── Divider ──
        render_divider(div1, buf);

        // ── Section 3: Tasks ──
        render_section_header(task_area, buf, "TASKS", None);
        let task_body = Rect {
            y: task_area.y + 1,
            height: task_area.height.saturating_sub(1),
            ..task_area
        };
        render_task_section(task_body, buf, self.data.tool_tasks);
    }
}

// ── Section renderers ──────────────────────────────────

/// Render the plan steps list, or a placeholder if empty.
fn render_plan_section(area: Rect, buf: &mut Buffer, plan: &PlanState) {
    if area.height == 0 {
        return;
    }
    if plan.is_empty() {
        render_placeholder(area, buf, "(no plan)");
        return;
    }
    for (i, step) in plan.steps.iter().enumerate() {
        let y = area.y + i as u16;
        if y >= area.bottom() {
            break;
        }
        render_plan_step(
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            },
            buf,
            step,
        );
    }
}

/// Render the background tasks list, or a placeholder if empty.
fn render_task_section(area: Rect, buf: &mut Buffer, tasks: &[ToolTaskInfo]) {
    if area.height == 0 {
        return;
    }
    if tasks.is_empty() {
        render_placeholder(area, buf, "(no tasks)");
        return;
    }
    for (i, task) in tasks.iter().enumerate() {
        let y = area.y + i as u16;
        if y >= area.bottom() {
            break;
        }
        render_task_row(
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            },
            buf,
            task,
        );
    }
}

// ── Row renderers ──────────────────────────────────────

/// Render a section header line: ` LABEL ` left, optional `N/M` right.
fn render_section_header(
    area: Rect,
    buf: &mut Buffer,
    label: &str,
    progress: Option<(usize, usize)>,
) {
    let mut spans = vec![Span::styled(
        format!(" {label} "),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )];

    if let Some((done, total)) = progress {
        let text = format!("{done}/{total} ");
        let used: usize = label.len() + 2 + text.len();
        let gap = (area.width as usize).saturating_sub(used);
        if gap > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
        }
        spans.push(Span::styled(text, Style::default().fg(Color::DarkGray)));
    }

    let line = Line::from(spans);
    Paragraph::new(line).render(
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        },
        buf,
    );
}

/// Render a horizontal divider line (`─` repeated).
fn render_divider(area: Rect, buf: &mut Buffer) {
    if area.width == 0 {
        return;
    }
    let style = Style::default().fg(Color::DarkGray);
    for x in area.x..area.x + area.width {
        buf.set_string(x, area.y, "─", style);
    }
}

/// Render a single plan step row.
fn render_plan_step(area: Rect, buf: &mut Buffer, step: &agentik_types::PlanStep) {
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
    let max_text_len = area.width.saturating_sub(icon_width) as usize;
    let text = truncate_str(&step.step, max_text_len);

    let line = Line::from(vec![Span::styled(icon, style), Span::styled(text, style)]);
    line.render(area, buf);
}

/// Render a single background task row.
fn render_task_row(area: Rect, buf: &mut Buffer, task: &ToolTaskInfo) {
    let (icon, color) = match &task.status {
        ToolTaskStatus::Running => ("⏳", Color::Yellow),
        ToolTaskStatus::Done { ok: true } => ("✓", Color::Green),
        ToolTaskStatus::Done { ok: false } => ("✗", Color::Red),
    };

    let icon_width = 3; // "⏳ " = 3 chars
    let max_text_len = area.width.saturating_sub(icon_width) as usize;
    let text = truncate_str(&format!("#{} {}", task.seq, task.name), max_text_len);

    let line = Line::from(vec![
        Span::styled(format!(" {icon} "), Style::default().fg(color)),
        Span::styled(
            text,
            Style::default()
                .fg(Color::Rgb(200, 200, 200))
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    line.render(area, buf);
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

    #[test]
    fn truncate_short() {
        assert_eq!(truncate_str("hi", 10), "hi");
    }

    #[test]
    fn truncate_long() {
        assert_eq!(truncate_str("hello world", 5), "hell…");
    }

    #[test]
    fn truncate_zero() {
        assert_eq!(truncate_str("hello", 0), "");
    }
}
