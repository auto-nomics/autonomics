//! Declarative network specification — the graph that the conductor
//! interprets at runtime.
//!
//! All types are `Serialize + Deserialize` so a network can be persisted,
//! inspected in a UI, and shared. This mirrors the DataEngine's DAG spec
//! pattern: declare the topology, then hand it to an engine that runs it.

use serde::{Deserialize, Serialize};

// ═══════════════════════════════════════════════════════════════════════
// NetworkSpec — top-level declaration
// ═══════════════════════════════════════════════════════════════════════

/// A complete declarative description of a multi-agent network.
///
/// Created by the user (or a builder helper), passed to
/// [`AgentNetwork::build`](crate::AgentNetwork::build), and optionally
/// persisted for reuse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSpec {
    /// Human-readable name for this network configuration.
    pub name: String,

    /// Agent nodes in the network.
    pub nodes: Vec<NodeSpec>,

    /// Directed edges defining message flow.
    pub edges: Vec<EdgeSpec>,

    /// When the network should terminate.
    pub termination: TerminationSpec,
}

impl NetworkSpec {
    /// Validate internal consistency:
    /// - At least one node exists.
    /// - Node names are unique.
    /// - Every edge references existing nodes.
    /// - Termination conditions reference existing nodes.
    ///
    /// Builds a [`NetworkGraph`](crate::NetworkGraph) under the hood for
    /// structural validation (duplicate detection, dangling edges).
    pub fn validate(&self) -> Result<(), String> {
        // Structural validation via the graph index.
        let graph = crate::graph::NetworkGraph::from_nodes_edges(&self.nodes, &self.edges)?;

        // Termination references — the graph ensures node names exist.
        let names: std::collections::HashSet<&str> =
            self.nodes.iter().map(|n| n.name.as_str()).collect();
        self.termination.validate(&names)?;

        let _ = graph; // suppress unused warning
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════
// NodeSpec
// ═══════════════════════════════════════════════════════════════════════

/// One agent node in the network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSpec {
    /// Unique name for this node within the network (used in edge
    /// references).
    pub name: String,

    /// The [`AgentProfile`](agentik_core::AgentProfile) name to instantiate.
    /// The caller must provide a matching profile when building the network.
    pub profile: String,

    /// If present, this message is injected immediately when the network
    /// starts, kicking off the agent. Nodes without an initial prompt wait
    /// until they receive a routed message from an upstream node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<String>,
}

// ═══════════════════════════════════════════════════════════════════════
// EdgeSpec
// ═══════════════════════════════════════════════════════════════════════

/// A directed edge from one node to another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeSpec {
    /// Source node name.
    pub from: String,

    /// Target node name.
    pub to: String,

    /// When this edge should fire (forward a message).
    pub trigger: EdgeTrigger,

    /// Optional message transform applied before forwarding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<TransformSpec>,
}

/// Determines when an edge forwards a message from its source to its target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EdgeTrigger {
    /// Fire when the source node emits `AgentEvent::Done` — i.e. its LLM
    /// turn finished without further tool calls. The accumulated text
    /// response is forwarded.
    OnDone,

    /// Fire when the source node calls the named tool. The tool's result
    /// text is forwarded instead of the LLM response.
    ///
    /// (Reserved for P2 — not yet implemented in the conductor.)
    OnToolCall { tool: String },

    /// Fire when the accumulated LLM response matches the given substring
    /// (case-insensitive).
    OnPattern { pattern: String },
}

/// A transform applied to a message before forwarding it across an edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformSpec {
    /// A `format!`-style template with placeholders:
    /// - `{content}` — the source node's response text
    /// - `{round}` — current round number (0-based)
    /// - `{from}` — source node name
    /// - `{to}` — target node name
    pub template: String,
}

impl TransformSpec {
    /// Render the template with the given context values.
    pub fn render(&self, content: &str, round: usize, from: &str, to: &str) -> String {
        self.template
            .replace("{content}", content)
            .replace("{round}", &round.to_string())
            .replace("{from}", from)
            .replace("{to}", to)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// TerminationSpec
// ═══════════════════════════════════════════════════════════════════════

/// Declares when the network should stop running.
///
/// Composable via [`TerminationSpec::Any`]: the first sub-condition that
/// triggers wins.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminationSpec {
    /// Terminate when any of the named nodes completes (emits `Done`).
    ///
    /// Use this for pipeline topologies where the terminal node's output
    /// is the final result.
    AnyNodeDone { nodes: Vec<String> },

    /// Terminate when the specified node's response contains `pattern`
    /// (case-insensitive substring match).
    ///
    /// Use this for adversarial loops where the reviewer must emit an
    /// explicit accept/reject signal.
    Condition { node: String, pattern: String },

    /// Terminate after any single node has completed `max` turns.
    ///
    /// A "turn" is one `Done` event from a node. In a cyclic network
    /// with 2 nodes and 2 edges, one round ≈ 2 turns (one per node).
    MaxRounds { max: usize },

    /// All of the sub-conditions must trigger. Rarely useful alone;
    /// typically combined inside [`Any`](Self::Any).
    All { specs: Vec<TerminationSpec> },

    /// Any of the sub-conditions triggering terminates the network.
    /// This is the most common top-level form.
    Any { specs: Vec<TerminationSpec> },
}

impl TerminationSpec {
    /// Validate that all node references point to known nodes.
    fn validate(&self, known: &std::collections::HashSet<&str>) -> Result<(), String> {
        match self {
            Self::AnyNodeDone { nodes } => {
                for n in nodes {
                    if !known.contains(n.as_str()) {
                        return Err(format!(
                            "termination references unknown node: {n}"
                        ));
                    }
                }
                Ok(())
            }
            Self::Condition { node, .. } => {
                if !known.contains(node.as_str()) {
                    return Err(format!(
                        "termination references unknown node: {node}"
                    ));
                }
                Ok(())
            }
            Self::All { specs } | Self::Any { specs } => {
                for s in specs {
                    s.validate(known)?;
                }
                Ok(())
            }
            Self::MaxRounds { .. } => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_spec() -> NetworkSpec {
        NetworkSpec {
            name: "test".into(),
            nodes: vec![
                NodeSpec {
                    name: "a".into(),
                    profile: "researcher".into(),
                    initial_prompt: Some("hello".into()),
                },
                NodeSpec {
                    name: "b".into(),
                    profile: "reviewer".into(),
                    initial_prompt: None,
                },
            ],
            edges: vec![
                EdgeSpec {
                    from: "a".into(),
                    to: "b".into(),
                    trigger: EdgeTrigger::OnDone,
                    transform: None,
                },
                EdgeSpec {
                    from: "b".into(),
                    to: "a".into(),
                    trigger: EdgeTrigger::OnDone,
                    transform: None,
                },
            ],
            termination: TerminationSpec::Any {
                specs: vec![
                    TerminationSpec::Condition {
                        node: "b".into(),
                        pattern: "ACCEPT".into(),
                    },
                    TerminationSpec::MaxRounds { max: 5 },
                ],
            },
        }
    }

    #[test]
    fn spec_validates() {
        assert!(sample_spec().validate().is_ok());
    }

    #[test]
    fn spec_rejects_dangling_edge() {
        let mut spec = sample_spec();
        spec.edges.push(EdgeSpec {
            from: "a".into(),
            to: "nonexistent".into(),
            trigger: EdgeTrigger::OnDone,
            transform: None,
        });
        assert!(spec.validate().is_err());
    }

    #[test]
    fn spec_rejects_duplicate_names() {
        let mut spec = sample_spec();
        spec.nodes.push(NodeSpec {
            name: "a".into(),
            profile: "x".into(),
            initial_prompt: None,
        });
        assert!(spec.validate().is_err());
    }

    #[test]
    fn spec_serializes_roundtrip() {
        let spec = sample_spec();
        let json = serde_json::to_string_pretty(&spec).unwrap();
        let back: NetworkSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec.name, back.name);
        assert_eq!(spec.nodes.len(), back.nodes.len());
        assert_eq!(spec.edges.len(), back.edges.len());
    }

    #[test]
    fn transform_renders_placeholders() {
        let t = TransformSpec {
            template: "[{from}→{to}] Round {round}: {content}".into(),
        };
        assert_eq!(
            t.render("hello", 3, "alice", "bob"),
            "[alice→bob] Round 3: hello"
        );
    }
}
