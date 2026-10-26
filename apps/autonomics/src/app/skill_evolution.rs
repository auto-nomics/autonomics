//! Skill-evolution dashboard: open/refresh, key handling, and the
//! client actions (trigger / refresh). All HTTP runs on spawned
//! client tasks; results land as [`AppEvent`]s so the render path
//! stays free of network calls.

use super::*;

impl App {
    pub(super) fn open_skill_evolution(&mut self) {
        self.state.skill_evolution.open();
        self.refresh_skill_evolution();
    }

    pub(super) fn close_skill_evolution(&mut self) {
        self.state.skill_evolution.close();
    }

    /// Fetch the KMS dashboard snapshot in parallel. The detail document
    /// for the selected row follows from [`Self::fetch_skill_detail`].
    pub(super) fn refresh_skill_evolution(&mut self) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("skill_evolution_refresh", move || async move {
            let (workers, skills, proposals, evidence) = tokio::join!(
                client.kms_worker_statuses(),
                client.kms_skills(),
                client.kms_skill_proposals(),
                client.kms_evidence(),
            );
            event_tx.send(crate::app_event::AppEvent::SkillEvolutionLoaded {
                workers: workers.map_err(|e| e.to_string()),
                skills: skills.map_err(|e| e.to_string()),
                proposals: proposals.map_err(|e| e.to_string()),
                evidence: evidence.map_err(|e| e.to_string()),
            });
        });
    }

    /// Lighter-weight worker/proposal poll fired by the tick thread while
    /// the dashboard is visible.
    pub(super) fn poll_skill_evolution(&mut self) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.state.skill_evolution_poll = Some(std::time::Instant::now());
        self.spawn_client_task("skill_evolution_poll", move || async move {
            let (workers, proposals) =
                tokio::join!(client.kms_worker_statuses(), client.kms_skill_proposals());
            event_tx.send(crate::app_event::AppEvent::KmsDashboardStatusPolled {
                workers: workers.map_err(|e| e.to_string()),
                proposals: proposals.map_err(|e| e.to_string()),
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
        if let Some(detail) = self.state.skill_evolution.skills.proposal_detail(&name) {
            self.state
                .skill_evolution
                .skills
                .apply_detail(name, Ok(detail));
            return;
        }
        // Event handling ignores replies for a previously selected row.
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("skill_detail_fetch", move || async move {
            let result = client.kms_skill_detail(&name).await.map(kms_skill_detail);
            event_tx.send(crate::app_event::AppEvent::SkillDetailLoaded {
                name,
                result: result.map_err(|e| e.to_string()),
            });
        });
    }

    /// Apply a human decision to the selected pending KMS proposal.
    pub(super) fn decide_selected_skill_proposal(&mut self, approved: bool) {
        let Some(id) = self.state.skill_evolution.skills.selected_proposal_id() else {
            self.state.toasts.info(
                "KMS skill proposals",
                Some("select a pending proposal first".to_string()),
            );
            return;
        };
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("kms_skill_proposal_decision", move || async move {
            let result = if approved {
                client.approve_kms_skill_proposal(id).await
            } else {
                client.reject_kms_skill_proposal(id).await
            };
            event_tx.send(crate::app_event::AppEvent::SkillProposalDecision {
                approved,
                result: result.map_err(|e| e.to_string()),
            });
        });
    }

    /// Key handling while the dashboard is open.
    pub(super) fn handle_skill_evolution_key(&mut self, key: &KeyEvent) {
        use crate::widgets::evolution_widget::EvolutionTab;

        let dashboard = &mut self.state.skill_evolution;
        match key.code {
            KeyCode::Esc => self.close_skill_evolution(),
            KeyCode::Char('q') if key.modifiers.is_empty() => self.close_skill_evolution(),
            KeyCode::Char('g') | KeyCode::Char('G') => self.refresh_skill_evolution(),
            KeyCode::Tab | KeyCode::BackTab => dashboard.switch_tab(),
            KeyCode::Char('1') => dashboard.active_tab = EvolutionTab::Skills,
            KeyCode::Char('2') => dashboard.active_tab = EvolutionTab::Observations,
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.decide_selected_skill_proposal(true);
            }
            KeyCode::Char('x') | KeyCode::Char('X') => {
                self.decide_selected_skill_proposal(false);
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

fn kms_skill_detail(
    detail: gateway::proto::KmsSkillDetailView,
) -> gateway::proto::SkillLibraryDetail {
    gateway::proto::SkillLibraryDetail {
        name: detail.skill_name,
        tier: detail.tier,
        tags: detail.tags,
        description: detail.description,
        installed: true,
        body: detail.body_md,
        workflows: detail
            .artifacts
            .iter()
            .filter(|artifact| artifact.relative_path.starts_with("workflow/"))
            .map(|artifact| {
                artifact
                    .relative_path
                    .trim_start_matches("workflow/")
                    .trim_end_matches(".toml")
                    .to_string()
            })
            .collect(),
        evals: detail
            .artifacts
            .iter()
            .filter(|artifact| artifact.relative_path.starts_with("evals/"))
            .map(|artifact| {
                artifact
                    .relative_path
                    .trim_start_matches("evals/")
                    .trim_end_matches(".toml")
                    .to_string()
            })
            .collect(),
        proposal: None,
        evidence_count: 0,
        usage_gets: 0,
        usage_search_hits: 0,
        usage_runs: 0,
        usage_evals: 0,
        usage_last_used: 0,
    }
}
