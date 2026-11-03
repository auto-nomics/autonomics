//! Test harness for the skill library dashboard widget.
//!
//! Compiles against the `autonomics-tui` library target — the same
//! code the TUI runs, no `#[path]` shims.
//!
//! Two feedback modes:
//!
//! **Snapshots** (default) — render frames through `TestBackend` and
//! print them as text blocks. With `--nocapture` the frames land
//! inline in your terminal; assertions keep each frame honest:
//!
//! ```text
//! cargo test -p autonomics-tui --test test_skill_dashboard -- --nocapture
//! cargo test -p autonomics-tui --test test_skill_dashboard snapshot_loaded -- --nocapture
//! ```
//!
//! **Interactive preview** (ignored by default) — runs the widget
//! live in your terminal against synthetic data, no gateway daemon
//! needed. j/k navigate/scroll, e toggles the error path:
//!
//! ```text
//! cargo test -p autonomics-tui --test test_skill_dashboard -- --ignored interactive_preview --nocapture
//! ```

use autonomics_tui::widgets::skill_library::{
    PanelFocus, SkillBrowserState, SkillLibraryState, SkillLibraryWidget,
};
use gateway::proto::{SkillLibraryDetail, SkillLibraryView};
use ratatui::{
    backend::TestBackend,
    buffer::Buffer,
    layout::Rect,
    prelude::{Buffer as _, Terminal},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Paragraph, StatefulWidget as _, Widget as _},
};

// ═══════════════════════════════════════════════════════════════════════
// Fixtures
// ═══════════════════════════════════════════════════════════════════════

/// One installed library row.
fn fixture_skill(name: &str, tier: &str) -> SkillLibraryView {
    SkillLibraryView {
        name: name.into(),
        tier: tier.into(),
        tags: vec!["meta".into()],
        description: "fixture skill description".into(),
        installed: true,
        usage_gets: if name.contains("hot-") { 12 } else { 0 },
        usage_search_hits: 1,
        usage_runs: 0,
        usage_evals: 0,
        usage_last_used: 0,
    }
}

/// The installed library: builtin + global + workspace rows.
fn fixture_library() -> Vec<SkillLibraryView> {
    vec![
        fixture_skill("skill-system", "builtin"),
        fixture_skill("hot-pathlib-glob", "global"),
        fixture_skill("sql-syntax", "global"),
        fixture_skill("agent-r-cache", "workspace"),
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
        usage_gets: view.usage_gets,
        usage_search_hits: view.usage_search_hits,
        usage_runs: view.usage_runs,
        usage_evals: view.usage_evals,
        usage_last_used: view.usage_last_used,
    }
}

/// Dashboard state with the library fixture loaded, the selection on
/// the first row, and that row's detail fetched. Built field-by-field
/// (not a struct literal) because the state's private `list_state`
/// field makes literals illegal outside its module.
fn fixture_state() -> SkillLibraryState {
    let mut state = SkillLibraryState::default();
    state.visible = true;
    state.browser.library = fixture_library();
    let selected = &state.browser.library[state.browser.selected];
    state.browser.detail = Some((selected.name.clone(), fixture_detail(selected)));
    state
}

// ═══════════════════════════════════════════════════════════════════════
// Snapshot harness
// ═══════════════════════════════════════════════════════════════════════

/// Render one dashboard frame at the given terminal size and return
/// the screen as text rows (trailing blanks trimmed per row).
fn render_dashboard(state: &mut SkillLibraryState, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            SkillLibraryWidget::new()
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
    let mut state = SkillLibraryState::default();
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
    // Left list: every tier.
    assert!(text.contains("Skills (4)"), "{text}");
    assert!(text.contains("skill-system"), "{text}");
    assert!(text.contains("sql-syntax"), "{text}");
    // Right pane: the selected skill's detail document.
    assert!(text.contains("Tier"), "{text}");
    assert!(text.contains("1 search"), "{text}");
    assert!(text.contains("Body — SKILL.md"), "{text}");
    // Footer chord hint.
    assert!(text.contains("g refresh"), "{text}");
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
    state.browser.library.clear();
    state.browser.detail = None;
    let text = render_dashboard(&mut state, 100, 24);
    show("empty library · 100×24", &text);
    assert!(text.contains("no skills in the library"), "{text}");
    assert!(text.contains("autonomics-skills install"), "{text}");
}

#[test]
fn snapshot_error_state() {
    let mut state = fixture_state();
    state.browser.error = Some("gateway: connection refused".into());
    let text = render_dashboard(&mut state, 100, 24);
    show("error · 100×24", &text);
    assert!(
        text.contains("skills unavailable: gateway: connection refused"),
        "{text}"
    );
    assert!(text.contains("press g to retry"), "{text}");
}

#[test]
fn snapshot_stale_detail_shows_loading() {
    let mut state = fixture_state();
    // The detail belongs to the previously selected row — the pane
    // must say "loading" rather than render a mismatched document.
    state.browser.detail = Some((
        "some-other-skill".into(),
        fixture_detail(&fixture_skill("some-other-skill", "global")),
    ));
    let text = render_dashboard(&mut state, 100, 24);
    show("stale detail · 100×24", &text);
    assert!(text.contains("loading detail"), "{text}");
}

#[test]
fn snapshot_overflow_selection_scrolled_into_view() {
    let mut state = fixture_state();
    state.browser.library = (0..16)
        .map(|i| fixture_skill(&format!("skill-{i:02}"), "global"))
        .collect();
    state.browser.detail = None;

    // 60×14 → popup ~12 rows → ~9 visible list rows for 16 skills.
    state.browser.selected = 15;
    let bottom = render_dashboard(&mut state, 60, 14);
    show("overflow · selection at bottom · 60×14", &bottom);
    assert!(bottom.contains("skill-15"), "selection visible: {bottom}");
    assert!(!bottom.contains("skill-00"), "top scrolled out: {bottom}");

    state.browser.selected = 0;
    let top = render_dashboard(&mut state, 60, 14);
    show("overflow · selection at top · 60×14", &top);
    assert!(top.contains("skill-00"), "selection visible: {top}");
    assert!(!top.contains("skill-15"), "bottom scrolled out: {top}");
}

#[test]
fn snapshot_marker_follows_selection() {
    let mut state = fixture_state();
    state.browser.selected = state
        .browser
        .library
        .iter()
        .position(|s| s.tier == "workspace")
        .unwrap();
    let selected_name = state.browser.library[state.browser.selected].name.clone();
    let text = render_dashboard(&mut state, 100, 24);
    show("selection on workspace row · 100×24", &text);
    assert!(text.contains('▶'), "browser selection is visible: {text}");
    assert_eq!(state.browser.selected_skill().unwrap().name, selected_name);
}

#[test]
fn skill_refresh_preserves_selection_and_stale_replies_do_not_replace_detail() {
    let mut state = fixture_state();
    let row = state.browser.selected_skill().unwrap().clone();
    state.browser.set_library(vec![row.clone()]);
    assert_eq!(state.browser.selected, 0);
    state
        .browser
        .apply_detail(row.name.clone(), Ok(fixture_detail(&row)));
    state
        .browser
        .apply_detail("stale-row".into(), Err("late error".into()));
    assert!(state.browser.detail_error.is_none());
    assert_eq!(state.browser.detail.as_ref().unwrap().0, row.name);
    state
        .browser
        .apply_detail(row.name.clone(), Err("current error".into()));
    assert!(state.browser.detail.is_none());
    assert_eq!(
        state.browser.detail_error.as_ref().unwrap().1,
        "current error"
    );
}

#[test]
fn focus_and_navigate_drive_detail_fetch_decisions() {
    let mut state = fixture_state();
    assert_eq!(state.browser.focus, PanelFocus::List);
    assert!(state.navigate(true), "selection move → fetch detail");
    state.browser.focus_detail();
    assert!(!state.navigate(true), "detail scroll → no fetch");
    state.browser.focus_list();
    assert!(state.navigate(false), "selection move → fetch detail");
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

    /// Simulate the app's detail fetch: attach the document for the
    /// row the selection just landed on.
    fn refetch_detail(state: &mut SkillBrowserState) {
        if let Some(view) = state.selected_skill() {
            let detail = fixture_detail(view);
            state.detail = Some((view.name.clone(), detail));
            state.detail_error = None;
        }
    }

    loop {
        terminal.draw(|frame| {
            render_backdrop(frame.area(), frame.buffer_mut());

            SkillLibraryWidget::new()
                .popup_width(frame.area().width * 9 / 10)
                .popup_height((frame.area().height * 3 / 4).max(12))
                .render(frame.area(), frame.buffer_mut(), &mut state);

            let hint = "harness: j/k move/scroll · ←/→ focus · e toggle error · Esc quit";
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
            KeyCode::Down | KeyCode::Char('j') => {
                if state.navigate(true) {
                    refetch_detail(&mut state.browser);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if state.navigate(false) {
                    refetch_detail(&mut state.browser);
                }
            }
            KeyCode::Left | KeyCode::Char('h') => state.browser.focus_list(),
            KeyCode::Right | KeyCode::Char('l') => state.browser.focus_detail(),
            KeyCode::Char('e') => {
                state.browser.error = match state.browser.error {
                    Some(_) => None,
                    None => Some("gateway: connection refused (harness-simulated)".into()),
                };
            }
            // g refresh: the harness state never goes stale, so this
            // is a no-op kept for muscle-memory parity with the app.
            KeyCode::Char('g') => {}
            _ => {}
        }
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
