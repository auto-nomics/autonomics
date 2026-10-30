//! Pane focus, sidebar selection, quick model/effort switching, and
//! navigation to delegated agents.

use super::*;
use crate::state::WorkspaceFocus;

fn adjacent_pane(focus: WorkspaceFocus, direction: char) -> WorkspaceFocus {
    use WorkspaceFocus::*;
    match (focus, direction) {
        (Chat | Input, 'l') => Tasks,
        (Plan | Tasks | Agents, 'h') => Chat,
        (Tasks, 'k') => Plan,
        (Agents, 'k') => Tasks,
        (Input, 'k') => Chat,
        (Plan, 'j') => Tasks,
        (Tasks, 'j') => Agents,
        (Agents, 'j') => Input,
        (Chat, 'j') => Input,
        _ => focus,
    }
}

fn populated_adjacent_pane(
    focus: WorkspaceFocus,
    direction: char,
    has_plan: bool,
    has_tasks: bool,
) -> WorkspaceFocus {
    use WorkspaceFocus::*;
    let populated = |pane| match pane {
        Plan => has_plan,
        Tasks => has_tasks,
        _ => true,
    };
    if matches!(focus, Chat | Input) && direction == 'l' {
        return [Tasks, Plan, Agents]
            .into_iter()
            .find(|&pane| populated(pane))
            .unwrap_or(Chat);
    }
    let mut next = adjacent_pane(focus, direction);
    while !populated(next) {
        let candidate = adjacent_pane(next, direction);
        if candidate == next {
            return Chat;
        }
        next = candidate;
    }
    next
}

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
    /// Ctrl+h/j/k/l pane movement (Vim-style) and Ctrl+Backspace (= `h`).
    /// Returns true when the key was a navigation chord.
    pub(super) fn handle_workspace_navigation(&mut self, key: &KeyEvent) -> bool {
        if !key.modifiers.contains(KeyModifiers::CONTROL)
            || key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SUPER)
            || self.state.sessions.is_empty()
            || self.state.active_tab_state().in_history_search
        {
            return false;
        }
        let direction = match key.code {
            KeyCode::Char(c @ ('h' | 'j' | 'k' | 'l')) => c,
            KeyCode::Backspace => 'h',
            _ => return false,
        };
        let ts = self.state.active_tab_state_mut();
        ts.focus_pane(populated_adjacent_pane(
            ts.focused_pane(),
            direction,
            !ts.plan.is_empty(),
            !ts.tool_tasks.is_empty(),
        ));
        true
    }

    /// Keys while a sidebar pane (plan / tasks / docked agents panel) holds
    /// focus.
    pub(super) fn handle_sidebar_key(&mut self, key: &KeyEvent) {
        let ts = self.state.active_tab_state_mut();
        if ts.focused_pane() == WorkspaceFocus::Agents && ts.sidebar.agent_filter.handle_key(key) {
            ts.sidebar.reset_agent_scroll();
            return;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return;
        }
        let ts = self.state.active_tab_state_mut();
        ts.sidebar.sync(ts.plan.steps.len(), &ts.tool_tasks);
        let pane = ts.focused_pane();
        match key.code {
            KeyCode::Esc => ts.focus_pane(WorkspaceFocus::Chat),
            // Like Vim's command entry, ':' opens commands from anywhere.
            KeyCode::Char(':') => self.state.command_palette.open(),
            // Enter on a docked agents-panel row switches that leaf in
            // (resolved by agent identity, never a stored index) — unless
            // the column cursor sits on the model or effort cell: Enter
            // there quick-switches that aspect of the selected agent
            // instead of moving the leaf.
            KeyCode::Enter if pane == WorkspaceFocus::Agents => {
                let agents = self.running_agents_snapshot();
                let selected = self
                    .state
                    .active_tab_state()
                    .sidebar
                    .selected_agent_id(&agents);
                if let Some(agent_id) = selected {
                    let column = self.state.active_tab_state().sidebar.agent_column;
                    let agent_name = agents
                        .iter()
                        .find(|agent| agent.agent_id == agent_id)
                        .map(|agent| agent.name.clone());
                    match column {
                        crate::widgets::sidebar::AgentColumn::Model => {
                            if let Some(agent) = agent_name {
                                self.open_agent_model_picker_single(&agent);
                            }
                        }
                        crate::widgets::sidebar::AgentColumn::Effort => {
                            if let Some(agent) = agent_name {
                                self.spawn_agent_effort_cycle(&agent);
                            }
                        }
                        crate::widgets::sidebar::AgentColumn::Name => {
                            if self.switch_active_agent(agent_id) {
                                self.state
                                    .active_tab_state_mut()
                                    .focus_pane(WorkspaceFocus::Chat);
                            }
                        }
                    }
                }
            }
            // h/l move the agents-panel column cursor (name → model →
            // effort), skipping columns hidden at the current sidebar
            // width. Plain h/l reach here only while the inline search is
            // closed (it owns plain characters while active), like 'm'.
            KeyCode::Char('h' | 'l')
                if pane == WorkspaceFocus::Agents && !ts.sidebar.agent_filter.searching =>
            {
                ts.sidebar
                    .advance_agent_column(key.code == KeyCode::Char('l'));
            }
            // Quick model / effort switching (agents panel, search closed).
            // m: model picker for the cursor row · M: model picker for the
            // whole filtered set · e: cycle the cursor row's effort one
            // stop · E: effort picker for the filtered set. The bulk paths
            // ask for confirmation (y/n) after Enter in the picker.
            KeyCode::Char('m')
                if pane == WorkspaceFocus::Agents && !ts.sidebar.agent_filter.searching =>
            {
                if let Some(agent) = self.selected_agent_name() {
                    self.open_agent_model_picker_single(&agent);
                }
            }
            KeyCode::Char('M')
                if pane == WorkspaceFocus::Agents && !ts.sidebar.agent_filter.searching =>
            {
                self.open_agent_model_picker_bulk();
            }
            KeyCode::Char('e')
                if pane == WorkspaceFocus::Agents && !ts.sidebar.agent_filter.searching =>
            {
                if let Some(agent) = self.selected_agent_name() {
                    self.spawn_agent_effort_cycle(&agent);
                }
            }
            KeyCode::Char('E')
                if pane == WorkspaceFocus::Agents && !ts.sidebar.agent_filter.searching =>
            {
                self.state.agent_effort_picker.open();
            }
            KeyCode::Enter if ts.focused_pane() == WorkspaceFocus::Tasks => {
                let selected = ts.sidebar.tasks.selected();
                let task = selected.and_then(|index| ts.tool_tasks.get(index));
                let target = task
                    .filter(|task| task.name == "delegate_to")
                    .and_then(|task| task.target_agent.clone());
                let Some(target) = target else {
                    if task.is_some_and(|task| task.name == "delegate_to") {
                        self.state.toasts.info(
                            "Agent unavailable",
                            Some("This task has no target agent available".into()),
                        );
                    }
                    return;
                };
                let Some(parent) = self.state.sessions.get(self.state.active_agent_idx) else {
                    return;
                };
                if let Some(index) =
                    delegated_agent_index(&self.state.sessions, &parent.name, &target)
                {
                    self.state.active_agent_idx = index;
                    self.state
                        .active_tab_state_mut()
                        .focus_pane(WorkspaceFocus::Chat);
                } else {
                    self.state.toasts.info(
                        "Agent unavailable",
                        Some(format!("Delegated agent {target} is no longer open")),
                    );
                }
            }
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Char('j' | 'k' | 'h' | 'l')
            | KeyCode::Home
            | KeyCode::End => {
                // Agents-panel cursor moves over the workspace-wide list,
                // anchored by agent identity so it survives refreshes.
                if pane == WorkspaceFocus::Agents {
                    let agents = self.running_agents_snapshot();
                    let agents = self
                        .state
                        .active_tab_state()
                        .sidebar
                        .filtered_agents(&agents);
                    if agents.is_empty() {
                        return;
                    }
                    let ts = self.state.active_tab_state_mut();
                    let index = ts.sidebar.selected_agent_index(&agents).unwrap_or(0);
                    let next = match key.code {
                        KeyCode::Up | KeyCode::Char('k') => index.saturating_sub(1),
                        KeyCode::Down | KeyCode::Char('j') => (index + 1).min(agents.len() - 1),
                        KeyCode::Home => 0,
                        KeyCode::End => agents.len() - 1,
                        _ => index,
                    };
                    ts.sidebar.select_agent(&agents, next);
                    return;
                }
                let tasks = ts.focused_pane() == WorkspaceFocus::Tasks;
                let (selected, count) = if tasks {
                    (ts.sidebar.tasks.selected(), ts.tool_tasks.len())
                } else {
                    (ts.sidebar.plan.selected(), ts.plan.steps.len())
                };
                if count == 0 {
                    return;
                }
                let next = match key.code {
                    KeyCode::Up | KeyCode::Char('k') => selected.unwrap_or(0).saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => {
                        (selected.unwrap_or(0) + 1).min(count - 1)
                    }
                    KeyCode::Home => 0,
                    KeyCode::End => count - 1,
                    _ => selected.unwrap_or(0),
                };
                if tasks {
                    let tasks_snapshot = ts.tool_tasks.clone();
                    ts.sidebar.select_task(Some(next), &tasks_snapshot);
                } else {
                    ts.sidebar.plan.select(Some(next));
                }
            }
            _ => {}
        }
    }

    // ── Agents-panel helpers ──────────────────────────────

    /// The docked panel's cursor row, as an agent path.
    pub(super) fn selected_agent_name(&self) -> Option<String> {
        let agents = self.running_agents_snapshot();
        self.state
            .active_tab_state()
            .sidebar
            .selected_agent_id(&agents)
            .and_then(|id| agents.iter().find(|agent| agent.agent_id == id))
            .map(|agent| agent.name.clone())
    }

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

    /// Open the model quick-picker for every agent passing the panel
    /// filter.
    pub(super) fn open_agent_model_picker_bulk(&mut self) {
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
        let agents = self.running_agents_snapshot();
        let filtered: Vec<String> = self
            .state
            .active_tab_state()
            .sidebar
            .filtered_agents(&agents)
            .into_iter()
            .map(|agent| agent.name)
            .collect();
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

    /// Keys while the agents-panel effort quick-picker is open. Enter arms
    /// the bulk set and routes through the shared y/n confirmation.
    pub(super) fn handle_agent_effort_picker_key(&mut self, key: &KeyEvent) {
        use crate::widgets::agent_effort_picker::AgentEffortPickerKeyOutcome;
        match self.state.agent_effort_picker.handle_key(*key) {
            AgentEffortPickerKeyOutcome::Consumed => {}
            AgentEffortPickerKeyOutcome::Close => self.state.agent_effort_picker.close(),
            AgentEffortPickerKeyOutcome::Confirm { stop } => {
                self.state.agent_effort_picker.close();
                let agents = self.running_agents_snapshot();
                let filtered = self
                    .state
                    .active_tab_state()
                    .sidebar
                    .filtered_agents(&agents);
                if filtered.is_empty() {
                    return;
                }
                let pairs: Vec<(String, bool)> = filtered
                    .into_iter()
                    .map(|agent| (agent.name, agent.effort.is_some()))
                    .collect();
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
