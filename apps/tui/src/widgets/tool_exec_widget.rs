use ratatui::{
    prelude::Widget,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::Block,
};

use crate::state::{ToolTaskInfo, ToolTaskStatus};

/// Compact panel showing background tool execution tasks.
///
/// Currently superseded by the sidebar's inline task section, but kept as a
/// standalone widget for potential reuse (e.g. a dedicated tools tab).
#[allow(dead_code)]
pub(crate) struct ToolExecWidget<'a> {
    pub tasks: &'a [ToolTaskInfo],
}

impl Widget for ToolExecWidget<'_> {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer) {
        if self.tasks.is_empty() {
            return;
        }

        let block = Block::bordered().title(" Tasks ");
        let inner = block.inner(area);
        block.render(area, buf);

        for (i, task) in self.tasks.iter().enumerate() {
            let row = inner.top().saturating_add(i as u16);
            if row >= inner.bottom() {
                break;
            }

            let (icon, color) = match &task.status {
                ToolTaskStatus::Running => ("⏳", Color::Yellow),
                ToolTaskStatus::Done { ok: true } => ("✓", Color::Green),
                ToolTaskStatus::Done { ok: false } => ("✗", Color::Red),
            };

            let line = Line::from(vec![
                ratatui::text::Span::styled(format!(" {icon} "), Style::default().fg(color)),
                ratatui::text::Span::styled(
                    task.name.clone(),
                    Style::default()
                        .fg(Color::Rgb(200, 200, 200))
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            line.render(
                ratatui::layout::Rect::new(inner.left(), row, inner.width, 1),
                buf,
            );
        }
    }
}
