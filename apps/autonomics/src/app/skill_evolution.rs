//! Skill-evolution dashboard: open/refresh, key handling, and the
//! client actions (trigger / approve / reject). All HTTP runs on
//! spawned client tasks; results land as [`AppEvent`]s so the render
//! path stays free of network calls.

use super::*;

impl App {
    pub(super) fn open_skill_evolution(&mut self) {
        self.state.skill_evolution.open();
        self.refresh_skill_evolution();
    }

    pub(super) fn close_skill_evolution(&mut self) {
        self.state.skill_evolution.close();
    }

    /// Fetch the dashboard snapshot (status + unified library in
    /// parallel). The detail document for the selected row follows
    /// from [`Self::fetch_skill_detail`] when the data lands.
    pub(super) fn refresh_skill_evolution(&mut self) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("skill_evolution_refresh", move || async move {
            let (status, library, observations) = tokio::join!(
                client.skill_evolution_status(),
                client.skill_library(),
                client.skill_observations(),
            );
            event_tx.send(crate::app_event::AppEvent::SkillEvolutionLoaded {
                status: status.map_err(|e| e.to_string()),
                library: library.map_err(|e| e.to_string()),
                observations: observations.map_err(|e| e.to_string()),
            });
        });
    }

    /// Fetch the detail document for the currently selected library
    /// row. Cheap and idempotent; skipped while the dashboard is
    /// closed (the next open refreshes anyway).
    pub(super) fn fetch_skill_detail(&mut self) {
        if !self.state.skill_evolution.visible {
            return;
        }
        let Some(name) = self
            .state
            .skill_evolution
            .skills
            .selected_skill()
            .map(|s| s.name.clone())
        else {
            return;
        };
        self.state.skill_evolution.skills.detail_error = None;
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

    /// Trigger one evolution cycle. `auto` overrides the review gate
    /// for this call (the TUI equivalent of `distill --auto`).
    pub(super) fn trigger_skill_evolution(&mut self, auto: bool) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("skill_evolution_trigger", move || async move {
            let result = client
                .trigger_skill_evolution(if auto { Some(true) } else { None })
                .await;
            event_tx.send(crate::app_event::AppEvent::SkillEvolutionTriggered(
                result.map_err(|e| e.to_string()),
            ));
        });
    }

    fn approve_skill_proposal(&mut self, name: String) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("skill_proposal_approve", move || async move {
            let result = client
                .approve_skill_proposal(&name)
                .await
                .map(|outcome| outcome.destination)
                .map_err(|e| e.to_string());
            event_tx.send(crate::app_event::AppEvent::SkillProposalActioned {
                action: "approved",
                result,
            });
        });
    }

    fn reject_skill_proposal(&mut self, name: String) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("skill_proposal_reject", move || async move {
            let result = client
                .reject_skill_proposal(&name)
                .await
                .map(|_| "cluster consumed".to_string())
                .map_err(|e| e.to_string());
            event_tx.send(crate::app_event::AppEvent::SkillProposalActioned {
                action: "rejected",
                result,
            });
        });
    }

    /// Key handling while the dashboard is open.
    pub(super) fn handle_skill_evolution_key(&mut self, key: &KeyEvent) {
        use crate::widgets::skill_evolution_widget::EvolutionTab;

        let dashboard = &mut self.state.skill_evolution;
        match key.code {
            KeyCode::Esc => self.close_skill_evolution(),
            KeyCode::Char('q') if key.modifiers.is_empty() => self.close_skill_evolution(),
            KeyCode::Char('g') | KeyCode::Char('G') => self.refresh_skill_evolution(),
            KeyCode::Tab | KeyCode::BackTab => dashboard.switch_tab(),
            KeyCode::Char('1') => dashboard.active_tab = EvolutionTab::Skills,
            KeyCode::Char('2') => dashboard.active_tab = EvolutionTab::Observations,
            KeyCode::Char('t') if key.modifiers.is_empty() => {
                self.trigger_skill_evolution(false);
            }
            KeyCode::Char('T') | KeyCode::Char('t') if key.modifiers == KeyModifiers::SHIFT => {
                self.trigger_skill_evolution(true);
            }
            // ←/→ (h/l) switch the pane that owns the vertical keys.
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                dashboard.focus_list();
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                dashboard.focus_detail();
            }
            // Vertical keys act on the focused pane: move the list
            // selection (fetching the newly selected row's detail), or
            // scroll the detail document.
            KeyCode::Down | KeyCode::Char('j') => {
                if dashboard.navigate(true) {
                    self.fetch_skill_detail();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if dashboard.navigate(false) {
                    self.fetch_skill_detail();
                }
            }
            // Actions act on the selected skill's pending proposal —
            // the row's `proposal_status` attribute is the gate.
            KeyCode::Char('a') | KeyCode::Char('A') => {
                if let Some(name) = dashboard.selected_pending().map(|s| s.name.clone()) {
                    self.approve_skill_proposal(name);
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if let Some(name) = dashboard.selected_pending().map(|s| s.name.clone()) {
                    self.reject_skill_proposal(name);
                }
            }
            _ => {}
        }
    }
}
