use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::{StatefulWidget, Widget},
    style::{Color, Modifier, Style},
    widgets::{Block, Padding, Paragraph},
};

use crate::state::{AgentStatus, AgentTabState, InputMode};
use crate::widgets::{
    chat_widget::{ChatWidget, ChatWidgetState},
    input_area::{InputWidget, InputWidgetState, PROMPT_GUTTER},
    status_bar::StatusBar,
    tool_exec_widget::ToolExecWidget,
};

/// Composite widget that renders the entire Agent tab: status bar, chat area with
/// scrollbar, borderless input area, and a keybinding hint footer.
pub struct AgentTabWidget<'a> {
    pub state: &'a mut AgentTabState,
    /// Active model name shown on the composer border.
    pub active_model: Option<&'a str>,
}

impl Widget for AgentTabWidget<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer) {
        let task_count = self.state.tool_tasks.len() as u16;
        let task_constraint = if task_count > 0 {
            Constraint::Length(task_count + 2) // +2 for border
        } else {
            Constraint::Length(0)
        };

        // Dynamic input height: the boxed composer grows with content
        // (word-wrapped), capped at MAX_INPUT_ROWS text rows. The widget draws
        // a rounded border box, so reserve +2 rows (top/bottom) and subtract
        // the border columns (+ gutter) from the wrap width.
        let running = self.state.status != crate::state::AgentStatus::Idle;
        // area.width − 2 (box borders) − 2 (❯ prefix gutter).
        let text_width = area.width.saturating_sub(PROMPT_GUTTER + 2);
        let input_text_rows = if running && self.state.input.is_empty() {
            // While the agent runs, keep the composer collapsed to one text row.
            1
        } else {
            self.state.input.display_height(text_width)
        };
        let input_constraint = Constraint::Length(input_text_rows + 2);

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // StatusBar (includes model name)
                task_constraint,       // ToolExecWidget (0 when empty)
                Constraint::Min(3),    // Chat (shrinks as input grows)
                input_constraint,      // Input (dynamic, borderless)
                Constraint::Length(1), // Footer hints
            ])
            .split(area);

        let ts = &mut *self.state;

        // ── StatusBar ──
        let status_bar = StatusBar {
            status: &ts.status,
            input_tokens: ts.input_tokens,
            output_tokens: ts.output_tokens,
            cache_read_tokens: ts.cache_read_tokens,
            model_name: self.active_model,
            is_compacting: ts.compact_state.is_compacting,
        };
        status_bar.render(layout[0], buf);

        // ── Tool execution panel ──
        if task_count > 0 {
            let tool_widget = ToolExecWidget {
                tasks: &ts.tool_tasks,
            };
            tool_widget.render(layout[1], buf);
        }

        // ── Chat area ──
        let chat_area = layout[2];
        let chat_block = Block::default().padding(Padding::new(2, 2, 2, 2));
        let chat_inner_area = chat_block.inner(chat_area);

        let viewport_height = chat_inner_area.height;
        let width = chat_inner_area.width;
        let cache_hit = ts.cached_version == ts.messages_version
            && ts.cached_width == width
            && !ts.cached_lines.is_empty();

        let cached = if cache_hit {
            Some(ts.cached_lines.clone())
        } else {
            None
        };

        let mut chat_state = ChatWidgetState::new(ts.scroll_offset);
        let chat_widget = ChatWidget {
            messages: &ts.messages,
            cached_lines: cached,
        };

        chat_widget.render(chat_inner_area, buf, &mut chat_state);
        ts.content_line_count = chat_state.total_lines;

        // Clamp scroll *after* render so we use the current frame's line count.
        ts.clamp_scroll(viewport_height);

        // Update cache: store freshly rendered lines if this was a cache miss.
        if let Some(fresh) = chat_state.rendered_lines.take() {
            ts.cached_lines = fresh;
            ts.cached_width = width;
            ts.cached_version = ts.messages_version;
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
        // Status label inlined in the top border (top-left), reflecting what
        // the agent is doing right now. While active, a braille spinner is
        // prepended for a visual loading indicator.
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
            // Editable only when composing (Input mode) and the agent is idle.
            // Browse mode hides the caret and the app routes keys away from the
            // composer in that mode (see App::handle_key).
            editable: !running && ts.input_mode == InputMode::Input,
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
///
/// A bright segment (`━`) slides across the border left→right, wraps around,
/// and repeats. The rest of the border remains the default `─`. The effect is
/// a "scrolling" progress indicator — like a download bar — driven by `frame`.
fn render_loading_bar(area: Rect, buf: &mut ratatui::prelude::Buffer, frame: u64) {
    // Bottom border row sits at y = area bottom; inner span excludes corners.
    let y = area.y + area.height - 1;
    let start = area.x + 1;
    let end = area.x + area.width.saturating_sub(1);
    let width = end.saturating_sub(start) as usize;
    if width < 4 {
        return;
    }

    // Segment length: ~1/4 of the border, clamped to a sensible range.
    let bar_len = (width / 4).clamp(3, 12);

    // Animation speed: advance 1 position every 2 frames (~30 fps at 60 fps render).
    let pos = ((frame / 2) as usize) % (width + bar_len);

    let bar_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    for i in 0..width {
        // The bar occupies indices [pos - bar_len, pos) clamped to [0, width).
        // When pos > width, the bar has partially wrapped to the start.
        let in_bar = if pos < bar_len {
            // Bar straddles the wrap boundary: tail at [0, pos) and
            // head at [width + pos - bar_len, width).
            i < pos || i >= width + pos - bar_len
        } else if pos > width {
            // Past the right edge: wrapped portion [0, pos - width).
            i < pos - width
        } else {
            // Normal: bar fully inside [pos - bar_len, pos).
            i >= pos - bar_len && i < pos
        };
        if in_bar {
            buf.set_string(start + i as u16, y, "━", bar_style);
        }
    }
}

/// Draw the 1-row keybinding hint at the bottom of the agent tab. The hint
/// changes with the active input mode so the relevant shortcuts are always
/// visible, mirroring codex's footer. While a Ctrl+R search is active, the
/// query and current match position are shown instead.
#[allow(clippy::too_many_arguments)]
fn render_footer_hint(
    area: Rect,
    buf: &mut ratatui::prelude::Buffer,
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
                " Enter edit  ↑↓ scroll  PageUp/PageDown  Home/End  Ctrl+G follow  Ctrl+C quit ".to_string()
            }
            InputMode::Input => {
                " Insert  Esc exit  Enter send  Shift+Enter newline  Ctrl+R history  Ctrl+G follow ".to_string()
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
