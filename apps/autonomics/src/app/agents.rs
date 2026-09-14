//! Agent creation, restoration, deletion, and lifecycle management.

use super::*;

impl App {
    /// Spawn a new agent from a profile asynchronously.
    ///
    /// Because the event loop is already inside `runtime.block_on(...)`, we
    /// can't call `block_on` again. Instead, we spawn the creation as a
    /// background task and send the result back via the app event channel.
    /// The `AgentSpawned` event is handled in `handle_app_event`.
    pub(super) fn spawn_agent_from_profile(&mut self, profile: &AgentProfile, agent_name: &str) {
        let target_path = agentik_types::AgentPath::root()
            .join(agent_name)
            .unwrap_or_else(|_| agentik_types::AgentPath::root());
        self.spawn_agent_at_path(profile, target_path);
    }

    /// Spawn an agent at its persisted hierarchical path. Tool-spawned
    /// children live below `/root/parent/child`; restoring them from only the
    /// leaf name would create a different runtime identity.
    pub(super) fn spawn_agent_at_path(
        &mut self,
        profile: &AgentProfile,
        target_path: agentik_types::AgentPath,
    ) {
        let agent_name = target_path.name();
        tracing::info!(
            profile = %profile.path,
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
        let profile_clone = profile.clone();
        let agent_name_owned = agent_name.to_string();
        let parent_path = target_path
            .parent()
            .unwrap_or_else(agentik_types::AgentPath::root);
        let model_spec = profile.preferred_model.clone();
        let profile_name_owned = profile.path.clone();
        let tx = self.app_event_tx.clone();

        tracing::debug!(has_override = model_spec.is_some(), "model spec passed");
        self.spawn_client_task(
            &format!("spawn_agent::{agent_name_owned}"),
            move || async move {
                tracing::debug!(
                    profile = %profile_clone.path,
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
                        &profile_clone,
                        model_spec.as_deref(),
                    )
                    .await
                    .map_err(|e| e.to_string());
                let event = match result {
                    Ok(name) => {
                        tracing::info!(profile = %profile_name_owned, agent = %name, "agent spawned and registered with daemon");
                        crate::app_event::AppEvent::AgentSpawned {
                            profile_name: profile_name_owned,
                            result: Ok(name),
                        }
                    }
                    Err(e) => {
                        tracing::error!(profile = %profile_name_owned, error = %e, "agent spawn failed");
                        crate::app_event::AppEvent::AgentSpawned {
                            profile_name: profile_name_owned,
                            result: Err(e),
                        }
                    }
                };
                tx.send(event);
            },
        );

        tracing::info!(profile = %profile.path, agent = %target_path, "spawning agent...");
    }

    /// Query stored agents and open the resume picker.
    pub(super) fn open_agent_picker(&mut self) {
        tracing::info!("open_agent_picker called");
        let client = self.client.clone();
        let tx = self.app_event_tx.clone();
        self.spawn_client_task("list_agents", move || async move {
            tracing::debug!("querying list_agents from the daemon's storage");
            match client.list_storage_agents().await {
                Ok(records) => {
                    tracing::info!(count = records.len(), "list_agents succeeded");
                    tx.send(crate::app_event::AppEvent::AgentRecordsLoaded(records));
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to list agents");
                }
            }
        });
    }

    /// Key handling while the agent resume picker popup is open.
    pub(super) fn handle_agent_picker_key(&mut self, key: &KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // If rename mode is active, route keys to the rename input.
        if self.state.agent_picker.rename_id.is_some() {
            match key.code {
                KeyCode::Esc => self.state.agent_picker.cancel_rename(),
                KeyCode::Backspace => {
                    self.state.agent_picker.rename_input.pop();
                }
                KeyCode::Char(c) if !ctrl => {
                    self.state.agent_picker.rename_input.push(c);
                }
                KeyCode::Enter => {
                    if let Some((agent_id, new_path)) = self.state.agent_picker.check_rename() {
                        self.rename_agent_record(agent_id, new_path);
                    }
                }
                _ => {}
            }
            return;
        }

        // If delete confirmation is active, route keys differently.
        if self.state.agent_picker.delete_confirm_id.is_some() {
            match key.code {
                KeyCode::Esc => self.state.agent_picker.cancel_delete(),
                KeyCode::Backspace => {
                    self.state.agent_picker.delete_confirm_input.pop();
                }
                KeyCode::Char(c) if !ctrl => {
                    self.state.agent_picker.delete_confirm_input.push(c);
                }
                KeyCode::Enter => {
                    // Only commit when the typed confirmation matches
                    // "yes" (case-insensitive). Partial / empty / typo
                    // input is silently ignored so the user can keep
                    // typing without losing context. Esc cancels.
                    if let Some(agent_id) = self.state.agent_picker.check_delete_confirm() {
                        self.delete_agent_record(agent_id);
                    }
                }
                _ => {}
            }
            return;
        }

        // Normal mode.
        match key.code {
            KeyCode::Esc => {
                self.state.agent_picker.close();
            }
            KeyCode::Up => self.state.agent_picker.move_up(),
            KeyCode::Down => self.state.agent_picker.move_down(),
            KeyCode::Left | KeyCode::Tab => self.state.agent_picker.toggle_expand(),
            KeyCode::Right => self.state.agent_picker.toggle_expand(),
            KeyCode::Backspace => self.state.agent_picker.pop_char(),
            KeyCode::Char('d') if ctrl => {
                self.state.agent_picker.start_delete_confirm();
            }
            KeyCode::Char('r') if ctrl => {
                self.state.agent_picker.start_rename();
            }
            KeyCode::Char(c) if !ctrl => self.state.agent_picker.push_char(c),
            KeyCode::Enter => {
                // If cursor is on a folder, toggle expand/collapse.
                let on_leaf = self.state.agent_picker.selected_item().is_some();
                if !on_leaf {
                    self.state.agent_picker.toggle_expand();
                    return;
                }
                if let Some(item) = self.state.agent_picker.selected_item() {
                    // If this agent is already open in a leaf, just switch focus
                    // instead of spawning a duplicate.
                    if let Some(idx) = self
                        .state
                        .sessions
                        .iter()
                        .position(|s| s.agent_id == item.id)
                    {
                        tracing::info!(
                            agent_id = %item.id,
                            leaf_idx = idx,
                            "agent already open — switching focus instead of restoring"
                        );
                        self.state.active_agent_idx = idx;
                        self.state.agent_picker.close();
                        return;
                    }

                    let config_json = item.config_json.clone();
                    // `item.path` is the full persisted AgentPath. Preserve its
                    // hierarchy so child agents restore their original runtime ID.
                    self.state.agent_picker.close();
                    let short_name = item.path.name().to_string();
                    let profile_path = item.path.as_str().trim_start_matches("/root/");
                    // Try to reconstruct the profile from the stored config_json.
                    // Fall back to looking up by short name in the current profiles.
                    let profile = serde_json::from_value::<AgentProfile>(config_json)
                        .ok()
                        .or_else(|| {
                            self.state
                                .profiles
                                .iter()
                                .find(|p| p.path == profile_path || p.path == short_name)
                                .cloned()
                        });
                    match profile {
                        Some(p) => self.spawn_agent_at_path(&p, item.path),
                        None => tracing::warn!(
                            agent = %item.path,
                            "could not reconstruct profile for agent record",
                        ),
                    }
                }
            }
            _ => {}
        }
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
    /// Key handling while the profile picker popup is open.
    pub(super) fn handle_profile_picker_key(&mut self, key: &KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.state.profile_picker.close();
            }
            KeyCode::Up => self.state.profile_picker.move_up(),
            KeyCode::Down => self.state.profile_picker.move_down(),
            // Expand/collapse folder nodes. Tab and Right both expand;
            // Left collapses.
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('+') => {
                self.state.profile_picker.toggle_expand();
            }
            KeyCode::Left | KeyCode::BackTab => {
                self.state.profile_picker.toggle_expand();
            }
            KeyCode::Backspace => self.state.profile_picker.pop_char(),
            KeyCode::Char(c) if !ctrl => self.state.profile_picker.push_char(c),
            KeyCode::Enter => {
                if let Some(item) = self.state.profile_picker.selected_item() {
                    let profile = item.profile;
                    self.state.profile_picker.close();
                    // Stash the selected profile; the name input popup
                    // will open next and prompt the user for an agent
                    // name (pre-filled with the profile name).
                    self.state.pending_profile = Some(profile.clone());
                    self.state
                        .name_input
                        .open(format!(" New Agent ({}) ", profile.name()), profile.name());
                }
            }
            _ => {}
        }
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

    /// Close the active agent leaf: terminate the background agent process
    /// and remove the corresponding handle + UI session state.
    ///
    /// This is the "close tab" operation. The agent's conversation history
    /// remains in storage and can be resumed later via the resume picker.
    pub(super) fn close_active_agent(&mut self) {
        let idx = self.state.active_agent_idx;
        let Some(session) = self.state.sessions.get(idx) else {
            tracing::warn!(idx, "close_active_agent: no session at index");
            return;
        };
        let name = session.name.clone();

        // Shutdown the agent via the daemon (relay task handles cleanup).
        self.shutdown_agent_remotely(&name);
        tracing::info!(agent = %name, idx, "agent leaf closed — daemon agent shut down");

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

        // Shutdown the agent via the daemon (relay task handles cleanup).
        self.shutdown_agent_remotely(&name);
        tracing::info!(agent = %name, %agent_id, "agent leaf closed by id — daemon agent shut down");

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
    /// Fire-and-forget agent shutdown on the daemon (leaf close).
    pub(super) fn shutdown_agent_remotely(&self, name: &str) {
        let client = self.client.clone();
        let name = name.to_string();
        self.spawn_client_task("shutdown_agent", move || async move {
            if let Err(e) = client.shutdown_agent(&name).await {
                tracing::warn!(agent = %name, error = %e, "daemon agent shutdown request failed");
            }
        });
    }
}
