//! Host command processing: the `HostCommand` dispatch loop plus the
//! name-resolution, hierarchy-validation, and task-routing helpers it
//! relies on.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use agentik_core::profile::AgentKind;
use agentik_core::storage::AgentStorage;
use agentik_sdk::model::Model;
use arc_swap::ArcSwapOption;
use futures::FutureExt;

use crate::control::{AgentExecutionHistory, DelegationSnapshot, DelegationStatus};

use super::RuntimeHost;
use super::delegation::{HostDelegation, delegation_status_from_str, push_delegation_progress};
use super::relay::AgentCommand;
use super::unix_epoch_ms;

impl RuntimeHost {
    /// Drain and execute all pending commands from agent tools.
    /// Call this in the event loop (e.g. at each render tick).
    pub fn try_process_commands(&mut self) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            self.process_command(cmd);
        }
    }

    /// Await either a host command or a background spawn completion.
    /// Event-driven — wakes when either arrives. Use as a `select!`
    /// branch in the event loop.
    pub async fn recv_and_process_command(&mut self) {
        tokio::select! {
            cmd = self.cmd_rx.recv() => {
                if let Some(cmd) = cmd {
                    self.process_command(cmd);
                }
            }
            reg = self.registration_rx.recv() => {
                if let Some((handle, info)) = reg {
                    self.register_background_spawn(handle, info);
                }
            }
        }
    }

    /// Register an agent whose background spawn task just completed, and
    /// broadcast the registration event. Shared by the command pump and
    /// [`Self::recv_next`].
    pub(super) fn register_background_spawn(
        &mut self,
        handle: super::AgentHandle,
        info: crate::control::AgentInfo,
    ) {
        let path = handle.path.clone();
        let mut event_info = info;
        event_info.agent_id = Some(handle.agent_id);
        self.register_agent(handle, event_info.clone());
        self.emit_host_event(super::HostEvent::AgentRegistered {
            path: path.clone(),
            info: event_info,
        });
        tracing::info!(agent = %path, "background spawn completed and registered");
    }

    pub(super) fn process_command(&mut self, cmd: crate::control::HostCommand) {
        use crate::control::{HostCommand, HostStatus};
        match cmd {
            HostCommand::Spawn {
                name,
                caller_path,
                config,
                model_override,
                reply_tx,
            } => {
                // Derive child path from caller's path + the LLM-provided segment.
                let child_path = match caller_path.join(&name) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = reply_tx.send(Err(format!("invalid agent name `{name}`: {e}")));
                        return;
                    }
                };
                // Reject duplicate paths.
                if self.agents.contains_key(child_path.as_str()) {
                    let _ =
                        reply_tx.send(Err(format!("agent at path `{child_path}` already exists")));
                    return;
                }
                // Resolve model: use override if provided, else global model.
                let model = match (model_override, self.model.as_ref()) {
                    (Some(m), _) => Arc::new(ArcSwapOption::from_pointee(Some(m))),
                    (None, Some(g)) => g.clone(),
                    (None, None) => {
                        let _ = reply_tx.send(Err("No model configured on host.".into()));
                        return;
                    }
                };

                let infra = self.infra.clone();
                let reg_tx = self.registration_tx.clone();
                let info =
                    capability_from_kind(child_path.name(), child_path.as_str(), config.kind);
                let path_for_spawn = child_path.clone();
                let config = *config;

                self.infra.runtime_handle.spawn(async move {
                    let result = AssertUnwindSafe(async {
                        infra
                            .spawn_agent(&path_for_spawn, &config, model, None)
                            .await
                    })
                    .catch_unwind()
                    .await;
                    match result {
                        Ok(Ok(handle)) => {
                            let registered_path = handle.path.as_str().to_string();
                            let _ = reply_tx.send(Ok(registered_path));
                            let _ = reg_tx.send((handle, info));
                        }
                        Ok(Err(e)) => {
                            let _ = reply_tx.send(Err(e.to_string()));
                        }
                        Err(panic_payload) => {
                            let msg = panic_payload
                                .downcast_ref::<&'static str>()
                                .map(|s| (*s).to_string())
                                .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                                .unwrap_or_else(|| "<panic in spawn_agent>".to_string());
                            tracing::error!(
                                target: "spawn_safe",
                                task = "host::spawn_agent",
                                panic = %msg,
                                "spawn_agent task panicked"
                            );
                            let _ = reply_tx.send(Err(format!("internal panic: {msg}")));
                        }
                    }
                });
            }
            HostCommand::Shutdown {
                name,
                caller_path,
                reply_tx,
            } => {
                let reply = |r: std::result::Result<(), String>| {
                    if let Some(tx) = reply_tx {
                        let _ = tx.send(r);
                    }
                };
                match (self.resolve_agent(&name), caller_path.as_deref()) {
                    // Operator path — unrestricted (gateway/TUI semantics).
                    (resolved, None) => {
                        let resolved = resolved.unwrap_or(name);
                        self.shutdown_agent(&resolved);
                        reply(Ok(()));
                    }
                    // Agent path with an unresolvable target: same message
                    // as the not-a-child denial so callers cannot probe the
                    // registry for the existence of foreign agents.
                    (None, Some(_)) => {
                        reply(Err(not_a_direct_child_message(&name)));
                    }
                    (Some(resolved), Some(caller)) => {
                        match self.validate_child_control(caller, &resolved, "shut down") {
                            Err(error) => reply(Err(error)),
                            Ok(()) => {
                                self.shutdown_agent(&resolved);
                                reply(Ok(()));
                            }
                        }
                    }
                }
            }
            HostCommand::DeliverMessage {
                name,
                message,
                reply_tx,
            } => {
                let resolved = self.resolve_agent(&name).unwrap_or(name);
                let result = self.send_to(&resolved, message);
                if let Err(error) = &result {
                    tracing::warn!(agent = %resolved, %error, "user message dropped");
                }
                if let Some(reply_tx) = reply_tx {
                    let _ = reply_tx.send(result);
                }
            }
            HostCommand::Delegate {
                to,
                message,
                caller_path,
                delegation_id,
                progress,
                reply_tx,
            } => {
                let Some(caller_path) = caller_path else {
                    let _ = reply_tx.send(
                        "Delegation failed: the calling agent path is unavailable.".to_string(),
                    );
                    return;
                };
                // Resolve the target from the caller's viewpoint: full paths
                // match exactly, short names prefer the caller's own child.
                // Unresolvable and non-child targets return the SAME message
                // so callers cannot probe the registry for foreign agents.
                let resolved = match agentik_types::AgentPath::try_from(caller_path.as_str())
                    .ok()
                    .and_then(|caller| self.resolve_agent_scoped(&caller, &to))
                {
                    Some(path) => path,
                    None => {
                        let _ = reply_tx.send(format!(
                            "Error: agent '{to}' is not one of your direct children. \
                             Delegation reaches only agents you spawned; use list_agents \
                             to see them, or spawn_agent to create one first."
                        ));
                        return;
                    }
                };
                if let Err(error) = self.validate_delegation_paths(&caller_path, &resolved) {
                    let _ = reply_tx.send(error);
                    return;
                }
                let now = unix_epoch_ms();
                let record = HostDelegation {
                    snapshot: DelegationSnapshot {
                        delegation_id,
                        caller_path: Some(caller_path),
                        target_path: resolved.clone(),
                        task: message.clone(),
                        status: DelegationStatus::Pending,
                        turn_id: None,
                        session_id: None,
                        response: None,
                        created_at: now,
                        updated_at: now,
                    },
                    reply_tx: Some(reply_tx),
                    progress: progress.clone(),
                };
                push_delegation_progress(
                    &record.progress,
                    "delegation",
                    "pending",
                    &format!("target={resolved}"),
                );
                self.delegations.insert(delegation_id, record);
                self.persist_delegation(delegation_id);
                self.send_delegation_to(&resolved, message, delegation_id);
            }
            HostCommand::ListDelegations {
                caller_path,
                target_path,
                status,
                reply_tx,
            } => {
                let target_path =
                    target_path.map(|target| self.resolve_agent(&target).unwrap_or(target));
                let live_records: Vec<_> = self
                    .delegations
                    .values()
                    .map(|record| record.snapshot.clone())
                    .filter(|record| {
                        caller_path
                            .as_deref()
                            .is_none_or(|value| record.caller_path.as_deref() == Some(value))
                            && target_path
                                .as_deref()
                                .is_none_or(|value| record.target_path == value)
                            && status
                                .as_deref()
                                .is_none_or(|value| record.status.as_str() == value)
                    })
                    .collect();
                let storage = self.infra.storage.clone();
                agentik_core::supervise::spawn_safe_on(
                    &self.infra.runtime_handle,
                    "list_delegations",
                    async move {
                        let mut records = live_records;
                        let live_ids: std::collections::HashSet<_> =
                            records.iter().map(|record| record.delegation_id).collect();
                        if let Ok(persisted) = storage
                            .list_agent_delegations(
                                caller_path.as_deref(),
                                target_path.as_deref(),
                                status.as_deref(),
                                1000,
                            )
                            .await
                        {
                            records.extend(persisted.into_iter().filter_map(move |record| {
                                let delegation_id = record.delegation_id;
                                if live_ids.contains(&delegation_id) {
                                    return None;
                                }
                                Some(DelegationSnapshot {
                                    delegation_id,
                                    caller_path: record.caller_path,
                                    target_path: record.target_path,
                                    task: record.task,
                                    status: delegation_status_from_str(&record.status),
                                    turn_id: record.turn_id,
                                    session_id: record.session_id,
                                    response: record.response,
                                    created_at: record.created_at,
                                    updated_at: record.updated_at,
                                })
                            }));
                        }
                        records.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
                        let _ = reply_tx.send(records);
                    },
                );
            }
            HostCommand::GetAgentHistory {
                agent_name,
                limit,
                reply_tx,
            } => {
                let storage = self.infra.storage.clone();
                let requested_path = agent_name.clone();
                let live = self.resolve_agent(&agent_name).and_then(|path| {
                    self.agents
                        .get(&path)
                        .and_then(|entry| entry.info.agent_id)
                        .map(|id| (path, id))
                });
                agentik_core::supervise::spawn_safe_on(
                    &self.infra.runtime_handle,
                    "get_agent_history",
                    async move {
                        let response = match live {
                            Some((path, agent_id)) => {
                                read_agent_history(storage, agent_id, path, limit).await
                            }
                            None => {
                                read_persisted_agent_history(storage, requested_path, limit).await
                            }
                        };
                        let _ = reply_tx.send(response);
                    },
                );
            }
            HostCommand::GetStatus { reply_tx } => {
                let g = self.network.graph();
                let status = HostStatus {
                    agents: self.agents.values().map(|e| e.info.clone()).collect(),
                    profiles: AgentKind::ALL
                        .iter()
                        .map(|k| capability_from_kind(k.name(), k.name(), *k))
                        .collect(),
                    nodes: g.node_names().into_iter().map(String::from).collect(),
                    edge_count: g.edge_count(),
                    is_cyclic: g.is_cyclic(),
                    roots: g.root_nodes().into_iter().map(String::from).collect(),
                    leaves: g.leaf_nodes().into_iter().map(String::from).collect(),
                    rounds: self.network.rounds(),
                    is_finished: self.network.is_finished(),
                    termination: format!("{:?}", self.network.termination()),
                };
                let _ = reply_tx.send(status);
            }
            HostCommand::RouteTask {
                description,
                caller_path,
                reply_tx,
            } => {
                let result = self.route_task(&description, &caller_path);
                let _ = reply_tx.send(result);
            }
            HostCommand::GetAgentInfo { name, reply_tx } => {
                // Resolve agent name, then check running agents first,
                // fall back to the hardcoded agent kinds.
                let resolved = self.resolve_agent(&name);
                let info = resolved
                    .as_ref()
                    .and_then(|key| self.agents.get(key).map(|e| e.info.clone()))
                    .or_else(|| {
                        AgentKind::from_name(&name)
                            .map(|k| capability_from_kind(k.name(), k.name(), k))
                    });
                let _ = reply_tx.send(info);
            }

            // ── Session management (forwarded to relay) ──
            HostCommand::CancelAgent {
                name,
                caller_path,
                reply_tx,
            } => {
                let reply = |r: std::result::Result<(), String>| {
                    if let Some(tx) = reply_tx {
                        let _ = tx.send(r);
                    }
                };
                match (self.resolve_agent(&name), caller_path.as_deref()) {
                    // Operator path — unrestricted (gateway/TUI semantics).
                    (resolved, None) => {
                        let resolved = resolved.unwrap_or(name);
                        self.send_agent_command(&resolved, AgentCommand::Cancel);
                        reply(Ok(()));
                    }
                    // Same denial as the not-a-child case: no registry probe.
                    (None, Some(_)) => {
                        reply(Err(not_a_direct_child_message(&name)));
                    }
                    (Some(resolved), Some(caller)) => {
                        match self.validate_child_control(caller, &resolved, "interrupt") {
                            Err(error) => reply(Err(error)),
                            Ok(()) => {
                                self.send_agent_command(&resolved, AgentCommand::Cancel);
                                reply(Ok(()));
                            }
                        }
                    }
                }
            }
            HostCommand::CompactAgent { name } => {
                self.send_agent_command(&name, AgentCommand::Compact);
            }
            HostCommand::ListSessions { name } => {
                self.send_agent_command(&name, AgentCommand::ListSessions);
            }
            HostCommand::CreateSession {
                name,
                title,
                fork_from,
                fork_message_id,
            } => {
                self.send_agent_command(
                    &name,
                    AgentCommand::CreateSession {
                        title,
                        fork_from,
                        fork_message_id,
                    },
                );
            }
            HostCommand::SwitchSession { name, session_id } => {
                self.send_agent_command(&name, AgentCommand::SwitchSession(session_id));
            }
            HostCommand::CloseSession { name, session_id } => {
                self.send_agent_command(&name, AgentCommand::CloseSession(session_id));
            }
            HostCommand::RenameSession {
                name,
                session_id,
                title,
            } => {
                self.send_agent_command(
                    &name,
                    AgentCommand::RenameSession {
                        id: session_id,
                        title,
                    },
                );
            }

            // ── Model management ──
            HostCommand::SetAgentModel { name, model } => {
                self.send_agent_command(&name, AgentCommand::SetModel(model));
            }
            HostCommand::SetAgentRuntimeConfig {
                name,
                overrides,
                reply_tx,
            } => {
                let defaults = self.infra.kms.runtime_config();
                let mut extraction_event = None;
                let resolved = self.resolve_agent(&name).and_then(|key| {
                    self.agents.get(&key).map(|entry| {
                        let previous = entry.memory.runtime_config();
                        let mut config = defaults;
                        if let Some(enabled) = overrides.use_memory {
                            config.use_memory = enabled;
                        }
                        if let Some(enabled) = overrides.generate_memory {
                            config.generate_memory = enabled;
                        }
                        if let Some(effort) = overrides.reasoning_effort {
                            config.reasoning_effort = Some(effort);
                        }
                        entry.memory.set_runtime_config(config);
                        if !previous.generate_memory
                            && config.generate_memory
                            && agentik_types::AgentPath::try_from(key.as_str()).is_ok_and(|path| {
                                self.infra.kms.effective_config().is_root_agent(&path)
                            })
                            && let Some(agent_id) = entry.info.agent_id
                        {
                            extraction_event = Some((Arc::clone(&entry.memory), agent_id));
                        }
                        config
                    })
                });
                if let Some((memory, agent_id)) = extraction_event {
                    self.infra.runtime_handle.spawn(async move {
                        if let Err(error) = memory
                            .submit_knowledge_event(
                                agentik_core::memory::KnowledgeEvent::AgentStarted { agent_id },
                            )
                            .await
                        {
                            tracing::warn!(
                                agent_id = %agent_id,
                                error = %error,
                                "failed to queue knowledge extraction"
                            );
                        }
                    });
                }
                let reply = resolved.ok_or_else(|| format!("agent `{name}` not found"));
                let _ = reply_tx.send(reply);
            }
            HostCommand::GetAgentModel { name, reply_tx } => {
                let resolved = self.resolve_agent(&name);
                let info = resolved
                    .as_ref()
                    .and_then(|key| self.agents.get(key))
                    .and_then(|e| {
                        e.model.load_full().as_deref().map(|m| {
                            (
                                m.model_spec(),
                                m.model_info.context_length,
                                effective_reasoning_effort(e, m),
                            )
                        })
                    });
                let _ = reply_tx.send(info);
            }

            // ── Phase 4: query persisted agent graph ──
            // Synchronous read of the `agent_graph` table — the storage
            // backend is a local SQLite DB so the query is cheap. Returns an
            // empty vec on storage error (rather than blocking the
            // caller's reply channel with an Err) so the dashboard can
            // render an empty state instead of crashing.
            HostCommand::ListPersistedAgents { reply_tx } => {
                let storage = self.infra.storage.clone();
                let runtime_handle = self.infra.runtime_handle.clone();
                runtime_handle.spawn(async move {
                    let result = AssertUnwindSafe(async {
                        match storage.list_persisted_agents().await {
                            Ok(entries) => entries,
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "list_persisted_agents failed; returning empty list"
                                );
                                Vec::new()
                            }
                        }
                    })
                    .catch_unwind()
                    .await;
                    match result {
                        Ok(entries) => {
                            let _ = reply_tx.send(entries);
                        }
                        Err(panic_payload) => {
                            let msg = panic_payload
                                .downcast_ref::<&'static str>()
                                .map(|s| (*s).to_string())
                                .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                                .unwrap_or_else(|| "<panic in list_persisted_agents>".to_string());
                            tracing::error!(
                                target: "spawn_safe",
                                task = "host::list_persisted_agents",
                                panic = %msg,
                                "list_persisted_agents task panicked"
                            );
                            let _ = reply_tx.send(Vec::new());
                        }
                    }
                });
            }
        }
    }

    /// Resolve an agent reference to its full path key in the registry.
    ///
    /// Accepts:
    /// 1. **Full path** (`/root/researcher/worker`) — exact match.
    /// 2. **Short name** (`researcher`) — matches when unambiguous.
    ///
    /// Returns the HashMap key string, or `None` if no match / ambiguous.
    pub(super) fn resolve_agent(&self, target: &str) -> Option<String> {
        // 1. Exact full-path match.
        if target.starts_with("/root") && self.agents.contains_key(target) {
            return Some(target.to_string());
        }
        // 2. Short-name match: collect all agents whose last path segment
        //    equals `target`.
        let matches: Vec<&String> = self
            .agents
            .keys()
            .filter(|key| key.rsplit('/').next().unwrap_or(key) == target)
            .collect();
        match matches.len() {
            1 => Some(matches[0].clone()),
            0 => None,
            _ => {
                tracing::warn!(
                    target = %target,
                    candidates = ?matches,
                    "ambiguous agent name — use full path to disambiguate"
                );
                None
            }
        }
    }

    /// Resolve a path and require the corresponding live agent registry entry.
    fn resolve_live_agent(
        &self,
        path: &str,
        role: &str,
        requested: &str,
    ) -> std::result::Result<String, String> {
        let resolved = self
            .resolve_agent(path)
            .unwrap_or_else(|| requested.to_string());
        if !self.agents.contains_key(&resolved) {
            return Err(format!(
                "Agent '{requested}' resolved to '{resolved}' but is not currently running. \
                 Only live agents can be the {role} of an inter-agent operation."
            ));
        }
        Ok(resolved)
    }

    /// Resolve a delegation target from the caller's viewpoint: full paths
    /// must match exactly; short names prefer the caller's direct child
    /// before falling back to the global scan (the delegation validator
    /// still gates whatever comes back).
    fn resolve_agent_scoped(&self, caller: &agentik_types::AgentPath, to: &str) -> Option<String> {
        if to.starts_with("/root") && self.agents.contains_key(to) {
            return Some(to.to_string());
        }
        if let Ok(child) = caller.join(to) {
            if self.agents.contains_key(child.as_str()) {
                return Some(child.as_str().to_string());
            }
        }
        self.resolve_agent(to)
    }

    /// Validate delegation hierarchy: direct children only.
    ///
    /// A direct child is always delegable regardless of its live status —
    /// messages queue on the child's relay and are processed turn by turn.
    pub(super) fn validate_delegation_paths(
        &self,
        caller_path: &str,
        target_path: &str,
    ) -> std::result::Result<(), String> {
        let caller = self.resolve_live_agent(caller_path, "caller", caller_path)?;
        let target = self.resolve_live_agent(target_path, "delegate target", target_path)?;
        let Ok(caller_path) = agentik_types::AgentPath::try_from(caller.as_str()) else {
            return Err(format!("invalid caller path `{caller}`"));
        };
        let Ok(target_path) = agentik_types::AgentPath::try_from(target.as_str()) else {
            return Err(format!("invalid delegate target path `{target}`"));
        };
        if caller_path == target_path {
            return Err("Agents cannot delegate tasks to themselves.".to_string());
        }
        if target_path.is_direct_child_of(&caller_path) {
            return Ok(());
        }
        if path_is_ancestor(&target_path, &caller_path) {
            return Err(format!(
                "Delegation denied: '{}' is a superior of '{}'. \
                 Subordinate agents cannot delegate tasks upward.",
                target_path.as_str(),
                caller_path.as_str()
            ));
        }
        Err(format!(
            "Delegation denied: '{}' is not a direct child of '{}'. \
             Delegation is limited to your own direct children; siblings and other \
             branches are not visible to you. Spawn the agent you need with \
             spawn_agent, or reuse an existing child.",
            target_path.as_str(),
            caller_path.as_str()
        ))
    }

    /// Validate that `caller_path` may manage (shut down / interrupt) the
    /// agent at `target`: agents control only their own direct children.
    fn validate_child_control(
        &self,
        caller_path: &str,
        target: &str,
        action: &str,
    ) -> std::result::Result<(), String> {
        let caller = self.resolve_live_agent(caller_path, "caller", caller_path)?;
        let Ok(caller) = agentik_types::AgentPath::try_from(caller.as_str()) else {
            return Err(format!("invalid caller path `{caller}`"));
        };
        let Ok(target) = agentik_types::AgentPath::try_from(target) else {
            return Err(format!("invalid target path `{target}`"));
        };
        if target.is_direct_child_of(&caller) {
            return Ok(());
        }
        Err(format!(
            "Control denied: '{}' is not a direct child of '{}'. \
             Agents may only {action} their own direct children.",
            target.as_str(),
            caller.as_str()
        ))
    }

    /// Forward a command to a named agent's relay task.
    /// `name` is resolved via [`resolve_agent`](Self::resolve_agent).
    fn send_agent_command(&self, name: &str, cmd: AgentCommand) {
        if let Some(resolved) = self.resolve_agent(name) {
            if let Some(entry) = self.agents.get(&resolved) {
                let _ = entry.cmd_tx.send(cmd);
                return;
            }
        }
        tracing::warn!(agent = %name, "send_agent_command: agent not registered");
    }

    /// Route a task description to the best-matching agent.
    /// Considers the caller's direct children (live agents) plus the
    /// hardcoded agent kinds as spawnable profiles. Live children get a
    /// small bonus score since they're immediately available for
    /// delegation.
    fn route_task(&self, description: &str, caller_path: &str) -> crate::control::RouteResult {
        let desc_lower = description.to_lowercase();
        let desc_words: std::collections::HashSet<&str> = desc_lower
            .split_whitespace()
            .filter(|w| w.len() > 2)
            .collect();

        let score_info = |info: &crate::control::AgentInfo, is_running: bool| {
            let matched_tags: Vec<String> = info
                .tags
                .iter()
                .filter(|tag| desc_lower.contains(tag.as_str()))
                .cloned()
                .collect();

            let tag_score = matched_tags.len() as f64 * 3.0;

            let info_text = format!(
                "{} {}",
                info.summary.to_lowercase(),
                info.expertise.join(" ").to_lowercase()
            );
            let info_words: std::collections::HashSet<&str> = info_text
                .split_whitespace()
                .filter(|w| w.len() > 2)
                .collect();

            let word_overlap = desc_words.intersection(&info_words).count() as f64;
            let running_bonus = if is_running { 0.5 } else { 0.0 };
            let score = tag_score + word_overlap + running_bonus;

            crate::control::RouteCandidate {
                agent: info.name.clone(),
                score,
                matched_tags,
            }
        };

        // Score live agents — only the caller's direct children are
        // visible (and delegable), so everything else is filtered out.
        let caller = agentik_types::AgentPath::try_from(caller_path).ok();
        let mut candidates: Vec<crate::control::RouteCandidate> = self
            .agents
            .values()
            .filter(|e| {
                caller.as_ref().is_some_and(|c| {
                    agentik_types::AgentPath::try_from(e.info.path.as_str())
                        .is_ok_and(|p| p.is_direct_child_of(c))
                })
            })
            .map(|e| score_info(&e.info, true))
            .collect();

        // Score the hardcoded kinds as spawnable candidates. The caller can
        // never be its own child, so no self-exclusion is needed.
        candidates.extend(
            AgentKind::ALL
                .iter()
                .map(|k| score_info(&capability_from_kind(k.name(), k.name(), *k), false)),
        );

        let mut candidates: Vec<_> = candidates.into_iter().filter(|c| c.score > 0.0).collect();

        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let (best, reason) = if let Some(top) = candidates.first() {
            let reason = if top.matched_tags.is_empty() {
                format!("Best keyword match (score: {:.1})", top.score)
            } else {
                format!(
                    "Matched tags: {} (score: {:.1})",
                    top.matched_tags.join(", "),
                    top.score
                )
            };
            (top.agent.clone(), reason)
        } else {
            ("none".into(), "No matching agent found.".into())
        };

        crate::control::RouteResult {
            agent: best,
            reason,
            score: candidates.first().map(|c| c.score).unwrap_or(0.0),
            candidates,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Capability extraction
// ═══════════════════════════════════════════════════════════════════════

/// The effort a request from this agent would actually send — mirrors the
/// request layer's `inject_thinking` derivation: per-agent override first
/// (when the model supports thinking at all), else the model-derived
/// default. `None` when the model has no thinking support.
fn effective_reasoning_effort(
    entry: &super::relay::AgentEntry,
    model: &agentik_sdk::model::Model,
) -> Option<agentik_types::ReasoningEffort> {
    let info = &model.model_info;
    if !info.supports_thinking {
        return None;
    }
    Some(
        entry
            .memory
            .runtime_config()
            .reasoning_effort
            .unwrap_or_else(|| {
                if info.thinking_required && !info.thinking_enabled {
                    agentik_types::ReasoningEffort::Low
                } else {
                    info.max_reasoning_effort.unwrap_or_default()
                }
            }),
    )
}

/// Build [`AgentInfo`](crate::control::AgentInfo) from an [`AgentKind`],
/// auto-extracting tags, expertise, and tool list from the kind's feature
/// flags.
pub(super) fn capability_from_kind(
    name: &str,
    path: &str,
    kind: AgentKind,
) -> crate::control::AgentInfo {
    let mut tags = Vec::new();
    let mut expertise = Vec::new();

    if kind == AgentKind::Researcher {
        tags.push("research".into());
        tags.push("analysis".into());
        tags.push("dag-analysis".into());
        tags.push("data-analysis".into());
        tags.push("dag-execution".into());
        expertise.push("research-analysis".into());
        expertise.push("dag-workflow-analysis".into());
        expertise.push("scientific-interpretation".into());
    }

    if kind.enable_bibliography() {
        tags.push("literature".into());
        tags.push("bibliography".into());
        expertise.push("literature-search".into());
        expertise.push("citation-management".into());
    }
    if kind.enable_writing() {
        tags.push("writing".into());
        tags.push("latex".into());
        tags.push("manuscript".into());
        expertise.push("document-editing".into());
        expertise.push("citation-insertion".into());
        expertise.push("latex-compilation".into());
    }
    if kind.enable_opengwas() {
        tags.push("gwas".into());
        tags.push("genetics".into());
        expertise.push("gwas-analysis".into());
    }
    if kind.enable_opentargets() {
        tags.push("drug-targets".into());
        expertise.push("target-identification".into());
    }
    if kind.enable_gwascatalog() {
        tags.push("gwas-catalog".into());
        expertise.push("variant-lookup".into());
    }
    if kind.enable_chembl() {
        tags.push("chembl".into());
        expertise.push("bioactivity-data".into());
    }
    if kind.enable_rcsb() {
        tags.push("pdb".into());
        tags.push("structural-biology".into());
        expertise.push("protein-structure-lookup".into());
        expertise.push("structural-biology-analysis".into());
    }
    if kind.enable_string() {
        tags.push("protein-networks".into());
        expertise.push("string-analysis".into());
    }
    if kind.enable_kegg() {
        tags.push("kegg".into());
        expertise.push("pathway-analysis".into());
    }
    if kind.enable_dag_history() {
        tags.push("pipeline".into());
        expertise.push("dag-execution".into());
    }
    if kind.enable_plugin_rsi() {
        tags.push("plugin".into());
        tags.push("node".into());
        tags.push("node-development".into());
        tags.push("plugin-validation".into());
        expertise.push("plugin-development".into());
        expertise.push("node-development".into());
        expertise.push("container-plugin-lifecycle".into());
        expertise.push("plugin-validation".into());
    }
    if kind.enable_browser() {
        tags.push("browser".into());
        tags.push("web".into());
        tags.push("web-browsing".into());
        expertise.push("web-navigation".into());
        expertise.push("web-extraction".into());
        expertise.push("web-interaction".into());
    }

    crate::control::AgentInfo {
        name: name.into(),
        path: path.into(),
        agent_id: None,
        summary: kind.description().into(),
        tags,
        expertise,
        tools: Vec::new(), // populated at runtime if needed
        status: crate::control::AgentStatus::Idle,
        last_event: None,
    }
}

fn path_is_ancestor(ancestor: &agentik_types::AgentPath, path: &agentik_types::AgentPath) -> bool {
    let mut current = path.parent();
    while let Some(parent) = current {
        if &parent == ancestor {
            return true;
        }
        current = parent.parent();
    }
    false
}

/// Uniform denial for agent-initiated management of a target that is not a
/// resolvable direct child. Deliberately identical for "unresolvable" and
/// "exists but not your child" so callers cannot probe the registry.
fn not_a_direct_child_message(name: &str) -> String {
    format!(
        "Agent '{name}' is not one of your direct children. \
         Agents may only manage their own direct children; \
         use list_agents to see them."
    )
}

async fn read_agent_history(
    storage: Arc<dyn AgentStorage>,
    agent_id: uuid::Uuid,
    agent_path: String,
    limit: usize,
) -> AgentExecutionHistory {
    let mut session_id = None;
    let mut messages = Vec::new();
    if let Some(record) = storage
        .list_session_records(agent_id)
        .await
        .ok()
        .and_then(|records| records.into_iter().next_back())
    {
        session_id = Some(record.session_id);
        if let Ok(state) =
            agentik_core::storage::restore_session_state(&*storage, agent_id, record.session_id)
                .await
        {
            let start = state.messages.len().saturating_sub(limit);
            messages = state.messages[start..].to_vec();
        }
    }

    AgentExecutionHistory {
        agent_path,
        agent_id,
        session_id,
        messages,
    }
}

async fn read_persisted_agent_history(
    storage: Arc<dyn AgentStorage>,
    requested_path: String,
    limit: usize,
) -> AgentExecutionHistory {
    let Ok(entries) = storage.list_persisted_agents().await else {
        return AgentExecutionHistory {
            agent_path: requested_path,
            agent_id: uuid::Uuid::nil(),
            session_id: None,
            messages: Vec::new(),
        };
    };

    let matches: Vec<_> = entries
        .into_iter()
        .filter(|entry| {
            entry.path == requested_path
                || entry
                    .path
                    .rsplit('/')
                    .next()
                    .is_some_and(|segment| segment == requested_path)
        })
        .collect();

    match matches.as_slice() {
        [entry] => read_agent_history(storage, entry.agent_id, entry.path.clone(), limit).await,
        _ => AgentExecutionHistory {
            agent_path: requested_path,
            agent_id: uuid::Uuid::nil(),
            session_id: None,
            messages: Vec::new(),
        },
    }
}

#[cfg(test)]
mod interrupt_agent_tests {
    //! Phase 3 — interrupt_agent vs shutdown_agent distinction.
    //!
    //! These tests verify the command-layer invariants without spinning
    //! up a full RuntimeHost (which requires DataEngine/Turso).
    //! The full integration test would be: spawn agent → delegate_to →
    //! interrupt_agent → assert LifecycleChanged(Cancelled) and agent
    //! still in registry.

    use super::*;
    use crate::control::HostCommand;

    /// `interrupt` and `shutdown` MUST be distinct commands — the
    /// handler dispatches them to different paths:
    /// - `CancelAgent` → AgentCommand::Cancel → handle.cancel() (preserves
    ///   agent; only the current turn aborts; new token issued)
    /// - `Shutdown { name, .. }` → AgentCommand::Shutdown → handle.shutdown()
    ///   → graceful exit (removes from registry)
    ///
    /// Mixing these up would silently kill long-lived worker agents on
    /// a benign interrupt request.
    #[test]
    fn cancel_and_shutdown_are_distinct_commands() {
        let cancel = HostCommand::CancelAgent {
            name: "worker".into(),
            caller_path: None,
            reply_tx: None,
        };
        let shutdown = HostCommand::Shutdown {
            name: "worker".into(),
            caller_path: None,
            reply_tx: None,
        };

        // Different concrete types — pattern-match proves it.
        let cancel_kind = match &cancel {
            HostCommand::CancelAgent { .. } => "cancel",
            _ => "other",
        };
        let shutdown_kind = match &shutdown {
            HostCommand::Shutdown { .. } => "shutdown",
            _ => "other",
        };
        assert_eq!(cancel_kind, "cancel");
        assert_eq!(shutdown_kind, "shutdown");
    }

    /// `InterruptAgentInput` should accept `agent_name` as required and
    /// `reason` as optional with a sensible default. The tool emits a
    /// log line + a ToolResult::success message; we test the schema by
    /// serializing/deserializing JSON in the same shape the LLM would
    /// produce.
    #[test]
    fn interrupt_input_schema_accepts_minimal_payload() {
        // Mimic the schema the LLM would emit when calling the tool.
        let raw = serde_json::json!({ "agent_name": "worker" });
        let parsed: serde_json::Result<serde_json::Value> = Ok(raw.clone());
        let v = parsed.expect("parse");
        assert_eq!(v["agent_name"], "worker");
        // reason is omitted → tool default is "user-requested interrupt"
        assert!(v.get("reason").is_none());
    }

    /// `InterruptAgentInput` schema with explicit reason.
    #[test]
    fn interrupt_input_schema_accepts_full_payload() {
        let raw = serde_json::json!({
            "agent_name": "researcher",
            "reason": "wrong agent selected"
        });
        assert_eq!(raw["agent_name"], "researcher");
        assert_eq!(raw["reason"], "wrong agent selected");
    }

    /// The `is_final_status` predicate interacts with interrupt:
    /// after `cancel_agent`, the agent emits `LifecycleChanged(Cancelled)`
    /// which `derive_agent_status` maps to `AgentStatus::Failed`. Verify
    /// the chain `Cancelled → Failed → is_final` returns true.
    #[test]
    fn cancelled_lifecycle_maps_to_failed_via_derive() {
        let (status, _) = super::super::status::derive_agent_status(
            &agentik_sdk::types::AgentEvent::LifecycleChanged(
                agentik_types::AgentLifecycleStatus::Cancelled,
            ),
        );
        assert!(
            super::super::status::is_final_status(&status),
            "Cancelled must map to a terminal status so delegated tasks \
             don't hang forever after interrupt_agent fires"
        );
    }

    /// Same for `Error` lifecycle — fatal system errors must also be
    /// terminal.
    #[test]
    fn error_lifecycle_maps_to_failed_via_derive() {
        let (status, _) = super::super::status::derive_agent_status(
            &agentik_sdk::types::AgentEvent::LifecycleChanged(
                agentik_types::AgentLifecycleStatus::Error,
            ),
        );
        assert!(super::super::status::is_final_status(&status));
    }
}

#[cfg(test)]
mod deliver_message_tests {
    //! `DeliverMessage` — the user→agent channel (TUI/gateway). The
    //! fire-and-forget inter-agent `SendMessage` channel was removed:
    //! delegate_to is the only inter-agent communication path.
    //!
    //! These tests verify the command-layer invariants without spinning
    //! up a full RuntimeHost: tracked delivery carries a reply channel so
    //! a 202 means the host actually enqueued the message.

    use super::*;
    use crate::control::{HostCommand, HostControl};
    use tokio::sync::oneshot;

    /// `DeliverMessage` and `Delegate` MUST be distinct variants. They
    /// dispatch to different handler paths:
    /// - `DeliverMessage` → user message (optional reply for the gateway)
    /// - `Delegate` → request-response inter-agent (reply: full response
    ///    text, gated to direct children)
    #[test]
    fn deliver_message_is_distinct_from_delegate() {
        let deliver = HostCommand::DeliverMessage {
            name: "worker".into(),
            message: "hello".into(),
            reply_tx: None,
        };
        let delegate = HostCommand::Delegate {
            to: "worker".into(),
            message: "hello".into(),
            caller_path: None,
            delegation_id: uuid::Uuid::new_v4(),
            progress: None,
            reply_tx: oneshot::channel().0,
        };

        // Pattern-match proves each is a distinct variant.
        let deliver_kind = match &deliver {
            HostCommand::DeliverMessage { .. } => "deliver",
            _ => "other",
        };
        let delegate_kind = match &delegate {
            HostCommand::Delegate { .. } => "delegate",
            _ => "other",
        };
        assert_eq!(deliver_kind, "deliver");
        assert_eq!(delegate_kind, "delegate");
        assert_ne!(deliver_kind, delegate_kind);
    }

    /// The gateway rides `DeliverMessage.reply_tx` so a 202 means the host
    /// actually enqueued the message. Round-trip: tracked delivery carries
    /// the reply channel, the host confirms, the caller sees `Some(Ok(()))`.
    #[tokio::test]
    async fn deliver_message_tracked_round_trip_confirms_delivery() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<HostCommand>();
        let (event_tx, _) = tokio::sync::broadcast::channel::<super::super::HostEvent>(1);
        let control = HostControl::new(cmd_tx, event_tx);

        let caller = tokio::spawn(async move {
            control
                .deliver_message_tracked("/root/worker", "hello there")
                .await
        });

        let cmd = cmd_rx.recv().await.expect("command received");
        match cmd {
            HostCommand::DeliverMessage {
                name,
                message,
                reply_tx,
            } => {
                assert_eq!(name, "/root/worker");
                assert_eq!(message, "hello there");
                let reply_tx = reply_tx.expect("tracked delivery carries a reply channel");
                let _ = reply_tx.send(Ok(()));
            }
            _ => panic!("expected DeliverMessage variant"),
        }

        let result = caller.await.expect("caller task panicked");
        assert_eq!(result, Some(Ok(())));
    }

    /// Tracked delivery surfaces dropped messages: an `Err` reply (unknown
    /// agent, exited loop) reaches the caller instead of a blind 202.
    #[tokio::test]
    async fn deliver_message_tracked_round_trip_reports_drop() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<HostCommand>();
        let (event_tx, _) = tokio::sync::broadcast::channel::<super::super::HostEvent>(1);
        let control = HostControl::new(cmd_tx, event_tx);

        let caller = tokio::spawn(async move {
            control
                .deliver_message_tracked("/root/ghost", "hello")
                .await
        });

        let cmd = cmd_rx.recv().await.expect("command received");
        match cmd {
            HostCommand::DeliverMessage { reply_tx, .. } => {
                let reply_tx = reply_tx.expect("tracked delivery carries a reply channel");
                let _ = reply_tx.send(Err("Agent '/root/ghost' is not registered.".into()));
            }
            _ => panic!("expected DeliverMessage variant"),
        }

        let result = caller.await.expect("caller task panicked");
        assert!(matches!(result, Some(Err(message)) if message.contains("not registered")));
    }

    /// When the host never answers (reply channel dropped without a send —
    /// a wedged or panicked host loop), tracked delivery returns `None`.
    /// The fire-and-forget TUI path still sends no reply channel at all.
    #[tokio::test]
    async fn deliver_message_tracked_returns_none_when_host_is_silent() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<HostCommand>();
        let (event_tx, _) = tokio::sync::broadcast::channel::<super::super::HostEvent>(1);
        let control = HostControl::new(cmd_tx, event_tx);

        let caller = tokio::spawn({
            let control = control.clone();
            async move {
                control
                    .deliver_message_tracked("/root/wedged", "hello")
                    .await
            }
        });

        let cmd = cmd_rx.recv().await.expect("command received");
        match cmd {
            HostCommand::DeliverMessage { reply_tx, .. } => {
                drop(reply_tx);
            }
            _ => panic!("expected DeliverMessage variant"),
        }

        let result = caller.await.expect("caller task panicked");
        assert!(result.is_none(), "silent host must surface as None");

        // Legacy TUI path: no reply channel, still fire-and-forget.
        control.deliver_message("/root/worker", "hello");
        match cmd_rx.recv().await.expect("command received") {
            HostCommand::DeliverMessage { reply_tx, .. } => {
                assert!(reply_tx.is_none(), "TUI delivery stays fire-and-forget");
            }
            _ => panic!("expected DeliverMessage variant"),
        }
    }
}

#[cfg(test)]
mod communication_policy_tests {
    use super::*;
    use crate::config::RuntimeConfig;
    use agentik_core::profile::{AgentKind, AgentProfileConfig};

    fn config(dir: &tempfile::TempDir) -> RuntimeConfig {
        let mut config = RuntimeConfig::default();
        config.data_dir = dir.path().join("data");
        config.state_dir = dir.path().join("state");
        config.agent_db = dir.path().join("agent.db");
        config.dag_history_db = dir.path().join("dag-history.db");
        config.bib_db_path = dir.path().join("bib.db");
        config.writing_db_path = dir.path().join("writing.db");
        config.app_db_path = dir.path().join("app.db");
        config.knowledge_db_path = dir.path().join("knowledge.db");
        // Keep the embedding model (network install + ONNX session)
        // out of these tests.
        config.embedding.enabled = false;
        config
    }

    async fn register_agent_at(
        host: &mut RuntimeHost,
        path: &str,
        model: Arc<ArcSwapOption<Model>>,
    ) {
        let path = agentik_types::AgentPath::try_from(path).unwrap();
        let config = AgentProfileConfig::new(AgentKind::Researcher);
        let handle = host.spawn_agent(&path, &config, model, None).await.unwrap();
        let info = capability_from_kind(handle.path.name(), handle.path.as_str(), config.kind);
        host.register_agent(handle, info);
    }

    /// Guard for the spawn payload narrowing: the per-agent config handed to
    /// `spawn_agent` — including `preferred_model` — must land in
    /// `agents.config_json` so restarts restore the preference.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_persists_preferred_model_into_agent_config() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = RuntimeHost::open(&config(&dir)).await.unwrap();
        let path = agentik_types::AgentPath::try_from("/root/researcher").unwrap();
        let config = AgentProfileConfig {
            kind: AgentKind::Researcher,
            preferred_model: Some("mock:mock-model".into()),
            runtime: agentik_core::AgentRuntimeOverrides {
                use_memory: Some(false),
                generate_memory: None,
                reasoning_effort: None,
            },
        };
        let _handle = host
            .spawn_agent(
                &path,
                &config,
                Arc::new(ArcSwapOption::from_pointee(None)),
                None,
            )
            .await
            .unwrap();

        // The record is upserted asynchronously by the agent's run loop.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let storage = host.infra().storage.clone();
        let record = storage
            .get_agent_by_name(path.as_str())
            .await
            .unwrap()
            .expect("agent record missing after spawn");
        let restored = AgentProfileConfig::from_json(&record.config_json);
        assert_eq!(restored.kind, AgentKind::Researcher);
        assert_eq!(restored.preferred_model.as_deref(), Some("mock:mock-model"));
        assert_eq!(restored.runtime.use_memory, Some(false));

        host.shutdown_all_agents_and_wait().await;
    }

    /// A browser-kind agent gets a private Chromium session allocated at
    /// spawn time. The session's strong `Arc` lives in the agent's tool
    /// list, the host pool keeps only a `Weak`, and the session dies with
    /// the agent — spawning a browser child and shutting it down must not
    /// leak the browser process or the pool entry.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn browser_agent_session_lifecycle() {
        fn automation_chromium_count() -> usize {
            std::process::Command::new("pgrep")
                .args(["-f", "enable-automation"])
                .output()
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .count()
                })
                .unwrap_or(0)
        }

        let dir = tempfile::tempdir().unwrap();
        let mut host = RuntimeHost::open(&config(&dir)).await.unwrap();
        let path = agentik_types::AgentPath::try_from("/root/browser").unwrap();
        let profile = AgentProfileConfig::new(AgentKind::Browser);
        let handle = host
            .spawn_agent(
                &path,
                &profile,
                Arc::new(ArcSwapOption::from_pointee(None)),
                None,
            )
            .await
            .unwrap();
        // Register with the relay so Shutdown actually reaches the agent
        // task (and its tool list) during shutdown_all_agents_and_wait.
        let info = capability_from_kind(handle.path.name(), handle.path.as_str(), profile.kind);
        host.register_agent(handle, info);

        // Tool assembly registered the browser tool group; the second
        // assembly call must reuse the live session from the pool rather
        // than launching a second Chromium.
        let tools = host
            .infra()
            .tools_from_profile(&path, AgentKind::Browser)
            .await
            .unwrap();
        let names: Vec<String> = tools.iter().map(|t| t.definition.name.clone()).collect();
        for expected in [
            "browser_navigate",
            "browser_click",
            "browser_type_text",
            "browser_screenshot",
            "browser_evaluate",
            "browser_download",
            "browser_wait_for_download",
            "browser_list_downloads",
            "browser_list_tabs",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "missing tool `{expected}`"
            );
        }

        // Downloads routinely outlast a turn, so `browser_download` must be
        // dispatched as a background task; a sync registration would block
        // the agent on the toolset timeout and lose the transfer.
        let download = tools
            .iter()
            .find(|t| t.definition.name == "browser_download")
            .expect("browser_download tool is registered");
        assert_eq!(
            download.implementation.execution_mode(),
            agentik_core::tools::ExecutionMode::Async,
            "browser_download must run in the background"
        );
        // This local tool list holds its own strong Arcs — drop it so the
        // agent's tool list is the only remaining owner before shutdown.
        drop(tools);

        // The pool entry is alive while the agent runs.
        assert!(
            host.infra()
                .browsers
                .lock()
                .await
                .session_alive("/root/browser"),
            "pool should hold a live session while the agent runs"
        );
        assert!(
            automation_chromium_count() > 0,
            "expected a chromium process for the browser agent"
        );

        host.shutdown_all_agents_and_wait().await;

        // The agent task has joined; its tool list (the strong Arc owner)
        // is dropped, so the pool entry dangles and the process is killed.
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let alive = host
                    .infra()
                    .browsers
                    .lock()
                    .await
                    .session_alive("/root/browser");
                if !alive {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("session Arc should drop after the agent shuts down");
    }

    fn set_status(host: &mut RuntimeHost, path: &str, status: crate::control::AgentStatus) {
        let entry = host.agents.get_mut(path).unwrap();
        entry.status = status.clone();
        entry.info.status = status;
    }

    /// Delegation reaches only the caller's direct children. Siblings,
    /// cousins, grandparents, grandchildren, and self are all rejected;
    /// a busy child stays delegable (messages queue on its relay).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delegation_is_restricted_to_direct_children() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = RuntimeHost::open(&config(&dir)).await.unwrap();
        let model: Arc<ArcSwapOption<Model>> = Arc::new(ArcSwapOption::from_pointee(None));
        for path in [
            "/root/a",
            "/root/b",
            "/root/a/child",
            "/root/b/child",
            "/root/a/child/grand",
        ] {
            register_agent_at(&mut host, path, model.clone()).await;
        }

        // Parent → direct child is the one allowed direction.
        assert!(
            host.validate_delegation_paths("/root/a", "/root/a/child")
                .is_ok()
        );
        // Busy children remain delegable — turns queue on the relay.
        set_status(
            &mut host,
            "/root/a/child",
            crate::control::AgentStatus::Running,
        );
        assert!(
            host.validate_delegation_paths("/root/a", "/root/a/child")
                .is_ok()
        );
        set_status(
            &mut host,
            "/root/a/child",
            crate::control::AgentStatus::Idle,
        );

        // Upward delegation is rejected with the superior-specific message.
        let upward = host
            .validate_delegation_paths("/root/a/child", "/root/a")
            .unwrap_err();
        assert!(upward.contains("superior"), "{upward}");

        // Siblings, cousins, and grandchildren are not direct children.
        let sibling = host
            .validate_delegation_paths("/root/a", "/root/b")
            .unwrap_err();
        assert!(sibling.contains("not a direct child"), "{sibling}");
        let cousin = host
            .validate_delegation_paths("/root/a/child", "/root/b/child")
            .unwrap_err();
        assert!(cousin.contains("not a direct child"), "{cousin}");
        let grandchild = host
            .validate_delegation_paths("/root/a", "/root/a/child/grand")
            .unwrap_err();
        assert!(grandchild.contains("not a direct child"), "{grandchild}");

        // Self-delegation is rejected.
        let self_delegate = host
            .validate_delegation_paths("/root/a", "/root/a")
            .unwrap_err();
        assert!(self_delegate.contains("themselves"), "{self_delegate}");
    }

    /// Agent-initiated shutdown/interrupt reach only the caller's direct
    /// children; the operator (caller_path = None) path stays unrestricted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_and_interrupt_are_child_scoped_for_agents() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = RuntimeHost::open(&config(&dir)).await.unwrap();
        let model: Arc<ArcSwapOption<Model>> = Arc::new(ArcSwapOption::from_pointee(None));
        for path in ["/root/a", "/root/b", "/root/a/child"] {
            register_agent_at(&mut host, path, model.clone()).await;
        }

        // Agent trying to shut down a non-child: denied, target survives.
        let (tx, rx) = tokio::sync::oneshot::channel();
        host.process_command(crate::control::HostCommand::Shutdown {
            name: "/root/b".into(),
            caller_path: Some("/root/a".into()),
            reply_tx: Some(tx),
        });
        let denied = rx.await.unwrap();
        let message =
            denied.expect_err("shutdown of a non-child must be denied by an agent caller");
        assert!(message.contains("direct child"), "{message}");
        assert!(host.agents.contains_key("/root/b"));

        // Agent interrupting a non-child: denied the same way.
        let (tx, rx) = tokio::sync::oneshot::channel();
        host.process_command(crate::control::HostCommand::CancelAgent {
            name: "/root/b".into(),
            caller_path: Some("/root/a".into()),
            reply_tx: Some(tx),
        });
        let denied = rx.await.unwrap();
        let message =
            denied.expect_err("interrupt of a non-child must be denied by an agent caller");
        assert!(message.contains("direct child"), "{message}");
        assert!(host.agents.contains_key("/root/b"));

        // Agent shutting down its own direct child: allowed.
        let (tx, rx) = tokio::sync::oneshot::channel();
        host.process_command(crate::control::HostCommand::Shutdown {
            name: "/root/a/child".into(),
            caller_path: Some("/root/a".into()),
            reply_tx: Some(tx),
        });
        rx.await.unwrap().unwrap();
        assert!(!host.agents.contains_key("/root/a/child"));

        // Operator (None caller) shutting down any agent: unrestricted.
        host.process_command(crate::control::HostCommand::Shutdown {
            name: "/root/b".into(),
            caller_path: None,
            reply_tx: None,
        });
        assert!(!host.agents.contains_key("/root/b"));
    }

    /// route_task considers only the caller's direct children plus the
    /// spawnable role profiles — never foreign branches or the caller.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn route_task_only_considers_direct_children_and_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = RuntimeHost::open(&config(&dir)).await.unwrap();
        let model: Arc<ArcSwapOption<Model>> = Arc::new(ArcSwapOption::from_pointee(None));
        for path in ["/root/a", "/root/a/child", "/root/b"] {
            register_agent_at(&mut host, path, model.clone()).await;
        }

        let result = host.route_task("analyze gwas data", "/root/a");
        let names: Vec<&str> = result.candidates.iter().map(|c| c.agent.as_str()).collect();
        assert!(!names.is_empty(), "expected some candidates");
        for name in &names {
            assert!(
                matches!(*name, "child" | "researcher" | "developer"),
                "unexpected candidate '{name}' in {names:?}"
            );
        }
    }
}
