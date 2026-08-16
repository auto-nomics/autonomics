//! Help overlay (`?` key) — shows the keymap.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

/// Full-screen key-binding help overlay.
pub struct HelpOverlayWidget;

impl Widget for HelpOverlayWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" help (Esc / q to close) ");
        let inner = block.inner(area);
        block.render(area, buf);

        let mut lines = vec![
            Line::from(Span::styled(
                "Navigation",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(" hjkl / arrows  pan canvas"),
            Line::from(" +/-           zoom"),
            Line::from(" [ / ]         select prev / next node"),
            Line::from(" Tab / BackTab  focus inspector / palette"),
            Line::from(""),
            Line::from(Span::styled(
                "Editing",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(" n             open node picker"),
            Line::from(" e             edit selected node (spec or SOP)"),
            Line::from(" d / Delete    delete selected node or edge"),
            Line::from(" space         start drawing an edge"),
            Line::from(" s             edit workflow-level SOP"),
            Line::from(""),
            Line::from(Span::styled(
                "System",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(" :             open command palette"),
            Line::from(" Ctrl+S        save"),
            Line::from(" Ctrl+R        run"),
            Line::from(" Ctrl+Z / Y    undo / redo"),
            Line::from(" h             snapshot history"),
            Line::from(" ?             this help"),
            Line::from(" Ctrl+C / q    quit"),
        ];

        Paragraph::new(std::mem::take(&mut lines))
            .wrap(Wrap { trim: false })
            .render(inner, buf);
    }
}
