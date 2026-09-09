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
    chat_widget::{ChatWidget, ChatWidgetState},
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
    pub agent_name: Option<&'a str>,
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

        // ── Top-level vertical split ──
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // StatusBar (full width)
                Constraint::Min(3),    // Middle: Chat + Sidebar (horizontal split)
                pending_constraint,    // Runtime-bound input awaiting commit
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
            is_compacting: ts.compact_state.is_compacting,
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
            ts.cached_msg_versions.resize(n, 0);
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

        let mut flat: Vec<ratatui::text::Line<'static>> = Vec::new();
        for (i, msg) in ts.messages.iter().enumerate() {
            if ts.cached_msg_versions[i] == ts.msg_versions[i] && !ts.cached_msg_lines[i].is_empty()
            {
                flat.extend_from_slice(&ts.cached_msg_lines[i]);
            } else {
                let rendered = super::chat_widget::render::render_line_with_settings(
                    msg,
                    chat_inner_area,
                    self.display,
                );
                flat.extend_from_slice(&rendered);
                ts.cached_msg_lines[i] = rendered;
                ts.cached_msg_versions[i] = ts.msg_versions[i];
            }
        }

        let mut chat_state = ChatWidgetState::new(ts.scroll_offset);
        let chat_widget = ChatWidget { lines: &flat };
        chat_widget.render(chat_inner_area, buf, &mut chat_state);
        ts.content_line_count = chat_state.total_lines;
        ts.clamp_scroll(viewport_height);

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
        input_widget.render(layout[3], buf, &mut input_state);

        // ── Animated loading bar on the input box bottom border ──
        if running {
            render_loading_bar(layout[3], buf, ts.frame);
        }

        // ── Footer hint line ──
        render_footer_hint(
            layout[4],
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
