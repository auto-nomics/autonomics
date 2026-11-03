//! Skill library dashboard: open/refresh and key handling. All HTTP
//! runs on spawned client tasks; results land as [`AppEvent`]s so the
//! render path stays free of network calls.

use super::*;

impl App {
    pub(super) fn open_skill_library(&mut self) {
        self.state.skill_library.open();
        self.refresh_skill_library();
    }

    pub(super) fn close_skill_library(&mut self) {
        self.state.skill_library.close();
    }

    /// Fetch the library snapshot. The detail document for the
    /// selected row follows from [`Self::fetch_skill_detail`] when
    /// the data lands.
    pub(super) fn refresh_skill_library(&mut self) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("skill_library_refresh", move || async move {
            let library = client.skill_library().await;
            event_tx.send(crate::app_event::AppEvent::SkillLibraryLoaded {
                library: library.map_err(|e| e.to_string()),
            });
        });
    }

    /// Fetch the detail document for the currently selected library
    /// row. Cheap and idempotent; skipped while the dashboard is
    /// closed (the next open refreshes anyway).
    pub(super) fn fetch_skill_detail(&mut self) {
        if !self.state.skill_library.visible {
            return;
        }
        let Some(name) = self
            .state
            .skill_library
            .browser
            .selected_skill()
            .map(|s| s.name.clone())
        else {
            return;
        };
        self.state.skill_library.browser.detail_error = None;
        // Event handling ignores replies for a previously selected row.
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("skill_detail_fetch", move || async move {
            let result = client.skill_library_detail(&name).await;
            event_tx.send(crate::app_event::AppEvent::SkillDetailLoaded {
                name,
                result: result.map_err(|e| e.to_string()),
            });
        });
    }

    /// Key handling while the dashboard is open.
    pub(super) fn handle_skill_library_key(&mut self, key: &KeyEvent) {
        let dashboard = &mut self.state.skill_library;
        match key.code {
            KeyCode::Esc => self.close_skill_library(),
            KeyCode::Char('q') if key.modifiers.is_empty() => self.close_skill_library(),
            KeyCode::Char('g') | KeyCode::Char('G') => self.refresh_skill_library(),
            // ←/→ (h/l) switch the pane that owns the vertical keys.
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                dashboard.browser.focus_list();
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                dashboard.browser.focus_detail();
            }
            // Vertical keys act on the focused pane: move the list
            // selection (fetching the newly selected row's detail), or
            // scroll the detail document.
            KeyCode::Down | KeyCode::Char('j') if dashboard.navigate(true) => {
                self.fetch_skill_detail();
            }
            KeyCode::Up | KeyCode::Char('k') if dashboard.navigate(false) => {
                self.fetch_skill_detail();
            }
            _ => {}
        }
    }
}
