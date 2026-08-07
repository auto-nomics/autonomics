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
    pub cache_creation_tokens: u64,
    /// Total prompt tokens billed for the latest turn (input + cache_read +
    /// cache_creation). Counts against the model's context window.
    pub context_used: u64,
    /// The active model's context window size, when known.
    pub context_window: Option<u64>,
    pub model_name: Option<&'a str>,
    pub is_compacting: bool,
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer) {
        // Compaction takes over the status display so the user can see the
        // agent is summarizing context, not idle/requesting.
        let (indicator, indicator_color, status_text) = if self.is_compacting {
            ("⟳", Color::Magenta, "compacting")
        } else {
            match self.status {
                AgentStatus::Idle => ("○", Color::Gray, "idle"),
                AgentStatus::Requesting => ("◐", Color::Yellow, "requesting"),
                AgentStatus::Streaming => ("●", Color::Green, "streaming"),
                AgentStatus::Retrying => ("↻", Color::Yellow, "retrying"),
                AgentStatus::Error => ("✗", Color::Red, "error"),
            }
        };

        let out_tok = format_tokens(self.output_tokens);
        let ctx_used = format_tokens(self.context_used);
        // `in:` shows the absolute prompt size (input + cache_read + cache_creation).
        // Anthropic's `input_tokens` is uncached-only; adding the cache portions
        // here gives the user the full token count billed for the prompt.
        let absolute_in = self.input_tokens
            + self.cache_read_tokens
            + self.cache_creation_tokens;
        let absolute_in_tok = format_tokens(absolute_in);
        let cache_hit = self.cache_read_tokens + self.cache_creation_tokens;

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
            Span::styled(format!("in: {}", absolute_in_tok), Style::default().fg(Color::Gray)),
        ];
        // Cache annotation: shown only when cache > 0.
        if cache_hit > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("(cache: {})", format_tokens(cache_hit)),
                Style::default().fg(Color::DarkGray),
            ));
        }
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("out: {}", out_tok),
            Style::default().fg(Color::Gray),
        ));

        // Context window usage — shows percentage, bar, and absolute tokens.
        if self.context_used > 0 {
            spans.push(Span::raw("  │  "));
            spans.push(Span::styled("ctx:", Style::default().fg(Color::Gray)));
            if let Some(window) = self.context_window {
                if window > 0 {
                    let pct = (self.context_used * 100) / window;
                    let pct_clamped = pct.min(100);
                    let bar = context_bar(pct_clamped);
                    let color = match pct_clamped {
                        0..=60 => Color::Green,
                        61..=85 => Color::Yellow,
                        _ => Color::Red,
                    };
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(
                        format!("{pct}%"),
                        Style::default().fg(color),
                    ));
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(bar, Style::default().fg(color)));
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(
                        format!("{}/{}k", ctx_used, window / 1_000),
                        Style::default().fg(Color::DarkGray),
                    ));
                } else {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(ctx_used, Style::default().fg(Color::Gray)));
                }
            } else {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(ctx_used, Style::default().fg(Color::Gray)));
            }
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
                buf.set_string(x, y, label, Style::default().fg(Color::DarkGray));
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

/// Build a 6-char progress bar from a 0..=100 percentage.
fn context_bar(pct: u64) -> String {
    const WIDTH: usize = 6;
    let filled = (pct as usize * WIDTH) / 100;
    let empty = WIDTH - filled;
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
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