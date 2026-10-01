//! Skill evolution dashboard: service status plus Skills and Observations tabs.

pub use crate::widgets::skill_browser_widget::PanelFocus;
use crate::widgets::{
    observation_browser_widget::{ObservationBrowserState, ObservationBrowserWidget},
    popup::{Popup, PopupControls},
    skill_browser_widget::{SkillBrowserState, SkillBrowserWidget},
};
use gateway::proto::SkillEvolutionStatus;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, StatefulWidget, Tabs, Widget},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EvolutionTab {
    #[default]
    Skills,
    Observations,
}

#[derive(Debug, Clone, Default)]
pub struct SkillEvolutionState {
    pub visible: bool,
    pub status: Option<SkillEvolutionStatus>,
    pub skills: SkillBrowserState,
    pub observations: ObservationBrowserState,
    pub active_tab: EvolutionTab,
    pub last_report: Option<String>,
    pub error: Option<String>,
}
impl SkillEvolutionState {
    pub fn open(&mut self) {
        self.visible = true;
    }
    pub fn close(&mut self) {
        self.visible = false;
    }
    pub fn switch_tab(&mut self) {
        self.active_tab = match self.active_tab {
            EvolutionTab::Skills => EvolutionTab::Observations,
            EvolutionTab::Observations => EvolutionTab::Skills,
        };
    }
    pub fn focus_list(&mut self) {
        match self.active_tab {
            EvolutionTab::Skills => self.skills.focus_list(),
            EvolutionTab::Observations => self.observations.focus = PanelFocus::List,
        }
    }
    pub fn focus_detail(&mut self) {
        match self.active_tab {
            EvolutionTab::Skills => self.skills.focus_detail(),
            EvolutionTab::Observations => self.observations.focus = PanelFocus::Detail,
        }
    }
    /// True when the skill selection changed and needs a detail fetch.
    pub fn navigate(&mut self, down: bool) -> bool {
        match self.active_tab {
            EvolutionTab::Skills => {
                if self.skills.focus == PanelFocus::Detail {
                    self.skills.scroll_detail(if down { 1 } else { -1 });
                    false
                } else {
                    let previous = self.skills.selected;
                    if down {
                        self.skills.select_next();
                    } else {
                        self.skills.select_prev();
                    }
                    previous != self.skills.selected
                }
            }
            EvolutionTab::Observations => {
                self.observations.navigate(down);
                false
            }
        }
    }
    pub fn selected_pending(&self) -> Option<&gateway::proto::SkillLibraryView> {
        (self.active_tab == EvolutionTab::Skills)
            .then(|| self.skills.selected_pending())
            .flatten()
    }
}

/// The dashboard as a centered popup: header (configuration +
/// counters + last cycle), browser tabs, and a key-binding footer.
/// Stateless beyond the chrome — all mutable
/// data lives in [`SkillEvolutionState`].
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

        // Header, tabs, active browser, and keyboard footer.
        let mut header = vec![config_line(status), counters_line(status)];
        if let Some(report) = &state.last_report {
            header.push(Line::styled(
                format!("last cycle: {report}"),
                Style::new().fg(Color::Cyan),
            ));
        }

        let regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(header.len() as u16),
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(inner);
        Widget::render(Paragraph::new(header), regions[0], buf);
        let tabs = Tabs::new(vec![
            format!("1 Skills ({})", state.skills.library.len()),
            format!("2 Observations ({})", state.observations.observations.len()),
        ])
        .select(usize::from(state.active_tab == EvolutionTab::Observations))
        .highlight_style(Style::new().fg(self.accent).add_modifier(Modifier::BOLD))
        .style(Style::new().fg(Color::DarkGray));
        Widget::render(tabs, regions[1], buf);
        match state.active_tab {
            EvolutionTab::Skills => StatefulWidget::render(
                SkillBrowserWidget {
                    accent: self.accent,
                },
                regions[2],
                buf,
                &mut state.skills,
            ),
            EvolutionTab::Observations => StatefulWidget::render(
                ObservationBrowserWidget {
                    accent: self.accent,
                },
                regions[2],
                buf,
                &mut state.observations,
            ),
        }
        Widget::render(
            Paragraph::new(vec![
                Line::raw("Tab/1/2 tabs · ←→ panel · ↑↓ move/scroll · Esc close"),
                Line::raw(match state.active_tab {
                    EvolutionTab::Skills => "a approve · r reject · t/T trigger · g refresh",
                    EvolutionTab::Observations => "t/T trigger · g refresh",
                }),
            ])
            .style(Style::new().fg(Color::DarkGray)),
            regions[3],
            buf,
        );
    }
}

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
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway::proto::{SkillLibraryDetail, SkillLibraryView};

    fn skill(name: &str, tier: &str, proposal_status: Option<&str>) -> SkillLibraryView {
        SkillLibraryView {
            name: name.into(),
            tier: tier.into(),
            tags: vec!["auto".into()],
            description: "does the thing".into(),
            installed: proposal_status.is_none() || tier != "proposed",
            proposal_status: proposal_status.map(Into::into),
            proposal_update: false,
            usage_gets: 1,
            usage_search_hits: 0,
            usage_runs: 0,
            usage_evals: 0,
            usage_last_used: 0,
        }
    }

    fn detail_for(name: &str) -> SkillLibraryDetail {
        SkillLibraryDetail {
            name: name.into(),
            tier: "global".into(),
            tags: vec!["auto".into()],
            description: "does the thing".into(),
            installed: true,
            body: "# Guide\n\nStep one.\n".into(),
            workflows: vec![],
            evals: vec![],
            proposal: None,
            evidence_count: 0,
            usage_gets: 0,
            usage_search_hits: 0,
            usage_runs: 0,
            usage_evals: 0,
            usage_last_used: 0,
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
    fn selection_moves_across_all_rows_with_wrap() {
        let mut state = SkillEvolutionState {
            skills: SkillBrowserState {
                library: vec![
                    skill("a-pending", "global", Some("pending")),
                    skill("b-approved", "global", Some("approved")),
                    skill("c-plain", "builtin", None),
                    skill("d-rejected", "global", Some("rejected")),
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(state.skills.selected, 0);
        state.skills.select_next();
        assert_eq!(state.skills.selected, 1, "visits history rows too");
        state.skills.select_next();
        state.skills.select_next();
        state.skills.select_next();
        assert_eq!(state.skills.selected, 0, "wraps forward");
        state.skills.select_prev();
        assert_eq!(state.skills.selected, 3, "wraps backwards");
        // Actions stay gated to pending rows regardless of where the
        // browser selection sits.
        state.skills.selected = 1;
        assert!(
            state.skills.selected_pending().is_none(),
            "approved row is not an action target"
        );
        assert_eq!(
            state.skills.selected_skill().map(|s| s.name.clone()),
            Some("b-approved".into()),
            "but the detail pane still shows it"
        );
    }

    #[test]
    fn no_pending_rows_means_no_selection_action() {
        let mut state = SkillEvolutionState {
            skills: SkillBrowserState {
                library: vec![skill("only-history", "builtin", None)],
                selected: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        state.skills.select_next();
        assert!(state.skills.selected_pending().is_none());
    }

    #[test]
    fn render_shows_library_and_detail_document() {
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
            last_report: Some("1 written".into()),
            skills: SkillBrowserState {
                library: vec![skill("sql-syntax", "global", Some("pending"))],
                detail: Some((
                    "sql-syntax".into(),
                    SkillLibraryDetail {
                        proposal: Some(gateway::proto::SkillProposalView {
                            name: "sql-syntax".into(),
                            status: "pending".into(),
                            authored_by: "distiller".into(),
                            update: false,
                            rationale: "three observations".into(),
                            cluster_hash: "c-abc".into(),
                            observation_count: 3,
                            created_at: 1,
                        }),
                        evidence_count: 3,
                        ..detail_for("sql-syntax")
                    },
                )),
                ..Default::default()
            },
            ..Default::default()
        };
        // The full document (metadata + proposal + body) needs a tall
        // popup to fit unscrolled; shorter terminals scroll it.
        let text = draw(&mut state, 100, 32);
        assert!(text.contains("generation 7"), "{text}");
        assert!(text.contains("Skills (1)"));
        assert!(text.contains("sql-syntax"));
        assert!(text.contains("pending"), "proposal status on the row");
        // Detail document: metadata + usage + proposal + body.
        assert!(text.contains("Tier"));
        assert!(text.contains("installed"));
        assert!(
            text.contains("never used"),
            "zero telemetry reads as unused"
        );
        assert!(text.contains("awaiting review"));
        // The rationale may word-wrap mid-phrase, so assert on a
        // fragment that fits one rendered row.
        assert!(text.contains("obs evidence"));
        assert!(text.contains("Body — SKILL.md"));
        assert!(text.contains("Step one."));
    }

    #[test]
    fn stale_detail_for_another_row_shows_loading() {
        let mut state = SkillEvolutionState {
            visible: true,
            status: Some(dummy_status()),
            skills: SkillBrowserState {
                library: vec![skill("a", "global", None), skill("b", "global", None)],
                selected: 1,
                // Detail fetched for row "a" while the selection has since
                // moved to "b" — must not render as b's detail.
                detail: Some(("a".into(), detail_for("a"))),
                ..Default::default()
            },
            ..Default::default()
        };
        let text = draw(&mut state, 100, 24);
        assert!(text.contains("loading detail"), "{text}");
        assert!(!text.contains("Step one"), "stale body must not show");
    }

    #[test]
    fn detail_fetch_failure_renders_the_error() {
        let mut state = SkillEvolutionState {
            visible: true,
            status: Some(dummy_status()),
            skills: SkillBrowserState {
                library: vec![skill("gone", "global", None)],
                detail_error: Some(("gone".into(), "404 no skill".into())),
                ..Default::default()
            },
            ..Default::default()
        };
        let text = draw(&mut state, 100, 24);
        assert!(text.contains("detail unavailable"), "{text}");
        assert!(text.contains("404"));
    }

    fn dummy_status() -> SkillEvolutionStatus {
        SkillEvolutionStatus {
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
            proposals_pending: 0,
            proposals_approved: 0,
            proposals_rejected: 0,
        }
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
        assert!(
            text.trim().is_empty(),
            "expected a blank frame, got {text:?}"
        );
    }

    #[test]
    fn selection_scrolls_into_view_when_queue_overflows() {
        let mut state = SkillEvolutionState {
            visible: true,
            status: Some(dummy_status()),
            skills: SkillBrowserState {
                library: (0..12)
                    .map(|i| skill(&format!("skill-{i:02}"), "global", None))
                    .collect(),
                selected: 11,
                ..Default::default()
            },
            ..Default::default()
        };
        let text = draw(&mut state, 80, 24);
        assert!(text.contains("skill-11"), "selected row must be visible");
        assert!(
            !text.contains("skill-00"),
            "first row must have scrolled out: {text}"
        );
    }

    #[test]
    fn focus_switches_between_panes_and_vertical_keys_follow() {
        let mut state = SkillEvolutionState {
            skills: SkillBrowserState {
                library: vec![skill("a", "global", None)],
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(state.skills.focus, PanelFocus::List, "starts on the list");
        state.skills.focus_detail();
        assert_eq!(state.skills.focus, PanelFocus::Detail);
        state.skills.scroll_detail(3);
        state.skills.scroll_detail(-1);
        state.skills.focus_list();
        assert_eq!(state.skills.focus, PanelFocus::List);
    }

    #[test]
    fn detail_scroll_resets_on_selection_change_and_clamps_at_render() {
        let long_body = "START\n".to_string() + &"filler line\n".repeat(40) + "END\n";
        let mut state = SkillEvolutionState {
            visible: true,
            status: Some(dummy_status()),
            skills: SkillBrowserState {
                library: vec![
                    skill("long-body", "global", None),
                    skill("b", "global", None),
                ],
                detail: Some((
                    "long-body".into(),
                    SkillLibraryDetail {
                        body: long_body,
                        ..detail_for("long-body")
                    },
                )),
                ..Default::default()
            },
            ..Default::default()
        };
        state.skills.focus_detail();
        state.skills.scroll_detail(500);
        // Render clamps the offset to the document height — START
        // scrolls out, END (the last line) stays on screen.
        let text = draw(&mut state, 100, 24);
        assert!(
            text.contains("END"),
            "last line reachable after clamp: {text}"
        );
        assert!(
            !text.contains("START"),
            "top must have scrolled away: {text}"
        );

        // Moving the selection restarts the next detail from the top.
        state.skills.select_next();
        assert_eq!(
            state.skills.detail_scroll, 0,
            "selection change resets scroll"
        );
    }
}
