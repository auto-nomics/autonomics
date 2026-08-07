//! # AgentNetwork — pure topology routing state machine.
//!
//! Takes `(agent_name, &AgentEvent)` as input, produces `Vec<RoutingAction>`
//! as output. No I/O, no async, no handle ownership. The caller (typically a
//! runtime host) is responsible for executing the returned actions.
//!
//! ## Usage pattern
//!
//! ```ignore
//! let mut network = AgentNetwork::new(spec)?;
//!
//! // Inject initial prompts (host does the actual send).
//! for (node, msg) in network.initial_messages() {
//!     host.send_to(&node, msg);
//! }
//!
//! // Event loop driven by the host.
//! while let Some((name, event)) = host.recv_any().await {
//!     for action in network.process_event(&name, &event) {
//!         match action {
//!             RoutingAction::Forward { to, message } => host.send_to(&to, message),
//!             RoutingAction::Finished { .. } => { /* cleanup */ }
//!         }
//!     }
//!     if network.is_finished() { break; }
//! }
//! ```

use std::collections::HashMap;

use agentik_sdk::types::AgentEvent;

use crate::graph::NetworkGraph;
use crate::spec::{EdgeSpec, EdgeTrigger, NetworkSpec, TerminationSpec};

// ═══════════════════════════════════════════════════════════════════════
// RoutingAction — what the network wants the host to do
// ═══════════════════════════════════════════════════════════════════════

/// An action returned by [`AgentNetwork::process_event`] for the host to
/// execute. The network itself has no I/O — it only computes *what* should
/// happen, not *how*.
#[derive(Debug, Clone)]
pub enum RoutingAction {
    /// Forward a message to the named target agent.
    Forward {
        /// Target agent name (must match a node in the spec).
        to: String,
        /// Message text (after edge transform, if any).
        message: String,
    },
    /// The network has terminated. The host should shut down all agents
    /// in the network.
    Finished {
        /// Why the network terminated.
        reason: TerminationReason,
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

/// Summary of a completed network run.
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
    Completed { rounds: usize },
}

// ═══════════════════════════════════════════════════════════════════════
// AgentNetwork — pure routing state machine
// ═══════════════════════════════════════════════════════════════════════

/// A pure topology routing state machine.
///
/// Created from a [`NetworkSpec`] via [`AgentNetwork::new`]. The caller feeds
/// agent events into [`process_event`](Self::process_event) and executes the
/// returned [`RoutingAction`]s.
///
/// This struct does **not** own any agent handles, spawn any tasks, or
/// perform any I/O. It is purely computational.
pub struct AgentNetwork {
    spec: NetworkSpec,
    /// Petgraph-backed structural index for O(out-degree) routing queries.
    graph: NetworkGraph,
    /// Accumulated LLM response text per node, cleared on each `Done`.
    response_buffers: HashMap<String, String>,
    /// How many times each node has completed a turn (emitted `Done`).
    completion_counts: HashMap<String, usize>,
    /// Set when a termination condition fires.
    finished: Option<TerminationReason>,
}

impl AgentNetwork {
    /// Create a routing state machine from a validated spec.
    pub fn new(spec: NetworkSpec) -> Result<Self, String> {
        spec.validate()?;
        let graph = NetworkGraph::from_spec(&spec)?;
        Ok(Self {
            spec,
            graph,
            response_buffers: HashMap::new(),
            completion_counts: HashMap::new(),
            finished: None,
        })
    }

    /// Borrow the petgraph-backed structural index.
    pub fn graph(&self) -> &NetworkGraph {
        &self.graph
    }

    /// Returns the initial prompts that the host should inject to kick-start
    /// the network. Each tuple is `(node_name, prompt_text)`.
    pub fn initial_messages(&self) -> Vec<(String, String)> {
        self.spec
            .nodes
            .iter()
            .filter_map(|n| {
                n.initial_prompt
                    .as_ref()
                    .map(|p| (n.name.clone(), p.clone()))
            })
            .collect()
    }

    /// Borrow the underlying spec.
    pub fn spec(&self) -> &NetworkSpec {
        &self.spec
    }

    /// Maximum completion count across all nodes (rough round count).
    pub fn rounds(&self) -> usize {
        *self.completion_counts.values().max().unwrap_or(&0)
    }

    /// Whether the network has terminated.
    pub fn is_finished(&self) -> bool {
        self.finished.is_some()
    }

    /// Why the network terminated, if it has.
    pub fn termination_reason(&self) -> Option<&TerminationReason> {
        self.finished.as_ref()
    }

    /// Feed an agent event into the routing state machine.
    ///
    /// Returns zero or more [`RoutingAction`]s for the host to execute.
    /// Once [`Finished`](RoutingAction::Finished) is returned, subsequent
    /// calls are no-ops.
    pub fn process_event(&mut self, from: &str, event: &AgentEvent) -> Vec<RoutingAction> {
        if self.finished.is_some() {
            return vec![];
        }

        match event {
            AgentEvent::LlmResponse(text) => {
                self.response_buffers
                    .entry(from.to_string())
                    .or_default()
                    .push_str(text);
                vec![]
            }

            AgentEvent::Done => {
                let response = self
                    .response_buffers
                    .remove(from)
                    .unwrap_or_default();

                // Update completion counts.
                let count = self.completion_counts.entry(from.to_string()).or_insert(0);
                *count += 1;
                let max_rounds = self.rounds();

                // Check termination BEFORE routing.
                if let Some(reason) =
                    check_termination_spec(&self.spec.termination, from, &response, max_rounds)
                {
                    self.finished = Some(reason.clone());
                    vec![RoutingAction::Finished { reason }]
                } else {
                    self.route_output(from, &response, max_rounds)
                }
            }

            // These events don't produce routing actions. The host can
            // observe them independently (e.g. for UI updates).
            _ => vec![],
        }
    }

    /// Compute routing actions for a completed node's response.
    ///
    /// Uses the petgraph index for O(out-degree) edge look-up instead of
    /// scanning all edges.
    fn route_output(&self, from: &str, response: &str, round: usize) -> Vec<RoutingAction> {
        self.graph
            .out_edges(from)
            .into_iter()
            .filter(|edge| edge_should_fire(edge, response))
            .map(|edge| {
                let message = match &edge.transform {
                    Some(t) => t.render(response, round, &edge.from, &edge.to),
                    None => response.to_string(),
                };
                RoutingAction::Forward {
                    to: edge.to.clone(),
                    message,
                }
            })
            .collect()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Free functions — pure logic (testable without agents)
// ═══════════════════════════════════════════════════════════════════════

/// Evaluate a [`TerminationSpec`] against the current state.
pub fn check_termination_spec(
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
        TerminationSpec::Condition {
            node: cond_node,
            pattern,
        } => {
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
pub fn edge_should_fire(edge: &EdgeSpec, response: &str) -> bool {
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
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{EdgeSpec, EdgeTrigger, NetworkSpec, NodeSpec, TerminationSpec};

    fn simple_spec() -> NetworkSpec {
        NetworkSpec {
            name: "test".into(),
            nodes: vec![
                NodeSpec {
                    name: "a".into(),
                    profile: "x".into(),
                    initial_prompt: Some("hello".into()),
                },
                NodeSpec {
                    name: "b".into(),
                    profile: "y".into(),
                    initial_prompt: None,
                },
            ],
            edges: vec![EdgeSpec {
                from: "a".into(),
                to: "b".into(),
                trigger: EdgeTrigger::OnDone,
                transform: None,
            }],
            termination: TerminationSpec::MaxRounds { max: 99 },
        }
    }

    // ── Termination logic tests ──

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

        assert!(check_termination_spec(&spec, "reviewer", "ACCEPT", 1).is_some());
        assert!(check_termination_spec(&spec, "researcher", "draft", 5).is_some());
        assert!(check_termination_spec(&spec, "reviewer", "REJECT", 2).is_none());
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

        assert!(check_termination_spec(&spec, "a", "done", 1).is_none());
        assert!(check_termination_spec(&spec, "a", "done", 3).is_some());
    }

    // ── Edge trigger tests ──

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

    // ── process_event integration tests ──

    #[test]
    fn test_process_event_accumulates_and_routes() {
        let mut net = AgentNetwork::new(simple_spec()).unwrap();

        // LlmResponse events accumulate, no actions.
        assert!(net
            .process_event("a", &AgentEvent::LlmResponse("hello ".into()))
            .is_empty());
        assert!(net
            .process_event("a", &AgentEvent::LlmResponse("world".into()))
            .is_empty());

        // Done triggers routing to "b".
        let actions = net.process_event("a", &AgentEvent::Done);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            RoutingAction::Forward { to, message } => {
                assert_eq!(to, "b");
                assert_eq!(message, "hello world");
            }
            _ => panic!("expected Forward"),
        }
        assert!(!net.is_finished());
    }

    #[test]
    fn test_process_event_terminates_on_condition() {
        let mut spec = simple_spec();
        spec.termination = TerminationSpec::Condition {
            node: "a".into(),
            pattern: "stop".into(),
        };
        let mut net = AgentNetwork::new(spec).unwrap();

        net.process_event("a", &AgentEvent::LlmResponse("please stop now".into()));
        let actions = net.process_event("a", &AgentEvent::Done);

        assert_eq!(actions.len(), 1);
        assert!(matches!(
            &actions[0],
            RoutingAction::Finished {
                reason: TerminationReason::Condition { .. }
            }
        ));
        assert!(net.is_finished());

        // Subsequent events are no-ops.
        assert!(net
            .process_event("a", &AgentEvent::LlmResponse("more".into()))
            .is_empty());
    }

    #[test]
    fn test_initial_messages() {
        let net = AgentNetwork::new(simple_spec()).unwrap();
        let msgs = net.initial_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, "a");
        assert_eq!(msgs[0].1, "hello");
    }
}
