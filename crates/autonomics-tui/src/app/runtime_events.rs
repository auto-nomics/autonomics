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
                    .info("Gateway stream lag", Some("Re-syncing state…".into()));

                self.spawn_reconcile("lag");
            }
            GatewayFrame::Disconnected => {
                self.state
                    .toasts
                    .info("Gateway disconnected", Some("Reconnecting…".into()));
            }
            GatewayFrame::Reconnected => {
                self.state
                    .toasts
                    .info("Gateway reconnected", Some("Re-syncing state…".into()));

                self.spawn_reconcile("reconnect");
            }
        }
    }

    /// Translate a daemon-side lifecycle notice into the matching
    /// application event.
    fn handle_gateway_notice(&mut self, notice: gateway::proto::GatewayNotice) {
        use gateway::proto::GatewayNotice;
        match notice {
            // Compute-target changes refresh the daemon-side registry and
            // reach live agents through the compute context provider; the
            // TUI has no compute panel, so no local action is required.
            GatewayNotice::ComputeChanged { .. } => {}
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
        let Some(target_idx) = route_target(
            &self.state.sessions,
            &agent_name,
            self.state.active_agent_idx,
        ) else {
            // The named identity has no open leaf (tool-spawned child, a
            // closed tab, or a yet-unregistered agent). Its events belong
            // to no view — never bleed them into the active agent's
            // window (message/status cross-talk).
            tracing::debug!(
                agent = %agent_name,
                "dropping agent event for identity without an open leaf"
            );
            return;
        };

        let is_context_list = matches!(event, AgentEvent::ContextList { .. });
        if matches!(
            event,
            AgentEvent::ContextActivated { .. } | AgentEvent::ContextList { .. }
        ) {
            // The backend restore emits no PlanUpdate for the activated
            // session's plan, so fetch it here (guarded by plan_requested).
            let activated = match &event {
                AgentEvent::ContextActivated { id, .. } => Some(*id),
                _ => None,
            };
            state::apply_context_event(&mut self.state, event, target_idx);
            if let Some(session_id) = activated {
                self.spawn_plan_load_for(target_idx, session_id);
            }
        } else {
            // Plans belong to the agent's sole context; route a PlanUpdate
            // naming a foreign id (stale frame) to the active context —
            // never dropped.
            let tab_state =
                self.state
                    .sessions
                    .get_mut(target_idx)
                    .map(|s| match s.context.as_mut() {
                        Some(context) => &mut context.tab_state,
                        None => &mut s.pending_tab_state,
                    });

            if let Some(ts) = tab_state {
                state::apply_event(ts, event);
            }
        }

        // After ContextList arrives, spawn background history loads when
        // the context's tab_state is still empty.
        if is_context_list {
            self.spawn_history_loads_for(target_idx);
        }
    }

    /// Apply an internal [`AppEvent`] to state.
    pub(super) fn handle_app_event(&mut self, event: crate::app_event::AppEvent) {
        match event {
            crate::app_event::AppEvent::AgentRecordsLoaded { query, records } => {
                // Stale-guard: a newer keystroke may have moved the query
                // while this page was in flight.
                if self.state.agents_popup.query() != query {
                    tracing::debug!(%query, current = %self.state.agents_popup.query(), "dropping stale agent records page");
                    return;
                }
                self.state.stored_records = records;
                self.refresh_agents_popup();
                tracing::info!(
                    count = self.state.stored_records.len(),
                    "agent records page applied"
                );
            }
            crate::app_event::AppEvent::SearchStoredAgents(query) => {
                // Only the newest query matters.
                if self.state.agents_popup.query() != query {
                    return;
                }
                self.fetch_stored_agents(&query);
            }
            crate::app_event::AppEvent::AgentDeleted(agent_id) => {
                self.apply_identity_delete(agent_id);
                tracing::info!(%agent_id, "identity removed after deletion");
            }
            crate::app_event::AppEvent::AgentRenamed { agent_id, new_path } => {
                self.apply_identity_rename(agent_id, new_path.clone());
                tracing::info!(%agent_id, new_path = %new_path, "identity renamed in storage");
            }
            crate::app_event::AppEvent::HistoryLoaded {
                agent_id,
                session_id,
                messages,
            } => {
                self.replay_history(agent_id, session_id, &messages);
            }

            crate::app_event::AppEvent::PlanLoaded {
                agent_id,
                session_id,
                plan,
            } => {
                // Route to the agent's sole context. Guard against stale
                // overlays: the loaded revision is only applied if it's
                // newer than what the TUI already has (e.g. the model
                // already called update_plan since resume).
                let session_idx = self
                    .state
                    .sessions
                    .iter()
                    .position(|s| s.agent_id == agent_id);
                let Some(session_idx) = session_idx else {
                    tracing::warn!(%agent_id, "plan loaded for unknown agent");
                    return;
                };

                let agent_session = &mut self.state.sessions[session_idx];

                let ts = match agent_session.context.as_mut() {
                    // Normal case: the announced context.
                    Some(context) if context.id == session_id => &mut context.tab_state,
                    // Ordering race (plan landed before the context was
                    // announced) — buffer into the pending state.
                    _ => &mut agent_session.pending_tab_state,
                };
                if plan.revision > ts.plan.revision {
                    ts.plan = state::PlanState {
                        steps: plan.update.plan,
                        revision: plan.revision,
                    };
                    self.dirty = true;

                    tracing::info!(
                        %agent_id,
                        %session_id,
                        revision = plan.revision,
                        "restored context plan to TUI from storage"
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

            crate::app_event::AppEvent::EffortApplied { agent, label, .. } => {
                tracing::info!(agent = %agent, effort = %label, "effort override applied");
            }

            crate::app_event::AppEvent::ModelCatalogLoaded(catalog) => {
                Self::load_model_config(&catalog, &mut self.state.model_config_state);
                if let Some(spec) = catalog.active_model.clone() {
                    self.state.active_model_spec = Some(spec.clone());
                    if let Some((provider, name)) = spec.rsplit_once(':') {
                        self.state.active_model_display = catalog
                            .models
                            .iter()
                            .find(|m| m.provider_name == provider && m.model_name == name)
                            .map(|m| state::ModelDisplay {
                                spec,
                                context_length: m.context_length,
                                reasoning_effort: None,
                            });
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
                // Authorize URL: copy to the clipboard (keeping the
                // lease) and best-effort open the browser.

                match crate::clipboard_copy::copy_to_clipboard(&url) {
                    Ok(lease) => {
                        self.clipboard_lease = lease;
                        let opened = Self::open_in_browser(&url);
                        let hint = if opened {
                            "Authorize URL copied; finish OpenAI authorization in the opened browser"
                        } else {
                            "Authorize URL copied to the clipboard; paste it into a browser to authorize"
                        };
                        tracing::info!(
                            "chatgpt login: authorize url dispatched (browser={opened})"
                        );
                        self.state
                            .toasts
                            .info("ChatGPT login", Some(hint.to_string()));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "chatgpt login url clipboard copy failed");

                        self.state.toasts.error(
                            "Copy failed",
                            Some(format!("{e}; the authorize URL is in the log")),
                        );

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
                        "ChatGPT login succeeded",
                        Some(format!(
                            "{} ({})",
                            info.email.as_deref().unwrap_or("email unknown"),
                            info.plan_type.as_deref().unwrap_or("plan unknown"),
                        )),
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, "chatgpt login failed");

                    self.state.toasts.error("ChatGPT login failed", Some(e));
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
            crate::app_event::AppEvent::FlowViewLoaded(result) => match result {
                Ok(view) => {
                    self.state.flow_view = Some(view);
                    self.state.flow_view_error = None;
                }
                Err(error) => {
                    self.state.flow_view_error = Some(error);
                }
            },
            crate::app_event::AppEvent::HpcConfigLoaded(result) => match result {
                Ok(reply) => self.apply_hpc_config_reply(reply),
                Err(error) => self.apply_hpc_config_failed(error),
            },
            crate::app_event::AppEvent::HpcTargetsLoaded(result) => {
                let panel = &mut self.state.hpc_panel;
                panel.loading = false;
                match result {
                    Ok(status) => {
                        let selected = status["state"]["policy"]["target_id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        let mut rows = Vec::new();
                        if let Some(targets) = status["state"]["targets"].as_array() {
                            for target in targets {
                                let backend = match target["backend"]["type"].as_str() {
                                    Some("slurm") => "slurm",
                                    Some("ssh") => "ssh",
                                    _ => "local",
                                };
                                let mut endpoints = Vec::new();
                                if let Some(map) = target["endpoints"].as_object() {
                                    for (alias, endpoint) in map {
                                        endpoints.push((
                                            alias.clone(),
                                            endpoint["hostname"]
                                                .as_str()
                                                .unwrap_or("?")
                                                .to_string(),
                                            endpoint["port"].as_u64().unwrap_or_default() as u16,
                                        ));
                                    }
                                }
                                let domains = target["data_domains"]
                                    .as_array()
                                    .map(|list| {
                                        list.iter()
                                            .filter_map(|domain| {
                                                let id = domain["id"].as_str()?;
                                                let prefix = domain["vfs_prefix"].as_str()?;
                                                let root = domain["remote_root"].as_str()?;
                                                Some(format!("{id} → {prefix} ({root})"))
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                rows.push(crate::widgets::hpc_panel_widget::HpcTargetRow {
                                    id: target["id"].as_str().unwrap_or("?").to_string(),
                                    revision: target["revision"].as_u64().unwrap_or_default(),
                                    selected: target["id"].as_str() == Some(selected.as_str()),
                                    enabled: target["enabled"].as_bool().unwrap_or(false),
                                    backend: backend.to_string(),
                                    endpoints,
                                    domains,
                                });
                            }
                        }
                        panel.targets = rows;
                        let mut mounts = Vec::new();
                        if let Some(list) = status["mounts"].as_array() {
                            for mount in list {
                                mounts.push(crate::widgets::hpc_panel_widget::HpcMountRow {
                                    path: mount["path"].as_str().unwrap_or("?").into(),
                                    backend: mount["backend"].as_str().unwrap_or("?").into(),
                                    read_only: mount["read_only"].as_bool().unwrap_or(false),
                                    domain: mount["domain"].as_str().map(str::to_string),
                                });
                            }
                        }
                        panel.mounts = mounts;
                        self.dirty = true;
                    }
                    Err(error) => {
                        panel.status = Some(format!("targets: {error}"));
                        self.dirty = true;
                    }
                }
            }
            crate::app_event::AppEvent::HpcJobsLoaded(reply) => {
                self.apply_hpc_jobs_reply(reply);
            }
            crate::app_event::AppEvent::HpcProfileSaved(result) => match result {
                Ok(reply) => self.apply_hpc_save_reply(reply),
                Err(error) => self.apply_hpc_save_failed(error),
            },
            crate::app_event::AppEvent::VfsListingLoaded(result) => {
                self.apply_vfs_listing(result);
            }
            crate::app_event::AppEvent::VfsTextLoaded(result) => {
                self.apply_vfs_text(result);
            }
            crate::app_event::AppEvent::VfsMountsLoaded(result) => {
                self.apply_vfs_mounts(result);
            }
            crate::app_event::AppEvent::VfsMutationDone(result) => {
                self.apply_vfs_mutation(result);
            }
            crate::app_event::AppEvent::BibliographySnapshotLoaded { token, result } => {
                self.apply_bibliography_snapshot(token, result);
            }
            crate::app_event::AppEvent::BibliographyDetailsLoaded {
                token,
                article_id,
                result,
            } => {
                self.apply_bibliography_details(token, article_id, result);
            }
            crate::app_event::AppEvent::ModelInfoLoaded { agent, info } => {
                self.model_info_pending.remove(&agent);
                if let Some(info) = info {
                    self.agent_model_cache.insert(agent, info);
                    self.dirty = true;

                    // The cached value is already the provider-exact spec:
                    // with the panel open, let the ● marker self-heal to
                    // the agent's real model.

                    self.sync_model_config_marker();
                }
            }
            crate::app_event::AppEvent::ModelSwapFailed { agent, error } => {
                self.state
                    .toasts
                    .error("Model swap failed", Some(format!("[{agent}] {error}")));
            }
            crate::app_event::AppEvent::SkillLibraryLoaded { library } => match library {
                Ok(library) => {
                    self.state.skill_library.browser.set_library(library);
                    self.fetch_skill_detail();
                }
                Err(error) => self.state.skill_library.browser.error = Some(error),
            },
            crate::app_event::AppEvent::SkillDetailLoaded { name, result } => {
                self.state.skill_library.browser.apply_detail(name, result);
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
            crate::app_event::AppEvent::ModelSaved {
                provider_name,
                result,
            } => match result {
                Ok(()) => {
                    tracing::info!("saved custom model: {provider_name}");
                    self.spawn_catalog_reload();
                    self.state
                        .toasts
                        .success("Custom model saved", Some(provider_name));
                }
                Err(e) => {
                    tracing::error!("failed to save custom model: {e}");
                    self.state.toasts.error("Model save failed", Some(e));
                }
            },
            crate::app_event::AppEvent::ModelDeleted {
                provider_name,
                model_name,
                result,
            } => match result {
                Ok(()) => {
                    tracing::info!("deleted custom model: {provider_name}:{model_name}");
                    self.spawn_catalog_reload();
                    self.state.toasts.success(
                        "Custom model deleted",
                        Some(format!("{provider_name}:{model_name}")),
                    );
                }
                Err(e) => {
                    tracing::error!("failed to delete custom model: {e}");
                    self.state.toasts.error("Model delete failed", Some(e));
                }
            },
            crate::app_event::AppEvent::EmbeddingConfigLoaded { result } => {
                self.handle_embedding_config_event(result, false);
            }
            crate::app_event::AppEvent::EmbeddingConfigSaved { result } => {
                self.handle_embedding_config_event(result, true);
            }
            crate::app_event::AppEvent::AgentConfigLoaded { agent, result } => {
                if self.state.agent_config.visible && self.state.agent_config.agent_path == agent {
                    match result {
                        Ok(view) => self.state.agent_config.loaded(view),
                        Err(error) => {
                            self.state.agent_config.failed();
                            self.state
                                .toasts
                                .error("Agent config load failed", Some(error));
                        }
                    }
                }
            }
            crate::app_event::AppEvent::AgentConfigSaved { agent, result } => {
                if self.state.agent_config.visible && self.state.agent_config.agent_path == agent {
                    match result {
                        Ok(view) => {
                            self.state.agent_config.saved(view);
                            self.state.toasts.success(
                                "Agent config saved",
                                Some(format!("{agent}: runtime settings updated")),
                            );
                            // The reasoning-effort override changes what
                            // the status bar shows — refetch the agent's
                            // model info so it reflects the new setting.
                            self.agent_model_cache.remove(&agent);
                            self.refresh_agent_model_info(&agent);
                        }
                        Err(error) => {
                            self.state.agent_config.failed();
                            self.state
                                .toasts
                                .error("Agent config save failed", Some(error));
                        }
                    }
                }
            }
            crate::app_event::AppEvent::AgentUpserted(info) => {
                let agent_id = info.agent_id.unwrap_or_default();
                self.state
                    .upsert_live_agent(&info.path, agent_id, info.status);
                // The reconcile snapshot never opens leaves on its own —
                // except the very first startup view (see
                // `apply_host_event`).
                if self.state.sessions.is_empty() && !self.state.opened_leaf {
                    let path = info.path.clone();
                    self.open_leaf_for(&path, agent_id);
                }
            }
            crate::app_event::AppEvent::ShowError { title, detail } => {
                self.state.toasts.error(title, detail);
            }
            crate::app_event::AppEvent::ContextListKnown { agent, context } => {
                self.apply_context_list(&agent, context);
                if let Some(idx) = self.state.sessions.iter().position(|s| s.name == agent) {
                    self.spawn_history_loads_for(idx);
                }
            }
        }
    }

    /// Apply a [`gateway::proto::HostEventView`] (agent registered /
    /// unregistered / status changed) by keeping the TUI's live-registry
    /// mirror in sync with the daemon's agent registry. Open leaves are
    /// view windows over identities: they never open on registration
    /// (except the user's own spawn/resume or the initial view) and never
    /// close on unregistration — a passivated agent's leaf simply goes
    /// dormant until a message re-activates it.
    pub(super) fn apply_host_event(&mut self, event: gateway::proto::HostEventView) {
        match event {
            gateway::proto::HostEventView::AgentRegistered { path, info, cause } => {
                let name = path.clone();
                let agent_id =
                    registered_agent_id(&gateway::proto::HostEventView::AgentRegistered {
                        path: path.clone(),
                        info: info.clone(),
                        cause,
                    })
                    .unwrap_or_else(uuid::Uuid::new_v4);
                self.state.upsert_live_agent(&name, agent_id, info.status);

                // The cause replaces registry-state guessing: only a
                // user-initiated spawn/resume may steal focus (and only
                // when this TUI asked for it); a wake-up hydrates an open
                // leaf; a tool child stays registry-only.
                let steal_focus = cause == gateway::proto::ActivationCauseView::UserInitiated
                    && self
                        .state
                        .pending_focus_agent_name
                        .as_deref()
                        .is_some_and(|wanted| wanted == info.name);
                if steal_focus {
                    self.state.pending_focus_agent_name = None;
                }

                if let Some(session) = self.state.sessions.iter_mut().find(|s| s.name == name) {
                    // An open leaf already covers this identity (e.g. a
                    // dormant view re-activated by a wake-up): refresh its
                    // identity binding and hydrate the context.
                    session.agent_id = agent_id;
                    self.hydrate_leaf(&name);
                    if steal_focus {
                        let idx = self
                            .state
                            .sessions
                            .iter()
                            .position(|s| s.name == name)
                            .expect("leaf exists");
                        self.state.active_agent_idx = idx;
                        tracing::info!(agent = %name, leaf_idx = idx, "stole focus to user-restored leaf");
                    }
                } else if steal_focus {
                    // User-initiated spawn/resume: open a leaf and focus it.
                    let idx = self.open_leaf_for(&name, agent_id);
                    self.state.active_agent_idx = idx;
                    self.dirty = true;
                    tracing::info!(agent = %name, leaf_idx = idx, "opened leaf for user spawn");
                } else if cause != gateway::proto::ActivationCauseView::ToolChild
                    && self.state.sessions.is_empty()
                    && !self.state.opened_leaf
                {
                    // Initial view: the very first live agent seen at
                    // startup becomes the starting leaf so the workspace
                    // is never blank; everything after this is silent.
                    self.open_leaf_for(&name, agent_id);
                } else {
                    // Tool-spawned child or wake-up without an open leaf:
                    // registry-only, completely silent.
                    tracing::info!(agent = %name, ?cause, "daemon agent registered (registry-only)");
                }
            }
            gateway::proto::HostEventView::AgentUnregistered { path, cause } => {
                // The live instance went away. The leaf stays: it is a view
                // over the identity, which survives in storage and
                // re-activates on the next message (idle passivation,
                // explicit shutdown) — except `evicted`, where the
                // identity event that follows owns the view teardown.
                self.state.remove_live_agent(&path);
                self.agent_model_cache.remove(&path);
                tracing::info!(agent = %path, ?cause, "daemon agent left the registry — leaf kept");
            }
            gateway::proto::HostEventView::IdentityRenamed {
                agent_id,
                old_path: _,
                new_path,
            } => {
                // Cross-client rename: every connected frontend receives
                // this, not just the one that issued the PATCH.
                if let Ok(path) = agentik_types::AgentPath::try_from(new_path.as_str()) {
                    self.apply_identity_rename(agent_id, path);
                } else {
                    tracing::warn!(%agent_id, path = %new_path, "identity renamed to an invalid path");
                }
            }
            gateway::proto::HostEventView::IdentityDeleted { agent_id, path: _ } => {
                self.apply_identity_delete(agent_id);
            }
            // Cross-agent runtime status update. Refreshes the
            // live-registry mirror (the unified picker reads it); the
            // per-session `AgentTabState.status` keeps being driven by
            // the agent's own `LifecycleChanged` event stream (via the
            // session tab), which has finer-grained variants.
            gateway::proto::HostEventView::AgentStatusChanged {
                path,
                status,
                last_event,
            } => {
                self.state.set_live_status(&path, status);
                tracing::debug!(
                    agent = %path,
                    status = %status.tag(),
                    last_event = ?last_event,
                    "daemon observed agent status change"
                );
            }

            // Per-agent model hot-swap observed on the daemon. The TUI's
            // model chip refreshes via its own `agent_model_info` polling,
            // so this arm is a log-only bridge.
            gateway::proto::HostEventView::AgentModelChanged {
                path,
                spec,
                context_length,
            } => {
                tracing::debug!(                    agent = %path,                    model = %spec,                    context = context_length,                    "daemon observed agent model change"                );
            }
        }
        // Any registry movement (registration, passivation, status) is
        // visible in an open unified popup — rebuild its rows in place.
        self.refresh_agents_popup();
    }

    /// Fold an identity rename into every TUI view: the stored record,
    /// any open leaf (rebound by identity id, not path), and the model
    /// render cache. Shared by the local PATCH reply (`AgentRenamed`)
    /// and the daemon-broadcast `IdentityRenamed` host event, which is
    /// how OTHER connected frontends' renames reach this one.
    fn apply_identity_rename(&mut self, agent_id: uuid::Uuid, new_path: agentik_types::AgentPath) {
        if let Some(record) = self
            .state
            .stored_records
            .iter_mut()
            .find(|record| record.id == agent_id)
        {
            record.name = new_path.as_str().to_string();
        }
        // Rebind any open leaf to the renamed path: a leaf is a view
        // over the identity (agent_id), not over a path snapshot.
        // Without this the leaf keeps routing events for its old
        // name forever — post-rename frames arrive under the new
        // path and would be dropped.
        if let Some(old_name) =
            rebind_leaf_name(&mut self.state.sessions, agent_id, new_path.as_str())
        {
            if let Some(display) = self.agent_model_cache.remove(&old_name) {
                self.agent_model_cache
                    .insert(new_path.as_str().to_string(), display);
            }
            self.dirty = true;
            tracing::info!(%agent_id, from = %old_name, "open leaf rebound to renamed path");
        }
        self.refresh_agents_popup();
    }

    /// Fold an identity deletion into every TUI view: drop the stored
    /// record and close any open leaf — all stored data has been erased
    /// so there is nothing left to view or resume. Shared by the local
    /// DELETE reply (`AgentDeleted`) and the daemon-broadcast
    /// `IdentityDeleted` host event.
    fn apply_identity_delete(&mut self, agent_id: uuid::Uuid) {
        self.state
            .stored_records
            .retain(|record| record.id != agent_id);
        self.close_agent_leaf_by_id(agent_id);
        self.refresh_agents_popup();
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

        let Some(context) = session.context.iter_mut().find(|c| c.id == session_id) else {
            tracing::warn!(%session_id, "history loaded for unknown context");
            return;
        };

        if !context.tab_state.messages.is_empty() {
            tracing::debug!(%session_id, "tab_state already populated, skipping");
            return;
        }

        context.tab_state.set_messages(lines);
        context.tab_state.scroll_to_bottom();

        self.dirty = true;

        tracing::info!(%session_id, count = context.tab_state.messages.len(), "history replayed");
    }

    /// Fold the agent's sole context (from hydration or a `ContextList`
    /// event that arrived before the agent's context existed) into the UI.
    pub(super) fn apply_context_list(&mut self, agent_path: &str, context: Option<ContextInfo>) {
        let Some(agent) = self
            .state
            .sessions
            .iter_mut()
            .find(|s| s.name == agent_path)
        else {
            return;
        };

        // One context per agent; keep a loaded transcript when the id
        // matches.
        let old = agent.context.take();
        agent.context = match context {
            Some(info) => match old {
                Some(old) if old.id == info.id => Some(state::AgentContext::from_info(info, old)),
                _ => Some(state::AgentContext::new(info)),
            },
            None => old,
        };
    }

    /// Spawn background history loads for all sessions of the agent at
    /// `agent_idx` that have empty `tab_state.messages` (plus their plans).
    pub(super) fn spawn_history_loads_for(&mut self, agent_idx: usize) {
        let Some(agent_session) = self.state.sessions.get(agent_idx) else {
            return;
        };
        let agent_id = agent_session.agent_id;

        let to_load: Option<uuid::Uuid> = agent_session
            .context
            .as_ref()
            .filter(|c| c.tab_state.messages.is_empty())
            .map(|c| c.id);

        let plan_to_load: Option<uuid::Uuid> = agent_session
            .context
            .as_ref()
            .filter(|c| !c.plan_requested)
            .map(|c| c.id);

        if let Some(session_id) = to_load {
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

        if let Some(session_id) = plan_to_load {
            self.spawn_plan_load_for(agent_idx, session_id);
        }
    }

    /// Spawn a per-session plan fetch for the sub-session `session_id` of
    /// the agent at `agent_idx`.
    ///
    /// The backend session restore loads the plan from storage but does NOT
    /// emit an `AgentEvent::PlanUpdate`, so the tab's `PlanState` would stay
    /// empty. The sub-session is marked `plan_requested` up front so repeat
    /// hydrations (every `ContextList`) don't re-fetch.
    pub(super) fn spawn_plan_load_for(&mut self, agent_idx: usize, session_id: uuid::Uuid) {
        let Some(agent_session) = self.state.sessions.get(agent_idx) else {
            return;
        };
        let agent_id = agent_session.agent_id;
        let already_requested = agent_session
            .context
            .as_ref()
            .filter(|c| c.id == session_id)
            .is_none_or(|c| c.plan_requested);

        if already_requested {
            return;
        }
        if let Some(agent_session) = self.state.sessions.get_mut(agent_idx) {
            if let Some(context) = agent_session
                .context
                .as_mut()
                .filter(|c| c.id == session_id)
            {
                context.plan_requested = true;
            }
        }
        let client = self.client.clone();
        let tx = self.app_event_tx.clone();
        self.spawn_client_task(&format!("restore_plan::{session_id}"), move || async move {
            if let Ok(Some(plan)) = client.load_plan(agent_id, session_id).await {
                if !plan.is_empty() {
                    tx.send(crate::app_event::AppEvent::PlanLoaded {
                        agent_id,
                        session_id,
                        plan,
                    });
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
                    for (agent_path, context) in snapshot.contexts {
                        tx.send(crate::app_event::AppEvent::ContextListKnown {
                            agent: agent_path,
                            context: Some(context),
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
                .map(|view| state::ModelDisplay {
                    spec: view.model,
                    context_length: view.context_length,
                    reasoning_effort: view.reasoning_effort,
                });
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

/// Pick the session an agent-scoped event applies to.
///
/// `None` means the event belongs to no view and must be dropped — the
/// caller must never fall back to the active tab, or another agent's
/// messages and status bleed into the foreground window.
fn route_target(
    sessions: &[super::AgentSession],
    agent_name: &str,
    active_idx: usize,
) -> Option<usize> {
    if agent_name.is_empty() {
        // Legacy anonymous frames attach to the active agent.
        return Some(active_idx);
    }
    sessions.iter().position(|s| s.name == agent_name)
}

/// Rebind the identity's open leaf to its renamed path. Returns the old
/// name when a leaf moved, `None` when the identity has no open leaf (or
/// already carries the new name — a no-op rename must not churn state).
fn rebind_leaf_name(
    sessions: &mut [super::AgentSession],
    agent_id: uuid::Uuid,
    new_name: &str,
) -> Option<String> {
    let session = sessions.iter_mut().find(|s| s.agent_id == agent_id)?;
    if session.name == new_name {
        return None;
    }
    Some(std::mem::replace(&mut session.name, new_name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str) -> crate::state::AgentSession {
        crate::state::AgentSession {
            name: name.to_string(),
            agent_id: uuid::Uuid::new_v4(),
            context: None,
            pending_tab_state: Default::default(),
        }
    }

    #[test]
    fn named_event_routes_to_its_own_leaf() {
        let sessions = [session("/root/alpha"), session("/root/beta")];
        assert_eq!(route_target(&sessions, "/root/beta", 0), Some(1));
    }

    #[test]
    fn unknown_agent_event_is_dropped_not_rerouted() {
        // Regression: an identity without an open leaf (tool-spawned
        // child, closed tab) used to fall through to the active tab,
        // rendering its messages and status into the foreground agent.
        let sessions = [session("/root/alpha")];
        assert_eq!(route_target(&sessions, "/root/ghost", 0), None);
    }

    #[test]
    fn empty_name_attaches_to_active_agent() {
        let sessions = [session("/root/alpha")];
        assert_eq!(route_target(&sessions, "", 0), Some(0));
    }

    #[test]
    fn rename_rebinds_leaf_by_identity() {
        // Regression: an open leaf kept its old path after the identity was
        // renamed, so all post-rename events were routed to no view (or,
        // before that, bled into the active tab).
        let id = uuid::Uuid::new_v4();
        let mut sessions = [session("/root/alpha")];
        sessions[0].agent_id = id;

        assert_eq!(
            rebind_leaf_name(&mut sessions, id, "/root/beta").as_deref(),
            Some("/root/alpha")
        );
        assert_eq!(sessions[0].name, "/root/beta");

        // A foreign identity's leaf is untouched.
        assert_eq!(
            rebind_leaf_name(&mut sessions, uuid::Uuid::new_v4(), "/root/gamma"),
            None
        );
        assert_eq!(sessions[0].name, "/root/beta");

        // No-op rename (already current) doesn't churn state.
        assert_eq!(rebind_leaf_name(&mut sessions, id, "/root/beta"), None);
    }
}
