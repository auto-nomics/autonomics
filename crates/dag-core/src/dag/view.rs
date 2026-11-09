//! Read-only view snapshots for interactive DAG consumers.
//!
//! The scheduler owns mutable state and a channel-based actor; UI code should
//! receive a small, owned snapshot instead of borrowing the graph across an
//! await point or reaching into engine internals.

use serde::{Deserialize, Serialize};

use super::status::RuntimeStatus;
use super::{DagNode, NodeId};

/// One declared input or output socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagPortView {
    pub index: u8,
    pub label: Option<String>,
    pub data_type: String,
}

/// One DAG node as an interactive renderer sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNodeView {
    pub id: NodeId,
    pub kind: String,
    pub status: RuntimeStatus,
    pub dirty: bool,
    pub inputs: Vec<DagPortView>,
    pub outputs: Vec<DagPortView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<super::live::ExecutionObservation>,
}

/// One dataflow edge, including exact port wiring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagEdgeView {
    pub from: NodeId,
    pub from_port: u8,
    pub to: NodeId,
    pub to_port: u8,
}

/// A stable, owned view of current DAG topology and execution state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DagTuiSnapshot {
    /// Nodes in deterministic id order.
    pub nodes: Vec<DagNodeView>,
    /// Edges in graph insertion order.
    pub edges: Vec<DagEdgeView>,
    #[serde(default)]
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default)]
    pub observed_at_ms: u64,
}

impl DagTuiSnapshot {
    /// DOT from the independent projection, including live phase/counters.
    /// JSON string quoting is also valid DOT string quoting; identifiers and
    /// labels can never inject additional graph statements.
    pub fn to_dot(&self) -> String {
        let quote = |text: &str| serde_json::to_string(text).expect("string serialization");
        let mut dot = String::from("digraph DAG {\n");
        for node in &self.nodes {
            let mut label = format!("{}\n{}\n{:?}", node.id, node.kind, node.status);
            if let Some(execution) = &node.execution {
                label.push_str(&format!("\n{}", execution.phase));
                if let Some(current) = execution.current {
                    label.push_str(&format!(
                        " {current}/{} {}",
                        execution.total.map_or("?".into(), |n| n.to_string()),
                        execution.unit.as_deref().unwrap_or("items")
                    ));
                }
            }
            dot.push_str(&format!(
                "  {} [label={}];\n",
                quote(&node.id),
                quote(&label)
            ));
        }
        for edge in &self.edges {
            dot.push_str(&format!(
                "  {} -> {} [label={}];\n",
                quote(&edge.from),
                quote(&edge.to),
                quote(&format!("{} -> {}", edge.from_port, edge.to_port))
            ));
        }
        dot.push_str("}\n");
        dot
    }

    pub fn node(&self, id: &str) -> Option<&DagNodeView> {
        self.nodes.iter().find(|node| node.id == id)
    }

    pub fn status_count(&self, status: RuntimeStatus) -> usize {
        self.nodes
            .iter()
            .filter(|node| node.status == status)
            .count()
    }
}
