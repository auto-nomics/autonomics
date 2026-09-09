//! Interactive DAG overlay: runtime fetch, key handling, and rendering.

use ratatui::{
    layout::Rect,
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, StatefulWidget, Widget as _},
};

use super::*;

impl App {
    /// Open the active agent's DAG view and refresh its snapshot.
    pub(super) fn open_dag_view(&mut self) {
        self.state.dag_view_visible = true;
        self.request_dag_snapshot();
    }

    pub(super) fn close_dag_view(&mut self) {
        self.state.dag_view_visible = false;
    }

    fn request_dag_snapshot(&mut self) {
        let Some(session_id) = self
            .state
            .sessions
            .get(self.state.active_agent_idx)
            .map(|session| session.name.clone())
        else {
            self.state.dag_view_error =
                Some("No active agent. The DAG view is scoped to an agent session.".to_string());
            return;
        };
        let Some(host) = self.host.as_ref() else {
            self.state.dag_view_error = Some("Runtime host unavailable.".to_string());
            return;
        };
        let client = host.data_engine_client(&session_id);
        let event_tx = self.app_event_tx.clone();
        self.runtime_handle.spawn(async move {
            let result = client.dag_tui_snapshot().await;
            event_tx.send(crate::app_event::AppEvent::DagSnapshotLoaded(
                result.map_err(|error| error.to_string()),
            ));
        });
    }

    pub(super) fn handle_dag_view_key(&mut self, key: &KeyEvent) {
        let Some(snapshot) = self.state.dag_snapshot.clone() else {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('r')) {
                self.state.dag_view_visible = false;
            }
            return;
        };

        match key.code {
            KeyCode::Esc => self.close_dag_view(),
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if key.modifiers.is_empty() {
                    self.request_dag_snapshot();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.dag_view_state.select_up(&snapshot);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.dag_view_state.select_down(&snapshot);
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.state.dag_view_state.select_left(&snapshot);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.state.dag_view_state.select_right(&snapshot);
            }
            KeyCode::Tab => self.state.dag_view_state.select_next(&snapshot),
            KeyCode::BackTab => self.state.dag_view_state.select_previous(&snapshot),
            _ => {}
        }
    }

    pub(super) fn render_dag_view(&mut self, area: Rect, buf: &mut Buffer) {
        let popup_width = area.width.saturating_sub(4).max(60).min(area.width);
        let popup_height = area.height.saturating_sub(4).max(16).min(area.height);
        let popup = crate::widgets::popup::Popup::new(
            " Data DAG ",
            crate::widgets::popup::PopupControls::new(false, false),
        )
        .width(popup_width)
        .height(popup_height)
        .accent(Color::Cyan);
        let inner = popup.render(area, buf);

        let Some(snapshot) = self.state.dag_snapshot.as_ref() else {
            let message = self
                .state
                .dag_view_error
                .as_deref()
                .unwrap_or("Loading DAG snapshot…");
            Paragraph::new(Line::from(Span::styled(
                message.to_string(),
                Style::default().fg(if self.state.dag_view_error.is_some() {
                    Color::Red
                } else {
                    Color::DarkGray
                }),
            )))
            .render(inner, buf);
            return;
        };

        let mut state = self.state.dag_view_state.clone();
        crate::widgets::dag_view::DagTuiWidget::new(snapshot)
            .focused(true)
            .render(inner, buf, &mut state);
        // Preserve only user mutations (selection/viewport); the widget uses a
        // temporary state so this method can keep taking `&self`.
        self.state.dag_view_state = state;
    }
}
