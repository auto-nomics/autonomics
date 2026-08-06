use tokio::sync::{mpsc, oneshot};

use crate::dag::RunReport;
use crate::dag::graph::PortOutputs;
use crate::dag::node_event::NodeEvent;
use crate::dag::runtime::RuntimeStatus;
use crate::error::Result as EngineResult;

/// Envelope that routes a [`DataEngineCmd`] to a session's actor.
///
/// In the per-agent architecture, each session has its own channel and
/// tokio task (`SessionServer`), so `session_id` is kept for logging only.
/// Metadata queries (list/spec/ports/doc) bypass this channel entirely —
/// they are served synchronously from `Arc<NodeRegistry>` on the client.
pub struct EngineMsg {
    pub session_id: String,
    pub cmd: DataEngineCmd,
}

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
        /// Commit message for the history snapshot. If `None`, a default is used.
        commit_message: Option<String>,
        reply: oneshot::Sender<EngineResult<RunReport>>,
    },
    GetOutput {
        id: String,
        reply: oneshot::Sender<EngineResult<Option<PortOutputs>>>,
    },
    /// Query a node's runtime status. Returns `None` if the DAG has never
    /// been run (no status entry exists for the node).
    GetNodeStatus {
        id: String,
        reply: oneshot::Sender<EngineResult<Option<RuntimeStatus>>>,
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
    /// Fetch a single snapshot (manifest + run report + metadata).
    GetSnapshot {
        snapshot_id: String,
        reply: oneshot::Sender<EngineResult<Option<crate::dag::Snapshot>>>,
    },
    /// Diff two snapshots' manifests.
    DiffSnapshots {
        old_id: String,
        new_id: String,
        reply: oneshot::Sender<EngineResult<String>>,
    },
    /// Query the current history ref name.
    GetDagRef {
        reply: oneshot::Sender<EngineResult<String>>,
    },
    UpdateNode {
        id: String,
        spec: serde_json::Value,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    /// Reverse-compile the current DAG to R or Python source code.
    CompileDag {
        target: crate::codegen::CodegenTarget,
        reply: oneshot::Sender<EngineResult<crate::codegen::CompiledScript>>,
    },
}
