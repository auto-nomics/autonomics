use tokio::sync::{mpsc, oneshot};

use crate::dag::RunReport;
use crate::dag::graph::PortOutputs;
use crate::dag::node_event::NodeEvent;
use crate::error::Result as EngineResult;
use schemars;

pub enum DataEngineCmd {
    AddNode {
        id: String,
        kind: String,
        spec: serde_json::Value,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    AddEdge {
        from: String,
        from_port: Option<u8>,
        to: String,
        to_port: Option<u8>,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    RunDag {
        /// Optional sink for streaming lightweight per-node events (status /
        /// progress / log / finished) out of the actor as the run progresses.
        /// `None` for the non-streaming path.
        event_tx: Option<mpsc::Sender<NodeEvent>>,
        reply: oneshot::Sender<EngineResult<RunReport>>,
    },
    GetOutput {
        id: String,
        reply: oneshot::Sender<EngineResult<Option<PortOutputs>>>,
    },
    RemoveNode {
        id: String,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    ViewDag {
        reply: oneshot::Sender<EngineResult<String>>,
    },
    ClearDag {
        reply: oneshot::Sender<EngineResult<()>>,
    },
    /// Clear the in-memory DAG and switch to a new history ref.
    NewDagRef {
        name: String,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    /// Switch the engine's history ref to an existing ref.
    SwitchDagRef {
        name: String,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    /// List all history refs.
    ListDagRefs {
        reply: oneshot::Sender<EngineResult<Vec<(String, String, bool)>>>,
    },
    /// Show snapshot lineage for a ref.
    DagLog {
        ref_name: Option<String>,
        limit: usize,
        reply: oneshot::Sender<EngineResult<Vec<crate::dag::Snapshot>>>,
    },
    /// Load a snapshot's DAG into memory without moving the ref.
    CheckoutDag {
        snapshot_id: String,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    /// Create a new ref from a snapshot + switch + load its DAG.
    BranchFromSnapshot {
        snapshot_id: String,
        ref_name: String,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    /// Query the current history ref name.
    GetDagRef {
        reply: oneshot::Sender<EngineResult<String>>,
    },
    GetNodeSpec {
        kind: String,
        reply: oneshot::Sender<EngineResult<schemars::Schema>>,
    },
    ListNodeFactories {
        reply: oneshot::Sender<EngineResult<Vec<crate::node_registry::NodeInfo>>>,
    },
    UpdateNode {
        id: String,
        spec: serde_json::Value,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    GetNodePorts {
        kind: String,
        reply: oneshot::Sender<EngineResult<crate::nodes::meta::NodePorts>>,
    },
    GetNodeDoc {
        kind: String,
        reply: oneshot::Sender<EngineResult<String>>,
    },
}
