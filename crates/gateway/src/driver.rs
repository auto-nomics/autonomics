//! The daemon's RuntimeHost driver.
//!
//! `RuntimeHost` multiplexing methods (`recv_any` / `recv_event` /
//! `recv_and_process_command`) are `&mut self` and — critically —
//! `recv_any` performs in-band bookkeeping (delegation ledger, topology
//! routing, status derivation) as the event is received. Exactly one task
//! may pump the host; this driver is that task in the daemon, replacing
//! the TUI's event loop (and its pointer-aliasing select hack) as the
//! host's single consumer.
//!
//! Everything it receives is published to the [`EventHub`] for the SSE
//! layer to fan out.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agentik_types::{AgentEvent, ContextInfo};
use runtime::{NextEvent, RuntimeHost};
use tokio_util::sync::CancellationToken;

use crate::hub::{EventHub, EventKind};
use crate::proto::HostEventView;

/// Driver-maintained cache of per-agent context lists, folded from
/// `AgentEvent::ContextList` so `GET /state` can answer synchronously
/// (the runtime's `list_contexts` is fire-and-forget with an event reply).
#[derive(Default, Clone)]
pub struct ContextCache {
    contexts: Arc<Mutex<HashMap<String, Vec<ContextInfo>>>>,
}

impl ContextCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one tagged agent event into the cache. Pure state transition,
    /// testable without a host.
    pub fn observe(&self, agent: &str, event: &AgentEvent) {
        let mut map = self.contexts.lock().expect("context cache mutex poisoned");
        match event {
            AgentEvent::ContextList { contexts } => {
                map.insert(agent.to_string(), contexts.clone());
            }
            _ => {}
        }
    }

    /// Drop an agent's cache entry (agent unregistered).
    pub fn remove_agent(&self, agent: &str) {
        self.contexts
            .lock()
            .expect("context cache mutex poisoned")
            .remove(agent);
    }

    /// Whether the agent's cached list already contains this context.
    pub fn contains_context(&self, agent: &str, context_id: uuid::Uuid) -> bool {
        self.contexts
            .lock()
            .expect("context cache mutex poisoned")
            .get(agent)
            .is_some_and(|list| list.iter().any(|s| s.id == context_id))
    }

    /// Snapshot for `GET /state`.
    pub fn snapshot(&self) -> HashMap<String, Vec<ContextInfo>> {
        self.contexts
            .lock()
            .expect("context cache mutex poisoned")
            .clone()
    }
}

/// Run the host pump until `shutdown` fires. Consumes the host — the
/// caller must not touch it afterwards (server-side state only holds
/// clones of `HostControl` / `SharedInfra`, which stay valid).
///
/// Returns normally on shutdown; the caller then performs graceful agent
/// shutdown (`shutdown_all_agents_and_wait`) on the returned host.
pub async fn run(
    mut host: RuntimeHost,
    hub: EventHub,
    contexts: ContextCache,
    shutdown: CancellationToken,
) -> RuntimeHost {
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::info!("gateway driver: shutdown requested");
                return host;
            }
            next = host.recv_next() => match next {
                NextEvent::Agent((name, event)) => {
                    // Keep the context cache warm for late-joining
                    // frontends: refresh after a turn completes on a
                    // context the cache hasn't seen. The request must NOT
                    // be sent mid-turn — the context loop swallows
                    // context-management internal events while a turn is
                    // in flight (a pre-existing runtime quirk), so
                    // TurnStarted-triggered requests would be lost.
                    if let AgentEvent::TurnCompleted { .. } = &event {
                        host.control().list_contexts(&name);
                    }
                    contexts.observe(&name, &event);
                    hub.publish(EventKind::Agent {
                        agent: name,
                        event,
                    });
                }
                NextEvent::Host(event) => {
                    match &event {
                        runtime::HostEvent::AgentRegistered { path, .. } => {
                            // Ask for the session list so the driver's
                            // cache (and therefore `GET /state`) can
                            // answer for frontends that connect later —
                            // the TUI used to do this on registration;
                            // now the daemon does it once for everyone.
                            host.control().list_contexts(path.as_str());
                        }
                        runtime::HostEvent::AgentUnregistered { path } => {
                            contexts.remove_agent(path.as_str());
                        }
                        runtime::HostEvent::AgentStatusChanged { .. } => {}
                        runtime::HostEvent::AgentModelChanged { .. } => {}
                    }
                    hub.publish(EventKind::Host(HostEventView::from(event)));
                }
                NextEvent::Command => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn context(id: Uuid, title: &str) -> ContextInfo {
        ContextInfo {
            id,
            title: Some(title.into()),
            created_at: 0,
            last_active: 0,
            parent_session_id: None,
            fork_point_message_id: None,
            telemetry: Default::default(),
        }
    }

    #[test]
    fn cache_folds_context_list() {
        let cache = ContextCache::new();
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        cache.observe(
            "/root/a",
            &AgentEvent::ContextList {
                contexts: vec![context(id1, "one"), context(id2, "two")],
            },
        );
        assert_eq!(cache.snapshot().get("/root/a").map(Vec::len), Some(2));
        assert!(cache.contains_context("/root/a", id2));
        cache.remove_agent("/root/a");
        assert!(!cache.snapshot().contains_key("/root/a"));
    }
}
