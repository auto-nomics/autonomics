use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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

/// Local wall-clock caption for a message timestamp, e.g. `14:32`.
///
/// The claude-code design system renders timestamps as muted captions
/// (Text Hierarchy: "Caption | Muted + dim | Token counts, timestamps").
fn format_clock(ts_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ts_ms)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

/// Dim caption span carrying the message clock, or nothing.
fn clock_caption(ts: Option<i64>, theme_text_faint: Color) -> Option<Span<'static>> {
    ts.map(format_clock)
        .filter(|clock| !clock.is_empty())
        .map(|clock| Span::styled(format!("· {clock}"), Style::default().fg(theme_text_faint)))
}

fn render_user_message(text: &str, width: u16, ts: Option<i64>) -> Vec<Line<'static>> {
    let theme = crate::theme::current();
    let message = sanitize_user_text(text);
    if message.trim().is_empty() {
        return Vec::new();
    }
    let content_width = width.saturating_sub(USER_HORIZONTAL_RESERVE).max(1);
    let style = Style::default().fg(theme.accent);

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
                    .fg(theme.accent)
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

    let mut lines = Vec::new();
    if let Some(caption) = clock_caption(ts, theme.text_faint) {
        lines.push(Line::from(vec![caption]));
    }
    lines.push(Line::from(""));
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

// Tool activity stays visually subordinate to the conversation. Long values are
// explicitly truncated so the transcript remains easy to scan.
const TOOL_RESULT_MAX_LINES: usize = 4;
const TOOL_ERROR_MAX_LINES: usize = 8;

/// Cut `text` to `max` display columns, marking the cut with `…`.
fn truncate_plain(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max {
        return text.to_string();
    }
    let mut out = String::new();
    for ch in text.chars() {
        let char_w = ch.width().unwrap_or(0);
        if UnicodeWidthStr::width(out.as_str()) + char_w > max.saturating_sub(1) {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

/// Cut a styled span row to `max` display columns, keeping styles intact.
fn truncate_spans(spans: Vec<Span<'static>>, max: usize) -> Vec<Span<'static>> {
    let mut used = 0usize;
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        if used >= max {
            break;
        }
        let w = UnicodeWidthStr::width(span.content.as_ref());
        if used + w <= max {
            used += w;
            out.push(span);
            continue;
        }
        let remaining = max - used;
        let mut cut = String::new();
        for ch in span.content.chars() {
            let char_w = ch.width().unwrap_or(0);
            if UnicodeWidthStr::width(cut.as_str()) + char_w > remaining.saturating_sub(1) {
                break;
            }
            cut.push(ch);
        }
        cut.push('…');
        out.push(Span::styled(cut, span.style));
        break;
    }
    out
}

/// Add a left-only rail with rounded ends to expanded tool activity.
fn tool_rail(lines: Vec<Line<'static>>, width: u16, color: Color) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
    let mut framed = vec![Line::from(Span::styled("╭", style))];
    for line in lines {
        let mut spans = vec![Span::styled(if width > 1 { "┃ " } else { "┃" }, style)];
        spans.extend(truncate_spans(line.spans, width.saturating_sub(2) as usize));
        framed.push(Line::from(spans));
    }
    framed.push(Line::from(Span::styled("╰", style)));
    framed
}

/// Show the tool name and a compact, single-line representation of its input.
fn render_tool_call(name: &str, input: &str, width: u16) -> Vec<Line<'static>> {
    let theme = crate::theme::current();
    let clean_name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let summary = match serde_json::from_str::<serde_json::Value>(input.trim()) {
        Ok(serde_json::Value::Object(map)) => map
            .iter()
            .map(|(key, value)| {
                let value = match value {
                    serde_json::Value::String(s) => {
                        s.split_whitespace().collect::<Vec<_>>().join(" ")
                    }
                    other => other.to_string(),
                };
                format!("{key}={value}")
            })
            .collect::<Vec<_>>()
            .join("  "),
        Ok(value) => value.to_string(),
        Err(_) => input.split_whitespace().collect::<Vec<_>>().join(" "),
    };
    let prefix = format!("• {clean_name}");
    let remaining = (width as usize).saturating_sub(UnicodeWidthStr::width(prefix.as_str()) + 2);
    let mut spans = vec![Span::styled(
        truncate_plain(&prefix, width as usize),
        Style::default()
            .fg(theme.accent_alt)
            .add_modifier(Modifier::BOLD),
    )];
    if !summary.is_empty() && remaining > 0 {
        spans.push(Span::styled(
            format!("  {}", truncate_plain(&summary, remaining)),
            Style::default().fg(theme.text_faint),
        ));
    }
    vec![Line::from(spans)]
}

/// Render a tool result as a compact status line followed by a bounded,
/// highlighted excerpt. The icon carries the status color; the body stays
/// readable (syntax colors for JSON, the theme's soft red on failure)
/// instead of painting the whole output in success/error.
fn render_tool_result(ok: bool, content: &str, width: u16) -> Vec<Line<'static>> {
    let theme = crate::theme::current();
    let (icon, status_color) = if ok {
        ("+", theme.success)
    } else {
        ("x", theme.error)
    };
    let max_lines = if ok {
        TOOL_RESULT_MAX_LINES
    } else {
        TOOL_ERROR_MAX_LINES
    };
    let line_cap = width.saturating_sub(4).max(1) as usize;

    let pretty = try_pretty_json(content);
    let total = pretty.lines().count();
    let mut status_spans = vec![Span::styled(
        format!("  {icon}"),
        Style::default()
            .fg(status_color)
            .add_modifier(Modifier::BOLD),
    )];
    if total > 1 {
        status_spans.push(Span::styled(
            format!(" {total} lines"),
            Style::default().fg(theme.text_faint),
        ));
    }
    let mut lines = vec![Line::from(status_spans)];

    if pretty.trim().is_empty() {
        return lines;
    }

    let token_lines = super::md_highlight::highlight_code(
        &pretty,
        Some("json"),
        super::md_theme::MdTokens::syntax_theme_name(),
        if ok { theme.text } else { theme.error },
        Color::Reset,
    );
    let take = total.min(max_lines);
    for token_line in token_lines.into_iter().take(take) {
        let row: Vec<Span<'static>> = token_line
            .into_iter()
            .map(|(text, style)| {
                if ok {
                    Span::styled(text, style)
                } else {
                    // Failure bodies read in the theme's soft red rather
                    // than a wall of syntax colors.
                    Span::styled(text, style.fg(theme.error))
                }
            })
            .collect();
        let mut spans: Vec<Span<'static>> = vec![Span::styled("  ", Style::default())];
        spans.extend(truncate_spans(row, line_cap));
        lines.push(Line::from(spans));
    }
    if total > take {
        lines.push(Line::from(Span::styled(
            format!("  … +{} more lines", total - take),
            Style::default().fg(theme.text_faint),
        )));
    }
    lines
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
    let theme = crate::theme::current();
    match msg {
        ChatLine::User { text, ts } => render_user_message(text, area.width, *ts),
        ChatLine::Assistant { text, usage, ts } => {
            let mut header_spans = vec![Span::styled(
                "Assistant",
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            )];
            if let Some(caption) = clock_caption(*ts, theme.text_faint) {
                header_spans.push(Span::styled(" ", Style::default()));
                header_spans.push(caption);
            }
            let mut lines: Vec<Line<'static>> = vec![Line::from(header_spans)];
            lines.extend(super::md_renderer::render_markdown_to_lines(
                text,
                area.width as usize,
            ));
            if let Some(u) = usage {
                lines.push(Line::from(Span::styled(
                    format_turn_usage(*u),
                    Style::default().fg(theme.text_faint),
                )));
            }
            lines
        }
        ChatLine::Thinking(text) => {
            let mut lines = Vec::new();
            let mut first = true;
            for line in text.lines() {
                let prefix = if first { ">> " } else { "   " };
                lines.push(Line::from(Span::styled(
                    format!("{prefix}{line}"),
                    Style::default().fg(theme.text_faint),
                )));
                first = false;
            }
            lines
        }
        ChatLine::ToolCall { name, input } => tool_rail(
            render_tool_call(name, input, area.width.saturating_sub(2)),
            area.width,
            crate::theme::current().accent_alt,
        ),
        ChatLine::ToolBackground { seq, name } => {
            vec![Line::from(Span::styled(
                format!("⏳ {name} (background task #{seq})"),
                Style::default()
                    .fg(theme.accent_alt)
                    .add_modifier(Modifier::DIM),
            ))]
        }
        ChatLine::ToolResult { ok, content } => tool_rail(
            render_tool_result(*ok, content, area.width.saturating_sub(2)),
            area.width,
            if *ok {
                crate::theme::current().success
            } else {
                crate::theme::current().error
            },
        ),
        ChatLine::Error(text) => {
            let mut lines = Vec::new();
            let mut first = true;
            for line in text.lines() {
                let prefix = if first { "x Error: " } else { "         " };
                lines.push(Line::from(Span::styled(
                    format!("{prefix}{line}"),
                    Style::default().fg(theme.error),
                )));
                first = false;
            }
            lines
        }
        ChatLine::Cancelled => {
            vec![Line::from(Span::styled(
                "⊘ Turn cancelled by user",
                Style::default().fg(theme.text_faint),
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
                    Style::default().fg(theme.warning),
                )));
                first = false;
            }
            lines
        }
        ChatLine::Compact(result) => vec![match result {
            CompactResult::Done(stats) => Line::from(Span::styled(
                format!(
                    "* compacted {} msgs → {} msgs · summary {} tok · freed {} · {:.1}s",
                    stats.messages_before,
                    stats.messages_after,
                    format_tokens(stats.summary_tokens),
                    format_tokens(stats.freed_tokens),
                    stats.duration_ms as f64 / 1000.0,
                ),
                Style::default().fg(theme.accent_alt),
            )),
            CompactResult::Failed(error) => Line::from(Span::styled(
                format!("x compaction failed: {error}"),
                Style::default().fg(theme.error),
            )),
            CompactResult::Skipped => Line::from(Span::styled(
                "* compaction skipped — conversation too short",
                Style::default().fg(theme.text_faint),
            )),
        }],
        ChatLine::Separator => {
            vec![Line::from(Span::styled(
                "─".repeat(area.width as usize),
                Style::default().fg(theme.text_faint),
            ))]
        }
        ChatLine::Injected { origin, text, ts } => {
            let label = match origin {
                agentik_types::MessageOrigin::Agent { path } => {
                    format!("agent · {path}")
                }
                agentik_types::MessageOrigin::System => "system".to_string(),
                agentik_types::MessageOrigin::User => "notice".to_string(),
            };
            let mut header = format!("⌁ {label}");
            if let Some(caption) = clock_caption(*ts, theme.text_faint) {
                header.push(' ');
                header.push_str(&caption.content);
            }
            let mut lines = vec![Line::from(Span::styled(
                header,
                Style::default()
                    .fg(theme.text_faint)
                    .add_modifier(Modifier::BOLD),
            ))];
            for line in text.lines() {
                lines.push(Line::from(Span::styled(
                    format!("   {line}"),
                    Style::default().fg(theme.text_faint),
                )));
            }
            lines
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
    // Chat-only view drops non-dialogue machinery lines entirely — no
    // collapsed placeholder, no table cells. Turn-level signals the user
    // still needs (errors, cancellation, separators) stay, as do the
    // dialogue turns.
    if display.chat_only
        && matches!(
            msg,
            ChatLine::Thinking(_)
                | ChatLine::ToolCall { .. }
                | ChatLine::ToolBackground { .. }
                | ChatLine::ToolResult { .. }
                | ChatLine::RetryableError { .. }
                | ChatLine::Compact(_)
        )
    {
        return Vec::new();
    }
    let theme = crate::theme::current();
    match msg {
        ChatLine::Thinking(text) if display.collapse_thinking => {
            let n = text.lines().count();
            vec![Line::from(Span::styled(
                format!(" >> [thinking — {n} lines, collapsed]"),
                Style::default()
                    .fg(theme.text_faint)
                    .add_modifier(Modifier::DIM),
            ))]
        }
        ChatLine::ToolCall { name, input } if display.collapse_tool_calls => {
            render_tool_call(name, input, area.width)
        }
        ChatLine::ToolResult { ok, content } if display.collapse_tool_results => {
            let (icon, color) = if *ok {
                ("+", theme.success)
            } else {
                ("x", theme.error)
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
        let lines = render_user_message("alpha beta gamma", 12, None);
        assert_eq!(lines.first().map(line_text).as_deref(), Some(""));
        assert_eq!(line_text(&lines[1]), "› alpha");
        assert_eq!(line_text(&lines[2]), "  beta");
        assert_eq!(line_text(&lines[3]), "  gamma");
        assert_eq!(lines.last().map(line_text).as_deref(), Some(""));
    }

    #[test]
    fn user_message_preserves_explicit_blank_lines_but_trims_trailing_newlines() {
        let lines = render_user_message("first\n\nsecond\n\n", 40, None);
        let rendered: Vec<_> = lines.iter().map(line_text).collect();
        assert_eq!(rendered, vec!["", "› first", "  ", "  second", ""]);
    }

    #[test]
    fn user_message_removes_terminal_control_sequences() {
        let lines = render_user_message("safe\u{1b}[2Jtext\u{7}", 40, None);
        assert_eq!(line_text(&lines[1]), "› safetext");
    }

    #[test]
    fn blank_user_message_renders_nothing() {
        assert!(render_user_message(" \n ", 40, None).is_empty());
    }

    /// `ts` must be a valid epoch so the clock is real; only its shape is
    /// asserted because the local timezone varies by machine.
    #[test]
    fn user_message_timestamp_renders_as_a_muted_clock_caption() {
        let lines = render_user_message("hello", 40, Some(1_733_133_600_000));
        let caption = line_text(&lines[0]);
        assert!(
            caption.starts_with("· ") && caption.ends_with(':') == false,
            "caption shape: {caption}"
        );
        let (clock_part, clock) = caption.split_once("· ").expect("· prefix");
        assert!(clock_part.is_empty());
        let (h, m) = clock.split_once(':').expect("HH:MM shape");
        assert_eq!(h.len(), 2);
        assert_eq!(m.len(), 2);
        // Caption sits above the blank separator, then the content rows.
        assert_eq!(line_text(&lines[1]), "");
        assert_eq!(line_text(&lines[2]), "› hello");
    }

    #[test]
    fn assistant_header_carries_the_turn_clock() {
        let msg = ChatLine::Assistant {
            text: "hi".to_string(),
            usage: None,
            ts: Some(1_733_133_600_000),
        };
        let lines = render(&msg, 80);
        let header = line_text(&lines[0]);
        assert!(
            header.starts_with("Assistant · ") && header.ends_with(':') == false,
            "header: {header}"
        );
        let clock = header.split_once("· ").expect("clock").1;
        assert_eq!(clock.len(), 5, "HH:MM");
    }

    #[test]
    fn dialogue_lines_without_ts_render_no_clock() {
        let user = ChatLine::User {
            text: "hello".to_string(),
            ts: None,
        };
        let lines = render(&user, 80);
        assert_eq!(line_text(&lines[0]), "", "no caption row without ts");

        let assistant = ChatLine::Assistant {
            text: "hi".to_string(),
            usage: None,
            ts: None,
        };
        let lines = render(&assistant, 80);
        assert_eq!(line_text(&lines[0]), "Assistant");
    }

    fn render(msg: &ChatLine, width: u16) -> Vec<Line<'static>> {
        match msg {
            ChatLine::ToolCall { name, input } => render_tool_call(name, input, width),
            ChatLine::ToolResult { ok, content } => render_tool_result(*ok, content, width),
            _ => render_line_owned(msg, Rect::new(0, 0, width, 20)),
        }
    }

    fn span_width(line: &Line<'_>) -> usize {
        line.spans
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum()
    }

    #[test]
    fn expanded_tool_rail_has_rounded_ends_and_fits_width() {
        let lines = tool_rail(vec![Line::from("long content")], 6, Color::Red);
        assert_eq!(
            lines.iter().map(line_text).collect::<Vec<_>>(),
            vec!["╭", "┃ lon…", "╰"]
        );
        assert!(lines.iter().all(|line| span_width(line) <= 6));
    }

    #[test]
    fn collapsed_tool_call_summarizes_multiline_json() {
        let msg = ChatLine::ToolCall {
            name: "Bash".into(),
            input: "{\n  \"command\": \"cargo test\"\n}".into(),
        };
        let settings = DisplaySettings {
            collapse_tool_calls: true,
            ..Default::default()
        };
        let lines = render_line_with_settings(&msg, Rect::new(0, 0, 80, 10), &settings);
        assert_eq!(lines.len(), 1);
        assert!(line_text(&lines[0]).contains("command=cargo test"));
        let narrow = render_line_with_settings(&msg, Rect::new(0, 0, 12, 10), &settings);
        assert!(span_width(&narrow[0]) <= 12);
    }

    #[test]
    fn tool_call_is_a_compact_summary() {
        let msg = ChatLine::ToolCall {
            name: "Bash".into(),
            input: r#"{"command":"cargo test","timeout":30}"#.into(),
        };
        let lines = render(&msg, 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            line_text(&lines[0]),
            "• Bash  command=cargo test  timeout=30"
        );
    }

    #[test]
    fn tool_call_handles_multiline_and_narrow_width() {
        let lines = render_tool_call("工具", "first\nsecond", 12);
        assert_eq!(lines.len(), 1);
        assert!(span_width(&lines[0]) <= 12);
        assert!(line_text(&lines[0]).ends_with('…'));
        assert!(span_width(&render_tool_call("工具", "x", 0)[0]) == 0);
    }

    #[test]
    fn tool_result_shows_status_icon_and_bounded_excerpt() {
        let items: Vec<String> = (0..20).map(|i| format!("\"row-{i}\"")).collect();
        let msg = ChatLine::ToolResult {
            ok: true,
            content: format!("[{}]", items.join(",")),
        };
        let lines = render(&msg, 80);
        let status = line_text(&lines[0]);
        assert!(
            status.starts_with("  +") && status.contains("22 lines"),
            "status line carries icon and count: {status}"
        );
        let joined = lines
            .iter()
            .map(|l| line_text(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("+18 more lines"),
            "4 excerpt lines shown of 22:\n{joined}"
        );
    }

    #[test]
    fn tool_result_failure_reads_in_error_color() {
        let msg = ChatLine::ToolResult {
            ok: false,
            content: "{\"error\": \"boom\"}".to_string(),
        };
        let lines = render(&msg, 80);
        let theme = crate::theme::current();
        assert!(line_text(&lines[0]).starts_with("  x"), "{lines:?}");
        let body = &lines[1].spans;
        assert!(
            body.iter().skip(1).all(|s| s.style.fg == Some(theme.error)),
            "failure body renders in the soft red token: {body:?}"
        );
    }

    #[test]
    fn tool_result_empty_content_is_just_the_status_line() {
        let msg = ChatLine::ToolResult {
            ok: true,
            content: String::new(),
        };
        let lines = render(&msg, 80);
        assert_eq!(lines.len(), 1);
        assert!(line_text(&lines[0]).starts_with("  +"));
    }
}
