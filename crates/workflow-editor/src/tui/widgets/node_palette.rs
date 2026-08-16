//! Left-pane node palette — list every registered `NodeFactory` grouped by
//! category, with a focus highlight when the palette has focus.

use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Widget};

use crate::registry::NodeRegistry;

/// Node palette widget.
pub struct NodePaletteWidget<'a> {
    /// Registry providing available node kinds.
    pub registry: &'a Arc<NodeRegistry>,
    /// Whether the palette currently owns keyboard focus.
    pub focus: bool,
}

impl<'a> Widget for NodePaletteWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let kinds = self.registry.list();
        let mut items: Vec<ListItem<'_>> = Vec::new();
        let mut last_cat = String::new();
        for k in &kinds {
            if k.category != last_cat {
                if !last_cat.is_empty() {
                    items.push(ListItem::new(Line::from("")));
                }
                items.push(ListItem::new(Line::from(Span::styled(
                    format!("▾ {}", k.category),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ))));
                last_cat = k.category.clone();
            }
            items.push(ListItem::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{:<14}", truncate(&k.kind, 14)),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    format!(" {}", truncate(&k.label, 12)),
                    Style::default().fg(Color::Gray),
                ),
            ])));
        }

        let border_style = if self.focus {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let list = List::new(items).highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
        // Render via Widget but apply border by re-buf-ing — list widget doesn't
        // take borders, so we draw a top border manually.
        list.render(area, buf);
        if area.height > 0 && area.width > 0 {
            buf[(area.left(), area.top())]
                .set_symbol("┌")
                .set_style(border_style);
            buf[(area.right() - 1, area.top())]
                .set_symbol("┐")
                .set_style(border_style);
            for y in area.top() + 1..area.bottom() - 1 {
                buf[(area.left(), y)]
                    .set_symbol("│")
                    .set_style(border_style);
                buf[(area.right() - 1, y)]
                    .set_symbol("│")
                    .set_style(border_style);
            }
            if area.bottom() > area.top() {
                buf[(area.left(), area.bottom() - 1)]
                    .set_symbol("└")
                    .set_style(border_style);
                buf[(area.right() - 1, area.bottom() - 1)]
                    .set_symbol("┘")
                    .set_style(border_style);
            }
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    let cs: Vec<char> = s.chars().collect();
    if cs.len() <= max {
        s.to_string()
    } else {
        let mut out: String = cs.into_iter().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
