use ratatui::{
    layout::Rect,
    prelude::Widget,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

use crate::state::{AgentStatus, CompactState};

/// 1-row status bar showing agent state and active model, drawn on a
/// dark-navy background strip spanning the full width.
///
/// Status is left-aligned; kms activity and the context-window readout
/// follow as separated segments, and the model name is right-aligned on
/// the same row so both fit in a single line. While compacting, the
/// status segment expands with live pass details (phase, scale,
/// generated tokens, elapsed seconds).
pub struct StatusBar<'a> {
    pub status: &'a AgentStatus,
    /// Total prompt tokens billed for the latest turn (input + cache_read +
    /// cache_creation). Counts against the model's context window.
    pub context_used: u64,
    /// The active model's context window size, when known.
    pub context_window: Option<u64>,
    pub model_name: Option<&'a str>,
    /// The reasoning effort the active agent's next request would send
    /// (rendered next to the model name, e.g. `model glm-5.3 ⋅ high`).
    pub reasoning_effort: Option<&'a str>,
    /// Live compaction state — consulted only while status is `Compacting`.
    pub compact: Option<&'a CompactState>,
    /// Resident KMS worker activity, e.g. `2/4 running`.
    pub kms_activity: Option<&'a str>,
    /// Render tick, drives the compacting spinner.
    pub frame: u64,
}

/// Rotating indicator for the compacting state.
const COMPACT_SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer) {
        // Compaction takes over the status display so the user can see the
        // agent is summarizing context, not idle/requesting.
        let (base_indicator, indicator_color, base_text) = match self.status {
            AgentStatus::Idle | AgentStatus::Aborted => ("○", Color::Gray, "idle"),
            AgentStatus::Requesting => ("◐", Color::Yellow, "requesting"),
            AgentStatus::Streaming => ("●", Color::Green, "streaming"),
            AgentStatus::ToolRunning => ("⚙", Color::Cyan, "tool running"),
            AgentStatus::Retrying => ("↻", Color::Yellow, "retrying"),
            AgentStatus::Error => ("✗", Color::Red, "error"),
            AgentStatus::Cancelled => ("⊘", Color::DarkGray, "cancelled"),
            AgentStatus::Compacting => ("◐", Color::Magenta, "compacting"),
            AgentStatus::Waiting => ("☐", Color::Blue, "waiting"),
        };

        // While compacting, replace the static label with a spinning
        // indicator and the live pass detail line.
        let indicator: std::borrow::Cow<str> = match (self.status, self.compact) {
            (AgentStatus::Compacting, Some(c)) if c.is_compacting => std::borrow::Cow::Owned(
                COMPACT_SPINNER[(self.frame % COMPACT_SPINNER.len() as u64) as usize].to_string(),
            ),
            _ => std::borrow::Cow::Borrowed(base_indicator),
        };
        let status_text: String = match (self.status, self.compact) {
            (AgentStatus::Compacting, Some(c)) if c.is_compacting => compact_status_text(c),
            _ => base_text.to_string(),
        };

        let ctx_used = format_tokens(self.context_used);

        // Dark-navy strip behind the whole row; every span (and the gap
        // filler) carries it so the bar reads as one solid band.
        let bg = Style::default().bg(Color::Rgb(24, 34, 46));
        let base = |style: Style| style.patch(bg);

        let mut spans = vec![
            Span::styled(indicator, base(Style::default().fg(indicator_color))),
            Span::styled(" ", bg),
            Span::styled(
                status_text,
                base(
                    Style::default()
                        .fg(indicator_color)
                        .add_modifier(Modifier::BOLD),
                ),
            ),
        ];

        if let Some(kms_activity) = self.kms_activity {
            // No separator between the status label and the kms readout —
            // they read as one left-hand group. The ctx segment keeps its
            // own separator below.
            spans.push(Span::styled("  ", bg));
            spans.push(Span::styled(
                "kms:",
                base(Style::default().fg(Color::DarkGray)),
            ));
            spans.push(Span::styled(" ", bg));
            spans.push(Span::styled(
                kms_activity,
                base(Style::default().fg(Color::Cyan)),
            ));
        }

        // Context window usage — shows percentage, bar, and absolute tokens.
        if self.context_used > 0 {
            spans.push(Span::styled("  │  ", bg));
            spans.push(Span::styled("ctx:", base(Style::default().fg(Color::Gray))));
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
                    spans.push(Span::styled(" ", bg));
                    spans.push(Span::styled(
                        format!("{pct}%"),
                        base(Style::default().fg(color)),
                    ));
                    spans.push(Span::styled(" ", bg));
                    spans.push(Span::styled(bar, base(Style::default().fg(color))));
                    spans.push(Span::styled(" ", bg));
                    spans.push(Span::styled(
                        format!("{}/{}k", ctx_used, window / 1_000),
                        base(Style::default().fg(Color::DarkGray)),
                    ));
                } else {
                    spans.push(Span::styled(" ", bg));
                    spans.push(Span::styled(
                        ctx_used,
                        base(Style::default().fg(Color::Gray)),
                    ));
                }
            } else {
                spans.push(Span::styled(" ", bg));
                spans.push(Span::styled(
                    ctx_used,
                    base(Style::default().fg(Color::Gray)),
                ));
            }
        }

        // Pad the row to its full width so the background strip is
        // continuous; the right-aligned model overlaps the padding.
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        let model_reserve = self
            .model_name
            .map(|_| {
                "model ".width()
                    + self
                        .reasoning_effort
                        .map(|e| format!(" ⋅ {e}").width())
                        .unwrap_or(0)
            })
            .unwrap_or(0);
        let pad = (area.width as usize).saturating_sub(used + model_reserve);
        spans.push(Span::styled(" ".repeat(pad), bg));

        let line = Line::from(spans);

        Paragraph::new(line).render(area, buf);

        // ── Model name + reasoning effort (right-aligned) ──
        if let Some(name) = self.model_name {
            // let label = "model ";
            let effort_seg = self.reasoning_effort.map(|effort| format!(" ⋅ {effort}"));
            let effort_str = effort_seg.as_deref().unwrap_or("");
            let total_w = name.width() + effort_str.width();
            if total_w < area.width as usize {
                let x = area.x + area.width - total_w as u16;
                let y = area.y;
                buf.set_string(x as u16, y, name, base(Style::default().fg(Color::Gray)));
                buf.set_string(
                    x + (name.width()) as u16,
                    y,
                    effort_str,
                    base(Style::default().fg(Color::Cyan)),
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

/// Live compaction detail line, e.g.
/// `compacting ▸ summarizing 38.4k tok (12 msgs) · 1.2k generated · 34s`.
fn compact_status_text(c: &CompactState) -> String {
    use agentik_types::CompactPhase;

    let phase = match c.phase {
        Some(CompactPhase::Summarizing) => "summarizing",
        Some(CompactPhase::Rebuilding) => "rebuilding",
        None => "preparing",
    };
    let mut text = format!("compacting ▸ {phase}");
    if let Some(plan) = &c.plan {
        text.push_str(&format!(
            " {} tok ({} msgs)",
            format_tokens(plan.head_tokens),
            plan.head_messages
        ));
    }
    if !c.summary.is_empty() {
        // chars/4 matches the backend's estimate heuristic.
        text.push_str(&format!(
            " · {} generated",
            format_tokens((c.summary.len() / 4) as u64)
        ));
    }
    if let Some(started) = c.started_at {
        text.push_str(&format!(" · {}s", started.elapsed().as_secs()));
    }
    text
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
