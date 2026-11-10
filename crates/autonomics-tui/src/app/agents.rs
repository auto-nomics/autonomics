//! Agent creation, restoration, deletion, and lifecycle management.

use super::*;

impl App {
    /// Spawn a new agent from a profile asynchronously.
    ///
    /// Because the event loop is already inside `runtime.block_on(...)`, we
    /// can't call `block_on` again. Instead, we spawn the creation as a
    /// background task and send the result back via the app event channel.
    /// The `AgentSpawned` event is handled in `handle_app_event`.
    pub(super) fn spawn_agent_from_profile(
        &mut self,
        kind: agentik_core::AgentKind,
        agent_name: &str,
    ) {
        let target_path = agentik_types::AgentPath::root()
            .join(agent_name)
            .unwrap_or_else(|_| agentik_types::AgentPath::root());
        self.spawn_agent_at_path(agentik_core::AgentProfileConfig::new(kind), target_path);
    }

    /// Spawn an agent at its persisted hierarchical path. Tool-spawned
    /// children live below `/root/parent/child`; restoring them from only the
    /// leaf name would create a different runtime identity.
    pub(super) fn spawn_agent_at_path(
        &mut self,
        config: agentik_core::AgentProfileConfig,
        target_path: agentik_types::AgentPath,
    ) {
        let agent_name = target_path.name();
        tracing::info!(
            kind = config.kind.name(),
            agent = %target_path,
            "spawn_agent_at_path called"
        );
        // Mark this spawn as user-initiated so the matching
        // `AgentRegistered` event can steal focus to the new leaf.
        // (Host-spawned agents don't set this and remain non-stealing.)
        self.state.pending_focus_agent_name = Some(agent_name.to_string());

        if self.state.active_model_spec.is_none() {
            tracing::warn!("no model configured — configure one in Config tab first");
            self.state.toasts.error(
                "Agent creation failed",
                Some("Configure a model before creating agents".into()),
            );
            return;
        }

        let client = self.client.clone();
        let kind = config.kind;
        let runtime = config.runtime.clone();
        let agent_name_owned = agent_name.to_string();
        let parent_path = target_path
            .parent()
            .unwrap_or_else(agentik_types::AgentPath::root);
        let model_spec = config.preferred_model.clone();
        let kind_name_owned = kind.name().to_string();
        let tx = self.app_event_tx.clone();

        tracing::debug!(has_override = model_spec.is_some(), "model spec passed");
        self.spawn_client_task(
            &format!("spawn_agent::{agent_name_owned}"),
            move || async move {
                tracing::debug!(
                    kind = kind_name_owned,
                    agent = %agent_name_owned,
                    "async spawn task started"
                );
                // The daemon resolves the spec (falling back to its
                // default model when unresolvable) and registers the
                // agent; the AgentRegistered host frame follows on the
                // event stream.
                let result = client
                    .spawn_agent(
                        &agent_name_owned,
                        parent_path.as_str(),
                        kind.name(),
                        Some(runtime),
                        model_spec.as_deref(),
                    )
                    .await
                    .map_err(|e| e.to_string());
                let event = match result {
                    Ok(name) => {
                        tracing::info!(kind = %kind_name_owned, agent = %name, "agent spawned and registered with daemon");
                        crate::app_event::AppEvent::AgentSpawned {
                            profile_name: kind_name_owned.clone(),
                            result: Ok(name),
                        }
                    }
                    Err(e) => {
                        tracing::error!(kind = %kind_name_owned, error = %e, "agent spawn failed");
                        crate::app_event::AppEvent::AgentSpawned {
                            profile_name: kind_name_owned,
                            result: Err(e),
                        }
                    }
                };
                tx.send(event);
            },
        );

        tracing::info!(kind = kind.name(), agent = %target_path, "spawning agent...");
    }

    /// Build the unified popup's identity rows: every live agent (from
    /// the registry mirror) plus every dormant identity on the loaded
    /// stored page, deduped by path and identity id. Shallow-path-first
    /// order; new-agent entries are the popup's own concern.
    pub(super) fn unified_agent_rows(&self) -> Vec<crate::widgets::agents_popup::AgentRow> {
        let active_name = self
            .state
            .sessions
            .get(self.state.active_agent_idx)
            .map(|s| s.name.clone());
        let record_for = |name: &str, id: uuid::Uuid| {
            self.state
                .stored_records
                .iter()
                .find(|record| record.name == name || record.id == id)
        };
        let mut rows: Vec<crate::widgets::agents_popup::AgentRow> = self
            .state
            .live_agents
            .iter()
            .map(|live| {
                let model_display = self.agent_model_cache.get(&live.name);
                crate::widgets::agents_popup::AgentRow {
                    name: live.name.clone(),
                    agent_id: live.agent_id,
                    status: live.status,
                    live: true,
                    is_active: active_name.as_deref() == Some(live.name.as_str()),
                    is_open: self.state.sessions.iter().any(|s| s.name == live.name),
                    model: model_display.map(|display| display.spec.clone()),
                    effort: model_display.and_then(|display| display.reasoning_effort.clone()),
                    config_json: record_for(&live.name, live.agent_id)
                        .map(|record| record.config_json.clone())
                        .unwrap_or_default(),
                }
            })
            .collect();
        // Dormant identities: stored rows with no live instance (by path
        // or identity id).
        for record in &self.state.stored_records {
            let live_by_name = self.state.live_agents.iter().any(|e| e.name == record.name);
            let live_by_id = self
                .state
                .live_agents
                .iter()
                .any(|e| e.agent_id == record.id);
            if live_by_name || live_by_id {
                continue;
            }
            if let Ok(path) = agentik_types::AgentPath::try_from(record.name.as_str()) {
                let preferred = record
                    .config_json
                    .get("preferred_model")
                    .and_then(|value| value.as_str())
                    .map(str::to_string);
                rows.push(crate::widgets::agents_popup::AgentRow {
                    name: path.as_str().to_string(),
                    agent_id: record.id,
                    status: agentik_types::AgentLifecycleStatus::Idle,
                    live: false,
                    is_active: active_name.as_deref() == Some(path.as_str()),
                    is_open: self.state.sessions.iter().any(|s| s.name == path.as_str()),
                    model: preferred,
                    effort: None,
                    config_json: record.config_json.clone(),
                });
            }
        }
        crate::widgets::agents_popup::sort_agent_rows(&mut rows);
        rows
    }

    /// Open a leaf (view window) for a live agent: pushes a session if
    /// none covers the path yet, requests the context list, and
    /// prefetches the model render cache. Returns the leaf index. Never
    /// steals focus by itself — the caller decides.
    pub(super) fn open_leaf_for(&mut self, name: &str, agent_id: uuid::Uuid) -> usize {
        if let Some(idx) = self.state.sessions.iter().position(|s| s.name == name) {
            return idx;
        }
        self.state.sessions.push(state::AgentSession {
            name: name.to_string(),
            agent_id,
            context: None,
            pending_tab_state: Default::default(),
        });
        self.state.opened_leaf = true;
        let idx = self.state.sessions.len() - 1;
        self.hydrate_leaf(name);
        idx
    }

    /// Hydrate a leaf from the shared context metadata endpoint and
    /// prefetch model info. Live `ContextList` events still refresh it. Used
    /// when a leaf opens and when a dormant leaf's identity re-activates.
    pub(super) fn hydrate_leaf(&mut self, name: &str) {
        let client = self.client.clone();
        let agent = name.to_string();
        let tx = self.app_event_tx.clone();
        self.spawn_client_task("list_contexts", move || async move {
            match client.list_contexts(&agent).await {
                Ok(response) => tx.send(crate::app_event::AppEvent::ContextListKnown {
                    agent,
                    context: response.context,
                }),
                Err(error) => tx.send(crate::app_event::AppEvent::ShowError {
                    title: "Failed to load agent context".into(),
                    detail: Some(error.to_string()),
                }),
            }
        });
        self.refresh_agent_model_info(name);
    }

    /// Switch the active leaf to the agent with this identity. Returns
    /// `false` when no open leaf matches (the agent was closed).
    pub(super) fn switch_active_agent(&mut self, agent_id: uuid::Uuid) -> bool {
        let Some(idx) = self
            .state
            .sessions
            .iter()
            .position(|s| s.agent_id == agent_id)
        else {
            return false;
        };
        self.state.active_agent_idx = idx;
        self.dirty = true;
        true
    }

    /// Fetch one bounded page of stored identities for the unified
    /// agents popup. The initial page is the newest identities;
    /// typing searches server-side via [`Self::schedule_stored_agent_search`].
    pub(super) fn fetch_stored_agents(&mut self, query: &str) {
        let client = self.client.clone();
        let tx = self.app_event_tx.clone();
        let query = query.to_string();
        self.spawn_client_task("search_stored_agents", move || async move {
            match client.search_storage_agents(&query, 100).await {
                Ok(records) => {
                    tracing::info!(count = records.len(), "stored agents page loaded");
                    tx.send(crate::app_event::AppEvent::AgentRecordsLoaded { query, records });
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to search stored agents");
                }
            }
        });
    }

    /// Debounced server-side identity search: 250ms after the last
    /// keystroke, fetch one bounded page for the popup's current query.
    /// The apply side drops pages whose query has already moved on.
    pub(super) fn schedule_stored_agent_search(&mut self) {
        let query = self.state.agents_popup.query().to_string();
        let tx = self.app_event_tx.clone();
        self.spawn_client_task("search_stored_agents_debounce", move || async move {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            tx.send(crate::app_event::AppEvent::SearchStoredAgents(query));
        });
    }

    /// Delete an agent record from storage and update the picker list.
    pub(super) fn delete_agent_record(&mut self, agent_id: uuid::Uuid) {
        tracing::info!(%agent_id, "deleting agent record");
        let client = self.client.clone();
        let tx = self.app_event_tx.clone();
        self.spawn_client_task("delete_agent_record", move || async move {
            match client.delete_agent_record(agent_id).await {
                Ok(()) => {
                    tracing::info!(%agent_id, "agent deleted from storage");
                    tx.send(crate::app_event::AppEvent::AgentDeleted(agent_id));
                }
                Err(e) => {
                    tracing::error!(%agent_id, error = %e, "failed to delete agent");
                }
            }
        });
    }

    /// Rename an agent record in storage: updates `agents.name` to the new
    /// full AgentPath. Fires `AgentRenamed` on success so the picker can
    /// refresh its in-memory item.
    pub(super) fn rename_agent_record(
        &mut self,
        agent_id: uuid::Uuid,
        new_path: agentik_types::AgentPath,
    ) {
        tracing::info!(%agent_id, new_path = %new_path, "renaming agent record");
        let client = self.client.clone();
        let new_name = new_path.as_str().to_string();
        let tx = self.app_event_tx.clone();
        self.spawn_client_task("rename_agent_record", move || async move {
            if let Err(e) = client.rename_agent_record(agent_id, &new_name).await {
                tracing::error!(%agent_id, error = %e, "failed to rename agent");
                return;
            }
            tracing::info!(%agent_id, new_path = %new_path, "agent renamed in storage");
            tx.send(crate::app_event::AppEvent::AgentRenamed { agent_id, new_path });
        });
    }
    /// Key handling while the delete-agent confirmation popup is open.
    ///
    /// y or Enter confirms; n or Esc cancels.
    pub(super) fn handle_delete_confirm_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                self.state.delete_agent_confirm = false;
                self.delete_active_agent();
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.state.delete_agent_confirm = false;
            }
            _ => {}
        }
    }

    /// Permanently delete the active agent: remove all stored data (sessions,
    /// snapshots, WAL messages, agent record) AND close the leaf.
    ///
    /// This is destructive — unlike `close_active_agent`, the conversation
    /// history cannot be resumed later.
    pub(super) fn delete_active_agent(&mut self) {
        let idx = self.state.active_agent_idx;
        let agent_id = self.state.sessions.get(idx).map(|s| s.agent_id);

        let Some(agent_id) = agent_id else {
            tracing::warn!("delete_active_agent: no active session");
            return;
        };

        // Delete all stored data for this agent (async, fire-and-forget).
        self.delete_agent_record(agent_id);

        // Close the leaf: shutdown the background task + remove UI state.
        self.close_active_agent();

        tracing::info!(%agent_id, "agent permanently deleted");
    }

    /// Close the active agent leaf: remove the UI session only.
    ///
    /// This is the "close tab" operation, and under virtual-actor mode it
    /// is a pure view change — the daemon owns the instance, which idles
    /// on until the idle TTL passivates it. Conversation history remains
    /// in storage and the resume picker re-opens it at any time.
    pub(super) fn close_active_agent(&mut self) {
        let idx = self.state.active_agent_idx;
        let Some(session) = self.state.sessions.get(idx) else {
            tracing::warn!(idx, "close_active_agent: no session at index");
            return;
        };
        let name = session.name.clone();
        tracing::info!(agent = %name, idx, "agent leaf closed — view detached, daemon instance untouched");

        // Remove UI session.
        self.state.sessions.remove(idx);

        // Adjust active index: clamp to the new last position.
        if self.state.sessions.is_empty() {
            self.state.active_agent_idx = 0;
        } else if self.state.active_agent_idx >= self.state.sessions.len() {
            self.state.active_agent_idx = self.state.sessions.len() - 1;
        }

        self.dirty = true;
    }

    /// Close a specific agent's leaf by `agent_id`, if one is currently open.
    ///
    /// Unlike `close_active_agent` which always targets `active_agent_idx`,
    /// this searches by agent identity — used when a deletion originates from
    /// the agent picker (where the deleted agent may not be the focused one).
    pub(super) fn close_agent_leaf_by_id(&mut self, agent_id: uuid::Uuid) {
        let Some(idx) = self
            .state
            .sessions
            .iter()
            .position(|s| s.agent_id == agent_id)
        else {
            return; // No open leaf for this agent — nothing to close.
        };

        let name = self.state.sessions[idx].name.clone();
        tracing::info!(agent = %name, %agent_id, "agent leaf closed by id — view detached, daemon instance untouched");

        // Remove UI session.
        self.state.sessions.remove(idx);

        // Adjust active index: clamp to the new last position.
        if self.state.sessions.is_empty() {
            self.state.active_agent_idx = 0;
        } else if self.state.active_agent_idx >= self.state.sessions.len() {
            self.state.active_agent_idx = self.state.sessions.len() - 1;
        } else if idx < self.state.active_agent_idx {
            // Removed a leaf before the active one — shift index down.
            self.state.active_agent_idx -= 1;
        }

        self.dirty = true;
    }
}

impl App {
    /// Cycle the visible agent leaf (Alt+Left/Right). Wraps around.
    /// View switching only — never touches agent lifecycle.
    pub(super) fn cycle_agent_tab(&mut self, direction: i32) {
        if self.state.sessions.is_empty() {
            return;
        }
        let count = self.state.sessions.len() as i32;
        let next = (self.state.active_agent_idx as i32 + direction).rem_euclid(count);
        self.state.active_agent_idx = next as usize;
        self.dirty = true;
    }
}
