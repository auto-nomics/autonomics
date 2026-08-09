use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::state::{ChatLine, DisplaySettings, TurnUsage};
use crate::widgets::status_bar::format_tokens;

/// Try to parse `text` as JSON and return a pretty-printed version.
/// Falls back to the original text if parsing fails.
fn try_pretty_json(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| text.to_string()),
            Err(_) => text.to_string(),
        }
    } else {
        text.to_string()
    }
}

/// Format a turn's token usage as a compact dim line, e.g. "↑1.2k ↓340 cache:800".
fn format_turn_usage(u: TurnUsage) -> String {
    let mut parts = Vec::new();
    if let Some(inp) = u.input_tokens {
        parts.push(format!("↑{}", format_tokens(inp)));
    }
    parts.push(format!("↓{}", format_tokens(u.output_tokens)));
    if let Some(cache) = u.cache_read_input_tokens {
        if cache > 0 {
            parts.push(format!("cache:{}", format_tokens(cache)));
        }
    }
    parts.join("  ")
}

/// Render a single `ChatLine` into one or more owned `Line<'static>`.
pub(crate) fn render_line_owned(msg: &ChatLine, area: Rect) -> Vec<Line<'static>> {
    match msg {
        ChatLine::User(text) => {
            let mut lines = vec![Line::from(Span::styled(
                "You",
                Style::default()
                    .fg(Color::Cyan)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ))];
            for line in text.lines() {
                lines.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::Cyan).bg(Color::DarkGray),
                )));
            }
            lines
        }
        ChatLine::Assistant { text, usage } => {
            let mut lines: Vec<Line<'static>> = vec![Line::from(Span::styled(
                "Assistant",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ))];
            lines.extend(super::md_renderer::render_markdown_to_lines(
                text,
                area.width as usize,
            ));
            if let Some(u) = usage {
                lines.push(Line::from(Span::styled(
                    format_turn_usage(*u),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            lines
        }
        ChatLine::Thinking(text) => {
            let mut lines = Vec::new();
            let mut first = true;
            for line in text.lines() {
                let prefix = if first { "💭 " } else { "   " };
                lines.push(Line::from(Span::styled(
                    format!("{prefix}{line}"),
                    Style::default().fg(Color::DarkGray),
                )));
                first = false;
            }
            lines
        }
        ChatLine::ToolCall { name, input } => {
            let mut lines = vec![Line::from(Span::styled(
                format!("🔧 Calling: {}", name),
                Style::default().fg(Color::Yellow),
            ))];
            if !input.is_empty() {
                let display = try_pretty_json(input);
                for line in display.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("   {}", line),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            lines
        }
        ChatLine::ToolBackground { seq, name } => {
            vec![Line::from(Span::styled(
                format!("⏳ Calling: {name} (running in background, task #{seq})"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::DIM),
            ))]
        }
        ChatLine::ToolResult { ok, content } => {
            let icon = if *ok { "✓" } else { "✗" };
            let color = if *ok { Color::Green } else { Color::Red };
            let display = try_pretty_json(content);
            let mut lines = Vec::new();
            let mut first = true;
            for line in display.lines() {
                let prefix = if first {
                    format!("{icon} ")
                } else {
                    "  ".to_string()
                };
                lines.push(Line::from(Span::styled(
                    format!("{prefix}{line}"),
                    Style::default().fg(color),
                )));
                first = false;
            }
            lines
        }
        ChatLine::Error(text) => {
            let mut lines = Vec::new();
            let mut first = true;
            for line in text.lines() {
                let prefix = if first { "✗ Error: " } else { "         " };
                lines.push(Line::from(Span::styled(
                    format!("{prefix}{line}"),
                    Style::default().fg(Color::Red),
                )));
                first = false;
            }
            lines
        }
        ChatLine::Cancelled => {
            vec![Line::from(Span::styled(
                "⊘ Turn cancelled by user",
                Style::default().fg(Color::DarkGray),
            ))]
        }
        ChatLine::RetryableError {
            message,
            attempt,
            max_retries,
        } => {
            let header = format!(
                "↻ Retry {attempt}/{max_retries}: "
            );
            let mut lines = Vec::new();
            let mut first = true;
            for line in message.lines() {
                let prefix = if first {
                    header.as_str()
                } else {
                    "              "
                };
                lines.push(Line::from(Span::styled(
                    format!("{prefix}{line}"),
                    Style::default().fg(Color::Yellow),
                )));
                first = false;
            }
            lines
        }
        ChatLine::Separator => {
            vec![Line::from(Span::styled(
                "─".repeat(area.width as usize),
                Style::default().fg(Color::DarkGray),
            ))]
        }
    }
}

/// Render with display settings (collapse toggles). Falls back to
/// `render_line_owned` for variants that aren't collapsed.
pub(crate) fn render_line_with_settings(
    msg: &ChatLine,
    area: Rect,
    display: &DisplaySettings,
) -> Vec<Line<'static>> {
    match msg {
        ChatLine::Thinking(text) if display.collapse_thinking => {
            let n = text.lines().count();
            vec![Line::from(Span::styled(
                format!(" 💭 [thinking — {n} lines, collapsed]"),
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
            ))]
        }
        ChatLine::ToolCall { name, input } if display.collapse_tool_calls => {
            let preview: String = if input.is_empty() {
                String::new()
            } else {
                let p: String = input.chars().take(40).collect();
                if input.len() > 40 { format!("({p}…)") } else { format!("({p})") }
            };
            vec![Line::from(Span::styled(
                format!(" 🔧 {name}{preview} [collapsed]"),
                Style::default().fg(Color::Yellow),
            ))]
        }
        ChatLine::ToolResult { ok, content } if display.collapse_tool_results => {
            let (icon, color) = if *ok { ("✓", Color::Green) } else { ("✗", Color::Red) };
            let n = content.chars().count();
            vec![Line::from(Span::styled(
                format!(" {icon} [result — {n} chars, collapsed]"),
                Style::default().fg(color),
            ))]
        }
        _ => render_line_owned(msg, area),
    }
}
