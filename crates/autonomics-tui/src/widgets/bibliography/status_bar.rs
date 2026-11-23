//! One-line keyboard and library status.

use ratatui::{
    layout::Rect,
    prelude::{Buffer, Widget},
    style::{Color, Style},
    text::{Line, Span},
};
use i18n::t;

pub(super) struct StatusBar<'a> {
    pub focus: &'a str,
    pub selected_id: Option<&'a str>,
    pub status: &'a str,
    pub error: Option<&'a str>,
    pub accent: Color,
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let theme = crate::theme::current();
        let ready = t!("tui.bib.ready").to_string();
        let right: &str = self.error.unwrap_or(if self.status.is_empty() {
            &ready
        } else {
            self.status
        });
        let right_style = if self.error.is_some() {
            Style::new().fg(theme.error)
        } else {
            Style::new().fg(theme.text_faint)
        };

        let selected = self.selected_id.map(str::to_string).unwrap_or_else(|| t!("tui.bib.none").to_string());
        let focus = format!("[{}] ", self.focus);
        let full = t!("tui.bib.helpSelected", selected = selected).to_string();
        // Narrow terminals keep the key hints and drop the verbose wording.
        let fits_full =
            focus.chars().count() + 1 + full.chars().count() + 2 + right.chars().count()
                <= area.width as usize;
        let hints = if fits_full {
            full
        } else {
            t!("tui.bib.helpShort", selected = selected).to_string()
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
