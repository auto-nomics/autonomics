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
//! needed. j/k navigate, a/r approve/reject, t fake cycle, e toggle
//! the error path:
//!
//! ```text
//! cargo test -p autonomics --test test_skill_dashboard -- --ignored interactive_preview --nocapture
//! ```

// ── Source shims: compile the real widget into this test crate ──
mod widgets;

use gateway::proto::{SkillEvolutionStatus, SkillProposalView};
use ratatui::{
    backend::TestBackend,
    buffer::Buffer,
    layout::Rect,
    prelude::{Buffer as _, Terminal},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Paragraph, StatefulWidget as _, Widget as _},
};
use widgets::skill_evolution_widget::{SkillEvolutionState, SkillEvolutionWidget};

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
    }
}

/// One proposal row; `rationale_len` pads the rationale to stress
/// truncation.
fn fixture_proposal(name: &str, status: &str, rationale_len: usize) -> SkillProposalView {
    SkillProposalView {
        name: name.into(),
        status: status.into(),
        authored_by: if name.contains("agent-") { "agent" } else { "distiller" }.into(),
        update: name.starts_with("update-"),
        rationale: "x".repeat(rationale_len),
        cluster_hash: "c-abc".into(),
        observation_count: 3,
        created_at: 1,
    }
}

/// Pending-first queue mixing create/update, distiller/agent, short
/// and over-long rationales.
fn fixture_queue() -> Vec<SkillProposalView> {
    vec![
        fixture_proposal("sql-syntax", "pending", 40),
        fixture_proposal("agent-r-cache", "pending", 90),
        fixture_proposal("update-ggplot-axis", "pending", 12),
        fixture_proposal("pathlib-glob", "approved", 24),
        fixture_proposal("agent-logfmt", "rejected", 24),
    ]
}

/// Dashboard state with the status/proposals fixtures loaded and the
/// selection parked on the first pending row. Built field-by-field
/// (not a struct literal) because the state's private `list_state`
/// field makes literals illegal outside its module.
fn fixture_state() -> SkillEvolutionState {
    let mut state = SkillEvolutionState::default();
    state.visible = true;
    state.status = Some(fixture_status());
    state.proposals = fixture_queue();
    state.selected = 0;
    state.last_report = Some("1 written, 0 auto-approved, 1 left pending".to_string());
    state
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
    let text = render_dashboard(&mut state, 100, 24);
    show("loaded · 100×24", &text);
    assert!(text.contains("generation 7"), "{text}");
    assert!(text.contains("propose-only"), "{text}");
    assert!(text.contains("observations 12"), "{text}");
    assert!(text.contains("proposals (pending first)"), "{text}");
    assert!(text.contains("sql-syntax"), "{text}");
    assert!(text.contains("· agent"), "agent-authored row is tagged, {text}");
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
fn snapshot_empty_queue() {
    let mut state = fixture_state();
    state.proposals.clear();
    state.last_report = None;
    let text = render_dashboard(&mut state, 100, 24);
    show("empty queue · 100×24", &text);
    assert!(text.contains("no proposals yet"), "{text}");
    assert!(text.contains("patterns surface at >= 3"), "{text}");
}

#[test]
fn snapshot_error_state() {
    let mut state = fixture_state();
    state.error = Some("gateway: connection refused".into());
    let text = render_dashboard(&mut state, 100, 24);
    show("error · 100×24", &text);
    assert!(text.contains("error: gateway: connection refused"), "{text}");
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
fn snapshot_overflow_selection_scrolled_into_view() {
    let mut state = fixture_state();
    state.proposals = (0..8)
        .map(|i| fixture_proposal(&format!("prop-{i:02}"), "pending", 20))
        .collect();

    // 60×14 → popup 54×12 → ~5 visible queue rows for 8 proposals.
    state.selected = 7;
    let bottom = render_dashboard(&mut state, 60, 14);
    show("overflow · selection at bottom · 60×14", &bottom);
    assert!(bottom.contains("prop-07"), "selection visible: {bottom}");
    assert!(!bottom.contains("prop-00"), "top scrolled out: {bottom}");

    state.selected = 0;
    let top = render_dashboard(&mut state, 60, 14);
    show("overflow · selection at top · 60×14", &top);
    assert!(top.contains("prop-00"), "selection visible: {top}");
    assert!(!top.contains("prop-07"), "bottom scrolled out: {top}");
}

#[test]
fn snapshot_selection_marker_only_on_pending_row() {
    let mut state = fixture_state();
    // Selection parked on a non-pending row: nothing is actionable,
    // so no row may carry the marker.
    state.selected = 3; // "pathlib-glob", approved
    let text = render_dashboard(&mut state, 100, 24);
    show("selection on history row · 100×24", &text);
    assert!(!text.contains('▶'), "history rows never highlight: {text}");
    assert!(state.selected_pending().is_none());
}

// ═══════════════════════════════════════════════════════════════════════
// Interactive preview (opt-in)
// ═══════════════════════════════════════════════════════════════════════

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

    let mut state = fixture_state();
    let mut next_canned = 0;
    const CANNED_PROPOSALS: &[&str] = &[
        "canned-fmt-strings",
        "canned-sheet-bounds",
        "canned-join-hint",
        "canned-rng-seed",
    ];

    loop {
        terminal.draw(|frame| {
            render_backdrop(frame.area(), frame.buffer_mut());

            SkillEvolutionWidget::new()
                .popup_width(frame.area().width * 9 / 10)
                .popup_height((frame.area().height * 3 / 4).max(12))
                .render(frame.area(), frame.buffer_mut(), &mut state);

            let hint = "harness: j/k move · a approve · r reject · t fake cycle · e toggle error · g noop · Esc quit";
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
            KeyCode::Down | KeyCode::Char('j') => state.select_next(),
            KeyCode::Up | KeyCode::Char('k') => state.select_prev(),
            KeyCode::Char('a') => fake_action(&mut state, "approved"),
            KeyCode::Char('r') => fake_action(&mut state, "rejected"),
            KeyCode::Char('e') => {
                state.error = match state.error {
                    Some(_) => None,
                    None => Some("gateway: connection refused (harness-simulated)".into()),
                };
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                let name = CANNED_PROPOSALS[next_canned % CANNED_PROPOSALS.len()];
                next_canned += 1;
                let auto = key.code == KeyCode::Char('T');
                let status = if auto { "approved" } else { "pending" };
                state.proposals.insert(
                    0,
                    fixture_proposal(name, status, 30),
                );
                let status_field = state.status.as_mut().unwrap();
                status_field.generation += 1;
                status_field.proposals_pending += usize::from(!auto);
                status_field.proposals_approved += usize::from(auto);
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

/// Approve/reject the selected pending proposal, then advance the
/// selection to the next pending row — mirrors what the app's
/// refresh does after the daemon acts.
fn fake_action(state: &mut SkillEvolutionState, new_status: &str) {
    let Some(index) = state.selected_pending().map(|_| state.selected) else {
        return;
    };
    state.proposals[index].status = new_status.into();
    if let Some(status) = state.status.as_mut() {
        status.proposals_pending -= 1;
        if new_status == "approved" {
            status.proposals_approved += 1;
        } else {
            status.proposals_rejected += 1;
        }
    }
    state.select_next();
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
