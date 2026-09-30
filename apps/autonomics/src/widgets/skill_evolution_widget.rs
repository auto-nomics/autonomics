//! Skill-evolution dashboard: the loop's control surface in the TUI.
//!
//! One overlay shows the whole picture — service configuration,
//! library generation, observation/proposal counts, the pending
//! queue with selection — and the key bindings act on it (trigger a
//! cycle, approve/reject the selected pending proposal). Data is a
//! snapshot fetched over the gateway API; the render path never
//! issues HTTP (same rule as every other widget).
//!
//! Standard [`StatefulWidget`] split, matching the session picker:
//! [`SkillEvolutionWidget`] owns only the chrome (popup geometry and
//! accent), [`SkillEvolutionState`] — held by the app state — is the
//! state:
//!
//! ```ignore
//! use ratatui::widgets::StatefulWidget as _;
//! SkillEvolutionWidget::new()
//!     .popup_width(w)
//!     .popup_height(h)
//!     .render(area, buf, &mut state);
//! ```

use gateway::proto::{SkillEvolutionStatus, SkillProposalView};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
};

use crate::widgets::popup::{Popup, PopupControls};

const KEYBINDINGS: &str = "t trigger · T trigger+auto · a approve · r reject · g refresh · Esc close";

// ═══════════════════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════════════════

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
    /// Render-time selection for the proposal list; re-synced from
    /// `selected` on every render so the list scrolls to it.
    list_state: ListState,
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

    /// The row index to highlight: the selection only counts when it
    /// sits on a pending proposal (history rows never highlight).
    fn selected_row(&self) -> Option<usize> {
        self.selected_pending().map(|_| self.selected)
    }

    /// Point the list state at [`Self::selected_row`] so the list
    /// keeps the actionable selection scrolled into view.
    fn sync_list_state(&mut self) {
        self.list_state.select(self.selected_row());
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Widget
// ═══════════════════════════════════════════════════════════════════════

/// The dashboard as a centered popup: header (configuration +
/// counters + last cycle), the proposal queue as a scrolling list,
/// and a key-binding footer. Stateless beyond the chrome — all
/// mutable data lives in [`SkillEvolutionState`].
pub struct SkillEvolutionWidget {
    /// Popup accent color (title/border).
    pub accent: Color,
    /// Popup outer width in cells.
    pub popup_width: u16,
    /// Popup outer height in cells.
    pub popup_height: u16,
}

impl SkillEvolutionWidget {
    pub fn new() -> Self {
        Self {
            accent: Color::Cyan,
            popup_width: 0,
            popup_height: 0,
        }
    }

    pub fn popup_width(mut self, w: u16) -> Self {
        self.popup_width = w;
        self
    }

    pub fn popup_height(mut self, h: u16) -> Self {
        self.popup_height = h;
        self
    }
}

impl Default for SkillEvolutionWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl StatefulWidget for SkillEvolutionWidget {
    type State = SkillEvolutionState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut SkillEvolutionState) {
        if !state.visible {
            return;
        }

        let popup = Popup::new(" Skill Evolution ", PopupControls::default())
            .width(self.popup_width)
            .height(self.popup_height)
            .accent(self.accent);
        let inner = popup.render(area, buf);

        // ── error / loading short-circuits ──
        if let Some(error) = &state.error {
            let lines = vec![
                Line::styled(format!("error: {error}"), Style::new().fg(Color::Yellow)),
                Line::styled(
                    "press g to retry, Esc to close",
                    Style::new().fg(Color::DarkGray),
                ),
            ];
            Widget::render(Paragraph::new(lines), inner, buf);
            return;
        }
        let Some(status) = &state.status else {
            Widget::render(
                Paragraph::new(Line::styled(
                    "loading evolution status…",
                    Style::new().fg(Color::DarkGray),
                )),
                inner,
                buf,
            );
            return;
        };

        // ── Layout: header (2–4) + queue (rest) + footer (1) ──
        let mut header = vec![config_line(status), counters_line(status)];
        if let Some(report) = &state.last_report {
            header.push(Line::styled(
                format!("last cycle: {report}"),
                Style::new().fg(Color::Cyan),
            ));
        }
        header.push(Line::default());

        let regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(header.len() as u16),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(inner);
        Widget::render(Paragraph::new(header), regions[0], buf);
        self.render_queue(regions[1], buf, state);
        Widget::render(
            Paragraph::new(Line::styled(
                KEYBINDINGS,
                Style::new().fg(Color::DarkGray),
            )),
            regions[2],
            buf,
        );
    }
}

impl SkillEvolutionWidget {
    /// The proposal queue: section title + scrolling list. The list
    /// state carries the selection; ratatui scrolls it into view.
    fn render_queue(&self, area: Rect, buf: &mut Buffer, state: &mut SkillEvolutionState) {
        let dim = Style::new().fg(Color::DarkGray);

        if state.proposals.is_empty() {
            let lines = vec![
                Line::styled("no proposals yet", dim),
                Line::styled(
                    "record observations (skill_observe / failing evals); patterns surface at >= 3",
                    dim,
                ),
            ];
            Widget::render(Paragraph::new(lines), area, buf);
            return;
        }

        let regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);
        Widget::render(
            Paragraph::new(Line::styled(
                "proposals (pending first)",
                Style::new().add_modifier(Modifier::BOLD),
            )),
            regions[0],
            buf,
        );

        let list_area = regions[1];
        let selected = state.selected_row();
        let items: Vec<ListItem<'static>> = state
            .proposals
            .iter()
            .enumerate()
            .map(|(index, proposal)| {
                ListItem::new(proposal_row(proposal, selected == Some(index), list_area.width))
            })
            .collect();
        state.sync_list_state();
        StatefulWidget::render(List::new(items), list_area, buf, &mut state.list_state);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Row / line builders
// ═══════════════════════════════════════════════════════════════════════

/// Service configuration + generation line.
fn config_line(status: &SkillEvolutionStatus) -> Line<'static> {
    let dim = Style::new().fg(Color::DarkGray);
    let service = if status.service_enabled {
        Span::styled("on".to_string(), Style::new().fg(Color::Cyan))
    } else {
        Span::styled("off".to_string(), Style::new().fg(Color::Yellow))
    };
    let gate = if status.auto_approve {
        Span::styled("auto-approve".to_string(), Style::new().fg(Color::Yellow))
    } else {
        Span::styled("propose-only".to_string(), Style::new().fg(Color::Cyan))
    };
    let sweep = status
        .sweep_interval_secs
        .map(|s| format!("sweep {}s", s))
        .unwrap_or_else(|| "no sweep".to_string());
    Line::from(vec![
        Span::styled("service ", dim),
        service,
        Span::styled("  ·  gate ", dim),
        gate,
        Span::styled(format!(" (cap {})", status.max_approvals_per_cycle), dim),
        Span::styled("  ·  ", dim),
        Span::styled(sweep, Style::new()),
        Span::styled(
            format!("  ·  generation {}", status.generation),
            Style::new().add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Observation / skill / proposal counters line.
fn counters_line(status: &SkillEvolutionStatus) -> Line<'static> {
    let dim = Style::new().fg(Color::DarkGray);
    let normal = Style::new();
    Line::from(vec![
        Span::styled("observations ", dim),
        Span::styled(format!("{}", status.observations), normal),
        Span::styled("   skills ", dim),
        Span::styled(
            format!(
                "{} auto / {} global / {} workspace / {} builtin",
                status.skills_auto, status.skills_global, status.skills_workspace, status.skills_builtin
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
    ])
}

/// One proposal row: marker + name + [kind · status] + observation
/// count, with the rationale truncated to the remaining width.
fn proposal_row(proposal: &SkillProposalView, selected: bool, width: u16) -> Line<'static> {
    let dim = Style::new().fg(Color::DarkGray);
    let normal = Style::new();
    let bold = Style::new().add_modifier(Modifier::BOLD);

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
        Span::styled(marker, Style::new().fg(Color::Cyan)),
        Span::styled(proposal.name.clone(), if selected { bold } else { normal }),
        Span::styled(
            format!(" [{} · {}{}]", kind, proposal.status, author),
            status_style,
        ),
        Span::styled(format!("  ({} obs)", proposal.observation_count), dim),
    ];
    if !proposal.rationale.is_empty() {
        let used = proposal.name.chars().count() + kind.len() + proposal.status.len() + 24;
        let available = (width as usize).saturating_sub(2).saturating_sub(used);
        spans.push(Span::styled(
            format!("  {}", truncate(&proposal.rationale, available)),
            dim,
        ));
    }
    Line::from(spans)
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

    fn draw(state: &mut SkillEvolutionState, width: u16, height: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::widgets::StatefulWidget as _;
        let mut terminal = ratatui::Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                SkillEvolutionWidget::new()
                    .popup_width(frame.area().width * 9 / 10)
                    .popup_height((frame.area().height * 3 / 4).max(12))
                    .render(frame.area(), frame.buffer_mut(), state)
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
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
        let mut state = SkillEvolutionState {
            visible: true,
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
        let text = draw(&mut state, 100, 20);
        assert!(text.contains("generation 7"), "{text}");
        assert!(text.contains("propose-only"));
        assert!(text.contains("sql-syntax"));
        assert!(text.contains("12"));
    }

    #[test]
    fn render_error_state_without_panicking() {
        let mut state = SkillEvolutionState {
            visible: true,
            error: Some("daemon unreachable".into()),
            ..Default::default()
        };
        let text = draw(&mut state, 80, 10);
        assert!(text.contains("daemon unreachable"));
    }

    #[test]
    fn hidden_state_renders_nothing() {
        let mut state = SkillEvolutionState {
            visible: false,
            status: None,
            ..Default::default()
        };
        let text = draw(&mut state, 80, 10);
        assert!(text.trim().is_empty(), "expected a blank frame, got {text:?}");
    }

    #[test]
    fn selection_scrolls_into_view_when_queue_overflows() {
        let mut state = SkillEvolutionState {
            visible: true,
            status: Some(SkillEvolutionStatus {
                service_enabled: true,
                generation: 1,
                auto_approve: false,
                max_approvals_per_cycle: 5,
                sweep_interval_secs: None,
                observations: 0,
                skills_workspace: 0,
                skills_global: 0,
                skills_builtin: 0,
                skills_auto: 0,
                proposals_pending: 12,
                proposals_approved: 0,
                proposals_rejected: 0,
            }),
            proposals: (0..12)
                .map(|i| proposal(&format!("prop-{i:02}"), "pending"))
                .collect(),
            selected: 11,
            ..Default::default()
        };
        // Popup: 72x18 outer → 70x16 inner → ~10 visible queue rows.
        let text = draw(&mut state, 80, 24);
        assert!(text.contains("prop-11"), "selected row must be visible");
        assert!(
            !text.contains("prop-00"),
            "first row must have scrolled out: {text}"
        );
    }
}
