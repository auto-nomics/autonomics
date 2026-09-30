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

    /// Fetch the dashboard snapshot (status + proposals in parallel).
    pub(super) fn refresh_skill_evolution(&mut self) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("skill_evolution_refresh", move || async move {
            let status = client.skill_evolution_status().await;
            let proposals = client.skill_proposals().await;
            event_tx.send(crate::app_event::AppEvent::SkillEvolutionLoaded {
                status: status.map_err(|e| e.to_string()),
                proposals: proposals.map_err(|e| e.to_string()),
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
        match key.code {
            KeyCode::Esc => self.close_skill_evolution(),
            KeyCode::Char('q') if key.modifiers.is_empty() => self.close_skill_evolution(),
            KeyCode::Char('g') | KeyCode::Char('G') => self.refresh_skill_evolution(),
            KeyCode::Char('t') if key.modifiers.is_empty() => {
                self.trigger_skill_evolution(false);
            }
            KeyCode::Char('T') | KeyCode::Char('t') if key.modifiers == KeyModifiers::SHIFT => {
                self.trigger_skill_evolution(true);
            }
            KeyCode::Down | KeyCode::Char('j') => self.state.skill_evolution.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.state.skill_evolution.select_prev(),
            KeyCode::Char('a') | KeyCode::Char('A') => {
                if let Some(proposal) = self.state.skill_evolution.selected_pending() {
                    let name = proposal.name.clone();
                    self.approve_skill_proposal(name);
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if let Some(proposal) = self.state.skill_evolution.selected_pending() {
                    let name = proposal.name.clone();
                    self.reject_skill_proposal(name);
                }
            }
            _ => {}
        }
    }
}
