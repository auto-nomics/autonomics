//! Flow overlay: the active agent's stage-model workflow at process
//! granularity — one row per process (kind, cardinality, error strategy)
//! with the last run's per-process outcome, plus the wire list. This is the
//! batch-workflow counterpart to the node-granular DAG view (Ctrl+O).

use data_engine::flow::FlowView;
use ratatui::{
    layout::Rect,
    prelude::Buffer,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

use super::*;
use ratatui::widgets::StatefulWidget as _;

impl App {
    pub(super) fn open_flow_view(&mut self) {
        self.state.flow_view_visible = true;
        self.request_flow_view();
    }

    pub(super) fn close_flow_view(&mut self) {
        self.state.flow_view_visible = false;
    }

    fn request_flow_view(&mut self) {
        let Some(session_id) = self
            .state
            .sessions
            .get(self.state.active_agent_idx)
            .map(|session| session.name.clone())
        else {
            self.state.flow_view_error =
                Some("No active agent. The flow view is scoped to an agent session.".to_string());
            return;
        };
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("flow_view", move || async move {
            let result = client.flow_view(&session_id).await;
            event_tx.send(crate::app_event::AppEvent::FlowViewLoaded(
                result.map_err(|error| error.to_string()),
            ));
        });
    }

    pub(super) fn handle_flow_view_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => self.close_flow_view(),
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if key.modifiers.is_empty() {
                    self.request_flow_view();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.flow_view_scroll = self.state.flow_view_scroll.saturating_add(1);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.flow_view_scroll = self.state.flow_view_scroll.saturating_sub(1);
            }
            _ => {}
        }
    }

    pub(super) fn render_flow_view(&mut self, area: Rect, buf: &mut Buffer) {
        let theme = crate::theme::current();
        let popup_width = area.width.saturating_sub(4).max(60).min(area.width);
        let popup = Rect {
            x: area.x + 2,
            y: area.y + 2,
            width: popup_width,
            height: area.height.saturating_sub(4).max(3),
        };

        let mut lines: Vec<Line> = Vec::new();
        let mut total_lines = 0usize;

        match (&self.state.flow_view, &self.state.flow_view_error) {
            (_, Some(error)) => {
                lines.push(Line::from(Span::styled(
                    format!("flow load failed: {error}"),
                    Style::default().fg(theme.error),
                )));
            }
            (None, None) => lines.push(Line::from("loading flow…")),
            (Some(view), None) => {
                collect_flow_lines(view, &theme, &mut lines);
                total_lines = lines.len();
            }
        }

        let visible = popup.height.saturating_sub(2) as usize; // borders
        let max_scroll = total_lines.saturating_sub(visible);
        self.state.flow_view_scroll = self.state.flow_view_scroll.min(max_scroll);
        let scroll = self.state.flow_view_scroll;

        let paragraph = Paragraph::new(lines)
            .scroll((scroll as u16, 0))
            .block(ratatui::widgets::Block::bordered().title(Span::styled(
                "Flow (process view) — r refresh · j/k scroll · esc close",
                Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
            )));
        paragraph.render(popup, buf);

        if max_scroll > 0 {
            let mut scrollbar_state = ScrollbarState::new(max_scroll).position(scroll);
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::new().fg(theme.text_faint))
                .render(popup, buf, &mut scrollbar_state);
        }
    }
}

/// Build the overlay's lines: run banner, per-process rows, wires.
fn collect_flow_lines(view: &FlowView, theme: &crate::theme::Theme, lines: &mut Vec<Line>) {
    use dag_engine::{Cardinality, ErrorMode};

    if view.flow.is_empty() {
        lines.push(Line::from(Span::styled(
            "no flow set for this session — the agent builds one via set_flow",
            Style::default().fg(theme.text_dim),
        )));
        return;
    }

    if let Some(run) = &view.last_run {
        let tone = if run.ok {
            theme.success
        } else {
            theme.error
        };
        lines.push(Line::from(vec![
            Span::styled("last run: ", Style::default().fg(theme.text_dim)),
            Span::styled(if run.ok { "ok" } else { "failed" }, Style::default().fg(tone).add_modifier(Modifier::BOLD)),
        ]));
        lines.push(Line::from(""));
    }

    for process in view.flow.processes() {
        let cardinality = match &process.cardinality {
            Cardinality::Singleton => "singleton".to_string(),
            Cardinality::Fanout { axis, .. } => format!("fanout · {axis}"),
        };
        let mut spans = vec![
            Span::styled(
                process.id.clone(),
                Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(process.kind.clone(), Style::default().fg(theme.text_dim)),
            Span::raw(format!("  [{cardinality}]")),
        ];
        if process.error_strategy.mode != ErrorMode::Fail {
            spans.push(Span::raw(format!(
                "  {:?}",
                process.error_strategy.mode
            )));
        }
        lines.push(Line::from(spans));

        if let Some(run) = &view.last_run {
            if let Some(summary) = run.processes.get(&process.id) {
                let tone = if summary.failed > 0 || summary.cancelled > 0 {
                    theme.error
                } else {
                    theme.success
                };
                lines.push(Line::from(Span::styled(
                    format!(
                        "    {} total · {} ok · {} failed · {} skipped · {} reused{}",
                        summary.total,
                        summary.succeeded,
                        summary.failed,
                        summary.skipped,
                        summary.reused,
                        if summary.ignored > 0 {
                            format!(" · {} ignored", summary.ignored)
                        } else {
                            String::new()
                        }
                    ),
                    Style::default().fg(tone),
                )));
            }
        }
    }

    if !view.flow.wires().is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "wires",
            Style::default().fg(theme.text_dim),
        )));
        for wire in view.flow.wires() {
            lines.push(Line::from(format!(
                "  {}:{} —{}→ {}:{}",
                wire.from, wire.from_port, wire.op, wire.to, wire.to_port
            )));
        }
    }
}
