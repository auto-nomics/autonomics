use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::state::{ChatLine, CompactResult, DisplaySettings, TurnUsage};
use crate::widgets::chat_widget::text_layout::wrap_spans;
use crate::widgets::status_bar::format_tokens;

/// Prefix used for the first visual row of a user-authored message.
const USER_PREFIX: &str = "› ";
/// Prefix used for wrapped and explicit continuation rows.
const USER_CONTINUATION_PREFIX: &str = "  ";
/// Columns reserved for the user-message prefix and a one-column right margin.
const USER_HORIZONTAL_RESERVE: u16 = USER_PREFIX.len() as u16 + 1;

/// Remove terminal escape/control characters while preserving tabs and newlines.
///
/// User input is untrusted enough to contain pasted CSI/OSC-style sequences. Rendering those
/// verbatim can corrupt or reconfigure the terminal, matching Codex's user-message sanitizer is
/// safer than treating the message as ordinary display text.
fn sanitize_user_text(text: &str) -> String {
    let mut sanitized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek().is_some_and(|next| *next == '[') {
            chars.next();
            // Skip through the final byte of a CSI sequence. Invalid incomplete
            // sequences simply drop the remainder instead of emitting an ESC.
            let _ = chars.find(|next| ('@'..='~').contains(next));
        } else if matches!(ch, '\n' | '\t') || !ch.is_control() {
            sanitized.push(ch);
        }
    }
    sanitized
}

fn render_user_message(text: &str, width: u16) -> Vec<Line<'static>> {
    let message = sanitize_user_text(text);
    if message.trim().is_empty() {
        return Vec::new();
    }
    let content_width = width.saturating_sub(USER_HORIZONTAL_RESERVE).max(1);
    let style = Style::default().fg(Color::Cyan);

    let mut content_lines = Vec::new();
    for logical_line in message.trim_end_matches(['\r', '\n']).split('\n') {
        let wrapped = wrap_spans(
            &[Span::styled(logical_line.to_string(), style)],
            content_width,
        );
        for row in wrapped {
            let prefix = if content_lines.is_empty() {
                USER_PREFIX
            } else {
                USER_CONTINUATION_PREFIX
            };
            let mut spans = Vec::with_capacity(row.spans.len() + 1);
            spans.push(Span::styled(
                prefix,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::DIM | Modifier::BOLD),
            ));
            spans.extend(
                row.spans
                    .into_iter()
                    .map(|span| Span::styled(span.content, span.style)),
            );
            content_lines.push(Line::from(spans));
        }
    }

    // Codex leaves quiet vertical space around each user turn. Trim only
    // trailing blank content rows; intentional blank lines inside the message
    // remain visible.
    while content_lines.last().is_some_and(|line| {
        line.spans.is_empty() || line.spans.iter().all(|span| span.content.trim().is_empty())
    }) {
        content_lines.pop();
    }
    if content_lines.is_empty() {
        return Vec::new();
    }

    let mut lines = vec![Line::from("")];
    lines.extend(content_lines);
    lines.push(Line::from(""));
    lines
}

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
        ChatLine::User(text) => render_user_message(text, area.width),
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
            let header = format!("↻ Retry {attempt}/{max_retries}: ");
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
        ChatLine::Compact(result) => vec![match result {
            CompactResult::Done(stats) => Line::from(Span::styled(
                format!(
                    "✦ compacted {} msgs → {} msgs · summary {} tok · freed {} · {:.1}s",
                    stats.messages_before,
                    stats.messages_after,
                    format_tokens(stats.summary_tokens),
                    format_tokens(stats.freed_tokens),
                    stats.duration_ms as f64 / 1000.0,
                ),
                Style::default().fg(Color::Magenta),
            )),
            CompactResult::Failed(error) => Line::from(Span::styled(
                format!("✗ compaction failed: {error}"),
                Style::default().fg(Color::Red),
            )),
            CompactResult::Skipped => Line::from(Span::styled(
                "✦ compaction skipped — conversation too short",
                Style::default().fg(Color::DarkGray),
            )),
        }],
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
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ))]
        }
        ChatLine::ToolCall { name, input } if display.collapse_tool_calls => {
            let preview: String = if input.is_empty() {
                String::new()
            } else {
                let p: String = input.chars().take(40).collect();
                if input.len() > 40 {
                    format!("({p}…)")
                } else {
                    format!("({p})")
                }
            };
            vec![Line::from(Span::styled(
                format!(" 🔧 {name}{preview} [collapsed]"),
                Style::default().fg(Color::Yellow),
            ))]
        }
        ChatLine::ToolResult { ok, content } if display.collapse_tool_results => {
            let (icon, color) = if *ok {
                ("✓", Color::Green)
            } else {
                ("✗", Color::Red)
            };
            let n = content.chars().count();
            vec![Line::from(Span::styled(
                format!(" {icon} [result — {n} chars, collapsed]"),
                Style::default().fg(color),
            ))]
        }
        _ => render_line_owned(msg, area),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn user_message_wraps_and_prefixes_each_visual_row() {
        let lines = render_user_message("alpha beta gamma", 12);
        assert_eq!(lines.first().map(line_text).as_deref(), Some(""));
        assert_eq!(line_text(&lines[1]), "› alpha");
        assert_eq!(line_text(&lines[2]), "  beta");
        assert_eq!(line_text(&lines[3]), "  gamma");
        assert_eq!(lines.last().map(line_text).as_deref(), Some(""));
    }

    #[test]
    fn user_message_preserves_explicit_blank_lines_but_trims_trailing_newlines() {
        let lines = render_user_message("first\n\nsecond\n\n", 40);
        let rendered: Vec<_> = lines.iter().map(line_text).collect();
        assert_eq!(rendered, vec!["", "› first", "  ", "  second", ""]);
    }

    #[test]
    fn user_message_removes_terminal_control_sequences() {
        let lines = render_user_message("safe\u{1b}[2Jtext\u{7}", 40);
        assert_eq!(line_text(&lines[1]), "› safetext");
    }

    #[test]
    fn blank_user_message_renders_nothing() {
        assert!(render_user_message(" \n ", 40).is_empty());
    }
}
