//! Snapshot history panel — list snapshots for the current workflow.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Widget};
use uuid::Uuid;

use crate::model::SnapshotInfo;

/// Snapshot list for the active workflow.
pub struct HistoryPanelWidget<'a> {
    /// Snapshots ordered newest first.
    pub history: &'a [SnapshotInfo],
    /// Identifier of the snapshot currently selected.
    pub selected: Option<Uuid>,
}

impl<'a> Widget for HistoryPanelWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" snapshot history ");
        let inner = block.inner(area);
        block.render(area, buf);

        let items: Vec<ListItem<'_>> = self
            .history
            .iter()
            .map(|s| {
                let is_current = Some(s.id) == self.selected;
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{} ", if is_current { "▸" } else { " " }),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(
                        format!("{:<10}", short_hash(&s.manifest_hash)),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!(" {}", s.commit_message),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(
                        format!("  {}", s.created_at.format("%Y-%m-%d %H:%M:%S")),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            })
            .collect();

        List::new(items)
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .render(inner, buf);
    }
}

fn short_hash(h: &str) -> String {
    h.chars().take(8).collect()
}
