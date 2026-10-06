use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::dag::graph::PortOutputs;
use crate::dag::node_event::NodeEvent;
use crate::dag::runtime::RuntimeStatus;
use crate::dag::{DagTuiSnapshot, InputHashing, LogicalGraph, RunReport};
use crate::dag_shell::DagShellOutcome;
use crate::data_engine::{ClearDagOutcome, LogicalInstallReport};
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

/// Per-call scheduler controls for `run_dag`.
#[derive(Clone, Copy, Debug, Default)]
pub struct RunDagOptions {
    /// `Some(true)` enables fingerprint-based reuse of successful nodes.
    /// `None` preserves the engine's current scheduler configuration.
    pub resume: Option<bool>,
    /// Override input identity depth for this and subsequent runs on this
    /// engine. `None` preserves the current setting.
    pub input_hashing: Option<InputHashing>,
    /// Fixed barrier size for in-flight assay waves. `None` preserves the
    /// scheduler's unbarriered behavior.
    pub wave_size: Option<usize>,
}

pub enum DataEngineCmd {
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
    AddLogicalGraph {
        graph: LogicalGraph,
        reply: oneshot::Sender<EngineResult<LogicalInstallReport>>,
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
    RunDag {
        /// Optional sink for streaming lightweight per-node events (status /
        /// progress / log / finished) out of the actor as the run progresses.
        /// `None` for the non-streaming path.
        event_tx: Option<mpsc::Sender<NodeEvent>>,
        /// Commit message for the history snapshot. If `None`, a default is used.
        commit_message: Option<String>,
        /// Who initiated the run (e.g. `"agent:/root/researcher"`), recorded
        /// in the run's audit trail. `None` leaves the run unattributed.
        trigger: Option<String>,
        options: RunDagOptions,
        reply: oneshot::Sender<EngineResult<RunReport>>,
        /// Cancellation token shared with the caller. When the caller drops
        /// the reply receiver (e.g. the agent task is cancelled), this token
        /// fires and the spawned DAG run task aborts early — releasing the
        /// engine mutex and resetting the `running` flag instead of leaving
        /// the session stuck.
        cancel_token: CancellationToken,
    },
    GetOutput {
        id: String,
        reply: oneshot::Sender<EngineResult<Option<PortOutputs>>>,
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
    /// Whether a node with the given id exists in the DAG (regardless of
    /// whether it has a retained spec).
    NodeExists {
        id: String,
        reply: oneshot::Sender<EngineResult<bool>>,
    },
    RemoveNode {
        id: String,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    ViewDag {
        reply: oneshot::Sender<EngineResult<String>>,
    },
    /// Return the structured topology/runtime snapshot used by TUI rendering.
    GetDagTuiSnapshot {
        reply: oneshot::Sender<EngineResult<DagTuiSnapshot>>,
    },
    ClearDag {
        reply: oneshot::Sender<EngineResult<ClearDagOutcome>>,
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
    /// Query the execution audit trail: recent runs, or one run by id.
    DagRunsLog {
        ref_name: Option<String>,
        limit: usize,
        /// When set, fetch this single run (with its full run report)
        /// instead of listing.
        run_id: Option<String>,
        reply: oneshot::Sender<EngineResult<Vec<crate::dag::RunRecord>>>,
    },
    /// Export one recorded run as provenance evidence (PROV-JSON / RO-Crate).
    ExportRun {
        /// Run id (unique prefix accepted).
        run_id: String,
        /// `prov` or `crate`.
        format: String,
        /// Absolute output directory.
        out_dir: std::path::PathBuf,
        reply: oneshot::Sender<EngineResult<crate::dag::ExportSummary>>,
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
}
