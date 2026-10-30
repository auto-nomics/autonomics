use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::{Buffer, StatefulWidget, Widget},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::StatefulWidgetRef,
    widgets::{Block, Padding, Paragraph},
};
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr};

use crate::state::{AgentStatus, AgentTabState, DisplaySettings, InputMode};
use crate::widgets::{
    chat_widget::{ChatWidget, ChatWidgetState, visual_row_count},
    input_area::{InputWidget, InputWidgetState, PROMPT_GUTTER},
    status_bar::StatusBar,
};

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

/// Renders a single agent's conversation surface (full width).
///
/// Layout:
/// ```text
/// ┌──────────────────────────────────────────────────┐
/// │ StatusBar (full width, 1 row)                    │
/// ├──────────────────────────────────────────────────┤
/// │ Chat area (scrollable transcript, full width)    │
/// ├──────────────────────────────────────────────────┤
/// │ Pending input / compaction preview (conditional) │
/// │ ╭─ running… ─────────────────────────────────╮   │
/// │ │ ❯ draft…                                   │   │
/// │ ╰────────────────────────────────────────────╯   │
/// └──────────────────────────────────────────────────┘
/// ```
pub struct AgentLeaf<'a> {
    pub active_model: Option<&'a str>,
    /// Context window size of the active model (tokens), when known.
    pub context_window: Option<u64>,
    /// The reasoning effort the active agent's next request would send.
    pub reasoning_effort: Option<&'a str>,
    pub kms_activity: Option<&'a str>,
    pub display: &'a DisplaySettings,
}

impl StatefulWidgetRef for AgentLeaf<'_> {
    type State = AgentTabState;

    fn render_ref(&self, area: Rect, buf: &mut Buffer, ts: &mut AgentTabState) {
        let running = ts.status.is_active() || ts.status == AgentStatus::Waiting;

        // Dynamic input height: the borderless composer grows with
        // content (word-wrapped); only the ❯ gutter is subtracted from
        // the wrap width.
        let text_width = area.width.saturating_sub(PROMPT_GUTTER);
        let input_text_rows = if running && ts.input.is_empty() {
            2
        } else {
            ts.input.display_height(text_width).max(2)
        };
        let input_constraint = Constraint::Length(input_text_rows);
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
                Constraint::Min(3),    // Chat (full width)
                pending_constraint,    // Runtime-bound input awaiting commit
                compact_constraint,    // Live compaction preview
                Constraint::Length(1), // StatusBar (sits on the shared band)
                Constraint::Length(1),
                input_constraint, // Composer (same band; borderless)
            ])
            .split(area);

        // ── Chat area (full width) ──
        let middle = layout[0];

        // Left/right keep a 2-column gutter; the top padding is 0 so the
        // transcript starts right under the status bar, and the bottom
        // keeps breathing room above the composer.
        let chat_block = Block::default().padding(Padding::new(2, 2, 0, 0));
        let chat_inner_area = chat_block.inner(middle);

        let viewport_height = chat_inner_area.height;
        let width = chat_inner_area.width;
        // Remember the viewport height for the h/l horizontal scroll step.
        ts.chat_viewport_height = viewport_height;

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

        // Runtime-bound prompts stay out of the transcript until the Session
        // commits them; show a compact preview immediately above the composer.
        if !pending_texts.is_empty() {
            render_pending_queue(layout[1], buf, &pending_texts);
        }

        // ── Live compaction preview (summary tail while compacting) ──
        if ts.compact_state.is_compacting {
            render_compact_preview(layout[2], buf, &ts.compact_state);
        }

        // ── StatusBar + composer: one merged band ──
        // The status bar sits directly above the composer and both share
        // the same dark-navy background, so lifecycle state and the input
        // read as a single bottom block. The chat's bottom padding is the
        // only separation from the transcript.
        let band = Style::default().bg(Color::Rgb(24, 34, 46));
        let status_bar = StatusBar {
            status: &ts.status,
            context_used: ts.latest_turn_context_used,
            context_window: self.context_window,
            model_name: self.active_model,
            reasoning_effort: self.reasoning_effort,
            compact: Some(&ts.compact_state),
            kms_activity: self.kms_activity,
            frame: ts.frame,
        };
        status_bar.render(layout[3], buf);

        let queued = ts.pending_queue_len();
        let placeholder: &str = if running {
            match ts.input_mode {
                InputMode::Input => "Type to queue a message… (Ctrl+P → Cancel agent to cancel)",
                InputMode::Browse => {
                    if queued > 0 {
                        "Agent running — pending input waits for next request…"
                    } else {
                        "Agent running… (Ctrl+P → Cancel agent to cancel)"
                    }
                }
            }
        } else {
            match ts.input_mode {
                InputMode::Browse => "Type a message, or press Enter to edit…",
                InputMode::Input => "Type a message…",
            }
        };

        let padding_band = Block::new().style(band);
        padding_band.render(layout[4], buf);

        // The composer is interactive even while the agent is running:
        // the user can type and enqueue messages. Only hide the caret
        // when not in Input mode.
        let input_widget = InputWidget {
            editable: ts.input_mode == InputMode::Input,
            bg: band,
            placeholder,
        };
        let mut input_state = InputWidgetState {
            input: &mut ts.input,
        };
        input_widget.render(layout[5], buf, &mut input_state);
    }
}

/// Live view of an in-flight compaction: pass header (trigger + scale) and
/// the tail of the summary being generated.
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
