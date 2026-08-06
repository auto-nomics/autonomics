//! # Conductor — the single multiplexing loop that drives a network.
//!
//! The conductor owns all agent relay tasks and multiplexes their events
//! through one channel. When a node finishes its turn (`AgentEvent::Done`),
//! the conductor:
//!
//! 1. Checks termination conditions → stops if any fires.
//! 2. Looks up outgoing edges → forwards the response to downstream nodes.
//!
//! Agents never communicate directly — the conductor is the sole router.
//! This avoids deadlocks in cyclic topologies and keeps the control flow
//! inspectable.

use std::collections::HashMap;
use std::sync::Arc;

use agentik_core::AgentProfile;
use agentik_sdk::types::AgentEvent;
use agentik_sdk::model::Model;
use arc_swap::ArcSwapOption;
use runtime::{AgentHandle, RuntimeHost};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use crate::error::NetworkError;
use crate::event::NetworkEvent;
use crate::spec::{EdgeSpec, EdgeTrigger, NetworkSpec, TerminationSpec};

// ═══════════════════════════════════════════════════════════════════════
// Outcome
// ═══════════════════════════════════════════════════════════════════════

/// The result of a completed network run.
#[derive(Debug, Clone)]
pub enum NetworkOutcome {
    /// A termination condition was triggered.
    Terminated {
        reason: TerminationReason,
        /// Maximum completion count across all nodes (rough round count).
        rounds: usize,
        /// Node that triggered the termination, if applicable.
        final_node: Option<String>,
        /// The final response text from `final_node`, if any.
        final_output: Option<String>,
    },
    /// All agents finished without an explicit termination condition firing.
    /// This happens in acyclic topologies where every node completes once.
    Completed {
        rounds: usize,
    },
}

/// Why the network terminated.
#[derive(Debug, Clone)]
pub enum TerminationReason {
    /// A `Condition` termination matched: the node's response contained
    /// the pattern.
    Condition { node: String, pattern: String },
    /// A `MaxRounds` termination triggered.
    MaxRounds { max: usize },
    /// An `AnyNodeDone` termination triggered.
    NodeDone { node: String },
}

// ═══════════════════════════════════════════════════════════════════════
// Internal relay types
// ═══════════════════════════════════════════════════════════════════════

/// An `AgentEvent` tagged with the name of the node that produced it.
struct TaggedEvent {
    node: String,
    event: AgentEvent,
}

/// Commands sent from the conductor to a per-node relay task.
enum NodeCommand {
    /// Inject a user message into the agent.
    Message(String),
    /// Shut the agent down (sent when the network terminates).
    Shutdown,
}

/// Internal handle for one node's relay task.
struct NodeEntry {
    /// Send commands to this node's relay (which calls
    /// `AgentHandle::send_message` / `shutdown`).
    cmd_tx: UnboundedSender<NodeCommand>,
    /// Join handle for the relay task — kept alive so the task isn't dropped.
    _relay_task: JoinHandle<()>,
}

// ═══════════════════════════════════════════════════════════════════════
// AgentNetwork
// ═══════════════════════════════════════════════════════════════════════

/// A running multi-agent network.
///
/// Built from a [`NetworkSpec`] via [`AgentNetwork::build`]. Once built,
/// call [`AgentNetwork::run`] to drive the conductor loop. The conductor
/// blocks until a termination condition fires or all agents go idle.
///
/// The struct is **not** `Clone` — it owns the relay tasks and the
/// multiplexed event receiver.
pub struct AgentNetwork {
    spec: NetworkSpec,
    /// Per-node command channels and relay task handles.
    nodes: HashMap<String, NodeEntry>,
    /// Multiplexed event stream from all relay tasks.
    tagged_rx: UnboundedReceiver<TaggedEvent>,
    /// External observer channel (TUI, logging).
    observer_tx: UnboundedSender<NetworkEvent>,
    /// Accumulated LLM response text per node, cleared on each `Done`.
    response_buffers: HashMap<String, String>,
    /// How many times each node has completed a turn (emitted `Done`).
    completion_counts: HashMap<String, usize>,
}

impl AgentNetwork {
    // ── Build ──────────────────────────────────────────────

    /// Build a running network from a spec.
    ///
    /// - `host` — the [`RuntimeHost`] that owns shared infrastructure.
    /// - `global_model` — the default model for all agents. Agents with a
    ///   `preferred_model` in their profile will get a dedicated slot.
    /// - `profiles` — a map from profile name to `AgentProfile`. Every
    ///   `NodeSpec::profile` must resolve to an entry here.
    /// - `observer_tx` — receives [`NetworkEvent`]s for external display.
    pub async fn build(
        spec: NetworkSpec,
        host: &RuntimeHost,
        global_model: Arc<ArcSwapOption<Model>>,
        profiles: HashMap<String, AgentProfile>,
        observer_tx: UnboundedSender<NetworkEvent>,
    ) -> Result<Self, NetworkError> {
        spec.validate().map_err(NetworkError::Topology)?;

        let (tagged_tx, tagged_rx) = mpsc::unbounded_channel::<TaggedEvent>();
        let mut nodes = HashMap::new();

        for node_spec in &spec.nodes {
            let profile = profiles
                .get(&node_spec.profile)
                .ok_or_else(|| NetworkError::ProfileNotFound(node_spec.profile.clone()))?
                .clone();

            let handle = host
                .spawn_agent(
                    &node_spec.name,
                    &profile,
                    global_model.clone(),
                    None, // P0: no per-agent model override
                )
                .await?;

            // Spawn the relay task that owns this AgentHandle.
            let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<NodeCommand>();
            let relay = NodeRelay {
                handle,
                cmd_rx,
                tagged_tx: tagged_tx.clone(),
                node_name: node_spec.name.clone(),
            };
            let relay_task = tokio::spawn(relay.run());

            nodes.insert(
                node_spec.name.clone(),
                NodeEntry {
                    cmd_tx,
                    _relay_task: relay_task,
                },
            );
        }

        // Inject initial prompts for nodes that have them.
        let network = Self {
            spec,
            nodes,
            tagged_rx,
            observer_tx,
            response_buffers: HashMap::new(),
            completion_counts: HashMap::new(),
        };

        // Send initial prompts AFTER the struct is built so we can use
        // `send_to_node`.
        let initial_prompts: Vec<(String, String)> = network
            .spec
            .nodes
            .iter()
            .filter_map(|n| {
                n.initial_prompt
                    .as_ref()
                    .map(|p| (n.name.clone(), p.clone()))
            })
            .collect();

        for (name, prompt) in initial_prompts {
            network.send_to_node(&name, prompt);
        }

        let _ = network.observer_tx.send(NetworkEvent::Started {
            name: network.spec.name.clone(),
            node_count: network.spec.nodes.len(),
        });

        Ok(network)
    }

    // ── Run loop ───────────────────────────────────────────

    /// Drive the conductor loop until termination.
    ///
    /// Consumes `self` — after the network terminates, agent handles are
    /// shut down and the struct is dropped.
    pub async fn run(mut self) -> NetworkOutcome {
        while let Some(tagged) = self.tagged_rx.recv().await {
            let node = tagged.node.clone();
            match tagged.event {
                AgentEvent::LlmResponse(text) => {
                    self.response_buffers
                        .entry(node.clone())
                        .or_default()
                        .push_str(&text);
                    let _ = self.observer_tx.send(NetworkEvent::AgentText {
                        node: node.clone(),
                        text,
                    });
                }

                AgentEvent::Done => {
                    let response = self
                        .response_buffers
                        .remove(&node)
                        .unwrap_or_default();

                    // Count completions.
                    let count = self
                        .completion_counts
                        .entry(node.clone())
                        .or_insert(0);
                    *count += 1;
                    let max_rounds =
                        *self.completion_counts.values().max().unwrap_or(&0);

                    // Check termination BEFORE routing.
                    if let Some(reason) =
                        self.check_termination(&node, &response, max_rounds)
                    {
                        let _ = self.observer_tx.send(NetworkEvent::Finished {
                            reason: format!("{reason:?}"),
                            rounds: max_rounds,
                            final_node: Some(node.clone()),
                        });
                        self.shutdown_all();
                        return NetworkOutcome::Terminated {
                            reason,
                            rounds: max_rounds,
                            final_node: Some(node),
                            final_output: Some(response),
                        };
                    }

                    // Route to downstream nodes.
                    self.route_output(&node, &response, max_rounds).await;
                }

                AgentEvent::ToolCall { name, .. } => {
                    let _ = self.observer_tx.send(NetworkEvent::ToolCall {
                        node: node.clone(),
                        tool: name,
                    });
                }

                AgentEvent::ToolResult { ok, .. } => {
                    let _ = self.observer_tx.send(NetworkEvent::ToolResult {
                        node: node.clone(),
                        ok,
                    });
                }

                AgentEvent::Error(e) => {
                    tracing::warn!(node = %node, error = %e, "agent error in network");
                    let _ = self.observer_tx.send(NetworkEvent::AgentError {
                        node: node.clone(),
                        error: e,
                    });
                }

                _ => {
                    // Streaming deltas, session events, etc. — not relevant
                    // to the conductor's routing logic.
                }
            }
        }

        // All relay tasks exited — all agents are done.
        let max_rounds = *self.completion_counts.values().max().unwrap_or(&0);
        let _ = self.observer_tx.send(NetworkEvent::Finished {
            reason: "all agents idle".into(),
            rounds: max_rounds,
            final_node: None,
        });
        NetworkOutcome::Completed {
            rounds: max_rounds,
        }
    }

    // ── Internal helpers ───────────────────────────────────

    /// Send a message to a node (forwarded via the relay task).
    fn send_to_node(&self, node: &str, text: String) {
        if let Some(entry) = self.nodes.get(node) {
            let _ = entry.cmd_tx.send(NodeCommand::Message(text));
        }
    }

    /// Check whether any termination condition is satisfied.
    fn check_termination(
        &self,
        node: &str,
        response: &str,
        max_rounds: usize,
    ) -> Option<TerminationReason> {
        check_termination_spec(&self.spec.termination, node, response, max_rounds)
    }

    /// Forward a node's response to all downstream nodes via outgoing edges.
    async fn route_output(&mut self, from: &str, response: &str, round: usize) {
        // Collect edges to fire (avoid borrowing self during send).
        let fires: Vec<&EdgeSpec> = self
            .spec
            .edges
            .iter()
            .filter(|e| e.from == from)
            .filter(|e| edge_should_fire(e, response))
            .collect();

        for edge in fires {
            let message = match &edge.transform {
                Some(t) => t.render(response, round, &edge.from, &edge.to),
                None => response.to_string(),
            };

            let _ = self.observer_tx.send(NetworkEvent::MessageRouted {
                from: edge.from.clone(),
                to: edge.to.clone(),
                round,
            });

            self.send_to_node(&edge.to, message);
        }
    }

    /// Shut down all node relay tasks.
    fn shutdown_all(&self) {
        for entry in self.nodes.values() {
            let _ = entry.cmd_tx.send(NodeCommand::Shutdown);
        }
    }
}

impl Drop for AgentNetwork {
    fn drop(&mut self) {
        // Best-effort shutdown if `run()` wasn't called or returned early.
        for entry in self.nodes.values() {
            let _ = entry.cmd_tx.send(NodeCommand::Shutdown);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Free functions — pure logic (testable without agents)
// ═══════════════════════════════════════════════════════════════════════

/// Evaluate a [`TerminationSpec`] against the current state.
fn check_termination_spec(
    spec: &TerminationSpec,
    node: &str,
    response: &str,
    max_rounds: usize,
) -> Option<TerminationReason> {
    match spec {
        TerminationSpec::AnyNodeDone { nodes } => {
            if nodes.iter().any(|n| n == node) {
                Some(TerminationReason::NodeDone {
                    node: node.to_string(),
                })
            } else {
                None
            }
        }
        TerminationSpec::Condition { node: cond_node, pattern } => {
            if node == cond_node
                && response.to_lowercase().contains(&pattern.to_lowercase())
            {
                Some(TerminationReason::Condition {
                    node: node.to_string(),
                    pattern: pattern.clone(),
                })
            } else {
                None
            }
        }
        TerminationSpec::MaxRounds { max } => {
            if max_rounds >= *max {
                Some(TerminationReason::MaxRounds { max: *max })
            } else {
                None
            }
        }
        TerminationSpec::All { specs } => {
            let reasons: Vec<TerminationReason> = specs
                .iter()
                .filter_map(|s| check_termination_spec(s, node, response, max_rounds))
                .collect();
            if reasons.len() == specs.len() && !reasons.is_empty() {
                reasons.into_iter().next()
            } else {
                None
            }
        }
        TerminationSpec::Any { specs } => specs
            .iter()
            .find_map(|s| check_termination_spec(s, node, response, max_rounds)),
    }
}

/// Check whether an edge's trigger condition is met.
fn edge_should_fire(edge: &EdgeSpec, response: &str) -> bool {
    match &edge.trigger {
        EdgeTrigger::OnDone => true, // We're already in the Done handler.
        EdgeTrigger::OnPattern { pattern } => {
            response.to_lowercase().contains(&pattern.to_lowercase())
        }
        EdgeTrigger::OnToolCall { .. } => {
            // P2: not yet wired — OnToolCall requires tracking tool-call
            // events per node and matching against the tool name. For now,
            // treat as always-fire (same as OnDone).
            tracing::warn!(
                from = %edge.from,
                to = %edge.to,
                "OnToolCall trigger not yet implemented, treating as OnDone"
            );
            true
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// NodeRelay — per-agent event relay task
// ═══════════════════════════════════════════════════════════════════════

/// A background task that owns one [`AgentHandle`] and relays:
/// - **Inbound**: `NodeCommand`s from the conductor → `send_message`/`shutdown`
/// - **Outbound**: `AgentEvent`s from the agent → `TaggedEvent`s to conductor
///
/// This indirection exists because `AgentHandle` is a monolithic struct —
/// the conductor can't hold `&mut` references to multiple handles
/// simultaneously in a `select!`. The relay task bridges that gap.
struct NodeRelay {
    handle: AgentHandle,
    cmd_rx: UnboundedReceiver<NodeCommand>,
    tagged_tx: UnboundedSender<TaggedEvent>,
    node_name: String,
}

impl NodeRelay {
    async fn run(mut self) {
        let node_name = self.node_name.clone();
        tracing::debug!(node = %node_name, "relay task started");

        loop {
            tokio::select! {
                biased;

                // Forward agent events to the conductor.
                event = self.handle.recv_event() => match event {
                    Some(ev) => {
                        if self.tagged_tx.send(TaggedEvent {
                            node: node_name.clone(),
                            event: ev,
                        }).is_err() {
                            tracing::debug!(
                                node = %node_name,
                                "conductor channel closed, relay exiting"
                            );
                            break;
                        }
                    }
                    None => {
                        tracing::debug!(
                            node = %node_name,
                            "agent event stream closed, relay exiting"
                        );
                        break;
                    }
                },

                // Forward conductor commands to the agent.
                cmd = self.cmd_rx.recv() => match cmd {
                    Some(NodeCommand::Message(text)) => {
                        self.handle.send_message(text);
                    }
                    Some(NodeCommand::Shutdown) | None => {
                        tracing::debug!(
                            node = %node_name,
                            "shutdown command received, relay exiting"
                        );
                        self.handle.shutdown();
                        break;
                    }
                }
            }
        }

        tracing::debug!(node = %node_name, "relay task exited");
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Tests — pure logic (no agents needed)
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_condition_termination_match() {
        let spec = TerminationSpec::Condition {
            node: "reviewer".into(),
            pattern: "ACCEPT".into(),
        };
        let reason = check_termination_spec(
            &spec,
            "reviewer",
            "My verdict: <verdict>ACCEPT</verdict>",
            3,
        );
        assert!(matches!(
            reason,
            Some(TerminationReason::Condition { .. })
        ));
    }

    #[test]
    fn test_condition_termination_no_match() {
        let spec = TerminationSpec::Condition {
            node: "reviewer".into(),
            pattern: "ACCEPT".into(),
        };
        let reason =
            check_termination_spec(&spec, "reviewer", "needs revision", 3);
        assert!(reason.is_none());
    }

    #[test]
    fn test_condition_termination_wrong_node() {
        let spec = TerminationSpec::Condition {
            node: "reviewer".into(),
            pattern: "ACCEPT".into(),
        };
        let reason =
            check_termination_spec(&spec, "researcher", "ACCEPT", 3);
        assert!(reason.is_none());
    }

    #[test]
    fn test_condition_case_insensitive() {
        let spec = TerminationSpec::Condition {
            node: "rev".into(),
            pattern: "accept".into(),
        };
        let reason =
            check_termination_spec(&spec, "rev", "I ACCEPT this paper", 1);
        assert!(reason.is_some());
    }

    #[test]
    fn test_max_rounds_termination() {
        let spec = TerminationSpec::MaxRounds { max: 3 };
        assert!(check_termination_spec(&spec, "any", "x", 2).is_none());
        assert!(check_termination_spec(&spec, "any", "x", 3).is_some());
        assert!(check_termination_spec(&spec, "any", "x", 4).is_some());
    }

    #[test]
    fn test_any_node_done_termination() {
        let spec = TerminationSpec::AnyNodeDone {
            nodes: vec!["final".into()],
        };
        assert!(check_termination_spec(&spec, "final", "done", 1).is_some());
        assert!(check_termination_spec(&spec, "middle", "done", 1).is_none());
    }

    #[test]
    fn test_any_composite_termination() {
        let spec = TerminationSpec::Any {
            specs: vec![
                TerminationSpec::Condition {
                    node: "reviewer".into(),
                    pattern: "ACCEPT".into(),
                },
                TerminationSpec::MaxRounds { max: 5 },
            ],
        };

        // Condition matches.
        assert!(check_termination_spec(
            &spec,
            "reviewer",
            "ACCEPT",
            1
        )
        .is_some());

        // MaxRounds matches.
        assert!(check_termination_spec(
            &spec,
            "researcher",
            "draft",
            5
        )
        .is_some());

        // Neither matches.
        assert!(check_termination_spec(
            &spec,
            "reviewer",
            "REJECT",
            2
        )
        .is_none());
    }

    #[test]
    fn test_all_composite_termination() {
        let spec = TerminationSpec::All {
            specs: vec![
                TerminationSpec::Condition {
                    node: "a".into(),
                    pattern: "done".into(),
                },
                TerminationSpec::MaxRounds { max: 3 },
            ],
        };

        // Only condition matches → All is NOT satisfied.
        assert!(check_termination_spec(&spec, "a", "done", 1).is_none());

        // Both match → All is satisfied.
        assert!(check_termination_spec(&spec, "a", "done", 3).is_some());
    }

    #[test]
    fn test_edge_on_done_always_fires() {
        let edge = EdgeSpec {
            from: "a".into(),
            to: "b".into(),
            trigger: EdgeTrigger::OnDone,
            transform: None,
        };
        assert!(edge_should_fire(&edge, "anything"));
    }

    #[test]
    fn test_edge_on_pattern() {
        let edge = EdgeSpec {
            from: "a".into(),
            to: "b".into(),
            trigger: EdgeTrigger::OnPattern {
                pattern: "ready".into(),
            },
            transform: None,
        };
        assert!(edge_should_fire(&edge, "I am ready"));
        assert!(edge_should_fire(&edge, "READY"));
        assert!(!edge_should_fire(&edge, "not yet"));
    }
}
