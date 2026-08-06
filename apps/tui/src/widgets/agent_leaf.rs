use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::{Buffer, StatefulWidget, Widget},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph, StatefulWidgetRef},
};

use crate::state::{AgentStatus, AgentTabState, DisplaySettings, InputMode};
use crate::widgets::{
    chat_widget::{ChatWidget, ChatWidgetState},
    input_area::{InputWidget, InputWidgetState, PROMPT_GUTTER},
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
    pub session_title: Option<&'a str>,
    pub session_index: usize,
    pub session_count: usize,
    pub display: &'a DisplaySettings,
}

impl StatefulWidgetRef for AgentLeaf<'_> {
    type State = AgentTabState;

    fn render_ref(&self, area: Rect, buf: &mut Buffer, ts: &mut AgentTabState) {
        let running = ts.status != crate::state::AgentStatus::Idle;

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

        // ── Top-level vertical split ──
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // StatusBar (full width)
                Constraint::Min(3),    // Middle: Chat + Sidebar (horizontal split)
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
                    Constraint::Min(20),          // Chat (flexible)
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
                    session_title: self.session_title,
                    session_index: self.session_index,
                    session_count: self.session_count,
                },
            };
            sidebar.render(sb_area, buf);
        }

        // ── Input area (boxed composer, ❯ prompt) ──
        let placeholder: &str = if running {
            "agent running… (Ctrl+C to cancel)"
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
        let title: String = match ts.status {
            AgentStatus::Requesting => format!("{spinner} thinking…"),
            AgentStatus::Streaming => format!("{spinner} responding…"),
            AgentStatus::Error => "error".to_string(),
            AgentStatus::Idle => match ts.input_mode {
                InputMode::Browse => "browse".to_string(),
                InputMode::Input => "compose".to_string(),
            },
        };

        let input_widget = InputWidget {
            disabled: running,
            editable: !running && ts.input_mode == InputMode::Input,
            title: &title,
            placeholder,
        };
        let mut input_state = InputWidgetState {
            input: &mut ts.input,
        };
        input_widget.render(layout[2], buf, &mut input_state);

        // ── Animated loading bar on the input box bottom border ──
        if running {
            render_loading_bar(layout[2], buf, ts.frame);
        }

        // ── Footer hint line ──
        render_footer_hint(
            layout[3],
            buf,
            ts.input_mode,
            running,
            ts.in_history_search,
            &ts.history_search_query,
            ts.history_search_selected,
            ts.history_search_matches.len(),
        );
    }
}

/// Braille spinner frames for the loading indicator.
const BRAILLE_SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
        " Ctrl+C cancel  Ctrl+R history ".to_string()
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
