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

// ── Tool call / result rendering ─────────────────────────────────────
//
// Follows the claude-code design system ported into `theme` (source:
// cola-runner/awesome-tui-design `designs/claude-code/DESIGN.md`): tool
// blocks are framed in the hot-pink family (`accent_alt`) over the
// documented tool-output surface, the tool's key argument sits in the
// border header, block bodies are syntax-highlighted rather than bare
// dim strings, and long output is elided with an explicit `+N more
// lines` marker instead of flooding the transcript.

/// Input lines shown inside a tool-call block before elision.
const TOOL_INPUT_MAX_LINES: usize = 12;
/// Excerpt lines shown for successful tool results.
const TOOL_RESULT_MAX_LINES: usize = 8;
/// Failure output matters more; allow a longer excerpt.
const TOOL_ERROR_MAX_LINES: usize = 24;

/// Input keys consulted, in order, for the tool-block header excerpt.
const HEADER_KEYS: [&str; 9] = [
    "command", "cmd", "file_path", "path", "file", "url", "query", "pattern", "q",
];

/// Best-effort key argument for the tool header — e.g. the shell command
/// for a process call or the path for a file reader. `None` when the
/// input is not an object carrying any known key.
fn header_excerpt(input: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(input.trim()).ok()?;
    let object = value.as_object()?;
    for key in HEADER_KEYS {
        if let Some(found) = object.get(key) {
            let excerpt = match found {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let excerpt = excerpt.trim().to_string();
            if !excerpt.is_empty() {
                return Some(excerpt);
            }
        }
    }
    None
}

/// Cut `text` to `max` display columns, marking the cut with `…`.
fn truncate_plain(text: &str, max: usize) -> String {
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

/// Draw a `┌─ header ─…┐` frame around `content` rows, hugging the content
/// up to `width` columns. Rows must already be truncated to the inner
/// width so the right border never wraps; padding and elision carry `bg`.
fn boxed_block(
    header: &str,
    content: Vec<Vec<Span<'static>>>,
    elision: Option<String>,
    width: u16,
    border: Style,
    elision_style: Style,
    bg: Color,
) -> Vec<Line<'static>> {
    let avail = (width as usize).max(8);
    let inner_cap = avail.saturating_sub(4).max(1);

    let mut header_text = header.to_string();
    if UnicodeWidthStr::width(header_text.as_str()) + 5 > avail {
        header_text = truncate_plain(&header_text, inner_cap);
    }
    let header_w = UnicodeWidthStr::width(header_text.as_str());

    let row_width = |row: &[Span<'static>]| -> usize {
        row.iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum()
    };
    let content_w = content.iter().map(|r| row_width(r)).max().unwrap_or(0);
    let elision_w = elision
        .as_deref()
        .map(|e| UnicodeWidthStr::width(e))
        .unwrap_or(0);

    let box_w = (header_w + 5)
        .max(content_w + 4)
        .max(elision_w + 4)
        .min(avail);
    let inner = box_w - 4;

    let mut lines = Vec::with_capacity(content.len() + 3);

    // Top border with the labeled header.
    let fill = box_w - header_w - 5;
    lines.push(Line::from(vec![
        Span::styled(format!("┌─ {header_text} "), border),
        Span::styled(format!("{}┐", "─".repeat(fill)), border),
    ]));

    for row in content {
        let pad = inner.saturating_sub(row_width(&row));
        let mut spans = Vec::with_capacity(row.len() + 3);
        spans.push(Span::styled("│ ", border));
        spans.extend(row);
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), Style::default().bg(bg)));
        }
        spans.push(Span::styled("│", border));
        lines.push(Line::from(spans));
    }

    if let Some(e) = elision {
        let pad = inner.saturating_sub(UnicodeWidthStr::width(e.as_str()));
        let mut spans = Vec::with_capacity(4);
        spans.push(Span::styled("│ ", border));
        spans.push(Span::styled(e, elision_style));
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), Style::default().bg(bg)));
        }
        spans.push(Span::styled("│", border));
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(Span::styled(
        format!("└{}┘", "─".repeat(box_w - 2)),
        border,
    )));
    lines
}

/// Render a tool call as a bordered block: `┌─ Name: key-arg ─┐` with the
/// pretty-printed, JSON-highlighted input inside.
fn render_tool_call(name: &str, input: &str, width: u16) -> Vec<Line<'static>> {
    let theme = crate::theme::current();
    let raised = theme.md.surface.raised;
    let border = Style::default().fg(theme.accent_alt).bg(raised);

    let mut header = name.to_string();
    if let Some(arg) = header_excerpt(input) {
        header.push_str(": ");
        header.push_str(&arg);
    }

    let mut content: Vec<Vec<Span<'static>>> = Vec::new();
    let mut elision = None;
    if !input.trim().is_empty() {
        let pretty = try_pretty_json(input);
        let token_lines = super::md_highlight::highlight_code(
            &pretty,
            Some("json"),
            super::md_theme::MdTokens::syntax_theme_name(),
            theme.md.syntax.code_fg,
            raised,
        );
        let total = token_lines.len();
        let take = total.min(TOOL_INPUT_MAX_LINES);
        for token_line in token_lines.into_iter().take(take) {
            let spans: Vec<Span<'static>> = token_line
                .into_iter()
                .map(|(text, style)| Span::styled(text, style))
                .collect();
            content.push(truncate_spans(spans, width.saturating_sub(4).max(1) as usize));
        }
        if total > take {
            elision = Some(format!("… +{} more lines", total - take));
        }
    }

    boxed_block(
        &header,
        content,
        elision,
        width,
        border,
        Style::default().fg(theme.text_faint).bg(raised),
        raised,
    )
}

/// Render a tool result as a compact status line followed by a bounded,
/// highlighted excerpt. The icon carries the status color; the body stays
/// readable (syntax colors for JSON, the theme's soft red on failure)
/// instead of painting the whole output in success/error.
fn render_tool_result(ok: bool, content: &str, width: u16) -> Vec<Line<'static>> {
    let theme = crate::theme::current();
    let (icon, status_color) = if ok {
        ("✓", theme.success)
    } else {
        ("✗", theme.error)
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
                let prefix = if first { "💭 " } else { "   " };
                lines.push(Line::from(Span::styled(
                    format!("{prefix}{line}"),
                    Style::default().fg(theme.text_faint),
                )));
                first = false;
            }
            lines
        }
        ChatLine::ToolCall { name, input } => render_tool_call(name, input, area.width),
        ChatLine::ToolBackground { seq, name } => {
            vec![Line::from(Span::styled(
                format!("⏳ {name} (background task #{seq})"),
                Style::default()
                    .fg(theme.accent_alt)
                    .add_modifier(Modifier::DIM),
            ))]
        }
        ChatLine::ToolResult { ok, content } => render_tool_result(*ok, content, area.width),
        ChatLine::Error(text) => {
            let mut lines = Vec::new();
            let mut first = true;
            for line in text.lines() {
                let prefix = if first { "✗ Error: " } else { "         " };
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
                    "✦ compacted {} msgs → {} msgs · summary {} tok · freed {} · {:.1}s",
                    stats.messages_before,
                    stats.messages_after,
                    format_tokens(stats.summary_tokens),
                    format_tokens(stats.freed_tokens),
                    stats.duration_ms as f64 / 1000.0,
                ),
                Style::default().fg(theme.accent_alt),
            )),
            CompactResult::Failed(error) => Line::from(Span::styled(
                format!("✗ compaction failed: {error}"),
                Style::default().fg(theme.error),
            )),
            CompactResult::Skipped => Line::from(Span::styled(
                "✦ compaction skipped — conversation too short",
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
                format!(" 💭 [thinking — {n} lines, collapsed]"),
                Style::default()
                    .fg(theme.text_faint)
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
                Style::default().fg(theme.accent_alt),
            ))]
        }
        ChatLine::ToolResult { ok, content } if display.collapse_tool_results => {
            let (icon, color) = if *ok {
                ("✓", theme.success)
            } else {
                ("✗", theme.error)
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
        render_line_owned(msg, Rect::new(0, 0, width, 20))
    }

    fn span_width(line: &Line<'_>) -> usize {
        line.spans
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum()
    }

    #[test]
    fn tool_call_renders_boxed_block_with_key_arg_header() {
        let msg = ChatLine::ToolCall {
            name: "Bash".to_string(),
            input: r#"{"command": "cargo test -p autonomics-tui"}"#.to_string(),
        };
        let lines = render(&msg, 80);
        // Pretty-printing expands the object to `{` / `"command": …` / `}`
        // — header + three framed input rows + bottom.
        assert_eq!(lines.len(), 5, "lines: {lines:?}");
        let top = line_text(&lines[0]);
        assert!(
            top.starts_with("┌─ Bash: cargo test -p autonomics-tui "),
            "header must carry tool name and key argument: {top}"
        );
        assert!(top.ends_with('┐'), "closed top border: {top}");
        assert!(line_text(&lines[1]).starts_with("│ "), "input framed: {lines:?}");
        assert!(line_text(&lines[4]).starts_with('└'), "closed bottom: {lines:?}");
        // Borders take the hot-pink family token, not warning.
        let theme = crate::theme::current();
        assert_eq!(lines[0].spans[0].style.fg, Some(theme.accent_alt));
    }

    #[test]
    fn tool_call_without_json_input_still_frames_the_name() {
        let msg = ChatLine::ToolCall {
            name: "Bash".to_string(),
            input: "plain text".to_string(),
        };
        let lines = render(&msg, 80);
        let top = line_text(&lines[0]);
        assert!(top.starts_with("┌─ Bash "), "no excerpt for non-JSON: {top}");
        assert!(line_text(&lines.last().unwrap()).starts_with('└'));
        // Box hugs the plain-text input row.
        assert!(lines.iter().all(|l| span_width(l) <= 80));
    }

    #[test]
    fn tool_call_input_elides_past_the_line_cap() {
        let items: Vec<String> = (0..40).map(|i| format!("\"item-{i}\"")).collect();
        let msg = ChatLine::ToolCall {
            name: "Bash".to_string(),
            input: format!("[{}]", items.join(",")),
        };
        let lines = render(&msg, 80);
        let joined = lines
            .iter()
            .map(|l| line_text(l))
            .collect::<Vec<_>>()
            .join("\n");
        // 42 pretty-printed lines -> 12 shown + explicit elision inside the box.
        assert!(
            joined.contains("+30 more lines"),
            "expected explicit elision marker:\n{joined}"
        );
        assert!(
            line_text(&lines.last().unwrap()).starts_with('└'),
            "frame still closed after elision"
        );
        assert!(lines.iter().all(|l| span_width(l) <= 80));
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
            status.starts_with("  ✓") && status.contains("22 lines"),
            "status line carries icon and count: {status}"
        );
        let joined = lines
            .iter()
            .map(|l| line_text(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("+14 more lines"),
            "8 excerpt lines shown of 22:\n{joined}"
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
        assert!(line_text(&lines[0]).starts_with("  ✗"), "{lines:?}");
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
        assert!(line_text(&lines[0]).starts_with("  ✓"));
    }
}
