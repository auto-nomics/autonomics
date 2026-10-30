//! Popup-driven agent management: quick model/effort switching, bulk
//! fan-out, and delegated-agent navigation.

use super::*;

/// Resolve short names relative to the delegating parent, never globally.
fn delegated_agent_index(sessions: &[AgentSession], parent: &str, target: &str) -> Option<usize> {
    let path = if target.starts_with('/') {
        target.to_owned()
    } else {
        format!("{parent}/{target}")
    };
    if path.rsplit_once('/')?.0 != parent {
        return None;
    }
    sessions.iter().position(|session| session.name == path)
}

impl App {
    // ── Agents-panel helpers ──────────────────────────────

    /// Open the model quick-picker for one agent.
    pub(super) fn open_agent_model_picker_single(&mut self, agent: &str) {
        let items = crate::widgets::agent_model_picker::collect_catalog_models(
            &self.state.model_config_state,
        );
        if items.is_empty() {
            self.state.toasts.error(
                "No models",
                Some("Configure a provider first (Model Config)".into()),
            );
            return;
        }
        let current = self.agent_model_cache.get(agent).map(|d| d.spec.clone());
        self.state.agent_model_picker.open_with(
            items,
            crate::widgets::agent_model_picker::PickerTarget::Single {
                agent: agent.to_string(),
            },
            current,
        );
    }

    /// Open the model quick-picker for every agent passing the popup's
    /// tab + query filter.
    pub(super) fn open_agent_model_picker_bulk(&mut self, filtered: Vec<String>) {
        let items = crate::widgets::agent_model_picker::collect_catalog_models(
            &self.state.model_config_state,
        );
        if items.is_empty() {
            self.state.toasts.error(
                "No models",
                Some("Configure a provider first (Model Config)".into()),
            );
            return;
        }
        if filtered.is_empty() {
            return;
        }
        self.state.agent_model_picker.open_with(
            items,
            crate::widgets::agent_model_picker::PickerTarget::AllFiltered { agents: filtered },
            None,
        );
    }

    /// Cycle one agent's effort override one stop (async read-modify-write
    /// over the agent's runtime config).
    pub(super) fn spawn_agent_effort_cycle(&mut self, agent: &str) {
        let client = self.client.clone();
        let agent = agent.to_string();
        let agent_for_refresh = agent.clone();
        let tx = self.app_event_tx.clone();
        self.spawn_client_task("cycle_agent_effort", move || async move {
            let current = client.agent_runtime_config(&agent).await.ok();
            let mut overrides = current.map(|view| view.runtime).unwrap_or_default();
            let next =
                crate::widgets::agent_effort_picker::next_effort_stop(overrides.reasoning_effort);
            overrides.reasoning_effort = next;
            let label = crate::widgets::agent_effort_picker::effort_stop_label(next);
            match client.set_agent_runtime_config(&agent, overrides).await {
                Ok(_) => tx.send(crate::app_event::AppEvent::EffortApplied { agent, label }),
                Err(error) => {
                    tracing::warn!(%error, agent = %agent, "effort cycle failed");
                }
            }
        });
        // The status line / effort column read from the render cache;
        // refetch so the new override shows promptly.
        self.agent_model_cache.remove(&agent_for_refresh);
        self.refresh_agent_model_info(&agent_for_refresh);
    }

    /// Apply a confirmed model selection from the quick-picker. The bulk
    /// path is gated by the shared y/n popup before anything is sent.
    pub(super) fn handle_agent_model_picker_confirm(
        &mut self,
        spec: String,
        target: crate::widgets::agent_model_picker::PickerTarget,
    ) {
        use crate::widgets::agent_model_picker::PickerTarget;
        match target {
            PickerTarget::Single { agent } => {
                self.apply_model_to_agents(vec![agent], &spec);
            }
            PickerTarget::AllFiltered { agents } => {
                self.state.bulk_confirm = Some(crate::state::BulkConfirm {
                    title: "切换模型".into(),
                    detail: format!("{spec} → {} 个过滤 agent", agents.len()),
                    action: crate::state::BulkAction::SetModels { agents, spec },
                });
            }
        }
    }

    /// Fire `PUT /model` for every named agent and refresh their model
    /// cache entries.
    pub(super) fn apply_model_to_agents(&mut self, agents: Vec<String>, spec: &str) {
        let client = self.client.clone();
        let spec = spec.to_string();
        let count = agents.len();
        let names = agents;
        let names_for_refresh = names.clone();
        let spec_for_toast = spec.clone();
        self.spawn_client_task("bulk_set_model", move || async move {
            let mut failed = Vec::new();
            for agent in &names {
                if let Err(error) = client.set_agent_model(agent, &spec).await {
                    tracing::warn!(%error, agent = %agent, "model swap failed");
                    failed.push(agent.clone());
                }
            }
            if failed.is_empty() {
                tracing::info!(count, spec = %spec, "bulk model swap applied");
            } else {
                tracing::warn!(count = failed.len(), "bulk model swap had failures");
            }
        });
        for agent in &names_for_refresh {
            self.agent_model_cache.remove(agent);
            self.refresh_agent_model_info(agent);
        }
        self.state.toasts.success(
            "Model",
            Some(format!("{spec_for_toast} → {count} agent(s)")),
        );
    }

    /// Apply a confirmed effort stop to the bulk set: every named agent
    /// gets the same override; non-thinking agents are skipped and
    /// counted.
    pub(super) fn apply_effort_to_agents(
        &mut self,
        agents: Vec<(String, bool)>,
        stop: Option<agentik_types::ReasoningEffort>,
    ) {
        let label = crate::widgets::agent_effort_picker::effort_stop_label(stop);
        let thinking: Vec<String> = agents
            .iter()
            .filter(|(_, thinking)| *thinking)
            .map(|(agent, _)| agent.clone())
            .collect();
        let skipped = agents.len() - thinking.len();
        let client = self.client.clone();
        let count = thinking.len();
        let stop_label = label.clone();
        self.spawn_client_task("bulk_set_effort", move || async move {
            for agent in &thinking {
                let overrides = match client.agent_runtime_config(agent).await {
                    Ok(view) => view.runtime,
                    Err(_) => agentik_core::AgentRuntimeOverrides::default(),
                };
                let mut overrides = overrides;
                overrides.reasoning_effort = stop;
                if let Err(error) = client.set_agent_runtime_config(agent, overrides).await {
                    tracing::warn!(%error, agent = %agent, "effort apply failed");
                }
            }
            tracing::info!(count, effort = %stop_label, "bulk effort applied");
        });
        for (agent, _) in &agents {
            self.agent_model_cache.remove(agent);
            self.refresh_agent_model_info(agent);
        }
        let detail = if skipped > 0 {
            format!("{label} → {count} agent(s),{skipped} 个非思考模型跳过")
        } else {
            format!("{label} → {count} agent(s)")
        };
        self.state.toasts.success("Effort", Some(detail));
    }

    /// y/n confirmation for bulk operations (model / effort fan-out).
    pub(super) fn handle_bulk_confirm_key(&mut self, key: &KeyEvent) {
        let Some(confirm) = self.state.bulk_confirm.take() else {
            return;
        };
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => match confirm.action {
                crate::state::BulkAction::SetModels { agents, spec } => {
                    self.apply_model_to_agents(agents, &spec);
                }
                crate::state::BulkAction::SetEffort { agents, stop } => {
                    self.apply_effort_to_agents(agents, stop);
                }
            },
            _ => {}
        }
    }
}

impl App {
    /// Keys while the agents-panel model quick-picker is open.
    pub(super) fn handle_agent_model_picker_key(&mut self, key: &KeyEvent) {
        use crate::widgets::agent_model_picker::AgentModelPickerKeyOutcome;
        match self.state.agent_model_picker.handle_key(*key) {
            AgentModelPickerKeyOutcome::Consumed => {}
            AgentModelPickerKeyOutcome::Close => self.state.agent_model_picker.close(),
            AgentModelPickerKeyOutcome::Confirm { spec, target } => {
                self.state.agent_model_picker.close();
                self.handle_agent_model_picker_confirm(spec, target);
            }
        }
    }

    /// Keys while the agents-popup effort quick-picker is open. Enter arms
    /// the bulk set and routes through the shared y/n confirmation.
    pub(super) fn handle_agent_effort_picker_key(&mut self, key: &KeyEvent) {
        use crate::widgets::agent_effort_picker::AgentEffortPickerKeyOutcome;
        match self.state.agent_effort_picker.handle_key(*key) {
            AgentEffortPickerKeyOutcome::Consumed => {}
            AgentEffortPickerKeyOutcome::Close => self.state.agent_effort_picker.close(),
            AgentEffortPickerKeyOutcome::Confirm { stop } => {
                self.state.agent_effort_picker.close();
                let pairs = std::mem::take(&mut self.state.agent_effort_picker.filter);
                if pairs.is_empty() {
                    return;
                }
                let label = crate::widgets::agent_effort_picker::effort_stop_label(stop);
                let count = pairs.len();
                self.state.bulk_confirm = Some(crate::state::BulkConfirm {
                    title: "批量设置 Effort".into(),
                    detail: format!("{label} → {count} 个过滤 agent"),
                    action: crate::state::BulkAction::SetEffort {
                        agents: pairs,
                        stop,
                    },
                });
            }
        }
    }
}

impl App {
    /// Keys while the agents popup (Ctrl+T) is open. The popup resolves
    /// every quick-switch action; this maps its outcomes onto App calls.
    pub(super) fn handle_agents_popup_key(&mut self, key: &KeyEvent) {
        use crate::widgets::agents_popup::AgentsPopupKeyOutcome;
        // Ctrl+W inside the popup closes the selected leaf, mirroring the
        // former picker's close behavior.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('w') {
            if let Some(agent) = self.state.agents_popup.selected_agent() {
                let id = agent.agent_id;
                self.state.agents_popup.close();
                self.close_agent_leaf_by_id(id);
            }
            return;
        }
        match self.state.agents_popup.handle_key(*key) {
            AgentsPopupKeyOutcome::Consumed => {}
            AgentsPopupKeyOutcome::Close => self.state.agents_popup.close(),
            AgentsPopupKeyOutcome::Switch(agent_id) => {
                self.state.agents_popup.close();
                self.switch_active_agent(agent_id);
            }
            AgentsPopupKeyOutcome::OpenModelPicker { agent } => {
                self.open_agent_model_picker_single(&agent);
            }
            AgentsPopupKeyOutcome::OpenModelPickerBulk { agents } => {
                self.open_agent_model_picker_bulk(agents);
            }
            AgentsPopupKeyOutcome::CycleEffort { agent } => {
                self.spawn_agent_effort_cycle(&agent);
            }
            AgentsPopupKeyOutcome::OpenEffortPicker { agents } => {
                self.state.agent_effort_picker.open_with_filter(agents);
            }
        }
    }

    /// Toggle the agents popup (Ctrl+T): opening snapshots the running
    /// agents into the picker.
    pub(super) fn toggle_agents_popup(&mut self) {
        if self.state.agents_popup.is_visible() {
            self.state.agents_popup.close();
            return;
        }
        let mut agents = self.running_agents_snapshot();
        crate::widgets::agents_popup::sort_running_agents(&mut agents);
        self.state.agents_popup.open_with(agents);
    }
}

impl App {
    /// Keys while the plan popup (<leader>p) is open.
    pub(super) fn handle_plan_popup_key(&mut self, key: &KeyEvent) {
        use crate::widgets::plan_popup::PlanPopupKeyOutcome;
        let steps = self.state.active_tab_state().plan.steps.len();
        if let PlanPopupKeyOutcome::Close = self.state.plan_popup.handle_key(*key, steps) {
            self.state.plan_popup.close();
        }
    }

    /// Keys while the tasks popup (<leader>T) is open. Enter jumps to the
    /// selected delegation's child agent leaf.
    pub(super) fn handle_tasks_popup_key(&mut self, key: &KeyEvent) {
        use crate::widgets::tasks_popup::TasksPopupKeyOutcome;
        let tasks = self.state.active_tab_state().tool_tasks.clone();
        match self.state.tasks_popup.handle_key(*key, &tasks) {
            TasksPopupKeyOutcome::Consumed => {}
            TasksPopupKeyOutcome::Close => self.state.tasks_popup.close(),
            TasksPopupKeyOutcome::Jump(target) => {
                self.state.tasks_popup.close();
                let Some(parent) = self.state.sessions.get(self.state.active_agent_idx) else {
                    return;
                };
                match delegated_agent_index(&self.state.sessions, &parent.name, &target) {
                    Some(index) => self.state.active_agent_idx = index,
                    None => {
                        self.state.toasts.info(
                            "Agent unavailable",
                            Some(format!("Delegated agent {target} is no longer open")),
                        );
                    }
                }
            }
        }
    }
}
