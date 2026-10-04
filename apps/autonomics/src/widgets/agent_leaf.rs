use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::{Buffer, StatefulWidget, Widget},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph, StatefulWidgetRef},
};
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr};

use crate::state::{AgentStatus, AgentTabState, DisplaySettings, InputMode};
use crate::widgets::{
    chat_widget::{ChatWidget, ChatWidgetState, visual_row_count},
    input_area::{InputWidget, InputWidgetState, PROMPT_GUTTER},
    session_list::SessionSummary,
    sidebar::{SideBar, SidebarData},
    status_bar::StatusBar,
};

/// Width of the sidebar as a percentage of the total leaf area.
/// At 120 cols → ~30 cols sidebar; at 80 cols → ~20 cols.
const SIDEBAR_PERCENT: u16 = 25;
/// Minimum sidebar width in columns. Below this the sidebar is hidden.
const SIDEBAR_MIN_WIDTH: u16 = 24;

/// Collect only rendered lines intersecting the requested logical-row window.
///
/// Message ownership stays in `AgentTabState::cached_msg_lines`; the returned
/// vector contains only the Ratatui lines needed by the current viewport.
fn collect_visible_lines(
    ts: &AgentTabState,
    scroll_offset: usize,
    viewport_height: u16,
    width: u16,
) -> (Vec<Line<'static>>, usize) {
    let viewport_end = scroll_offset.saturating_add(viewport_height as usize);
    let mut visible = Vec::new();
    let mut logical_row = 0usize;
    let mut window_start = None;

    for (message_idx, cached) in ts.cached_msg_lines.iter().enumerate() {
        let message_start = logical_row;
        let Some(message_rows) = ts.cached_msg_row_counts.get(message_idx) else {
            break;
        };
        logical_row = message_start.saturating_add(*message_rows);
        if logical_row <= scroll_offset || message_start >= viewport_end {
            continue;
        }

        let mut line_start = message_start;
        for line in cached {
            let rows = visual_row_count(line, width);
            let line_end = line_start.saturating_add(rows);
            if line_end > scroll_offset && line_start < viewport_end {
                if window_start.is_none() {
                    window_start = Some(line_start);
                }
                visible.push(line.clone());
            } else if line_start >= viewport_end {
                break;
            }
            line_start = line_end;
        }
    }

    let relative_offset = scroll_offset.saturating_sub(window_start.unwrap_or(0));
    (visible, relative_offset)
}

/// Renders a single agent's conversation surface with a right-hand sidebar.
///
/// Layout:
/// ```text
/// ┌──────────────────────────────────────────────────┐
/// │ StatusBar (full width, 1 row)                    │
/// ├──────────────────────────────┬───────────────────┤
/// │ Chat area                    │ Sidebar           │
/// │ (scrollable transcript)      │ - Task Plan       │
/// │                              │ - Sessions        │
/// │                              │ - Background Tasks│
/// ├──────────────────────────────┴───────────────────┤
/// │ Input (full width, dynamic height)               │
/// │ Footer hints (full width, 1 row)                 │
/// └──────────────────────────────────────────────────┘
/// ```
pub struct AgentLeaf<'a> {
    pub active_model: Option<&'a str>,
    pub context_window: Option<u64>,
    pub sessions: &'a [SessionSummary],
    pub display: &'a DisplaySettings,
}

impl StatefulWidgetRef for AgentLeaf<'_> {
    type State = AgentTabState;

    fn render_ref(&self, area: Rect, buf: &mut Buffer, ts: &mut AgentTabState) {
        let running = ts.status.is_active() || ts.status == AgentStatus::Waiting;

        // Dynamic input height: the boxed composer grows with content
        // (word-wrapped), capped at MAX_INPUT_ROWS text rows. The widget draws
        // a rounded border box, so reserve +2 rows (top/bottom) and subtract
        // the border columns (+ gutter) from the wrap width.
        // area.width − 2 (box borders) − 2 (❯ prefix gutter).
        let text_width = area.width.saturating_sub(PROMPT_GUTTER + 2);
        let input_text_rows = if running && ts.input.is_empty() {
            1
        } else {
            ts.input.display_height(text_width)
        };
        let input_constraint = Constraint::Length(input_text_rows + 2);
        let pending_texts = ts.pending_queue_preview_texts();
        let pending_constraint = Constraint::Length(pending_queue_height(
            pending_texts.len(),
            area.width,
            area.height,
        ));
        let compact_constraint =
            Constraint::Length(compact_preview_height(&ts.compact_state, area.height));

        // ── Top-level vertical split ──
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // StatusBar (full width)
                Constraint::Min(3),    // Middle: Chat + Sidebar (horizontal split)
                pending_constraint,    // Runtime-bound input awaiting commit
                compact_constraint,    // Live compaction preview
                input_constraint,      // Input (full width)
                Constraint::Length(1), // Footer hints
            ])
            .split(area);

        // ── StatusBar ──
        let status_bar = StatusBar {
            status: &ts.status,
            input_tokens: ts.latest_turn_input_tokens,
            output_tokens: ts.latest_turn_output_tokens,
            cache_read_tokens: ts.latest_turn_cache_read_tokens,
            cache_creation_tokens: ts.latest_turn_cache_creation_tokens,
            context_used: ts.latest_turn_context_used,
            context_window: self.context_window,
            model_name: self.active_model,
            compact: Some(&ts.compact_state),
            frame: ts.frame,
        };
        status_bar.render(layout[0], buf);

        // ── Middle: Chat + Sidebar horizontal split ──
        let middle = layout[1];

        // Decide whether to show the sidebar based on available width.
        let sidebar_width = (middle.width * SIDEBAR_PERCENT / 100).max(SIDEBAR_MIN_WIDTH);
        let show_sidebar = middle.width >= SIDEBAR_MIN_WIDTH * 3;

        let (chat_area, sidebar_area) = if show_sidebar {
            let h_split = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Min(20),               // Chat (flexible)
                    Constraint::Length(sidebar_width), // Sidebar (fixed)
                ])
                .split(middle);
            (h_split[0], Some(h_split[1]))
        } else {
            (middle, None)
        };

        // ── Chat area ──
        let chat_block = Block::default().padding(Padding::new(2, 2, 2, 2));
        let chat_inner_area = chat_block.inner(chat_area);

        let viewport_height = chat_inner_area.height;
        let width = chat_inner_area.width;

        // ── Incremental per-message render cache ──
        let n = ts.messages.len();
        if ts.cached_msg_lines.len() != n {
            ts.cached_msg_lines.resize(n, Vec::new());
            ts.cached_msg_row_counts.resize(n, 0);
            ts.cached_msg_versions.resize(n, 0);
        } else if ts.cached_msg_row_counts.len() != n {
            ts.cached_msg_row_counts.resize(n, 0);
        }
        if ts.cached_msg_width != width {
            ts.cached_msg_width = width;
            for v in &mut ts.cached_msg_versions {
                *v = 0;
            }
        }
        if ts.cached_display_version != self.display.version {
            ts.cached_display_version = self.display.version;
            for v in &mut ts.cached_msg_versions {
                *v = 0;
            }
        }

        let mut total_rows = 0usize;
        for (i, msg) in ts.messages.iter().enumerate() {
            if ts.cached_msg_versions[i] == ts.msg_versions[i] && !ts.cached_msg_lines[i].is_empty()
            {
                if ts.cached_msg_row_counts[i] == 0 {
                    ts.cached_msg_row_counts[i] = ts.cached_msg_lines[i]
                        .iter()
                        .map(|line| visual_row_count(line, width))
                        .sum();
                }
            } else {
                let rendered = super::chat_widget::render::render_line_with_settings(
                    msg,
                    chat_inner_area,
                    self.display,
                );
                ts.cached_msg_lines[i] = rendered;
                ts.cached_msg_row_counts[i] = ts.cached_msg_lines[i]
                    .iter()
                    .map(|line| visual_row_count(line, width))
                    .sum();
                ts.cached_msg_versions[i] = ts.msg_versions[i];
            }
            total_rows += ts.cached_msg_row_counts[i];
        }

        ts.content_line_count = total_rows;
        ts.clamp_scroll(viewport_height);

        let (visible_lines, relative_scroll_offset) =
            collect_visible_lines(ts, ts.scroll_offset, viewport_height, width);
        let mut chat_state = ChatWidgetState::new(relative_scroll_offset);
        let chat_widget = ChatWidget {
            lines: &visible_lines,
            total_rows,
        };
        chat_widget.render(chat_inner_area, buf, &mut chat_state);

        // ── Sidebar ──
        if let Some(sb_area) = sidebar_area {
            let sidebar = SideBar {
                data: SidebarData {
                    plan: &ts.plan,
                    tool_tasks: &ts.tool_tasks,
                    sessions: self.sessions,
                },
            };
            sidebar.render(sb_area, buf);
        }

        // Runtime-bound prompts stay out of the transcript until the Session
        // commits them; show a compact preview immediately above the composer.
        if !pending_texts.is_empty() {
            render_pending_queue(layout[2], buf, &pending_texts);
        }

        // ── Live compaction preview (summary tail while compacting) ──
        if ts.compact_state.is_compacting {
            render_compact_preview(layout[3], buf, &ts.compact_state);
        }

        // ── Input area (boxed composer, ❯ prompt) ──
        let queued = ts.pending_queue_len();
        let placeholder: &str = if running {
            match ts.input_mode {
                InputMode::Input => "Type to queue a message… (Ctrl+C cancels turn)",
                InputMode::Browse => {
                    if queued > 0 {
                        "Agent running — pending input waits for next request…"
                    } else {
                        "Agent running… (Ctrl+C to cancel)"
                    }
                }
            }
        } else {
            match ts.input_mode {
                InputMode::Browse => "Type a message, or press Enter to edit…",
                InputMode::Input => "Type a message…",
            }
        };
        let spinner = if running {
            BRAILLE_SPINNER[(ts.frame / 4) as usize % BRAILLE_SPINNER.len()]
        } else {
            ""
        };
        let title: String = if running {
            let base = if ts.cancel_pending {
                "⏹ cancelling… (Ctrl+C again to force quit)".to_string()
            } else {
                match ts.status {
                    AgentStatus::Requesting => format!("{spinner} thinking…"),
                    AgentStatus::Streaming => format!("{spinner} responding…"),
                    AgentStatus::Retrying => format!("{spinner} retrying…"),
                    AgentStatus::Compacting => format!("{spinner} compacting…"),
                    AgentStatus::Waiting => format!("{spinner} waiting…"),
                    AgentStatus::Error => "error".to_string(),
                    AgentStatus::Cancelled => "cancelled".to_string(),
                    _ => format!("{spinner} running…"),
                }
            };
            if queued > 0 {
                format!("{base} ({queued} pending)")
            } else {
                base
            }
        } else {
            match ts.input_mode {
                InputMode::Browse => "browse".to_string(),
                InputMode::Input => "compose".to_string(),
            }
        };

        // The composer is interactive even while the agent is running:
        // the user can type and enqueue messages. Only hide the caret
        // when not in Input mode.
        let input_widget = InputWidget {
            disabled: running,
            editable: ts.input_mode == InputMode::Input,
            title: &title,
            placeholder,
        };
        let mut input_state = InputWidgetState {
            input: &mut ts.input,
        };
        input_widget.render(layout[4], buf, &mut input_state);

        // ── Animated loading bar on the input box bottom border ──
        if running {
            render_loading_bar(layout[4], buf, ts.frame);
        }

        // ── Footer hint line ──
        render_footer_hint(
            layout[5],
            buf,
            ts.input_mode,
            running,
            queued,
            ts.in_history_search,
            &ts.history_search_query,
            ts.history_search_selected,
            ts.history_search_matches.len(),
        );
    }
}

/// Braille spinner frames for the loading indicator.
const BRAILLE_SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Rows of the live compaction preview: header + up to 3 summary tail
/// lines. Zero (collapsed) unless a pass is in flight.
fn compact_preview_height(compact: &crate::state::CompactState, total_height: u16) -> u16 {
    if !compact.is_compacting || total_height < 10 {
        return 0;
    }
    1 + COMPACT_PREVIEW_LINES.min(
        compact
            .summary
            .lines()
            .count()
            .max(if compact.phase.is_some() { 1 } else { 0 }),
    ) as u16
}

/// Rows of summary text shown in the preview block.
const COMPACT_PREVIEW_LINES: usize = 3;

/// Live view of an in-flight compaction: pass header (trigger + scale) and
/// the tail of the summary being generated.
fn render_compact_preview(area: Rect, buf: &mut Buffer, compact: &crate::state::CompactState) {
    use agentik_types::{CompactPhase, CompactTrigger};

    if area.width < 8 || area.height == 0 {
        return;
    }

    let phase = match compact.phase {
        Some(CompactPhase::Summarizing) => "summarizing",
        Some(CompactPhase::Rebuilding) => "rebuilding",
        None => "preparing",
    };
    let trigger = compact
        .plan
        .as_ref()
        .map(|p| match p.trigger {
            CompactTrigger::Manual => "manual".to_string(),
            CompactTrigger::MidTurn { used_pct } => format!("auto · mid-turn · ctx {used_pct}%"),
            CompactTrigger::PreRequest { used_pct } => {
                format!("auto · pre-request · ctx {used_pct}%")
            }
        })
        .unwrap_or_default();

    let header_style = Style::default()
        .fg(Color::Magenta)
        .add_modifier(Modifier::BOLD);
    let summary_style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC);

    let mut lines = vec![Line::styled(
        format!("⟳ compacting — {phase}  ({trigger})"),
        header_style,
    )];
    if compact.summary.is_empty() {
        if compact.phase.is_some() {
            lines.push(Line::styled("  …", summary_style));
        }
    } else {
        let tail: Vec<&str> = compact
            .summary
            .lines()
            .rev()
            .take(area.height as usize - 1)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        for line in tail {
            lines.push(Line::styled(format!("  {line}"), summary_style));
        }
    }
    // Clip to the allotted area — the height calc and the tail extraction
    // can disagree by a row on mid-flight resizes.
    lines.truncate(area.height as usize);
    Paragraph::new(lines).render(area, buf);
}

const PENDING_PREVIEW_LIMIT: usize = 3;

/// Height used for the compact pending-input band above the composer.
fn pending_queue_height(count: usize, width: u16, height: u16) -> u16 {
    if count == 0 || width < 8 || height < 7 {
        return 0;
    }

    let mut height = 1 + count.min(PENDING_PREVIEW_LIMIT);
    if count > PENDING_PREVIEW_LIMIT {
        height += 1;
    }
    u16::try_from(height).unwrap_or(u16::MAX)
}

/// Render submitted-but-uncommitted input without placing it in the transcript.
fn render_pending_queue(area: Rect, buf: &mut Buffer, messages: &[String]) {
    if area.width < 8 {
        return;
    }

    let header_style = Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::BOLD);
    let message_style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC);
    let mut lines = vec![Line::styled(
        format!("Pending input ({})", messages.len()),
        header_style,
    )];
    lines.extend(
        messages
            .iter()
            .take(PENDING_PREVIEW_LIMIT)
            .map(|message| Line::styled(preview_line(message, area.width), message_style)),
    );

    if messages.len() > PENDING_PREVIEW_LIMIT {
        lines.push(Line::styled(
            format!("  + {} more", messages.len() - PENDING_PREVIEW_LIMIT),
            message_style,
        ));
    }

    Paragraph::new(lines).render(area, buf);
}

/// Collapse a multiline prompt and fit it on one terminal row.
fn preview_line(message: &str, width: u16) -> String {
    let collapsed = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let prefix = "  > ";
    let available = usize::from(width).saturating_sub(UnicodeWidthStr::width(prefix));
    if available == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(collapsed.as_str()) <= available {
        return format!("{prefix}{collapsed}");
    }

    let keep = available.saturating_sub(1);
    let mut truncated = String::new();
    let mut used = 0;
    for ch in collapsed.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if used + ch_width > keep {
            break;
        }
        truncated.push(ch);
        used += ch_width;
    }
    format!("{prefix}{truncated}…")
}

/// Draw an animated loading bar on the bottom border of the input box.
fn render_loading_bar(area: Rect, buf: &mut Buffer, frame: u64) {
    let y = area.y + area.height - 1;
    let start = area.x + 1;
    let end = area.x + area.width.saturating_sub(1);
    let width = end.saturating_sub(start) as usize;
    if width < 4 {
        return;
    }

    let bar_len = (width / 4).clamp(3, 12);
    let pos = ((frame / 2) as usize) % (width + bar_len);

    let bar_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    for i in 0..width {
        let in_bar = if pos < bar_len {
            i < pos || i >= width + pos - bar_len
        } else if pos > width {
            i < pos - width
        } else {
            i >= pos - bar_len && i < pos
        };
        if in_bar {
            buf.set_string(start + i as u16, y, "━", bar_style);
        }
    }
}

/// Draw the 1-row keybinding hint at the bottom of the agent tab.
#[allow(clippy::too_many_arguments)]
fn render_footer_hint(
    area: Rect,
    buf: &mut Buffer,
    mode: InputMode,
    running: bool,
    queued: usize,
    searching: bool,
    query: &str,
    selected: usize,
    total: usize,
) {
    if area.width == 0 {
        return;
    }
    let hint = if searching {
        if total == 0 {
            format!(" search: {query}  (no match)  Esc cancel ")
        } else {
            format!(
                " search: {query}  ({}/{total})  ↑↓ navigate  Enter select  Esc cancel ",
                selected + 1
            )
        }
    } else if running {
        match mode {
            InputMode::Input => {
                " Enter queue  Shift+Enter newline  Esc exit  Ctrl+C cancel turn ".to_string()
            }
            InputMode::Browse => {
                if queued > 0 {
                    format!(" {queued} pending  Enter compose  Ctrl+C cancel turn ")
                } else {
                    " Enter compose  Ctrl+C cancel turn  Ctrl+R history ".to_string()
                }
            }
        }
    } else {
        match mode {
            InputMode::Browse => {
                " Enter edit  ↑↓ scroll  PageUp/PageDown  Home/End  Ctrl+G follow  Ctrl+C quit "
                    .to_string()
            }
            InputMode::Input => {
                " Insert  Esc exit  Enter send  Shift+Enter newline  Ctrl+R history  Ctrl+G follow "
                    .to_string()
            }
        }
    };
    let style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM);
    Paragraph::new(hint)
        .style(style)
        .alignment(Alignment::Right)
        .render(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_line_collector_returns_only_the_viewport() {
        let mut ts = AgentTabState::default();
        ts.cached_msg_lines = (0..100)
            .map(|i| vec![Line::raw(format!("line {i:03}"))])
            .collect();
        ts.cached_msg_row_counts = vec![1; 100];

        let (visible, relative_offset) =
            collect_visible_lines(&ts, ts.cached_msg_lines.len() - 2, 2, 20);

        assert_eq!(visible.len(), 2);
        assert_eq!(relative_offset, 0);
        assert_eq!(visible[0].to_string(), "line 098");
        assert_eq!(visible[1].to_string(), "line 099");
    }

    #[test]
    fn visible_line_collector_keeps_offset_inside_a_wrapped_line() {
        let mut ts = AgentTabState::default();
        ts.cached_msg_lines = vec![vec![Line::raw("aaaaaa")]];
        ts.cached_msg_row_counts = vec![3];

        let (visible, relative_offset) = collect_visible_lines(&ts, 2, 1, 2);

        assert_eq!(visible.len(), 1);
        assert_eq!(relative_offset, 2);
    }

    #[test]
    fn pending_preview_has_bounded_height() {
        assert_eq!(pending_queue_height(0, 80, 24), 0);
        assert_eq!(pending_queue_height(2, 80, 24), 3);
        assert_eq!(pending_queue_height(5, 80, 24), 5);
        assert_eq!(pending_queue_height(2, 4, 24), 0);
        assert_eq!(pending_queue_height(2, 80, 6), 0);
    }

    #[test]
    fn pending_preview_collapses_and_truncates() {
        assert_eq!(preview_line("  hello\n  world  ", 40), "  > hello world");
        assert_eq!(preview_line("abcdefghij", 12), "  > abcdefg…");
        assert_eq!(preview_line("中文消息", 10), "  > 中文…");
    }
}
