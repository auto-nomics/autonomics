//! Skill evolution dashboard: service status plus Skills and Observations tabs.

pub mod observation_browser;

pub mod skill_browser;

use self::observation_browser::{ObservationBrowserState, ObservationBrowserWidget};

pub use self::skill_browser::PanelFocus;

use self::skill_browser::{SkillBrowserState, SkillBrowserWidget};

use crate::widgets::popup::{Popup, PopupControls};

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

/// Dashboard-level KMS summary assembled from worker and proposal reads.
#[derive(Debug, Clone)]
pub struct KmsDashboardStatus {
    pub service_enabled: bool,
    pub generation: u64,
    pub sweep_interval_secs: Option<u64>,
    pub observations: usize,
    pub skills_workspace: usize,
    pub skills_global: usize,
    pub skills_builtin: usize,
    pub skills_auto: usize,
    pub proposals_pending: usize,
    pub proposals_applied: usize,
    pub phase: String,
    pub phase_queued: usize,
    pub phase_triggers: Vec<String>,
    pub phase_elapsed_ms: u64,
    pub cycles_completed: u64,
    pub last_cycle_at: Option<i64>,
    pub last_cycle_duration_ms: Option<u64>,
    pub last_triggers: Vec<String>,
    pub last_error: Option<String>,
    pub dropped_commands: u64,
    pub cycle_skipped_empty: u64,
}

#[derive(Debug, Clone, Default)]
pub struct SkillEvolutionState {
    pub visible: bool,
    pub status: Option<KmsDashboardStatus>,
    pub skills: SkillBrowserState,
    pub observations: ObservationBrowserState,
    pub active_tab: EvolutionTab,
    pub last_report: Option<String>,
    pub error: Option<String>,
    /// Generation seen on the last status snapshot.
    #[allow(dead_code)]
    pub seen_generation: u64,
    /// Worker cycles seen on the last status snapshot.
    #[allow(dead_code)]
    pub seen_cycles_completed: u64,
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

        let popup = Popup::new(" KMS Skill Evolution ", PopupControls::default())
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
                    "loading KMS skill governance…",
                    Style::new().fg(Color::DarkGray),
                )),
                inner,
                buf,
            );

            return;
        };

        // Header, tabs, active browser, and keyboard footer.

        let mut header = vec![config_line(status), counters_line(status)];

        header.push(activity_line(status));

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
            format!("2 Evidence ({})", state.observations.observations.len()),
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
                    EvolutionTab::Skills => "a approve · x reject · g refresh",
                    EvolutionTab::Observations => "g refresh",
                }),
            ])
            .style(Style::new().fg(Color::DarkGray)),
            regions[3],
            buf,
        );
    }
}

/// Service configuration + generation line.

fn config_line(status: &KmsDashboardStatus) -> Line<'static> {
    let dim = Style::new().fg(Color::DarkGray);

    let service = if status.service_enabled {
        Span::styled("on".to_string(), Style::new().fg(Color::Cyan))
    } else {
        Span::styled("off".to_string(), Style::new().fg(Color::Yellow))
    };

    let sweep = status
        .sweep_interval_secs
        .map(|s| format!("sweep {}s", s))
        .unwrap_or_else(|| "no sweep".to_string());

    Line::from(vec![
        Span::styled("service ", dim),
        service,
        Span::styled("  ·  human approval", Style::new().fg(Color::Cyan)),
        Span::styled(format!("  ·  {sweep}"), Style::new()),
        Span::styled(
            format!("  ·  snapshot {}", status.generation),
            Style::new().add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Observation / skill / proposal counters line.

fn counters_line(status: &KmsDashboardStatus) -> Line<'static> {
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
                "{} approved · {} pending",
                status.proposals_applied, status.proposals_pending
            ),
            normal,
        ),
    ])
}

/// Live worker activity line — the "is a distillation in flight?" /

/// "how did the last one go?" readout that motivates polling.

fn activity_line(status: &KmsDashboardStatus) -> Line<'static> {
    let dim = Style::new().fg(Color::DarkGray);

    let normal = Style::new();

    let mut spans = Vec::new();

    // 1) The current phase — colored so the difference between

    //    idle / coalescing / distilling is unmistakable at a glance.

    let (label, style) = match status.phase.as_str() {
        "idle" => ("distillation idle".to_string(), normal),
        "coalescing" => (
            if status.phase_queued == 1 {
                "distillation queued: 1 trigger, batch forming…".to_string()
            } else {
                format!(
                    "distillation queued: {} triggers, batch forming…",
                    status.phase_queued
                )
            },
            Style::new().fg(Color::Yellow),
        ),
        "distilling" => (
            format!(
                "distilling… {}.{}s ({})",
                status.phase_elapsed_ms / 1000,
                (status.phase_elapsed_ms % 1000) / 100,
                if status.phase_triggers.is_empty() {
                    "manual".to_string()
                } else {
                    status.phase_triggers.join(", ")
                }
            ),
            Style::new().fg(Color::Green),
        ),
        _ => (
            format!("distillation unknown phase ({})", status.phase),
            Style::new().fg(Color::Red),
        ),
    };

    spans.push(Span::styled(label, style));

    // 2) Idle line also gets the previous-run rollup — last cycle

    //    time + duration + triggers. Skipped when no cycle has run

    //    yet to avoid an awkward "last cycle —" placeholder.

    if status.phase == "idle" {
        if let Some(at) = status.last_cycle_at {
            let dt = chrono::DateTime::from_timestamp(at, 0)
                .map(|t| t.format("%H:%M:%S").to_string())
                .unwrap_or_else(|| "—".to_string());

            let dur = status
                .last_cycle_duration_ms
                .map(|ms| format!("{}.{}s", ms / 1000, (ms % 1000) / 100))
                .unwrap_or_else(|| "—".to_string());

            let triggers = if status.last_triggers.is_empty() {
                "—".to_string()
            } else {
                status.last_triggers.join(", ")
            };

            spans.push(Span::styled("  ·  last cycle ", dim));

            spans.push(Span::styled(format!("{dt} ({dur} · {triggers})"), normal));
        }

        spans.push(Span::styled("  ·  ", dim));

        spans.push(Span::styled(
            format!("{} cycles total", status.cycles_completed),
            normal,
        ));

        // Empty-pool short-circuits: the worker never ran a cycle

        // body, so the last-cycle row above may be missing or stale.

        // Surface a small footnote so the operator sees "yeah, the

        // daemon is alive, it just had nothing to do".

        if status.cycle_skipped_empty > 0 {
            spans.push(Span::styled("  ·  ", dim));

            spans.push(Span::styled(
                format!("{} skipped (empty pool)", status.cycle_skipped_empty),
                dim,
            ));
        }
    }

    // 3) Dropped commands are an observability signal, not an error,

    //    but worth surfacing so the operator can correlate activity

    //    gaps with channel pressure.

    if status.dropped_commands > 0 {
        spans.push(Span::styled("  ·  ", dim));

        spans.push(Span::styled(
            format!("⚠ {} dropped", status.dropped_commands),
            Style::new().fg(Color::Red),
        ));
    }

    // 4) Last cycle error — visible in any phase so a failed manual

    //    run isn't lost under the distilling spinner.

    if let Some(err) = &status.last_error {
        spans.push(Span::styled("  ·  last error ", dim));

        spans.push(Span::styled(
            truncate_error(err),
            Style::new().fg(Color::Red),
        ));
    }

    Line::from(spans)
}

fn truncate_error(err: &str) -> String {
    const MAX: usize = 80;

    if err.len() <= MAX {
        err.to_string()
    } else {
        format!("{}…", &err[..MAX])
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    use gateway::proto::{
        KmsSkillArtifactView, KmsSkillLibraryView, KmsSkillProposalView, SkillLibraryDetail,
        SkillLibraryView,
    };

    use uuid::Uuid;

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

    fn kms_skill(name: &str) -> KmsSkillLibraryView {
        KmsSkillLibraryView {
            id: Uuid::new_v4(),
            skill_name: name.into(),
            node_path: format!("/procedures/{name}"),
            version: 1,
            title: name.into(),
            description: "does the thing".into(),
            tags: vec!["auto".into()],
            tier: "workspace".into(),
            source_tier: "workspace".into(),
            created_at: 1,
            artifact_paths: vec!["workflow/main.toml".into()],
        }
    }

    fn kms_proposal(name: &str, status: &str) -> KmsSkillProposalView {
        KmsSkillProposalView {
            id: Uuid::new_v4(),
            skill_name: name.into(),
            node_path: format!("/procedures/{name}"),
            status: status.into(),
            title: name.into(),
            body_md: format!("{name} proposed body"),
            rationale: "three failures".into(),
            source_evidence_ids: vec!["e1".into(), "e2".into(), "e3".into()],
            artifacts: vec![KmsSkillArtifactView {
                relative_path: "workflow/main.toml".into(),
                content: "description = 'x'".into(),
            }],
            content_hash: format!("hash-{name}"),
            approved_skill_version_id: None,
            created_at: 2,
            updated_at: 2,
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

        assert_eq!(
            state.skills.selected_skill().map(|s| s.name.clone()),
            Some("b-approved".into()),
            "history rows still show their detail document"
        );
    }

    #[test]
    fn kms_projection_and_pending_proposals_merge_into_governed_browser() {
        let mut browser = SkillBrowserState::default();

        browser.set_kms_library(
            vec![kms_skill("alpha"), kms_skill("beta")],
            vec![
                kms_proposal("alpha", "pending"),
                kms_proposal("gamma", "pending"),
            ],
        );

        assert_eq!(browser.library.len(), 3);

        assert_eq!(
            browser
                .library
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "gamma", "beta"]
        );

        assert!(browser.library[0].proposal_update);

        assert!(!browser.library[1].installed);

        assert!(browser.selected_proposal_id().is_some());

        let detail = browser.proposal_detail("alpha").unwrap();

        assert!(detail.body.contains("alpha proposed body"));

        assert_eq!(detail.evidence_count, 3);

        assert_eq!(browser.proposal_generation, 2_000);
    }

    #[test]
    fn render_shows_library_and_detail_document() {
        let mut state = SkillEvolutionState {
            visible: true,
            status: Some(KmsDashboardStatus {
                service_enabled: true,
                generation: 7,
                sweep_interval_secs: Some(21600),
                observations: 12,
                skills_workspace: 0,
                skills_global: 1,
                skills_builtin: 1,
                skills_auto: 1,
                proposals_pending: 1,
                proposals_applied: 2,
                phase: "idle".into(),
                phase_queued: 0,
                phase_triggers: Vec::new(),
                phase_elapsed_ms: 0,
                cycles_completed: 3,
                last_cycle_at: None,
                last_cycle_duration_ms: None,
                last_triggers: Vec::new(),
                last_error: None,
                dropped_commands: 0,
                cycle_skipped_empty: 0,
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

        assert!(text.contains("snapshot 7"), "{text}");

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

        assert!(text.contains("human approval required"));

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

    fn dummy_status() -> KmsDashboardStatus {
        KmsDashboardStatus {
            service_enabled: true,
            generation: 1,
            sweep_interval_secs: None,
            observations: 0,
            skills_workspace: 0,
            skills_global: 0,
            skills_builtin: 0,
            skills_auto: 0,
            proposals_pending: 0,
            proposals_applied: 0,
            phase: "idle".into(),
            phase_queued: 0,
            phase_triggers: Vec::new(),
            phase_elapsed_ms: 0,
            cycles_completed: 0,
            last_cycle_at: None,
            last_cycle_duration_ms: None,
            last_triggers: Vec::new(),
            last_error: None,
            dropped_commands: 0,
            cycle_skipped_empty: 0,
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
}
