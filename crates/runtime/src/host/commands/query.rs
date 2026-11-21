//! Read-side commands: host status, agent info, persisted history, and
//! keyword task routing.

use std::sync::Arc;

use agentik_core::profile::AgentKind;
use agentik_core::storage::AgentStorage;

use super::super::RuntimeHost;
use super::capability_from_kind;
use crate::control::{AgentExecutionHistory, HostStatus};

impl RuntimeHost {
    /// `HostCommand::GetAgentHistory` — live agents resolve by registry
    /// entry (agent id); anything else falls back to storage lookup by
    /// path.
    pub(super) fn handle_agent_history(
        &mut self,
        agent_name: String,
        limit: usize,
        reply_tx: tokio::sync::oneshot::Sender<AgentExecutionHistory>,
    ) {
        let storage = self.infra.storage.clone();
        let requested_path = agent_name.clone();
        let live = self.resolve_agent(&agent_name).and_then(|path| {
            self.agents
                .get(&path)
                .and_then(|entry| entry.info.agent_id)
                .map(|id| (path, id))
        });
        let _ = agentik_core::supervise::spawn_safe(
            &self.infra.runtime_handle,
            "get_agent_history",
            async move {
                let response = match live {
                    Some((path, agent_id)) => {
                        read_agent_history(storage, agent_id, path, limit).await
                    }
                    None => read_persisted_agent_history(storage, requested_path, limit).await,
                };
                let _ = reply_tx.send(response);
            },
        );
    }

    /// `HostCommand::GetStatus` — registry snapshot plus network
    /// topology summary.
    pub(super) fn handle_get_status(&mut self, reply_tx: tokio::sync::oneshot::Sender<HostStatus>) {
        let g = self.network.graph();
        let status = HostStatus {
            agents: self
                .agents
                .values()
                .map(|e| self.snapshot_info(e))
                .collect(),
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

    /// `HostCommand::GetAgentInfo` — running agents first, then the
    /// hardcoded agent kinds as a fallback.
    pub(super) fn handle_get_agent_info(
        &mut self,
        name: String,
        reply_tx: tokio::sync::oneshot::Sender<Option<crate::control::AgentInfo>>,
    ) {
        let resolved = self.resolve_agent(&name);
        let info = resolved
            .as_ref()
            .and_then(|key| self.agents.get(key).map(|e| self.snapshot_info(e)))
            .or_else(|| {
                AgentKind::from_name(&name).map(|k| capability_from_kind(k.name(), k.name(), k))
            });
        let _ = reply_tx.send(info);
    }

    /// `HostCommand::ListSubAgents` — resolve the agent (live registry
    /// first, storage by path as fallback), then query its direct
    /// children from the persisted identity registry. Dormant
    /// virtual-actor identities are included; liveness is the caller's
    /// join with `GetStatus`.
    pub(super) fn handle_list_sub_agents(
        &mut self,
        agent_name: String,
        reply_tx: tokio::sync::oneshot::Sender<Vec<agentik_core::storage::AgentRecord>>,
    ) {
        // The registry key IS the identity path; fall back to the
        // stored record so a dormant parent still lists its children.
        let parent_path = self
            .resolve_agent(&agent_name)
            .or_else(|| {
                // Short-name/unknown to the live registry: a full path
                // passes through verbatim.
                agentik_types::AgentPath::try_from(agent_name.as_str())
                    .ok()
                    .map(|path| path.as_str().to_string())
            })
            .unwrap_or(agent_name);
        let storage = self.infra.storage.clone();
        let _ = agentik_core::supervise::spawn_safe(
            &self.infra.runtime_handle,
            "list_sub_agents",
            async move {
                let records = storage
                    .list_child_agents(&parent_path)
                    .await
                    .unwrap_or_default();
                let _ = reply_tx.send(records);
            },
        );
    }

    /// Route a task description to the best-matching agent.
    /// Considers the caller's direct children (live agents) plus the
    /// hardcoded agent kinds as spawnable profiles. Live children get a
    /// small bonus score since they're immediately available for
    /// delegation.
    pub(super) fn route_task(
        &self,
        description: &str,
        caller_path: &str,
    ) -> crate::control::RouteResult {
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
    // Identity resolution goes straight to the `agents` table (name IS
    // the full path) — there is no separate persisted agent graph.
    let record = match storage.get_agent_by_name(&requested_path).await {
        Ok(Some(record)) => record,
        _ => {
            return AgentExecutionHistory {
                agent_path: requested_path,
                agent_id: uuid::Uuid::nil(),
                session_id: None,
                messages: Vec::new(),
            };
        }
    };
    read_agent_history(storage, record.id, record.name, limit).await
}

#[cfg(test)]
mod routing_tests {
    use super::super::test_support::{config, register_agent_at};
    use crate::host::RuntimeHost;

    /// ListSubAgents returns exactly the direct children of the named
    /// agent from storage — grandchildren and foreign branches excluded,
    /// and dormant identities (passivated after their record was
    /// upserted) still listed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_sub_agents_returns_direct_children_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = RuntimeHost::open(&config(&dir)).await.unwrap();
        let model: std::sync::Arc<arc_swap::ArcSwapOption<agentik_sdk::model::Model>> =
            std::sync::Arc::new(arc_swap::ArcSwapOption::from_pointee(None));
        for path in ["/root/a", "/root/a/child", "/root/a/child/grand", "/root/b"] {
            register_agent_at(&mut host, path, model.clone()).await;
        }
        // Let the run-loop bootstraps upsert the records.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        // Passivate one child: its identity must survive in the listing.
        host.shutdown_agent("/root/a/child");
        assert!(!host.contains_agent("/root/a/child"));

        let control = host.control();
        let children = control
            .list_sub_agents("/root/a")
            .await
            .expect("host answered");
        let names: Vec<&str> = children.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["/root/a/child"], "grandchild and sibling excluded");
    }

    /// route_task considers only the caller's direct children plus the
    /// spawnable role profiles — never foreign branches or the caller.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn route_task_only_considers_direct_children_and_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = RuntimeHost::open(&config(&dir)).await.unwrap();
        let model: std::sync::Arc<arc_swap::ArcSwapOption<agentik_sdk::model::Model>> =
            std::sync::Arc::new(arc_swap::ArcSwapOption::from_pointee(None));
        for path in ["/root/a", "/root/a/child", "/root/b"] {
            register_agent_at(&mut host, path, model.clone()).await;
        }

        let result = host.route_task("analyze gwas data", "/root/a");
        let names: Vec<&str> = result.candidates.iter().map(|c| c.agent.as_str()).collect();
        assert!(!names.is_empty(), "expected some candidates");
        for name in &names {
            assert!(
                matches!(*name, "child" | "researcher" | "developer" | "hpc-ops"),
                "unexpected candidate '{name}' in {names:?}"
            );
        }
    }
}
