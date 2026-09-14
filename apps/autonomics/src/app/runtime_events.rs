//! Bridge between gateway events and TUI application state.

use super::history::messages_to_chatlines;
use super::*;

fn registered_agent_id(info: &gateway::proto::HostEventView) -> Option<uuid::Uuid> {
    match info {
        gateway::proto::HostEventView::AgentRegistered { info, .. } => info.agent_id,
        _ => None,
    }
}

impl App {
    /// Dispatch one SSE frame from the gateway pump.
    pub(super) fn handle_gateway_frame(&mut self, frame: gateway::client::GatewayFrame) {
        use gateway::client::{FrameInner, GatewayFrame};

        match frame {
            GatewayFrame::Sequenced {
                inner: FrameInner::Agent { agent, event },
                ..
            } => self.route_agent_event(agent, event),
            GatewayFrame::Sequenced {
                inner: FrameInner::Host(view),
                ..
            } => self.apply_host_event(view),
            GatewayFrame::Sequenced {
                inner: FrameInner::Notice(notice),
                ..
            } => self.handle_gateway_notice(notice),
            GatewayFrame::Lag { .. } => {
                // Frames were dropped (frozen consumer beyond the replay
                // ring). Folded deltas may be stale — reconcile from a
                // fresh `/state` snapshot.
                tracing::warn!("gateway stream lagged; re-hydrating from /state");
                self.state
                    .toasts
                    .info("Gateway 事件流断档", Some("正在重新同步状态…".into()));
                self.spawn_reconcile("lag");
            }
            GatewayFrame::Disconnected => {
                self.state
                    .toasts
                    .info("Gateway 连接中断", Some("正在自动重连…".into()));
            }
            GatewayFrame::Reconnected => {
                self.state
                    .toasts
                    .info("Gateway 已重连", Some("正在重新同步状态…".into()));
                self.spawn_reconcile("reconnect");
            }
        }
    }

    /// Translate a daemon-side lifecycle notice into the matching
    /// application event.
    fn handle_gateway_notice(&mut self, notice: gateway::proto::GatewayNotice) {
        use gateway::proto::GatewayNotice;
        match notice {
            GatewayNotice::ModelChanged { spec, reason } => {
                self.handle_app_event(crate::app_event::AppEvent::ActiveModelChanged {
                    spec,
                    reason,
                });
            }
            GatewayNotice::ChatgptLogin { result } => {
                self.handle_app_event(crate::app_event::AppEvent::ChatgptLoginCompleted { result });
            }
            GatewayNotice::CatalogFetched {
                provider,
                result: Ok(count),
            } => {
                self.handle_app_event(crate::app_event::AppEvent::RemoteCatalogFetched {
                    provider_name: provider,
                    result: Ok(count),
                });
            }
            GatewayNotice::CatalogFetched {
                provider,
                result: Err(error),
            } => {
                self.handle_app_event(crate::app_event::AppEvent::RemoteCatalogFetched {
                    provider_name: provider,
                    result: Err(error),
                });
            }
        }
    }

    /// Route one agent event to the matching session tab — the same
    /// routing the fat-client loop performed on `host.recv_any()`.
    fn route_agent_event(&mut self, agent_name: String, event: AgentEvent) {
        let target_idx = if !agent_name.is_empty() {
            self.state
                .sessions
                .iter()
                .position(|s| s.name == agent_name)
                .unwrap_or(self.state.active_agent_idx)
        } else {
            self.state.active_agent_idx
        };

        let is_session_list = matches!(event, AgentEvent::SessionList { .. });
        if matches!(
            event,
            AgentEvent::SessionActivated { .. }
                | AgentEvent::SessionPaused { .. }
                | AgentEvent::SessionClosed { .. }
                | AgentEvent::SessionList { .. }
        ) {
            state::apply_session_event(&mut self.state, event, target_idx);
        } else {
            // Route to the correct tab's tab_state.
            let tab_state = self.state.sessions.get_mut(target_idx).map(|s| {
                if s.active_sub_session_idx < s.sub_sessions.len() {
                    &mut s.sub_sessions[s.active_sub_session_idx].tab_state
                } else {
                    &mut s.pending_tab_state
                }
            });
            if let Some(ts) = tab_state {
                state::apply_event(ts, event);
            }
        }

        // After SessionList arrives, spawn background history loads for
        // sessions that have empty tab_state.messages.
        if is_session_list {
            self.spawn_history_loads_for(target_idx);
        }
    }

    /// Apply an internal [`AppEvent`] to state.
    pub(super) fn handle_app_event(&mut self, event: crate::app_event::AppEvent) {
        match event {
            crate::app_event::AppEvent::AgentRecordsLoaded(records) => {
                self.state.agent_picker.set_records(&records);
                self.state.agent_picker.open();
                tracing::info!(count = records.len(), "agent records loaded for picker");
            }
            crate::app_event::AppEvent::AgentDeleted(agent_id) => {
                self.state.agent_picker.remove_by_id(agent_id);
                // If this agent's leaf is currently open, close it too — all
                // stored data has been erased so there's nothing to resume.
                self.close_agent_leaf_by_id(agent_id);
                tracing::info!(%agent_id, "agent removed from picker after deletion");
            }
            crate::app_event::AppEvent::AgentRenamed { agent_id, new_path } => {
                self.state
                    .agent_picker
                    .apply_rename(agent_id, new_path.clone());
                tracing::info!(%agent_id, new_path = %new_path, "agent renamed in picker");
            }
            crate::app_event::AppEvent::HistoryLoaded {
                agent_id,
                session_id,
                messages,
            } => {
                self.replay_history(agent_id, session_id, &messages);
            }
            crate::app_event::AppEvent::PlanLoaded { agent_id, plan } => {
                // Only route to the agent's active tab_state. Guard against
                // stale overlay: the loaded revision is only applied if it's
                // newer than what the TUI already has (e.g. the model already
                // called update_plan since resume).
                let session_idx = self
                    .state
                    .sessions
                    .iter()
                    .position(|s| s.agent_id == agent_id);
                let Some(session_idx) = session_idx else {
                    tracing::warn!(%agent_id, "plan loaded for unknown agent");
                    return;
                };
                let ts = if session_idx == self.state.active_agent_idx {
                    self.state.active_tab_state_mut()
                } else {
                    // Route to the session's pending_tab_state so it shows up
                    // when the user switches to that agent.
                    &mut self.state.sessions[session_idx].pending_tab_state
                };
                if plan.revision > ts.plan.revision {
                    ts.plan = state::PlanState {
                        steps: plan.update.plan,
                        revision: plan.revision,
                    };
                    self.dirty = true;
                    tracing::info!(
                        %agent_id,
                        revision = plan.revision,
                        "restored agent plan to TUI from storage"
                    );
                }
            }
            crate::app_event::AppEvent::AgentSpawned {
                profile_name,
                result,
            } => match result {
                Ok(name) => {
                    // The daemon has already registered the agent and sent
                    // the AgentRegistered host frame (or will shortly). The
                    // apply_host_event handler creates the session tab and
                    // requests the session list. Here we just log success.
                    tracing::info!(profile = %profile_name, agent = %name, "agent spawned and registered with daemon");
                }
                Err(e) => {
                    // Clear the focus flag so a stale request doesn't
                    // mis-attach to a later, unrelated registration.
                    self.state.pending_focus_agent_name = None;
                    tracing::error!(profile = %profile_name, error = %e, "failed to spawn agent");
                    self.state.toasts.error("Agent creation failed", Some(e));
                }
            },
            crate::app_event::AppEvent::ModelCatalogLoaded(catalog) => {
                Self::load_model_config(&catalog, &mut self.state.model_config_state);
                if let Some(spec) = catalog.active_model.clone() {
                    self.state.active_model_spec = Some(spec.clone());
                    if let Some((provider, name)) = spec.split_once(':') {
                        self.state.active_model_display = catalog
                            .models
                            .iter()
                            .find(|m| m.provider_name == provider && m.model_name == name)
                            .map(|m| (m.model_name.clone(), m.context_length));
                    }
                }
            }
            crate::app_event::AppEvent::ActiveModelChanged { spec, reason } => {
                tracing::info!(spec = ?spec, reason = %reason, "daemon active model changed");
                self.spawn_catalog_reload();
            }
            crate::app_event::AppEvent::RemoteCatalogFetched {
                provider_name,
                result,
            } => match result {
                Ok(count) => {
                    self.spawn_catalog_reload();
                    self.state.toasts.success(
                        "Catalogue refreshed",
                        Some(format!("{provider_name}: {count} models")),
                    );
                }
                Err(e) => {
                    tracing::warn!(provider = %provider_name, error = %e, "remote catalogue fetch failed");
                    self.state.toasts.error("Fetch failed", Some(e));
                }
            },
            crate::app_event::AppEvent::ChatgptLoginUrl(url) => {
                // 授权 URL：复制到剪贴板（保住 lease）+ 尽力开浏览器。
                match crate::clipboard_copy::copy_to_clipboard(&url) {
                    Ok(lease) => {
                        self.clipboard_lease = lease;
                        let opened = Self::open_in_browser(&url);
                        let hint = if opened {
                            "授权链接已复制；请在打开的浏览器中完成 OpenAI 授权"
                        } else {
                            "授权链接已复制到剪贴板，请粘贴到浏览器打开并完成授权"
                        };
                        tracing::info!(
                            "chatgpt login: authorize url dispatched (browser={opened})"
                        );
                        self.state
                            .toasts
                            .info("ChatGPT 登录", Some(hint.to_string()));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "chatgpt login url clipboard copy failed");
                        self.state
                            .toasts
                            .error("复制失败", Some(format!("{e}；授权 URL 见日志")));
                        tracing::info!(url = %url, "chatgpt login authorize url");
                    }
                }
            }
            crate::app_event::AppEvent::ChatgptLoginCompleted { result } => match result {
                Ok(info) => {
                    tracing::info!(
                        email = info.email.as_deref().unwrap_or("unknown"),
                        "chatgpt login persisted by daemon"
                    );
                    self.spawn_catalog_reload();
                    self.state.toasts.success(
                        "ChatGPT 登录成功",
                        Some(format!(
                            "{}（{}）",
                            info.email.as_deref().unwrap_or("email unknown"),
                            info.plan_type.as_deref().unwrap_or("plan unknown"),
                        )),
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, "chatgpt login failed");
                    self.state.toasts.error("ChatGPT 登录失败", Some(e));
                }
            },
            crate::app_event::AppEvent::DagSnapshotLoaded(result) => match result {
                Ok(snapshot) => {
                    self.state.dag_snapshot = Some(snapshot);
                    self.state.dag_view_error = None;
                }
                Err(error) => {
                    self.state.dag_view_error = Some(error);
                }
            },
            crate::app_event::AppEvent::ModelInfoLoaded { agent, info } => {
                self.model_info_pending.remove(&agent);
                if let Some(info) = info {
                    self.agent_model_cache.insert(agent, info);
                    self.dirty = true;
                }
            }
            crate::app_event::AppEvent::ProviderSaved {
                provider_name,
                result,
            } => match result {
                Ok(()) => {
                    tracing::info!("saved provider config: {provider_name}");
                    self.spawn_catalog_reload();
                }
                Err(e) => {
                    tracing::error!("failed to save provider config: {e}");
                    self.state.toasts.error("Save failed", Some(e));
                }
            },
            crate::app_event::AppEvent::AgentUpserted(info) => {
                if !self.state.sessions.iter().any(|s| s.name == info.path) {
                    self.state.sessions.push(state::AgentSession {
                        name: info.path.clone(),
                        agent_id: info.agent_id.unwrap_or_default(),
                        sub_sessions: Vec::new(),
                        active_sub_session_idx: 0,
                        pending_tab_state: Default::default(),
                    });
                    self.refresh_agent_model_info(&info.path);
                }
            }
            crate::app_event::AppEvent::SessionListKnown { agent, sessions } => {
                self.apply_session_list(&agent, sessions);
                if let Some(idx) = self.state.sessions.iter().position(|s| s.name == agent) {
                    self.spawn_history_loads_for(idx);
                }
            }
        }
    }

    /// Apply a [`gateway::proto::HostEventView`] (agent registered /
    /// unregistered / status changed) by keeping the TUI's session list in
    /// sync with the daemon's agent registry. This is the key bridge that
    /// makes tool-spawned agents visible in the TUI.
    pub(super) fn apply_host_event(&mut self, event: gateway::proto::HostEventView) {
        match event {
            gateway::proto::HostEventView::AgentRegistered { path, info } => {
                let name = path.clone();
                // Check if the TUI already knows about this agent.
                if self.state.sessions.iter().any(|s| s.name == name) {
                    return;
                }
                // Did the user explicitly ask to land on this leaf?
                // Matches by short name (`info.name`) since the spawn
                // was requested with the picker-visible short name.
                let steal_focus = self
                    .state
                    .pending_focus_agent_name
                    .as_deref()
                    .is_some_and(|wanted| wanted == info.name);
                // Add a new session tab for the tool-spawned agent.
                // Do NOT steal focus — the user may be interacting with
                // another agent. The new tab appears but focus stays
                // where the user left it.
                let agent_id =
                    registered_agent_id(&gateway::proto::HostEventView::AgentRegistered {
                        path: path.clone(),
                        info: info.clone(),
                    })
                    .unwrap_or_else(uuid::Uuid::new_v4);
                self.state.sessions.push(state::AgentSession {
                    name: name.clone(),
                    agent_id,
                    sub_sessions: Vec::new(),
                    active_sub_session_idx: 0,
                    pending_tab_state: Default::default(),
                });
                if steal_focus {
                    let new_idx = self.state.sessions.len() - 1;
                    self.state.active_agent_idx = new_idx;
                    self.state.pending_focus_agent_name = None;
                    tracing::info!(
                        agent = %name,
                        leaf_idx = new_idx,
                        "stole focus to user-restored leaf"
                    );
                }

                // Ask the daemon for the agent's session list. The response
                // arrives as `AgentEvent::SessionList` through the event
                // stream and is routed to this session's tab.
                let client = self.client.clone();
                let agent = name.clone();
                self.spawn_client_task("list_sessions", move || async move {
                    let _ = client.list_sessions(&agent).await;
                });

                // Prefetch the new agent's model info for the render cache.
                self.refresh_agent_model_info(&name);
                tracing::info!(agent = %name, "daemon agent registered to TUI (no focus steal)");
            }
            gateway::proto::HostEventView::AgentUnregistered { path } => {
                self.state.sessions.retain(|s| s.name != path);
                self.agent_model_cache.remove(&path);
                if self.state.active_agent_idx >= self.state.sessions.len() {
                    self.state.active_agent_idx = self.state.sessions.len().saturating_sub(1);
                }
                tracing::info!(agent = %path, "daemon agent unregistered from TUI");
            }
            // Cross-agent runtime status update. The per-session
            // `AgentTabState.status` is driven by the agent's own
            // `LifecycleChanged` event stream (via the session tab), so
            // this arm is a log-only bridge — it does NOT overwrite the
            // tab's status, which has finer-grained variants.
            gateway::proto::HostEventView::AgentStatusChanged {
                path,
                status,
                last_event,
            } => {
                tracing::debug!(
                    agent = %path,
                    status = %status.tag(),
                    last_event = ?last_event,
                    "daemon observed agent status change"
                );
            }
        }
    }

    /// Replay conversation history loaded from the daemon into the TUI's
    /// `tab_state.messages` as `ChatLine` entries.
    pub(super) fn replay_history(
        &mut self,
        agent_id: uuid::Uuid,
        session_id: uuid::Uuid,
        messages: &[Message],
    ) {
        let session_idx = self
            .state
            .sessions
            .iter()
            .position(|s| s.agent_id == agent_id);
        let Some(session_idx) = session_idx else {
            tracing::warn!(%agent_id, "history loaded for unknown agent");
            return;
        };

        let lines = messages_to_chatlines(messages);
        if lines.is_empty() {
            return;
        }

        let session = &mut self.state.sessions[session_idx];
        // Find the sub-session by session_id.
        let sub = session.sub_sessions.iter_mut().find(|s| s.id == session_id);
        let Some(sub) = sub else {
            tracing::warn!(%session_id, "history loaded for unknown sub-session");
            return;
        };

        if !sub.tab_state.messages.is_empty() {
            tracing::debug!(%session_id, "tab_state already populated, skipping");
            return;
        }
        sub.tab_state.set_messages(lines);
        sub.tab_state.scroll_to_bottom();
        self.dirty = true;
        tracing::info!(%session_id, count = sub.tab_state.messages.len(), "history replayed");
    }

    /// Fold a session list (from hydration or a `SessionList` event that
    /// arrived before the agent's sub-sessions existed) into the UI.
    pub(super) fn apply_session_list(&mut self, agent_path: &str, sessions: Vec<SessionInfo>) {
        let Some(agent) = self
            .state
            .sessions
            .iter_mut()
            .find(|s| s.name == agent_path)
        else {
            return;
        };
        for info in sessions {
            if agent.sub_sessions.iter().any(|s| s.id == info.id) {
                continue;
            }
            let mut sub = state::SubSession::new(info.id, info.title.clone());
            sub.last_active = info.last_active;
            sub.created_at = info.created_at;
            agent.sub_sessions.push(sub);
        }
    }

    /// Spawn background history loads for all sessions of the agent at
    /// `agent_idx` that have empty `tab_state.messages` (plus its plan).
    pub(super) fn spawn_history_loads_for(&mut self, agent_idx: usize) {
        let Some(agent_session) = self.state.sessions.get(agent_idx) else {
            return;
        };
        let agent_id = agent_session.agent_id;

        let to_load: Vec<uuid::Uuid> = agent_session
            .sub_sessions
            .iter()
            .filter(|s| s.tab_state.messages.is_empty())
            .map(|s| s.id)
            .collect();

        for session_id in to_load {
            let client = self.client.clone();
            let tx = self.app_event_tx.clone();
            self.spawn_client_task(
                &format!("load_session_history::{session_id}"),
                move || async move {
                    match client.session_history(agent_id, session_id).await {
                        Ok(messages) => {
                            if !messages.is_empty() {
                                tx.send(crate::app_event::AppEvent::HistoryLoaded {
                                    agent_id,
                                    session_id,
                                    messages,
                                });
                            }
                        }
                        Err(e) => {
                            tracing::warn!(%session_id, error = %e, "failed to load session history");
                        }
                    }
                },
            );
        }

        // ── Restore the agent's persistent plan ──
        // The backend `Agent::run()` bootstrap loads the plan from storage
        // but does NOT emit an `AgentEvent::PlanUpdate`, so the TUI's
        // `PlanState` would stay empty. We fetch it here and push a
        // `PlanLoaded` event to surface the restored checklist.
        let client = self.client.clone();
        let tx = self.app_event_tx.clone();
        self.spawn_client_task("restore_plan", move || async move {
            if let Ok(Some(plan)) = client.load_plan(agent_id).await {
                if !plan.is_empty() {
                    tx.send(crate::app_event::AppEvent::PlanLoaded { agent_id, plan });
                }
            }
        });
    }

    /// Spawn history loads for every known agent (hydration).
    pub(super) fn spawn_all_history_loads(&mut self) {
        let count = self.state.sessions.len();
        for idx in 0..count {
            self.spawn_history_loads_for(idx);
        }
    }

    /// Spawn a background task that re-fetches `/state` and reconciles
    /// the UI against it (after stream lag or reconnect): unknown agents
    /// are added, session lists folded, empty transcripts loaded, and the
    /// model catalog refreshed. Agents that disappeared are removed via
    /// `AgentUnregistered` host frames, not here.
    fn spawn_reconcile(&mut self, reason: &str) {
        let client = self.client.clone();
        let tx = self.app_event_tx.clone();
        let reason = reason.to_string();
        self.spawn_client_task("reconcile_state", move || async move {
            match client.state().await {
                Ok(snapshot) => {
                    tx.send(crate::app_event::AppEvent::ModelCatalogLoaded(
                        snapshot.model_catalog.clone(),
                    ));
                    for info in &snapshot.agents {
                        tx.send(crate::app_event::AppEvent::AgentUpserted(info.clone()));
                    }
                    for (agent_path, sessions) in snapshot.sessions {
                        tx.send(crate::app_event::AppEvent::SessionListKnown {
                            agent: agent_path,
                            sessions,
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(reason = %reason, error = %e, "reconcile state fetch failed");
                }
            }
        });
    }

    /// Fire-and-forget catalog reload (after provider/model/catalog
    /// changes daemon-side).
    pub(super) fn spawn_catalog_reload(&mut self) {
        let client = self.client.clone();
        let tx = self.app_event_tx.clone();
        self.spawn_client_task("reload_model_config", move || async move {
            match client.model_catalog().await {
                Ok(catalog) => {
                    tx.send(crate::app_event::AppEvent::ModelCatalogLoaded(catalog));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "model catalog reload failed");
                }
            }
        });
    }

    /// Fetch an agent's model info into the render cache (if not cached
    /// and not already in flight).
    pub(super) fn refresh_agent_model_info(&mut self, agent: &str) {
        if self.agent_model_cache.contains_key(agent) || self.model_info_pending.contains(agent) {
            return;
        }
        self.model_info_pending.insert(agent.to_string());
        let client = self.client.clone();
        let agent = agent.to_string();
        let tx = self.app_event_tx.clone();
        self.spawn_client_task("agent_model_info", move || async move {
            let info = client
                .agent_model_info(&agent)
                .await
                .ok()
                .map(|view| (view.model, view.context_length));
            tx.send(crate::app_event::AppEvent::ModelInfoLoaded { agent, info });
        });
    }

    /// Helper: spawn a client-call background task on the app runtime.
    /// Mirrors the old `agentik_core::supervise::spawn_safe_on_drop`
    /// pattern used for host calls.
    pub(super) fn spawn_client_task<F, Fut>(&self, name: &str, make: F)
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let task = make();
        agentik_core::supervise::spawn_safe_on_drop(&self.runtime_handle, name, task);
    }
}
