//! Standalone Ratatui preview for the Data DAG widget.

use std::io::{self, Stdout};

#[path = "../widgets/dag_view.rs"]
mod dag_view;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::tty::IsTty;
use dag_core::dag::{DagEdgeView, DagNodeView, DagTuiSnapshot, RuntimeStatus};
use ratatui::widgets::StatefulWidget;
use ratatui::{Terminal, backend::CrosstermBackend};

use dag_view::{DagTuiWidget, DagViewState};

fn node(id: &str, kind: &str, status: RuntimeStatus, dirty: bool) -> DagNodeView {
    DagNodeView {
        id: id.to_string(),
        kind: kind.to_string(),
        status,
        dirty,
        inputs: Vec::new(),
        outputs: Vec::new(),
    }
}

fn edge(from: &str, to: &str, from_port: usize, to_port: usize) -> DagEdgeView {
    DagEdgeView {
        from: from.to_string(),
        from_port: from_port as u8,
        to: to.to_string(),
        to_port: to_port as u8,
    }
}

fn pipeline() -> DagTuiSnapshot {
    DagTuiSnapshot {
        nodes: vec![
            node("catalog", "source", RuntimeStatus::Success, false),
            node("validate", "schema", RuntimeStatus::Success, false),
            node("features", "transform", RuntimeStatus::Success, false),
            node("model", "estimate", RuntimeStatus::Running, true),
            node("diagnostics", "report", RuntimeStatus::Pending, true),
            node("export", "sink", RuntimeStatus::Pending, true),
        ],
        edges: vec![
            edge("catalog", "validate", 0, 0),
            edge("validate", "features", 0, 0),
            edge("features", "model", 0, 0),
            edge("features", "diagnostics", 0, 0),
            edge("model", "export", 0, 0),
            edge("diagnostics", "export", 0, 0),
        ],
    }
}

fn fanout() -> DagTuiSnapshot {
    DagTuiSnapshot {
        nodes: vec![
            node("cohort", "source", RuntimeStatus::Success, false),
            node("age", "split", RuntimeStatus::Success, false),
            node("sex", "split", RuntimeStatus::Success, false),
            node("site", "split", RuntimeStatus::Failed, true),
            node("summary", "aggregate", RuntimeStatus::Pending, true),
        ],
        edges: vec![
            edge("cohort", "age", 0, 0),
            edge("cohort", "sex", 0, 0),
            edge("cohort", "site", 0, 0),
            edge("age", "summary", 0, 0),
            edge("sex", "summary", 0, 0),
            edge("site", "summary", 0, 0),
        ],
    }
}

fn long_path() -> DagTuiSnapshot {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for stage in 0..8 {
        let id = format!("stage-{stage}");
        nodes.push(node(
            &id,
            "step",
            if stage == 6 {
                RuntimeStatus::Failed
            } else if stage == 7 {
                RuntimeStatus::Skipped
            } else {
                RuntimeStatus::Success
            },
            stage >= 6,
        ));
        if stage > 0 {
            let previous = format!("stage-{}", stage - 1);
            edges.push(edge(&previous, &id, 0, 0));
        }
    }
    DagTuiSnapshot { nodes, edges }
}

fn fixtures() -> [DagTuiSnapshot; 3] {
    [pipeline(), fanout(), long_path()]
}

fn main() -> io::Result<()> {
    if !io::stdout().is_tty() {
        eprintln!("dag_tui_preview needs an interactive terminal");
        return Ok(());
    }

    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;

    let mut snapshots = fixtures();
    let mut fixture_index = 0usize;
    let mut state = DagViewState::default();
    let result = event_loop(
        &mut terminal,
        &mut state,
        &mut snapshots,
        &mut fixture_index,
    );

    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut DagViewState,
    snapshots: &mut [DagTuiSnapshot; 3],
    fixture_index: &mut usize,
) -> io::Result<()> {
    loop {
        let snapshot = &snapshots[*fixture_index];
        let mut render_state = state.clone();
        terminal.draw(|frame| {
            DagTuiWidget::new(snapshot)
                .focused(true)
                .fixture_help(true)
                .render(frame.area(), frame.buffer_mut(), &mut render_state);
        })?;
        *state = render_state;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let snapshot = &snapshots[*fixture_index];
        match (key.code, key.modifiers) {
            (KeyCode::Char('q' | 'Q'), _) | (KeyCode::Esc, _) => return Ok(()),
            (KeyCode::Char('c' | 'C'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(());
            }
            (KeyCode::Left | KeyCode::Char('h'), _) => state.select_up(snapshot),
            (KeyCode::Right | KeyCode::Char('l'), _) => state.select_down(snapshot),
            (KeyCode::Up | KeyCode::Char('k'), _) => state.select_left(snapshot),
            (KeyCode::Down | KeyCode::Char('j'), _) => state.select_right(snapshot),
            (KeyCode::Tab, _) => state.select_next(snapshot),
            (KeyCode::BackTab, _) => state.select_previous(snapshot),
            (KeyCode::Char(' '), _) => {
                *fixture_index = (*fixture_index + 1) % snapshots.len();
                state.reset();
            }
            (KeyCode::Char('r' | 'R'), _) => {
                state.reset();
            }
            _ => {}
        }
    }
}
