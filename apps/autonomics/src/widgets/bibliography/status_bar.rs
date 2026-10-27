//! One-line keyboard and library status.

use ratatui::{
    layout::Rect,
    prelude::{Buffer, Widget},
    style::{Color, Style},
    text::{Line, Span},
};

pub(super) struct StatusBar<'a> {
    pub focus: &'a str,
    pub selected_id: Option<&'a str>,
    pub status: &'a str,
    pub error: Option<&'a str>,
    pub accent: Color,
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let right = self.error.unwrap_or(if self.status.is_empty() {
            "ready"
        } else {
            self.status
        });
        let right_style = if self.error.is_some() {
            Style::new().fg(Color::Red)
        } else {
            Style::new().fg(Color::DarkGray)
        };

        let selected = self.selected_id.unwrap_or("none");
        let focus = format!("[{}] ", self.focus);
        let full = format!(
            "Tab switch · / search · j/k move · h/l tabs/tree · R descendants · r reload · q quit · selected {selected}"
        );
        // Narrow terminals keep the key hints and drop the verbose wording.
        let fits_full =
            focus.chars().count() + 1 + full.chars().count() + 2 + right.chars().count()
                <= area.width as usize;
        let hints = if fits_full {
            full
        } else {
            format!("Tab / j/k h/l · R descendants · r reload · q quit · {selected}")
        };

        let left_len = focus.chars().count() + 1 + hints.chars().count() + 2;
        let available = area.width.saturating_sub(left_len as u16).saturating_sub(2);
        let right: String = right.chars().take(available.max(3) as usize).collect();
        Widget::render(
            ratatui::widgets::Paragraph::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(focus, Style::new().fg(self.accent)),
                Span::raw(hints),
                Span::styled(format!("  {right}"), right_style),
            ])),
            area,
            buf,
        );
    }
}
