//! Skill-evolution dashboard: the loop's control surface in the TUI.
//!
//! One overlay shows the whole picture — service configuration,
//! library generation, observation/proposal counts, the pending
//! queue with selection — and the key bindings act on it (trigger a
//! cycle, approve/reject the selected pending proposal). Data is a
//! snapshot fetched over the gateway API; the render path never
//! issues HTTP (same rule as every other widget).

use gateway::proto::{SkillEvolutionStatus, SkillProposalView};

/// Dashboard state: data snapshots plus selection. Owned by the app
/// state; refreshes land via [`crate::app_event::AppEvent`].
#[derive(Debug, Clone, Default)]
pub struct SkillEvolutionState {
    pub visible: bool,
    pub status: Option<SkillEvolutionStatus>,
    /// Proposal listing, pending first (the endpoint's order).
    pub proposals: Vec<SkillProposalView>,
    pub selected: usize,
    /// One-line summary of the most recent cycle (from a trigger run
    /// in this TUI session).
    pub last_report: Option<String>,
    /// Last refresh error, rendered in place of the status block.
    pub error: Option<String>,
}

impl SkillEvolutionState {
    pub fn open(&mut self) {
        self.visible = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    /// Move the selection within the pending-proposal rows only —
    /// approved/rejected rows are history, not targets.
    pub fn pending_indices(&self) -> Vec<usize> {
        self.proposals
            .iter()
            .enumerate()
            .filter(|(_, p)| p.status == "pending")
            .map(|(i, _)| i)
            .collect()
    }

    pub fn select_next(&mut self) {
        let pending = self.pending_indices();
        if pending.is_empty() {
            return;
        }
        let pos = pending.iter().position(|i| *i == self.selected);
        self.selected = match pos {
            Some(p) if p + 1 < pending.len() => pending[p + 1],
            Some(_) => pending[0],
            None => pending[0],
        };
    }

    pub fn select_prev(&mut self) {
        let pending = self.pending_indices();
        if pending.is_empty() {
            return;
        }
        let pos = pending.iter().position(|i| *i == self.selected);
        self.selected = match pos {
            Some(p) if p > 0 => pending[p - 1],
            Some(_) => *pending.last().unwrap(),
            None => pending[0],
        };
    }

    /// The pending proposal under the selection, if any.
    pub fn selected_pending(&self) -> Option<&SkillProposalView> {
        self.proposals
            .get(self.selected)
            .filter(|p| p.status == "pending")
    }
}

/// Render the dashboard into the popup's inner area.
pub fn render_skill_evolution(
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
    state: &mut SkillEvolutionState,
) {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};

    let dim = Style::new().fg(Color::DarkGray);
    let normal = Style::new().fg(Color::Reset);
    let accent = Style::new().fg(Color::Cyan);
    let warn = Style::new().fg(Color::Yellow);
    let bold = Style::new().add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line> = Vec::new();

    if let Some(error) = &state.error {
        lines.push(Line::styled(format!("error: {error}"), warn));
        lines.push(Line::styled("press g to retry, Esc to close", dim));
        render_lines(area, buf, lines, state);
        return;
    }

    let Some(status) = &state.status else {
        lines.push(Line::styled("loading evolution status…", dim));
        render_lines(area, buf, lines, state);
        return;
    };

    // ── configuration line ──
    let service = if status.service_enabled {
        Span::styled("on".to_string(), accent)
    } else {
        Span::styled("off".to_string(), warn)
    };
    let gate = if status.auto_approve {
        Span::styled("auto-approve".to_string(), warn)
    } else {
        Span::styled("propose-only".to_string(), accent)
    };
    let cap = status.max_approvals_per_cycle.to_string();
    let sweep = status
        .sweep_interval_secs
        .map(|s| format!("sweep {}s", s))
        .unwrap_or_else(|| "no sweep".to_string());
    lines.push(Line::from(vec![
        Span::styled("service ", dim),
        service,
        Span::styled("  ·  gate ", dim),
        gate,
        Span::styled(format!(" (cap {cap})"), dim),
        Span::styled("  ·  ", dim),
        Span::styled(sweep, normal),
        Span::styled(format!("  ·  generation {}", status.generation), bold),
    ]));

    // ── counters line ──
    lines.push(Line::from(vec![
        Span::styled("observations ", dim),
        Span::styled(format!("{}", status.observations), normal),
        Span::styled("   skills ", dim),
        Span::styled(
            format!(
                "{} auto / {} global / {} workspace / {} builtin",
                status.skills_auto,
                status.skills_global,
                status.skills_workspace,
                status.skills_builtin
            ),
            normal,
        ),
        Span::styled("   proposals ", dim),
        Span::styled(
            format!(
                "{} pending · {} approved · {} rejected",
                status.proposals_pending, status.proposals_approved, status.proposals_rejected
            ),
            normal,
        ),
    ]));

    if let Some(report) = &state.last_report {
        lines.push(Line::styled(format!("last cycle: {report}"), accent));
    }
    lines.push(Line::default());

    // ── proposal queue ──
    if state.proposals.is_empty() {
        lines.push(Line::styled("no proposals yet", dim));
        lines.push(Line::styled(
            "record observations (skill_observe / failing evals); patterns surface at >= 3",
            dim,
        ));
    } else {
        lines.push(Line::styled("proposals (pending first)", bold));
        for (index, proposal) in state.proposals.iter().enumerate() {
            let selected = index == state.selected;
            let status_style = match proposal.status.as_str() {
                "pending" => Style::new().fg(Color::Yellow),
                "approved" => Style::new().fg(Color::Green),
                _ => Style::new().fg(Color::DarkGray),
            };
            let marker = if selected { "▶ " } else { "  " };
            let kind = if proposal.update { "update" } else { "create" };
            let author = if proposal.authored_by == "agent" {
                " · agent"
            } else {
                ""
            };
            let mut spans = vec![
                Span::styled(marker.to_string(), accent),
                Span::styled(proposal.name.clone(), if selected { bold } else { normal }),
                Span::styled(
                    format!(" [{} · {}{}]", kind, proposal.status, author),
                    status_style,
                ),
                Span::styled(format!("  ({} obs)", proposal.observation_count), dim),
            ];
            if !proposal.rationale.is_empty() {
                let used: usize =
                    proposal.name.chars().count() + kind.len() + proposal.status.len() + 24;
                let width = area.width.saturating_sub(2) as usize;
                spans.push(Span::styled(
                    format!(
                        "  {}",
                        truncate(&proposal.rationale, width.saturating_sub(used))
                    ),
                    dim,
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    lines.push(Line::default());
    lines.push(Line::styled(
        "t trigger · T trigger+auto · a approve · r reject · g refresh · Esc close",
        dim,
    ));

    render_lines(area, buf, lines, state);
}

fn render_lines(
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
    lines: Vec<ratatui::text::Line<'static>>,
    state: &SkillEvolutionState,
) {
    use ratatui::widgets::Widget as _;
    let visible_height = area.height.saturating_sub(0) as usize;
    // Keep the selection in view: scroll so the selected row is visible.
    let mut scroll = 0;
    if lines.len() > visible_height && state.selected >= visible_height.saturating_sub(6) {
        scroll =
            (state.selected + 7 - visible_height).min(lines.len().saturating_sub(visible_height));
    }
    let paragraph = ratatui::widgets::Paragraph::new(lines).scroll((scroll as u16, 0));
    paragraph.render(area, buf);
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(name: &str, status: &str) -> SkillProposalView {
        SkillProposalView {
            name: name.into(),
            status: status.into(),
            authored_by: "distiller".into(),
            update: false,
            rationale: "three observations".into(),
            cluster_hash: "c-abc".into(),
            observation_count: 3,
            created_at: 1,
        }
    }

    #[test]
    fn selection_moves_within_pending_rows_only() {
        let mut state = SkillEvolutionState {
            proposals: vec![
                proposal("a-pending", "pending"),
                proposal("b-approved", "approved"),
                proposal("c-pending", "pending"),
                proposal("d-rejected", "rejected"),
            ],
            ..Default::default()
        };
        assert_eq!(state.selected, 0);
        state.select_next();
        assert_eq!(state.selected, 2, "skips the approved row");
        state.select_next();
        assert_eq!(state.selected, 0, "wraps to the first pending");
        state.select_prev();
        assert_eq!(state.selected, 2, "wraps backwards");
        assert_eq!(
            state.selected_pending().map(|p| p.name.clone()),
            Some("c-pending".into())
        );
    }

    #[test]
    fn no_pending_rows_means_no_selection_action() {
        let mut state = SkillEvolutionState {
            proposals: vec![proposal("only-history", "approved")],
            selected: 0,
            ..Default::default()
        };
        state.select_next();
        assert!(state.selected_pending().is_none());
    }

    #[test]
    fn render_shows_configuration_and_pending_rows() {
        use ratatui::backend::TestBackend;
        let mut terminal = ratatui::Terminal::new(TestBackend::new(100, 20)).unwrap();
        let mut state = SkillEvolutionState {
            status: Some(SkillEvolutionStatus {
                service_enabled: true,
                generation: 7,
                auto_approve: false,
                max_approvals_per_cycle: 5,
                sweep_interval_secs: Some(21600),
                observations: 12,
                skills_workspace: 0,
                skills_global: 1,
                skills_builtin: 1,
                skills_auto: 1,
                proposals_pending: 1,
                proposals_approved: 0,
                proposals_rejected: 0,
            }),
            proposals: vec![proposal("sql-syntax", "pending")],
            last_report: Some("1 written".into()),
            ..Default::default()
        };
        terminal
            .draw(|frame| render_skill_evolution(frame.area(), frame.buffer_mut(), &mut state))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("generation 7"), "{text}");
        assert!(text.contains("propose-only"));
        assert!(text.contains("sql-syntax"));
        assert!(text.contains("12"));
    }

    #[test]
    fn render_error_state_without_panicking() {
        use ratatui::backend::TestBackend;
        let mut terminal = ratatui::Terminal::new(TestBackend::new(80, 10)).unwrap();
        let mut state = SkillEvolutionState {
            error: Some("daemon unreachable".into()),
            ..Default::default()
        };
        terminal
            .draw(|frame| render_skill_evolution(frame.area(), frame.buffer_mut(), &mut state))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("daemon unreachable"));
    }
}
