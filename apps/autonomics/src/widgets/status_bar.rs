use ratatui::{
    layout::Rect,
    prelude::Widget,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr};

use crate::state::{AgentStatus, CompactState};
use crate::widgets::agent_workspace::display_name;

/// 1-row status line showing the active agent's name, agent state, token
/// counts, and active model. `AgentLeaf` renders it into the composer row
/// while the input buffer is empty, so it costs no dedicated row of its own.
///
/// The agent state is a colored symbol prefixed directly to the name
/// (`○ bmc_a_1`) instead of a segment of its own; the textual label is
/// dropped, and when there is no name to anchor the symbol the old
/// `symbol label` segment returns. Name + tokens are left-aligned; the
/// model name is right-aligned on the same row so both fit in a single
/// line. The name is shown without the shared workspace-root prefix and is
/// truncated (keeping the tail — the distinguishing part of
/// workspace-path names) when the remaining segments leave no room. While
/// compacting, a live pass-detail segment (phase, scale, generated tokens,
/// elapsed seconds) keeps its spot after the name.
pub struct StatusBar<'a> {
    /// Active agent's name; follows the status symbol. `None` (or no room)
    /// falls back to the `symbol label` status segment.
    pub agent_name: Option<&'a str>,
    pub status: &'a AgentStatus,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    /// Total prompt tokens billed for the latest turn (input + cache_read +
    /// cache_creation). Counts against the model's context window.
    pub context_used: u64,
    /// The active model's context window size, when known.
    pub context_window: Option<u64>,
    pub model_name: Option<&'a str>,
    /// The reasoning effort the active agent's next request would send
    /// (rendered next to the model name, e.g. `glm-5.3 ⋅ high`).
    pub reasoning_effort: Option<&'a str>,
    /// Live compaction state — consulted only while status is `Compacting`.
    pub compact: Option<&'a CompactState>,
    /// Resident KMS worker activity, e.g. `2/4 running`. Rendered as its
    /// own separated segment after the agent name.
    pub kms_activity: Option<&'a str>,
    /// Render tick, drives the compacting spinner.
    pub frame: u64,
}

/// Rotating indicator for the compacting state.
const COMPACT_SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];

/// Segment separator used throughout the status bar.
const SEPARATOR: &str = "  │  ";

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer) {
        if area.width == 0 {
            return;
        }
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

        let out_tok = format_tokens(self.output_tokens);
        let ctx_used = format_tokens(self.context_used);
        // `i:` shows the absolute prompt size (input + cache_read + cache_creation).
        // Anthropic's `input_tokens` is uncached-only; adding the cache portions
        // here gives the user the full token count billed for the prompt.
        let absolute_in = self.input_tokens + self.cache_read_tokens + self.cache_creation_tokens;
        let absolute_in_tok = format_tokens(absolute_in);
        let cache_hit = self.cache_read_tokens + self.cache_creation_tokens;

        // Leading pair: status symbol followed by the agent name (fitted
        // below). With no name to anchor the symbol, fall back to the old
        // `symbol label` segment so the state stays readable.
        let mut spans = vec![
            Span::styled(indicator, Style::default().fg(indicator_color)),
            Span::raw(" "),
        ];
        // True while a compaction pass is actually in flight.
        let actively_compacting = matches!(
            (self.status, self.compact),
            (AgentStatus::Compacting, Some(c)) if c.is_compacting
        );
        let mut tail: Vec<Span> = Vec::new();
        // While compacting, the live pass detail keeps its own segment after
        // the name — it is progress readout, not a static label.
        if actively_compacting {
            tail.push(Span::raw(SEPARATOR));
            tail.push(Span::styled(
                status_text.clone(),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        // KMS worker readout keeps its own separated segment in the tail so
        // the name stays glued to the status symbol (`○ name`) instead of to
        // the activity text (`kms: 1/6 runningname`).
        if let Some(kms_activity) = self.kms_activity {
            tail.push(Span::raw(SEPARATOR));
            tail.push(Span::styled("kms:", Style::default().fg(Color::DarkGray)));
            tail.push(Span::raw(" "));
            tail.push(Span::styled(kms_activity, Style::default().fg(Color::Cyan)));
        }
        tail.push(Span::raw(SEPARATOR));
        tail.push(Span::styled(
            format!("i: {}", absolute_in_tok),
            Style::default().fg(Color::Gray),
        ));
        // Cache annotation: shown only when cache > 0.
        if cache_hit > 0 {
            tail.push(Span::raw(" "));
            tail.push(Span::styled(
                format!("({})", format_tokens(cache_hit)),
                Style::default().fg(Color::DarkGray),
            ));
        }
        tail.push(Span::raw("  "));
        tail.push(Span::styled(
            format!("o: {}", out_tok),
            Style::default().fg(Color::Gray),
        ));

        // Context window usage — absolute tokens only, unlabeled (the
        // used/window fraction is self-describing); the fraction's color
        // alone carries the pressure signal (dark gray normally, then
        // yellow, red).
        if self.context_used > 0 {
            tail.push(Span::raw(SEPARATOR));
            if let Some(window) = self.context_window {
                if window > 0 {
                    let pct = (self.context_used * 100) / window;
                    let pct_clamped = pct.min(100);
                    let color = match pct_clamped {
                        0..=60 => Color::DarkGray,
                        61..=85 => Color::Yellow,
                        _ => Color::Red,
                    };
                    tail.push(Span::styled(
                        format!("{}/{}k", ctx_used, window / 1_000),
                        Style::default().fg(color),
                    ));
                } else {
                    tail.push(Span::styled(ctx_used, Style::default().fg(Color::Gray)));
                }
            } else {
                tail.push(Span::styled(ctx_used, Style::default().fg(Color::Gray)));
            }
        }

        // ── Active agent name (follows the status symbol) ──
        // Fit the name into whatever width the symbol, token segments, and
        // the right-aligned model segment leave free; skip it entirely on
        // terminals too narrow to show a useful fragment.
        let mut name_rendered = false;
        if let Some(name) = self.agent_name {
            let model_reserve: usize = self
                .model_name
                .map(|name| {
                    let effort = self
                        .reasoning_effort
                        .map(|effort| format!(" ⋅ {effort}"))
                        .unwrap_or_default();
                    name.width() + effort.width()
                })
                .unwrap_or(0);
            let used: usize = spans
                .iter()
                .chain(tail.iter())
                .map(|s| s.content.width())
                .sum();
            let avail = (area.width as usize).saturating_sub(used + model_reserve + 1);
            if avail >= 4 {
                spans.push(Span::styled(
                    fit_name(display_name(name), avail),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ));
                name_rendered = true;
            }
        }
        if !name_rendered {
            // No name (or no room): keep the textual status label next to
            // the symbol. Its text is the compacting detail while
            // compacting, so drop the duplicate `separator + detail` head
            // from the tail.
            if actively_compacting {
                tail.drain(0..2);
            }
            spans.push(Span::styled(
                status_text,
                Style::default()
                    .fg(indicator_color)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        spans.extend(tail);

        let line = Line::from(spans);

        Paragraph::new(line).render(area, buf);

        // ── Model name + reasoning effort (right-aligned, no label: the
        // provider-qualified name is self-describing, and the freed columns
        // go to the agent name's truncation budget) ──
        if let Some(name) = self.model_name {
            let effort_seg = self.reasoning_effort.map(|effort| format!(" ⋅ {effort}"));
            let effort_str = effort_seg.as_deref().unwrap_or("");
            let total_w = name.width() + effort_str.width();
            if total_w < area.width as usize {
                let x = area.x + area.width - total_w as u16;
                let y = area.y;
                buf.set_string(x, y, name, Style::default().fg(Color::Gray));
                buf.set_string(
                    x + name.width() as u16,
                    y,
                    effort_str,
                    Style::default().fg(Color::Cyan),
                );
            }
        }
    }
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

/// Fit an agent name into `max` display columns. When truncating, keep the
/// tail: agent names are often workspace paths whose distinguishing part
/// (`…/fig4`) lives at the end, while the head (`/root/…`) is shared.
fn fit_name(name: &str, max: usize) -> String {
    if name.width() <= max {
        return name.to_string();
    }
    if max < 2 {
        return "…".to_string();
    }
    let budget = max - 1; // leading ellipsis
    let mut kept = String::new();
    let mut used = 0usize;
    for ch in name.chars().rev() {
        let ch_width = ch.width().unwrap_or(0);
        if used + ch_width > budget {
            break;
        }
        kept.insert(0, ch);
        used += ch_width;
    }
    format!("…{kept}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render a 1-row status bar into a fresh buffer and return the row as
    /// a plain string (styled spans joined).
    fn rendered(bar: StatusBar<'_>, width: u16) -> String {
        let mut buf = ratatui::prelude::Buffer::empty(Rect::new(0, 0, width, 1));
        bar.render(Rect::new(0, 0, width, 1), &mut buf);
        buf.content
            .iter()
            .map(|cell| cell.symbol().to_string())
            .collect()
    }

    #[test]
    fn name_follows_the_symbol_and_model_sits_right() {
        let idle = AgentStatus::Idle;
        let text = rendered(
            StatusBar {
                agent_name: Some("/root/gwas"),
                status: &idle,
                input_tokens: 1_234,
                output_tokens: 5,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                context_used: 1_239,
                context_window: Some(200_000),
                model_name: Some("zai:glm-5.3"),
                reasoning_effort: Some("high"),
                compact: None,
                kms_activity: None,
                frame: 0,
            },
            80,
        );
        assert!(text.starts_with("○ gwas"), "got: {text}");
        assert!(text.contains("i: 1.2k"), "got: {text}");
        assert!(text.contains("1.2k/200k"), "got: {text}");
        assert!(text.ends_with("zai:glm-5.3 ⋅ high"), "got: {text}");
    }

    #[test]
    fn no_name_falls_back_to_the_status_label() {
        let err = AgentStatus::Error;
        let text = rendered(
            StatusBar {
                agent_name: None,
                status: &err,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                context_used: 0,
                context_window: None,
                model_name: None,
                reasoning_effort: None,
                compact: None,
                kms_activity: None,
                frame: 0,
            },
            40,
        );
        assert!(text.contains("✗ error"), "got: {text}");
    }

    #[test]
    fn truncation_keeps_the_distinguishing_tail() {
        let fitted = fit_name("/root/very/long/path/fig4", 10);
        assert!(fitted.starts_with('…'), "got: {fitted}");
        assert!(fitted.ends_with("fig4"), "got: {fitted}");
    }

    #[test]
    fn token_formatter_uses_compact_units() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_500), "1.5k");
        assert_eq!(format_tokens(2_500_000), "2.5M");
    }
}
