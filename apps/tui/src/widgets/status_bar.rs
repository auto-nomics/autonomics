use ratatui::{
    layout::Rect,
    prelude::Widget,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

use crate::state::AgentStatus;

/// 1-row status bar showing agent state, token counts, and active model.
///
/// Status + tokens are left-aligned; the model name is right-aligned on the
/// same row so both fit in a single line.
pub struct StatusBar<'a> {
    pub status: &'a AgentStatus,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub model_name: Option<&'a str>,
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer) {
        let (indicator, indicator_color) = match self.status {
            AgentStatus::Idle => ("○", Color::Gray),
            AgentStatus::Requesting => ("◐", Color::Yellow),
            AgentStatus::Streaming => ("●", Color::Green),
            AgentStatus::Error => ("✗", Color::Red),
        };

        let status_text = match self.status {
            AgentStatus::Idle => "idle",
            AgentStatus::Requesting => "requesting",
            AgentStatus::Streaming => "streaming",
            AgentStatus::Error => "error",
        };

        let in_tok = format_tokens(self.input_tokens);
        let out_tok = format_tokens(self.output_tokens);

        let mut spans = vec![
            Span::styled(indicator, Style::default().fg(indicator_color)),
            Span::raw(" "),
            Span::styled(
                status_text,
                Style::default()
                    .fg(indicator_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  │  "),
            Span::styled(format!("in: {}", in_tok), Style::default().fg(Color::Gray)),
            Span::raw("  "),
            Span::styled(
                format!("out: {}", out_tok),
                Style::default().fg(Color::Gray),
            ),
        ];
        if self.cache_read_tokens > 0 {
            let cache_tok = format_tokens(self.cache_read_tokens);
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                format!("cache: {}", cache_tok),
                Style::default().fg(Color::DarkGray),
            ));
        }

        let line = Line::from(spans);

        Paragraph::new(line).render(area, buf);

        // ── Model name (right-aligned) ──
        if let Some(name) = self.model_name {
            let label = "model ";
            let label_w = label.width();
            let name_w = name.width();
            let total_w = label_w + name_w;
            if total_w < area.width as usize {
                let x = area.x + area.width - total_w as u16;
                let y = area.y;
                buf.set_string(
                    x,
                    y,
                    label,
                    Style::default().fg(Color::DarkGray),
                );
                buf.set_string(
                    x + label_w as u16,
                    y,
                    name,
                    Style::default().fg(Color::Gray),
                );
            }
        }
    }
}

pub(crate) fn format_tokens(n: u64) -> String {
    if n < 1_000 {
        format!("{}", n)
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}
