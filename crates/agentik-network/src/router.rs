//! # AgentNetwork — persistent mutable topology routing engine.
//!
//! Lives as long as [`RuntimeHost`](runtime::RuntimeHost) — it is created
//! once and mutated in-place as agents are added/removed. The topology graph
//! (nodes + edges + termination) changes dynamically; the `AgentNetwork`
//! instance itself never needs to be rebuilt.
//!
//! Routing is driven externally: the host calls
//! [`process_event`](AgentNetwork::process_event) with each agent event and
//! executes the returned [`RoutingAction`]s.

use std::collections::HashMap;

use agentik_sdk::types::AgentEvent;

use crate::graph::NetworkGraph;
use crate::spec::{EdgeSpec, EdgeTrigger, NetworkSpec, NodeSpec, TerminationSpec};

// ═══════════════════════════════════════════════════════════════════════
// RoutingAction
// ═══════════════════════════════════════════════════════════════════════

/// An action returned by [`AgentNetwork::process_event`] for the host to
/// execute.
#[derive(Debug, Clone)]
pub enum RoutingAction {
    /// Forward a message to the named target agent.
    Forward { to: String, message: String },
    /// The network has terminated.
    Finished { reason: TerminationReason },
}

/// Why the network terminated.
#[derive(Debug, Clone)]
pub enum TerminationReason {
    Condition { node: String, pattern: String },
    MaxRounds { max: usize },
    NodeDone { node: String },
}

/// Summary of a completed network run.
#[derive(Debug, Clone)]
pub enum NetworkOutcome {
    Terminated {
        reason: TerminationReason,
        rounds: usize,
        final_node: Option<String>,
        final_output: Option<String>,
    },
    Completed { rounds: usize },
}

// ═══════════════════════════════════════════════════════════════════════
// AgentNetwork — persistent mutable topology
// ═══════════════════════════════════════════════════════════════════════

/// A persistent, mutable multi-agent topology routing engine.
///
/// Created once (typically by [`RuntimeHost`](runtime::RuntimeHost)) and
/// mutated in-place as agents are added/removed. The underlying
/// [`NetworkGraph`] (petgraph) supports dynamic node/edge operations.
///
/// Routing state (response buffers, completion counts) is resettable via
/// [`reset_run_state`](Self::reset_run_state) without touching the topology.
pub struct AgentNetwork {
    /// Mutable petgraph-backed topology.
    graph: NetworkGraph,
    /// Termination condition for the current topology.
    termination: TerminationSpec,
    /// Accumulated LLM response text per node, cleared on each `Done`.
    response_buffers: HashMap<String, String>,
    /// How many times each node has completed a turn (emitted `Done`).
    completion_counts: HashMap<String, usize>,
    /// Set when a termination condition fires. Cleared by `reset_run_state`.
    finished: Option<TerminationReason>,
}

impl AgentNetwork {
    /// Create an empty network with no nodes/edges.
    pub fn new() -> Self {
        Self {
            graph: NetworkGraph::new(),
            termination: TerminationSpec::MaxRounds { max: 0 },
            response_buffers: HashMap::new(),
            completion_counts: HashMap::new(),
            finished: None,
        }
    }

    /// Build from a complete spec (batch construction).
    pub fn from_spec(spec: NetworkSpec) -> Result<Self, String> {
        spec.validate()?;
        let graph = NetworkGraph::from_nodes_edges(&spec.nodes, &spec.edges)?;
        Ok(Self {
            graph,
            termination: spec.termination,
            response_buffers: HashMap::new(),
            completion_counts: HashMap::new(),
            finished: None,
        })
    }

    // ── Topology mutation (delegates to NetworkGraph) ──────

    /// Add a node to the topology.
    pub fn add_node(&mut self, node: NodeSpec) -> Result<(), String> {
        self.graph.add_node(node)
    }

    /// Remove a node and all its edges from the topology.
    pub fn remove_node(&mut self, name: &str) -> Option<NodeSpec> {
        let removed = self.graph.remove_node(name);
        // Clean up routing state for the removed node.
        self.response_buffers.remove(name);
        self.completion_counts.remove(name);
        removed
    }

    /// Add a directed edge between two nodes.
    pub fn add_edge(&mut self, edge: EdgeSpec) -> Result<(), String> {
        self.graph.add_edge(edge)
    }

    /// Convenience: add an edge by endpoints + trigger.
    pub fn connect(
        &mut self,
        from: &str,
        to: &str,
        trigger: EdgeTrigger,
        transform: Option<crate::spec::TransformSpec>,
    ) -> Result<(), String> {
        self.graph.add_edge(EdgeSpec {
            from: from.into(),
            to: to.into(),
            trigger,
            transform,
        })
    }

    /// Remove all edges from `from` to `to`.
    pub fn disconnect(&mut self, from: &str, to: &str) -> usize {
        self.graph.remove_edges(from, to)
    }

    /// Set the termination condition for the current topology.
    pub fn set_termination(&mut self, termination: TerminationSpec) {
        self.termination = termination;
    }

    // ── Run-state management ───────────────────────────────

    /// Clear routing state (buffers, counts, finished flag) without
    /// touching the topology graph. Call this to start a fresh run on the
    /// same topology.
    pub fn reset_run_state(&mut self) {
        self.response_buffers.clear();
        self.completion_counts.clear();
        self.finished = None;
    }

    /// Whether the network has terminated.
    pub fn is_finished(&self) -> bool {
        self.finished.is_some()
    }

    /// Why the network terminated, if it has.
    pub fn termination_reason(&self) -> Option<&TerminationReason> {
        self.finished.as_ref()
    }

    /// Maximum completion count across all nodes.
    pub fn rounds(&self) -> usize {
        *self.completion_counts.values().max().unwrap_or(&0)
    }

    // ── Queries ────────────────────────────────────────────

    /// Borrow the underlying graph.
    pub fn graph(&self) -> &NetworkGraph {
        &self.graph
    }

    /// Borrow the termination spec.
    pub fn termination(&self) -> &TerminationSpec {
        &self.termination
    }

    /// Returns initial prompts that the host should inject.
    pub fn initial_messages(&self) -> Vec<(String, String)> {
        self.graph
            .node_names()
            .into_iter()
            .filter_map(|name| {
                self.graph.node(name).and_then(|n| {
                    n.initial_prompt.as_ref().map(|p| (name.to_string(), p.clone()))
                })
            })
            .collect()
    }

    // ── Routing (pure computation, driven by host) ─────────

    /// Feed an agent event into the routing state machine.
    ///
    /// Returns zero or more [`RoutingAction`]s for the host to execute.
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

                let count = self
                    .completion_counts
                    .entry(from.to_string())
                    .or_insert(0);
                *count += 1;
                let max_rounds = self.rounds();

                if let Some(reason) =
                    check_termination_spec(&self.termination, from, &response, max_rounds)
                {
                    self.finished = Some(reason.clone());
                    vec![RoutingAction::Finished { reason }]
                } else {
                    self.route_output(from, &response, max_rounds)
                }
            }

            _ => vec![],
        }
    }

    /// Compute routing actions for a completed node's response.
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

impl Default for AgentNetwork {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Free functions — pure logic
// ═══════════════════════════════════════════════════════════════════════

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

pub fn edge_should_fire(edge: &EdgeSpec, response: &str) -> bool {
    match &edge.trigger {
        EdgeTrigger::OnDone => true,
        EdgeTrigger::OnPattern { pattern } => {
            response.to_lowercase().contains(&pattern.to_lowercase())
        }
        EdgeTrigger::OnToolCall { .. } => {
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

    fn build_simple_network() -> AgentNetwork {
        let mut net = AgentNetwork::new();
        net.add_node(NodeSpec {
            name: "a".into(),
            profile: "x".into(),
            initial_prompt: Some("hello".into()),
        })
        .unwrap();
        net.add_node(NodeSpec {
            name: "b".into(),
            profile: "y".into(),
            initial_prompt: None,
        })
        .unwrap();
        net.connect("a", "b", EdgeTrigger::OnDone, None).unwrap();
        net.set_termination(TerminationSpec::MaxRounds { max: 99 });
        net
    }

    #[test]
    fn process_event_accumulates_and_routes() {
        let mut net = build_simple_network();

        assert!(net
            .process_event("a", &AgentEvent::LlmResponse("hello ".into()))
            .is_empty());
        assert!(net
            .process_event("a", &AgentEvent::LlmResponse("world".into()))
            .is_empty());

        let actions = net.process_event("a", &AgentEvent::Done);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            RoutingAction::Forward { to, message } => {
                assert_eq!(to, "b");
                assert_eq!(message, "hello world");
            }
            _ => panic!("expected Forward"),
        }
    }

    #[test]
    fn terminates_on_condition() {
        let mut net = build_simple_network();
        net.set_termination(TerminationSpec::Condition {
            node: "a".into(),
            pattern: "stop".into(),
        });

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
    fn reset_run_state_preserves_topology() {
        let mut net = build_simple_network();
        net.process_event("a", &AgentEvent::LlmResponse("test".into()));
        net.process_event("a", &AgentEvent::Done);

        assert_eq!(net.rounds(), 1);
        net.reset_run_state();
        assert_eq!(net.rounds(), 0);
        assert!(!net.is_finished());

        // Topology is intact.
        assert_eq!(net.graph().node_count(), 2);
        assert_eq!(net.graph().edge_count(), 1);
    }

    #[test]
    fn dynamic_node_removal_cleans_routing_state() {
        let mut net = build_simple_network();
        net.process_event("b", &AgentEvent::LlmResponse("stuff".into()));
        assert!(net.response_buffers.contains_key("b"));

        net.remove_node("b");
        assert!(!net.response_buffers.contains_key("b"));
        assert!(!net.completion_counts.contains_key("b"));
        assert_eq!(net.graph().node_count(), 1);
    }

    #[test]
    fn initial_messages() {
        let net = build_simple_network();
        let msgs = net.initial_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, "a");
        assert_eq!(msgs[0].1, "hello");
    }

    // ── Termination logic tests ──

    #[test]
    fn test_condition_termination_match() {
        let spec = TerminationSpec::Condition {
            node: "reviewer".into(),
            pattern: "ACCEPT".into(),
        };
        let reason =
            check_termination_spec(&spec, "reviewer", "<verdict>ACCEPT</verdict>", 3);
        assert!(matches!(reason, Some(TerminationReason::Condition { .. })));
    }

    #[test]
    fn test_condition_case_insensitive() {
        let spec = TerminationSpec::Condition {
            node: "rev".into(),
            pattern: "accept".into(),
        };
        assert!(check_termination_spec(&spec, "rev", "I ACCEPT", 1).is_some());
    }

    #[test]
    fn test_max_rounds_termination() {
        let spec = TerminationSpec::MaxRounds { max: 3 };
        assert!(check_termination_spec(&spec, "any", "x", 2).is_none());
        assert!(check_termination_spec(&spec, "any", "x", 3).is_some());
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
        assert!(check_termination_spec(&spec, "x", "y", 5).is_some());
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
}
