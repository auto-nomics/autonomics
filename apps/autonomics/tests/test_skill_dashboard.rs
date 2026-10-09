//! Test harness for the skill-evolution dashboard widget.
//!
//! The app crate is binary-only (no lib target), so the widget
//! sources are pulled in with `#[path]` module shims — the same code
//! the binary compiles, no duplication.
//!
//! Two feedback modes:
//!
//! **Snapshots** (default) — render frames through `TestBackend` and
//! print them as text blocks. With `--nocapture` the frames land
//! inline in your terminal; assertions keep each frame honest:
//!
//! ```text
//! cargo test -p autonomics --test test_skill_dashboard -- --nocapture
//! cargo test -p autonomics --test test_skill_dashboard snapshot_loaded -- --nocapture
//! ```
//!
//! **Interactive preview** (ignored by default) — runs the widget
//! live in your terminal against synthetic data, no gateway daemon
//! needed. Tab/1/2 switch tabs, j/k navigate/scroll, a/r approve/reject,
//! t runs a fake cycle, e toggles
//! the error path:
//!
//! ```text
//! cargo test -p autonomics --test test_skill_dashboard -- --ignored interactive_preview --nocapture
//! ```

// ── Source shims: compile the real widget into this test crate ──
mod widgets;

use gateway::proto::{
    SkillEvolutionStatus, SkillLibraryDetail, SkillLibraryView, SkillObservationView,
    SkillProposalView,
};
use ratatui::{
    backend::TestBackend,
    buffer::Buffer,
    layout::Rect,
    prelude::{Buffer as _, Terminal},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Paragraph, StatefulWidget as _, Widget as _},
};
use widgets::skill_evolution_widget::{
    EvolutionTab, PanelFocus, SkillEvolutionState, SkillEvolutionWidget,
};

// ═══════════════════════════════════════════════════════════════════════
// Fixtures
// ═══════════════════════════════════════════════════════════════════════

/// A healthy, mid-flight evolution status.
fn fixture_status() -> SkillEvolutionStatus {
    SkillEvolutionStatus {
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
        proposals_pending: 3,
        proposals_approved: 1,
        proposals_rejected: 1,
        phase: "idle".into(),
        phase_queued: 0,
        phase_triggers: Vec::new(),
        phase_elapsed_ms: 0,
        cycles_completed: 8,
        last_cycle_at: Some(1_700_000_000),
        last_cycle_duration_ms: Some(1234),
        last_triggers: vec!["observation".into(), "timer".into()],
        last_error: None,
        dropped_commands: 0,
        cycle_skipped_empty: 0,
        last_skipped_reason: None,
    }
}

/// One library row; `proposal_status` is the skill's pipeline
/// attribute (`None` = outside the pipeline).
fn fixture_skill(name: &str, tier: &str, proposal_status: Option<&str>) -> SkillLibraryView {
    SkillLibraryView {
        name: name.into(),
        tier: tier.into(),
        tags: vec!["auto".into()],
        description: "fixture skill description".into(),
        installed: tier != "proposed",
        proposal_status: proposal_status.map(Into::into),
        proposal_update: name.starts_with("update-"),
        usage_gets: if name.contains("hot-") { 12 } else { 0 },
        usage_search_hits: 1,
        usage_runs: 0,
        usage_evals: 0,
        usage_last_used: 0,
    }
}

/// The unified library: builtin + global + a proposed row, pipeline
/// statuses mixed.
fn fixture_library() -> Vec<SkillLibraryView> {
    vec![
        fixture_skill("skill-system", "builtin", None),
        fixture_skill("hot-pathlib-glob", "global", Some("approved")),
        fixture_skill("sql-syntax", "proposed", Some("pending")),
        fixture_skill("agent-r-cache", "proposed", Some("pending")),
        fixture_skill("update-ggplot-axis", "global", Some("pending")),
        fixture_skill("agent-logfmt", "global", Some("rejected")),
    ]
}

/// The detail document for a library row.
fn fixture_detail(view: &SkillLibraryView) -> SkillLibraryDetail {
    SkillLibraryDetail {
        name: view.name.clone(),
        tier: view.tier.clone(),
        tags: view.tags.clone(),
        description: view.description.clone(),
        installed: view.installed,
        body: format!(
            "# {}\n\nOperational guidance for {}.\n",
            view.name, view.name
        ),
        workflows: Vec::new(),
        evals: Vec::new(),
        proposal: view
            .proposal_status
            .as_deref()
            .map(|status| SkillProposalView {
                name: view.name.clone(),
                status: status.into(),
                authored_by: if view.name.contains("agent-") {
                    "agent"
                } else {
                    "distiller"
                }
                .into(),
                update: view.proposal_update,
                rationale: format!("evidence behind {}", view.name),
                cluster_hash: "c-abc".into(),
                observation_count: 3,
                created_at: 1,
            }),
        evidence_count: if view.proposal_status.is_some() { 3 } else { 0 },
        usage_gets: view.usage_gets,
        usage_search_hits: view.usage_search_hits,
        usage_runs: view.usage_runs,
        usage_evals: view.usage_evals,
        usage_last_used: view.usage_last_used,
    }
}

/// Dashboard state with the status/library fixtures loaded, the
/// selection parked on the first pending row, and that row's detail
/// fetched. Built field-by-field (not a struct literal) because the
/// state's private `list_state` field makes literals illegal outside
/// its module.
fn fixture_state() -> SkillEvolutionState {
    let mut state = SkillEvolutionState::default();
    state.visible = true;
    state.status = Some(fixture_status());
    state.skills.library = fixture_library();
    state.skills.selected = state
        .skills
        .library
        .iter()
        .position(|s| s.proposal_status.as_deref() == Some("pending"))
        .unwrap_or(0);
    let selected = &state.skills.library[state.skills.selected];
    state.skills.detail = Some((selected.name.clone(), fixture_detail(selected)));
    state.last_report = Some("1 written, 0 auto-approved, 1 left pending".to_string());
    state
        .observations
        .set_observations(vec![fixture_observation("obs-1", "SQL header mismatch")]);
    state
}

fn fixture_observation(id: &str, summary: &str) -> SkillObservationView {
    SkillObservationView {
        id: id.into(),
        created_at: 1,
        kind: "failure".into(),
        source: "agent".into(),
        summary: summary.into(),
        body: "Validate the CSV header before loading.\nEvidence body ends here.".into(),
        node_kind: Some("sql".into()),
        error: Some("missing column".into()),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Snapshot harness
// ═══════════════════════════════════════════════════════════════════════

/// Render one dashboard frame at the given terminal size and return
/// the screen as text rows (trailing blanks trimmed per row).
fn render_dashboard(state: &mut SkillEvolutionState, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            SkillEvolutionWidget::new()
                .popup_width(frame.area().width * 9 / 10)
                .popup_height((frame.area().height * 3 / 4).max(12))
                .render(frame.area(), frame.buffer_mut(), state)
        })
        .unwrap();
    buffer_to_text(terminal.backend().buffer())
}

/// Flatten a rendered buffer into newline-joined rows.
fn buffer_to_text(buf: &Buffer) -> String {
    let width = buf.area.width as usize;
    buf.content()
        .chunks(width)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Print a frame with a title banner — output only appears under
/// `--nocapture`, so plain `cargo test` stays quiet.
fn show(title: &str, text: &str) {
    println!("\n━━━ {title} ━━━");
    println!("{text}");
}

// ═══════════════════════════════════════════════════════════════════════
// Snapshot tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn open_close_toggles_visibility() {
    let mut state = SkillEvolutionState::default();
    state.open();
    assert!(state.visible);
    state.close();
    assert!(!state.visible);
}

#[test]
fn snapshot_loaded() {
    let mut state = fixture_state();
    let text = render_dashboard(&mut state, 100, 32);
    show("loaded · 100×32", &text);
    assert!(text.contains("generation 7"), "{text}");
    assert!(text.contains("propose-only"), "{text}");
    assert!(text.contains("observations 12"), "{text}");
    // Left list: every tier, statuses inline. The update-ggplot-axis
    // row carries proposal_update=true — it must render distinctly
    // from plain pending creates.
    assert!(text.contains("Skills (6)"), "{text}");
    assert!(text.contains("sql-syntax"), "{text}");
    assert!(text.contains("pending"), "{text}");
    assert!(
        text.contains("pending↑"),
        "update proposal rows must be distinguishable: {text}"
    );
    // Right pane: the selected skill's detail document.
    assert!(text.contains("Tier"), "{text}");
    assert!(text.contains("not installed yet"), "proposed row note");
    assert!(text.contains("awaiting review"), "{text}");
    assert!(text.contains("Body — SKILL.md"), "{text}");
}

#[test]
fn snapshot_loaded_across_widths() {
    for (width, height) in [(120u16, 40u16), (100, 24), (80, 24), (60, 14)] {
        let mut state = fixture_state();
        let text = render_dashboard(&mut state, width, height);
        show(&format!("loaded · {width}×{height}"), &text);
        assert!(text.contains("sql-syntax"), "{width}×{height}: {text}");
    }
}

#[test]
fn snapshot_empty_library() {
    let mut state = fixture_state();
    state.skills.library.clear();
    state.skills.detail = None;
    state.last_report = None;
    let text = render_dashboard(&mut state, 100, 24);
    show("empty library · 100×24", &text);
    assert!(text.contains("no skills in the library"), "{text}");
}

#[test]
fn snapshot_error_state() {
    let mut state = fixture_state();
    state.error = Some("gateway: connection refused".into());
    let text = render_dashboard(&mut state, 100, 24);
    show("error · 100×24", &text);
    assert!(
        text.contains("error: gateway: connection refused"),
        "{text}"
    );
    assert!(text.contains("press g to retry"), "{text}");
}

#[test]
fn snapshot_loading_state() {
    let mut state = fixture_state();
    state.status = None;
    let text = render_dashboard(&mut state, 100, 24);
    show("loading · 100×24", &text);
    assert!(text.contains("loading evolution status"), "{text}");
}

#[test]
fn snapshot_stale_detail_shows_loading() {
    let mut state = fixture_state();
    // The detail belongs to the previously selected row — the pane
    // must say "loading" rather than render a mismatched document.
    state.skills.detail = Some((
        "some-other-skill".into(),
        fixture_detail(&fixture_skill("some-other-skill", "global", None)),
    ));
    let text = render_dashboard(&mut state, 100, 24);
    show("stale detail · 100×24", &text);
    assert!(text.contains("loading detail"), "{text}");
}

#[test]
fn snapshot_overflow_selection_scrolled_into_view() {
    let mut state = fixture_state();
    state.skills.library = (0..8)
        .map(|i| fixture_skill(&format!("prop-{i:02}"), "global", Some("pending")))
        .collect();
    state.skills.detail = None;

    // 60×14 → popup 54×12 → ~5 visible list rows for 8 skills.
    state.skills.selected = 7;
    let bottom = render_dashboard(&mut state, 60, 14);
    show("overflow · selection at bottom · 60×14", &bottom);
    assert!(bottom.contains("prop-07"), "selection visible: {bottom}");
    assert!(!bottom.contains("prop-00"), "top scrolled out: {bottom}");

    state.skills.selected = 0;
    let top = render_dashboard(&mut state, 60, 14);
    show("overflow · selection at top · 60×14", &top);
    assert!(top.contains("prop-00"), "selection visible: {top}");
    assert!(!top.contains("prop-07"), "bottom scrolled out: {top}");
}

#[test]
fn snapshot_marker_follows_selection_and_actions_stay_gated() {
    let mut state = fixture_state();
    // Navigation is a browser: the marker sits on ANY selected row,
    // including history (approved/rejected) rows.
    state.skills.selected = state
        .skills
        .library
        .iter()
        .position(|s| s.proposal_status.as_deref() == Some("rejected"))
        .unwrap();
    let selected_name = state.skills.library[state.skills.selected].name.clone();
    let text = render_dashboard(&mut state, 100, 24);
    show("selection on history row · 100×24", &text);
    assert!(text.contains('▶'), "browser selection is visible: {text}");
    assert!(
        state.selected_pending().is_none(),
        "history row is not an action target"
    );
    assert_eq!(state.skills.selected_skill().unwrap().name, selected_name);
}

// ═══════════════════════════════════════════════════════════════════════
// Interactive preview (opt-in)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn snapshot_observations_tab() {
    let mut state = fixture_state();
    state.active_tab = EvolutionTab::Observations;
    let text = render_dashboard(&mut state, 120, 40);
    assert!(text.contains("Observations (1)"), "{text}");
    for expected in [
        "SQL header mismatch",
        "failure",
        "agent",
        "missing column",
        "CSV header",
        "Evidence",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(!text.contains("Body — SKILL.md"), "{text}");
    assert!(
        state.selected_pending().is_none(),
        "review actions are disabled on observations"
    );
}

#[test]
fn observations_loading_empty_and_error() {
    let mut state = fixture_state();
    state.active_tab = EvolutionTab::Observations;
    state.observations = Default::default();
    assert!(render_dashboard(&mut state, 100, 24).contains("loading observations"));
    state.observations.set_observations(vec![]);
    assert!(render_dashboard(&mut state, 100, 24).contains("no observations recorded"));
    state.observations.error = Some("offline".into());
    assert!(render_dashboard(&mut state, 100, 24).contains("observations unavailable: offline"));
}

#[test]
fn tab_navigation_keeps_independent_selection_and_focus() {
    let mut state = fixture_state();
    let skill_index = state.skills.selected;
    state.focus_detail();
    state.switch_tab();
    state.observations.set_observations(vec![
        fixture_observation("a", "First"),
        fixture_observation("b", "Second"),
    ]);
    assert!(
        !state.navigate(true),
        "observation selection never fetches skill details"
    );
    assert_eq!(state.observations.selected, 1);
    state.focus_detail();
    state.switch_tab();
    assert_eq!(state.skills.selected, skill_index);
    assert_eq!(state.skills.focus, PanelFocus::Detail);
    assert!(
        !state.navigate(true),
        "detail scrolling never fetches skill details"
    );
    state.focus_list();
    assert!(state.navigate(true));
    state.switch_tab();
    assert_eq!(state.observations.selected, 1);
    assert_eq!(state.observations.focus, PanelFocus::Detail);
}

#[test]
fn observations_refresh_and_wrapped_scroll_preserve_context() {
    let mut state = fixture_state();
    state.active_tab = EvolutionTab::Observations;
    let mut row = fixture_observation("target", "Unicode evidence");
    row.body = "中文证据 abcdefghijklmnopqrstuvwxyz0123456789END".to_string();
    state
        .observations
        .set_observations(vec![fixture_observation("other", "Other"), row.clone()]);
    state.navigate(true);
    state.focus_detail();
    for _ in 0..1000 {
        state.navigate(true);
    }
    let text = render_dashboard(&mut state, 80, 24);
    assert!(text.contains("END"), "last wrapped line reachable: {text}");
    assert!(!text.contains("START"));
    state.observations.set_observations(vec![row]);
    assert_eq!(state.observations.selected, 0);
    assert!(
        render_dashboard(&mut state, 80, 24).contains("END"),
        "refresh preserves scroll for same ID"
    );
    state
        .observations
        .set_observations(vec![fixture_observation("new", "New")]);
    assert!(
        render_dashboard(&mut state, 100, 32).contains("Summary"),
        "new selection starts at top"
    );
}

#[test]
fn skill_refresh_preserves_selection_and_stale_replies_do_not_replace_detail() {
    let mut state = fixture_state();
    let row = state.skills.selected_skill().unwrap().clone();
    state.skills.set_library(vec![row.clone()]);
    assert_eq!(state.skills.selected, 0);
    state
        .skills
        .apply_detail(row.name.clone(), Ok(fixture_detail(&row)));
    state
        .skills
        .apply_detail("stale-row".into(), Err("late error".into()));
    assert!(state.skills.detail_error.is_none());
    assert_eq!(state.skills.detail.as_ref().unwrap().0, row.name);
    state
        .skills
        .apply_detail(row.name.clone(), Err("current error".into()));
    assert!(state.skills.detail.is_none());
    assert_eq!(
        state.skills.detail_error.as_ref().unwrap().1,
        "current error"
    );
}

/// Live preview of the dashboard in the user's terminal against
/// synthetic data — no gateway daemon. Drives the real widget with
/// the same popup geometry the app uses.
#[test]
#[ignore = "interactive; run with -- --ignored interactive_preview --nocapture"]
fn interactive_preview() -> std::io::Result<()> {
    use std::io::{self, IsTerminal, stdout};

    if !stdout().is_terminal() {
        eprintln!("interactive_preview needs a real terminal (tty)");
        return Ok(());
    }

    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(stdout(), crossterm::terminal::EnterAlternateScreen)?;
    let mut terminal = Terminal::new(ratatui::backend::CrosstermBackend::new(stdout()))?;
    terminal.hide_cursor()?;

    let result = run_interactive_loop(&mut terminal);

    // Teardown runs on every path — a panic mid-loop is the only way
    // to skip it, and the harness doc says how to recover
    // (`reset`).
    let _ = terminal.show_cursor();
    crossterm::execute!(stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    crossterm::terminal::disable_raw_mode()?;
    result
}

fn run_interactive_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
) -> std::io::Result<()> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind};
    use std::time::Duration;

    use widgets::skill_evolution_widget::EvolutionTab;

    let mut state = fixture_state();
    let mut next_canned = 0;
    const CANNED_SKILLS: &[&str] = &[
        "canned-fmt-strings",
        "canned-sheet-bounds",
        "canned-join-hint",
        "canned-rng-seed",
    ];

    /// Simulate the app's detail fetch: attach the document for the
    /// row the selection just landed on.
    fn refetch_detail(state: &mut SkillEvolutionState) {
        if let Some(view) = state.skills.selected_skill() {
            let detail = fixture_detail(view);
            state.skills.detail = Some((view.name.clone(), detail));
            state.skills.detail_error = None;
        }
    }

    loop {
        terminal.draw(|frame| {
            render_backdrop(frame.area(), frame.buffer_mut());

            SkillEvolutionWidget::new()
                .popup_width(frame.area().width * 9 / 10)
                .popup_height((frame.area().height * 3 / 4).max(12))
                .render(frame.area(), frame.buffer_mut(), &mut state);

            let hint = "harness: Tab tabs · j/k move/scroll · ←/→ focus · a approve · r reject · t fake cycle · e toggle error · Esc quit";
            let bottom = Rect::new(
                frame.area().x,
                frame.area().bottom().saturating_sub(1),
                frame.area().width,
                1,
            );
            Paragraph::new(Line::styled(hint, Style::new().fg(Color::DarkGray)))
                .render(bottom, frame.buffer_mut());
        })?;

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
            KeyCode::Tab | KeyCode::BackTab => state.switch_tab(),
            KeyCode::Char('1') => state.active_tab = EvolutionTab::Skills,
            KeyCode::Char('2') => state.active_tab = EvolutionTab::Observations,
            KeyCode::Down | KeyCode::Char('j') => {
                if state.navigate(true) {
                    refetch_detail(&mut state);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if state.navigate(false) {
                    refetch_detail(&mut state);
                }
            }
            KeyCode::Left | KeyCode::Char('h') => state.focus_list(),
            KeyCode::Right | KeyCode::Char('l') => state.focus_detail(),
            KeyCode::Char('a') => fake_action(&mut state, "approved"),
            KeyCode::Char('r') => fake_action(&mut state, "rejected"),
            KeyCode::Char('e') => {
                state.error = match state.error {
                    Some(_) => None,
                    None => Some("gateway: connection refused (harness-simulated)".into()),
                };
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                let name = CANNED_SKILLS[next_canned % CANNED_SKILLS.len()];
                next_canned += 1;
                let auto = key.code == KeyCode::Char('T');
                let status = if auto { "approved" } else { "pending" };
                let view = fixture_skill(name, "proposed", Some(status));
                state.skills.library.insert(2, view);
                if let Some(status) = state.status.as_mut() {
                    status.generation += 1;
                    status.proposals_pending += usize::from(!auto);
                    status.proposals_approved += usize::from(auto);
                }
                state.last_report = Some(format!(
                    "{name} written{}, 1 considered",
                    if auto { " + auto-approved" } else { "" }
                ));
            }
            // g refresh: the harness state never goes stale, so this
            // is a no-op kept for muscle-memory parity with the app.
            KeyCode::Char('g') => {}
            _ => {}
        }
    }
}

/// Apply a fake proposal action and refresh the library while preserving
/// the selection, matching the app's refresh after the daemon acts.
fn fake_action(state: &mut SkillEvolutionState, new_status: &str) {
    let Some(index) = state.selected_pending().map(|_| state.skills.selected) else {
        return;
    };
    state.skills.library[index].proposal_status = Some(new_status.into());
    if new_status == "approved" {
        state.skills.library[index].installed = true;
        state.skills.library[index].tier = "global".into();
    }
    if let Some(status) = state.status.as_mut() {
        status.proposals_pending -= 1;
        if new_status == "approved" {
            status.proposals_approved += 1;
        } else {
            status.proposals_rejected += 1;
        }
    }
    let library = state
        .skills
        .library
        .iter()
        .filter(|row| row.installed || row.proposal_status.as_deref() == Some("pending"))
        .cloned()
        .collect();
    state.skills.set_library(library);
    if let Some(view) = state.skills.selected_skill() {
        let detail = fixture_detail(view);
        state.skills.detail = Some((view.name.clone(), detail));
    }
}

/// Dim mock app content behind the popup so the overlay's `Clear`
/// backdrop is visible while debugging.
fn render_backdrop(area: Rect, buf: &mut Buffer) {
    let dim = Style::new().fg(Color::DarkGray).add_modifier(Modifier::DIM);
    let lines: Vec<Line> = [
        "user:       run the QC pipeline on batch-12",
        "assistant:  DAG built — 9 nodes, 2 containers …",
        "user:       also re-run the survey weights",
        "assistant:  svydesign() called with nest=TRUE …",
        "user:       ship it",
    ]
    .iter()
    .map(|l| Line::styled((*l).to_string(), dim))
    .collect();
    Paragraph::new(lines).render(area, buf);
}
