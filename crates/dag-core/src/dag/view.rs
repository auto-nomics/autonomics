//! Read-only view snapshots for interactive DAG consumers.
//!
//! The scheduler owns mutable state and a channel-based actor; UI code should
//! receive a small, owned snapshot instead of borrowing the graph across an
//! await point or reaching into engine internals.

use serde::{Deserialize, Serialize};

use super::runtime::RuntimeStatus;
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
}

impl DagTuiSnapshot {
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
