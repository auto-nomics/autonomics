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

use agentik_types::{AgentEvent, SessionInfo};
use runtime::{NextEvent, RuntimeHost};
use tokio_util::sync::CancellationToken;

use crate::hub::{EventHub, EventKind};
use crate::proto::HostEventView;

/// Driver-maintained cache of per-agent session lists, folded from
/// `AgentEvent::SessionList` so `GET /state` can answer synchronously
/// (the runtime's `list_sessions` is fire-and-forget with an event reply).
#[derive(Default, Clone)]
pub struct SessionCache {
    sessions: Arc<Mutex<HashMap<String, Vec<SessionInfo>>>>,
}

impl SessionCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one tagged agent event into the cache. Pure state transition,
    /// testable without a host.
    pub fn observe(&self, agent: &str, event: &AgentEvent) {
        let mut map = self.sessions.lock().expect("session cache mutex poisoned");
        match event {
            AgentEvent::SessionList { sessions } => {
                map.insert(agent.to_string(), sessions.clone());
            }
            AgentEvent::SessionClosed { id } => {
                if let Some(list) = map.get_mut(agent) {
                    list.retain(|s| s.id != *id);
                }
            }
            _ => {}
        }
    }

    /// Drop an agent's cache entry (agent unregistered).
    pub fn remove_agent(&self, agent: &str) {
        self.sessions
            .lock()
            .expect("session cache mutex poisoned")
            .remove(agent);
    }

    /// Whether the agent's cached list already contains this session.
    pub fn contains_session(&self, agent: &str, session_id: uuid::Uuid) -> bool {
        self.sessions
            .lock()
            .expect("session cache mutex poisoned")
            .get(agent)
            .is_some_and(|list| list.iter().any(|s| s.id == session_id))
    }

    /// Snapshot for `GET /state`.
    pub fn snapshot(&self) -> HashMap<String, Vec<SessionInfo>> {
        self.sessions
            .lock()
            .expect("session cache mutex poisoned")
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
    sessions: SessionCache,
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
                    // Keep the session cache warm for late-joining
                    // frontends: refresh after a turn completes on a
                    // session the cache hasn't seen. The request must NOT
                    // be sent mid-turn — the session loop swallows
                    // session-management internal events while a turn is
                    // in flight (a pre-existing runtime quirk), so
                    // TurnStarted-triggered requests would be lost.
                    if let AgentEvent::TurnCompleted { session_id, .. } = &event {
                        if !sessions.contains_session(&name, *session_id) {
                            host.control().list_sessions(&name);
                        }
                    }
                    sessions.observe(&name, &event);
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
                            host.control().list_sessions(path.as_str());
                        }
                        runtime::HostEvent::AgentUnregistered { path } => {
                            sessions.remove_agent(path.as_str());
                        }
                        runtime::HostEvent::AgentStatusChanged { .. } => {}
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

    fn session(id: Uuid, title: &str) -> SessionInfo {
        SessionInfo {
            id,
            title: Some(title.into()),
            created_at: 0,
            last_active: 0,
        }
    }

    #[test]
    fn cache_folds_session_list_and_closed() {
        let cache = SessionCache::new();
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        cache.observe(
            "/root/a",
            &AgentEvent::SessionList {
                sessions: vec![session(id1, "one"), session(id2, "two")],
            },
        );
        assert_eq!(cache.snapshot().get("/root/a").map(Vec::len), Some(2));
        cache.observe("/root/a", &AgentEvent::SessionClosed { id: id1 });
        let list = cache.snapshot().get("/root/a").cloned().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id2);
        cache.remove_agent("/root/a");
        assert!(!cache.snapshot().contains_key("/root/a"));
    }
}
