//! # AgentNetwork — persistent mutable topology routing engine.
//!
//! Lives as long as [`RuntimeHost`](runtime::RuntimeHost) — it is created
//! once and mutated in-place as agents are added/removed.
//!
//! ## Edge semantics
//!
//! Each edge has a [`EdgeKind`](crate::EdgeKind):
//! - **Delegate** (default): request-response. When source Dones, its output
//!   is sent to the target. When the target Dones, its response is returned
//!   to the source. No message loss — every request gets a response.
//! - **Push**: fire-and-forget. Source Done → forward to target. Used for
//!   pipelines where no response is expected.
//!
//! Delegation tracking is automatic: the network maintains a
//! `pending_delegations` map internally. The host only needs to execute
//! the returned [`RoutingAction`]s.

use std::collections::HashMap;

use agentik_sdk::types::AgentEvent;

use crate::graph::NetworkGraph;
use crate::spec::{EdgeKind, EdgeSpec, EdgeTrigger, NetworkSpec, NodeSpec, TerminationSpec};

// ═══════════════════════════════════════════════════════════════════════
// RoutingAction
// ═══════════════════════════════════════════════════════════════════════

/// An action returned by [`AgentNetwork::process_event`] for the host to
/// execute. The host simply calls `send_to(to, message)` for each `Send`.
#[derive(Debug, Clone)]
pub enum RoutingAction {
    /// Send a message to the named target agent.
    /// This covers both delegation requests and delegation responses —
    /// the network tracks internally which is which.
    Send { to: String, message: String },
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
/// Edge semantics:
/// - **Delegate** edges (default): request-response. The source's output is
///   forwarded to the target; when the target Dones, its response returns
///   to the source.
/// - **Push** edges: fire-and-forget forwarding (pipeline semantics).
///
/// Delegation state is tracked internally via `pending_delegations`.
pub struct AgentNetwork {
    graph: NetworkGraph,
    termination: TerminationSpec,
    response_buffers: HashMap<String, String>,
    completion_counts: HashMap<String, usize>,
    finished: Option<TerminationReason>,
    /// Tracks who delegated to whom: delegatee → delegator.
    /// When the delegatee Dones, its response returns to the delegator.
    pending_delegations: HashMap<String, String>,
}

impl AgentNetwork {
    pub fn new() -> Self {
        Self {
            graph: NetworkGraph::new(),
            // Empty AnyNodeDone never matches — effectively "no termination"
            // until the user explicitly calls set_termination.
            termination: TerminationSpec::AnyNodeDone { nodes: vec![] },
            response_buffers: HashMap::new(),
            completion_counts: HashMap::new(),
            finished: None,
            pending_delegations: HashMap::new(),
        }
    }

    pub fn from_spec(spec: NetworkSpec) -> Result<Self, String> {
        spec.validate()?;
        let graph = NetworkGraph::from_nodes_edges(&spec.nodes, &spec.edges)?;
        Ok(Self {
            graph,
            termination: spec.termination,
            response_buffers: HashMap::new(),
            completion_counts: HashMap::new(),
            finished: None,
            pending_delegations: HashMap::new(),
        })
    }

    // ── Topology mutation ──────────────────────────────────

    pub fn add_node(&mut self, node: NodeSpec) -> Result<(), String> {
        self.graph.add_node(node)
    }

    pub fn remove_node(&mut self, name: &str) -> Option<NodeSpec> {
        let removed = self.graph.remove_node(name);
        self.response_buffers.remove(name);
        self.completion_counts.remove(name);
        self.pending_delegations.remove(name);
        // Also clean up any delegations WHERE this node was the delegator.
        self.pending_delegations.retain(|_, v| v != name);
        removed
    }

    pub fn add_edge(&mut self, edge: EdgeSpec) -> Result<(), String> {
        self.graph.add_edge(edge)
    }

    pub fn connect(
        &mut self,
        from: &str,
        to: &str,
        trigger: EdgeTrigger,
        kind: EdgeKind,
        transform: Option<crate::spec::TransformSpec>,
    ) -> Result<(), String> {
        self.graph.add_edge(EdgeSpec {
            from: from.into(),
            to: to.into(),
            trigger,
            kind,
            transform,
        })
    }

    pub fn disconnect(&mut self, from: &str, to: &str) -> usize {
        self.graph.remove_edges(from, to)
    }

    pub fn set_termination(&mut self, termination: TerminationSpec) {
        self.termination = termination;
    }

    // ── Run-state management ───────────────────────────────

    pub fn reset_run_state(&mut self) {
        self.response_buffers.clear();
        self.completion_counts.clear();
        self.finished = None;
        self.pending_delegations.clear();
    }

    pub fn is_finished(&self) -> bool {
        self.finished.is_some()
    }

    pub fn termination_reason(&self) -> Option<&TerminationReason> {
        self.finished.as_ref()
    }

    pub fn rounds(&self) -> usize {
        *self.completion_counts.values().max().unwrap_or(&0)
    }

    // ── Queries ────────────────────────────────────────────

    pub fn graph(&self) -> &NetworkGraph {
        &self.graph
    }

    pub fn termination(&self) -> &TerminationSpec {
        &self.termination
    }

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

    /// Number of pending delegations (agents waiting for a response).
    pub fn pending_count(&self) -> usize {
        self.pending_delegations.len()
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

                // Check termination first.
                if let Some(reason) =
                    check_termination_spec(&self.termination, from, &response, max_rounds)
                {
                    self.finished = Some(reason.clone());
                    return vec![RoutingAction::Finished { reason }];
                }

                // Priority 1: If this node was delegated-to, return the
                // response to the delegator. Do NOT also forward downstream —
                // the delegation response takes exclusive priority.
                if let Some(delegator) = self.pending_delegations.remove(from) {
                    return vec![RoutingAction::Send {
                        to: delegator,
                        message: response,
                    }];
                }

                // Priority 2: Forward along outgoing edges.
                let mut actions = vec![];
                for edge in self.graph.out_edges(from) {
                    if !edge_should_fire(edge, &response) {
                        continue;
                    }
                    let message = match &edge.transform {
                        Some(t) => t.render(&response, max_rounds, &edge.from, &edge.to),
                        None => response.to_string(),
                    };
                    match &edge.kind {
                        EdgeKind::Delegate => {
                            // Track: when `to` Dones, response returns to `from`.
                            self.pending_delegations
                                .insert(edge.to.clone(), from.to_string());
                        }
                        EdgeKind::Push => {
                            // Fire-and-forget — no tracking.
                        }
                    }
                    actions.push(RoutingAction::Send {
                        to: edge.to.clone(),
                        message,
                    });
                }

                // If no outgoing edges and no delegation return, this node's
                // output is terminal (pipeline end / standalone). The host
                // already has the response — no action needed.
                actions
            }

            _ => vec![],
        }
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
    use crate::spec::{EdgeSpec, EdgeKind, EdgeTrigger, NetworkSpec, NodeSpec, TerminationSpec};

    // ── Delegate edge tests ──

    #[test]
    fn delegate_edge_sends_request_then_returns_response() {
        let mut net = AgentNetwork::new();
        net.add_node(NodeSpec { name: "a".into(), profile: "x".into(), initial_prompt: None }).unwrap();
        net.add_node(NodeSpec { name: "b".into(), profile: "y".into(), initial_prompt: None }).unwrap();
        net.connect("a", "b", EdgeTrigger::OnDone, EdgeKind::Delegate, None).unwrap();
        net.set_termination(TerminationSpec::MaxRounds { max: 99 });

        // A Dones → delegates to B.
        net.process_event("a", &AgentEvent::LlmResponse("request".into()));
        let actions = net.process_event("a", &AgentEvent::Done);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], RoutingAction::Send { to, .. } if to == "b"));
        assert_eq!(net.pending_count(), 1); // B is pending for A.

        // B Dones → response returns to A.
        net.process_event("b", &AgentEvent::LlmResponse("response".into()));
        let actions = net.process_event("b", &AgentEvent::Done);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            RoutingAction::Send { to, message } => {
                assert_eq!(to, "a");
                assert_eq!(message, "response");
            }
            _ => panic!("expected Send back to a"),
        }
        assert_eq!(net.pending_count(), 0); // Delegation resolved.
    }

    #[test]
    fn delegate_edge_does_not_lose_leaf_output() {
        // A→[Delegate]B (B is a leaf). B's response goes back to A.
        let mut net = AgentNetwork::new();
        net.add_node(NodeSpec { name: "a".into(), profile: "x".into(), initial_prompt: None }).unwrap();
        net.add_node(NodeSpec { name: "b".into(), profile: "y".into(), initial_prompt: None }).unwrap();
        net.connect("a", "b", EdgeTrigger::OnDone, EdgeKind::Delegate, None).unwrap();
        net.set_termination(TerminationSpec::MaxRounds { max: 99 });

        net.process_event("a", &AgentEvent::LlmResponse("do work".into()));
        net.process_event("a", &AgentEvent::Done);

        // B Dones — response returns to A, NOT lost.
        net.process_event("b", &AgentEvent::LlmResponse("here's the result".into()));
        let actions = net.process_event("b", &AgentEvent::Done);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], RoutingAction::Send { to, .. } if to == "a"));
    }

    // ── Push edge tests ──

    #[test]
    fn push_edge_forgets_after_forwarding() {
        // A→[Push]B→[Push]C pipeline.
        let mut net = AgentNetwork::new();
        for name in ["a", "b", "c"] {
            net.add_node(NodeSpec { name: name.into(), profile: name.into(), initial_prompt: None }).unwrap();
        }
        net.connect("a", "b", EdgeTrigger::OnDone, EdgeKind::Push, None).unwrap();
        net.connect("b", "c", EdgeTrigger::OnDone, EdgeKind::Push, None).unwrap();
        net.set_termination(TerminationSpec::AnyNodeDone { nodes: vec!["c".into()] });

        // A Dones → push to B.
        net.process_event("a", &AgentEvent::LlmResponse("data".into()));
        let actions = net.process_event("a", &AgentEvent::Done);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], RoutingAction::Send { to, .. } if to == "b"));
        assert_eq!(net.pending_count(), 0); // Push doesn't track.

        // B Dones → push to C.
        net.process_event("b", &AgentEvent::LlmResponse("processed".into()));
        let actions = net.process_event("b", &AgentEvent::Done);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], RoutingAction::Send { to, .. } if to == "c"));

        // C Dones → leaf node, termination fires (AnyNodeDone).
        net.process_event("c", &AgentEvent::LlmResponse("final".into()));
        let actions = net.process_event("c", &AgentEvent::Done);
        assert!(actions.iter().any(|a| matches!(a, RoutingAction::Finished { .. })));
    }

    // ── Arena-style delegation loop ──

    #[test]
    fn arena_loop_with_single_delegate_edge() {
        // writer→[Delegate]reviewer (no reverse edge needed!)
        let mut net = AgentNetwork::new();
        net.add_node(NodeSpec { name: "writer".into(), profile: "w".into(), initial_prompt: Some("write".into()) }).unwrap();
        net.add_node(NodeSpec { name: "reviewer".into(), profile: "r".into(), initial_prompt: None }).unwrap();
        net.connect("writer", "reviewer", EdgeTrigger::OnDone, EdgeKind::Delegate, None).unwrap();
        net.set_termination(TerminationSpec::Condition {
            node: "reviewer".into(),
            pattern: "ACCEPT".into(),
        });

        // Round 1: writer Dones → delegate to reviewer.
        net.process_event("writer", &AgentEvent::LlmResponse("draft v1".into()));
        let actions = net.process_event("writer", &AgentEvent::Done);
        assert!(actions.iter().any(|a| matches!(a, RoutingAction::Send { to, .. } if to == "reviewer")));

        // Reviewer Dones with REJECT → response returns to writer, no termination.
        net.process_event("reviewer", &AgentEvent::LlmResponse("REJECT - needs work".into()));
        let actions = net.process_event("reviewer", &AgentEvent::Done);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], RoutingAction::Send { to, message } if to == "writer"));
        assert!(!net.is_finished());

        // Round 2: writer Dones → delegate to reviewer again.
        net.process_event("writer", &AgentEvent::LlmResponse("draft v2".into()));
        let actions = net.process_event("writer", &AgentEvent::Done);
        assert!(actions.iter().any(|a| matches!(a, RoutingAction::Send { to, .. } if to == "reviewer")));

        // Reviewer Dones with ACCEPT → termination fires.
        net.process_event("reviewer", &AgentEvent::LlmResponse("Good work. ACCEPT".into()));
        let actions = net.process_event("reviewer", &AgentEvent::Done);
        assert!(actions.iter().any(|a| matches!(a, RoutingAction::Finished { .. })));
        assert!(net.is_finished());
    }

    // ── State management tests ──

    #[test]
    fn reset_clears_delegations() {
        let mut net = AgentNetwork::new();
        net.add_node(NodeSpec { name: "a".into(), profile: "x".into(), initial_prompt: None }).unwrap();
        net.add_node(NodeSpec { name: "b".into(), profile: "y".into(), initial_prompt: None }).unwrap();
        net.connect("a", "b", EdgeTrigger::OnDone, EdgeKind::Delegate, None).unwrap();

        net.process_event("a", &AgentEvent::LlmResponse("x".into()));
        net.process_event("a", &AgentEvent::Done);
        assert_eq!(net.pending_count(), 1);

        net.reset_run_state();
        assert_eq!(net.pending_count(), 0);
        assert_eq!(net.rounds(), 0);
    }

    #[test]
    fn remove_node_cleans_delegation_state() {
        let mut net = AgentNetwork::new();
        net.add_node(NodeSpec { name: "a".into(), profile: "x".into(), initial_prompt: None }).unwrap();
        net.add_node(NodeSpec { name: "b".into(), profile: "y".into(), initial_prompt: None }).unwrap();
        net.connect("a", "b", EdgeTrigger::OnDone, EdgeKind::Delegate, None).unwrap();

        net.process_event("a", &AgentEvent::LlmResponse("x".into()));
        net.process_event("a", &AgentEvent::Done);
        // pending["b"] = "a"
        assert_eq!(net.pending_count(), 1);

        net.remove_node("b");
        assert_eq!(net.pending_count(), 0);
    }

    #[test]
    fn delegated_node_does_not_also_forward() {
        // A→[Delegate]B, B→[Push]C.
        // When B Dones in response to A's delegation, B returns to A
        // and does NOT also push to C.
        let mut net = AgentNetwork::new();
        for name in ["a", "b", "c"] {
            net.add_node(NodeSpec { name: name.into(), profile: name.into(), initial_prompt: None }).unwrap();
        }
        net.connect("a", "b", EdgeTrigger::OnDone, EdgeKind::Delegate, None).unwrap();
        net.connect("b", "c", EdgeTrigger::OnDone, EdgeKind::Push, None).unwrap();
        net.set_termination(TerminationSpec::MaxRounds { max: 99 });

        // A delegates to B.
        net.process_event("a", &AgentEvent::LlmResponse("req".into()));
        net.process_event("a", &AgentEvent::Done);

        // B Dones → returns to A only (NOT pushes to C).
        net.process_event("b", &AgentEvent::LlmResponse("resp".into()));
        let actions = net.process_event("b", &AgentEvent::Done);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], RoutingAction::Send { to, .. } if to == "a"));
        // C was NOT contacted.
        assert!(actions.iter().all(|a| !matches!(a, RoutingAction::Send { to, .. } if to == "c")));
    }

    // ── Termination logic tests ──

    #[test]
    fn test_condition_termination_match() {
        let spec = TerminationSpec::Condition { node: "reviewer".into(), pattern: "ACCEPT".into() };
        assert!(check_termination_spec(&spec, "reviewer", "<verdict>ACCEPT</verdict>", 3).is_some());
    }

    #[test]
    fn test_condition_case_insensitive() {
        let spec = TerminationSpec::Condition { node: "rev".into(), pattern: "accept".into() };
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
                TerminationSpec::Condition { node: "reviewer".into(), pattern: "ACCEPT".into() },
                TerminationSpec::MaxRounds { max: 5 },
            ],
        };
        assert!(check_termination_spec(&spec, "reviewer", "ACCEPT", 1).is_some());
        assert!(check_termination_spec(&spec, "x", "y", 5).is_some());
        assert!(check_termination_spec(&spec, "reviewer", "REJECT", 2).is_none());
    }

    #[test]
    fn test_edge_on_done_always_fires() {
        let edge = EdgeSpec { from: "a".into(), to: "b".into(), trigger: EdgeTrigger::OnDone, kind: EdgeKind::Delegate, transform: None };
        assert!(edge_should_fire(&edge, "anything"));
    }

    #[test]
    fn test_edge_on_pattern() {
        let edge = EdgeSpec { from: "a".into(), to: "b".into(), trigger: EdgeTrigger::OnPattern { pattern: "ready".into() }, kind: EdgeKind::Delegate, transform: None };
        assert!(edge_should_fire(&edge, "I am ready"));
        assert!(!edge_should_fire(&edge, "not yet"));
    }
}
