use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::dag::DagTuiSnapshot;
use crate::dag::runtime::RuntimeStatus;
use crate::dag_shell::DagShellOutcome;
use crate::error::Result as EngineResult;

/// Envelope that routes a [`DataEngineCmd`] to an agent's actor.
///
/// In the per-agent architecture, each agent has its own channel and
/// tokio task ([`AgentEngineActor`](crate::runtime::AgentEngineActor)).
/// Metadata queries (list/spec/ports/doc) bypass this channel entirely —
/// they are served synchronously from `Arc<NodeRegistry>` on the client.
pub struct EngineMsg {
    pub cmd: DataEngineCmd,
}

pub enum DataEngineCmd {
    SetFlow {
        /// Replace the session's authored flow wholesale. Validated first;
        /// an invalid graph leaves the previous flow untouched.
        flow: crate::workflow::FlowGraph,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    GetFlow {
        reply: oneshot::Sender<EngineResult<crate::workflow::FlowGraph>>,
    },
    GetFlowView {
        reply: oneshot::Sender<EngineResult<crate::flow::FlowView>>,
    },
    RunFlow {
        /// Authored stage-model graph. `Some` replaces the session's flow
        /// before running (set-and-run in one step); `None` runs the flow
        /// already stored for this session.
        flow: Option<crate::workflow::FlowGraph>,
        options: crate::flow::FlowOptions,
        reply: oneshot::Sender<EngineResult<crate::workflow::FlowRunReport>>,
        /// Fires when the caller drops the reply receiver; aborts the flow
        /// run instead of leaving instances executing unowned.
        cancel_token: CancellationToken,
    },
    RunDagShell {
        script: String,
        dry_run: bool,
        timeout_ms: Option<u64>,
        reply: oneshot::Sender<EngineResult<DagShellOutcome>>,
    },
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
    DeleteEdge {
        from: String,
        from_port: u8,
        to: String,
        to_port: u8,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    /// Read artifact bytes for one output path (`vfs://` URI resolved through
    /// the mounted object storage, absolute host path otherwise). Lets the
    /// tool layer render payload files (e.g. `evidence` artifacts) inline
    /// without its own storage handle. Size-capped on the engine side.
    ReadFile {
        path: String,
        reply: oneshot::Sender<EngineResult<Vec<u8>>>,
    },
    /// Query a node's runtime status. Returns `None` if the DAG has never
    /// been run (no status entry exists for the node).
    GetNodeStatus {
        id: String,
        reply: oneshot::Sender<EngineResult<Option<RuntimeStatus>>>,
    },
    /// Query an existing node instance's retained `(kind, spec)`. Returns
    /// `None` if `id` does not exist or the node has no retained spec (added
    /// via the raw `add_node` path).
    GetNode {
        id: String,
        reply: oneshot::Sender<EngineResult<Option<(String, serde_json::Value)>>>,
    },
    /// Whether a node with the given id exists in the flow (regardless of
    /// whether it has a retained spec).
    NodeExists {
        id: String,
        reply: oneshot::Sender<EngineResult<bool>>,
    },
    RemoveNode {
        id: String,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    /// Return the structured topology/runtime snapshot used by TUI rendering.
    GetDagTuiSnapshot {
        reply: oneshot::Sender<EngineResult<DagTuiSnapshot>>,
    },
    /// Query the execution audit trail: recent runs, or one run by id.
    DagRunsLog {
        ref_name: Option<String>,
        limit: usize,
        /// When set, fetch this single run (with its full run report)
        /// instead of listing.
        run_id: Option<String>,
        reply: oneshot::Sender<EngineResult<Vec<crate::dag::RunRecord>>>,
    },
    UpdateNode {
        id: String,
        spec: serde_json::Value,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    /// Stop this engine actor. Replies `false` when a DAG run is active.
    Shutdown {
        reply: oneshot::Sender<EngineResult<bool>>,
    },
}
