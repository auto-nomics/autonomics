//! Node execution events flowing back to the scheduler over the single mpsc
//! that already drives [`super::graph::DAG::run`].
//!
//! The channel carries [`NodeEvent`]; [`NodeEventKind::Done`] is the only
//! authoritative terminal signal — the scheduler advances the ready queue on
//! it, exactly as it did when the channel element was a bare [`JobResult`].
//! `Status` / `Progress` / `Log` are best-effort observations a node emits
//! *mid-`execute`* via a [`NodeReporter`]; the scheduler forwards/stores them
//! but its correctness never depends on them (they may be dropped on a full
//! channel — see [`NodeReporter`]).

use serde::Serialize;
use tokio::sync::mpsc;

use crate::dag::DagError;
use crate::dag::NodeId;
use crate::dag::graph::PortOutputs;
use crate::dag::runtime::RuntimeStatus;

// NOTE: `NodeEvent`/`NodeEventKind` deliberately do NOT derive `Serialize` —
// `Done(JobResult)` carries a `DagError` (which wraps the non-Serializable
// `DataFusionError`). When we surface events to the agent we translate to a
// Serializable DTO, mirroring how `runtime::DagErrorReport` already bridges
// `DagError` for `NodeReport`.

/// Terminal outcome of a dispatched node — the only variant the scheduler
/// *acts* on. Lifted unchanged from its previous home in `graph.rs` so the
/// `Success`/`Failed` pattern matches there keep working.
#[derive(Debug)]
pub enum JobResult {
    Success {
        id: NodeId,
        outputs: PortOutputs,
        duration: std::time::Duration,
    },
    Failed {
        id: NodeId,
        error: DagError,
        duration: std::time::Duration,
    },
}

impl JobResult {
    /// The node id this result pertains to, regardless of variant.
    pub fn node_id(&self) -> &str {
        match self {
            JobResult::Success { id, .. } => id,
            JobResult::Failed { id, .. } => id,
        }
    }

    /// Elapsed wall-clock of the node's `execute()`.
    pub fn duration(&self) -> std::time::Duration {
        match self {
            JobResult::Success { duration, .. } | JobResult::Failed { duration, .. } => *duration,
        }
    }
}

/// Severity for a [`NodeEventKind::Log`] event. Mirrors `tracing::Level` but is
/// a small `Serialize` enum so the event stays dependency-light and agent-
/// surfaceable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// What a node is telling the world right now.
#[derive(Debug)]
pub enum NodeEventKind {
    /// A lifecycle transition the scheduler also tracks independently; useful
    /// as an explicit, timestamped observation for observers (e.g. a TUI).
    Status { status: RuntimeStatus },
    /// Incremental progress for long-running nodes (e.g. MiXeR "fitting block
    /// 3/100"). `current`/`total` are opaque counters chosen by the node.
    Progress { current: u64, total: u64 },
    /// A free-form log line.
    Log { level: EventLevel, message: String },
    /// Authoritative terminal result — drives the scheduler's ready-queue
    /// advancement. Sent by the dispatch wrapper after `execute` returns, not
    /// by the node itself.
    Done(JobResult),
    /// Lightweight terminal observation for EXTERNAL observers only: carries
    /// the node's final status and elapsed time but **no output DataFrames**
    /// (unlike [`Done`], which holds `PortOutputs` for routing). The scheduler
    /// emits this to the optional external event sink in [`super::graph::DAG::run`];
    /// it never flows through the internal scheduling channel.
    Finished {
        status: RuntimeStatus,
        elapsed_ms: u64,
    },
}

/// One observation from one node.
#[derive(Debug)]
pub struct NodeEvent {
    pub node_id: NodeId,
    pub kind: NodeEventKind,
}

impl NodeEvent {
    pub fn new(node_id: impl Into<NodeId>, kind: NodeEventKind) -> Self {
        Self {
            node_id: node_id.into(),
            kind,
        }
    }
}

/// Node-facing handle for emitting mid-`execute` observations.
///
/// Cloneable and `Send` so the scheduler can hand a copy into the spawned
/// `'static` task alongside the cloned node. Ephemeral events (`Status` /
/// `Progress` / `Log`) use `try_send`: if the channel is saturated they are
/// **dropped silently** — losing a log line is acceptable, and this prevents a
/// chatty node from back-pressuring or deadlocking the run. The authoritative
/// `Done` is never sent through here.
#[derive(Clone)]
pub struct NodeReporter {
    node_id: NodeId,
    tx: mpsc::Sender<NodeEvent>,
}

impl NodeReporter {
    /// Bind a reporter to `node_id`; every emission is tagged with it so the
    /// node never has to repeat its own id.
    pub fn new(node_id: impl Into<NodeId>, tx: mpsc::Sender<NodeEvent>) -> Self {
        Self {
            node_id: node_id.into(),
            tx,
        }
    }

    /// A reporter whose emissions go nowhere — for tests / dry-runs that invoke
    /// [`super::graph::DAG::run`] or `DagNode::execute` directly, outside the
    /// scheduler. Emissions are silently dropped (the channel is buffered and
    /// unread).
    pub fn noop() -> Self {
        let (tx, _rx) = mpsc::channel::<NodeEvent>(1);
        // `_rx` drops here; `try_send` then sees no receiver and the emission
        // is silently dropped — exactly the "goes nowhere" semantics we want.
        Self::new("noop", tx)
    }

    #[inline]
    fn emit(&self, kind: NodeEventKind) {
        // try_send: fire-and-forget. A full/closed channel just drops the
        // observation — never blocks the node, never panics.
        let _ = self.tx.try_send(NodeEvent::new(self.node_id.clone(), kind));
    }

    pub fn status(&self, status: RuntimeStatus) {
        self.emit(NodeEventKind::Status { status });
    }

    pub fn progress(&self, current: u64, total: u64) {
        self.emit(NodeEventKind::Progress { current, total });
    }

    pub fn log(&self, level: EventLevel, message: impl Into<String>) {
        self.emit(NodeEventKind::Log {
            level,
            message: message.into(),
        });
    }

    pub fn info(&self, message: impl Into<String>) {
        self.log(EventLevel::Info, message);
    }

    pub fn warn(&self, message: impl Into<String>) {
        self.log(EventLevel::Warn, message);
    }

    pub fn error(&self, message: impl Into<String>) {
        self.log(EventLevel::Error, message);
    }
}
