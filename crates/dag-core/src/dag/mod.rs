//! DAG engine primitives: node abstractions, the executor stack, channels,
//! history, and live views.
//!
//! Node abstractions ([`DagNode`] trait, [`NodePorts`], [`NodeInput`]) live in
//! [`crate::node`] and are re-exported here for convenience so
//! existing `use dag_core::dag::{DagNode, ...}` paths keep working.
//!
//! - [`execution`] — task executor contracts and the local/remote/Slurm backends.
//! - [`channel`] — the [`ChannelNode`] dataflow operators consumed by the flow engine.
//! - [`history`] — run-audit records and flow workspace/cache persistence.
//! - [`live`] — the run-time projection consumed by TUI/GUI views.
//! - [`error`] — [`DagError`].

pub mod channel;
pub mod error;
pub mod execution;
pub mod history;
pub mod live;
pub mod node_event;
pub mod port_outputs;
pub mod runtime;
pub mod status;
pub mod view;

/// Compatibility path for `PortOutputs`. The legacy node-graph module that
/// owned this name was removed; the type itself is the flow engine's
/// per-port output map and stays at `dag::port_outputs`.
pub mod graph {
    pub use super::port_outputs::PortOutputs;
}

// Re-export node abstractions from the node module for backward compatibility
// and so that dag internals can use `super::DagNode` etc.
pub use crate::node::{DagNode, NodeId, NodeInput, NodePorts};

pub use channel::{ChannelBranch, ChannelNode, ChannelOperator};
pub use error::{DagError, NodeError};
pub use execution::{
    ApptainerContainer, ApptainerMode, ApptainerNetwork, ApptainerSettings, DagTaskGcPolicy,
    DagTaskGcReport, DagTaskWatermarkReport, LocalDirectoryArtifactStore, LocalTaskExecutor,
    PlacementTaskExecutor, ProcessRoutingTaskExecutor, ProcessTaskOutput, ProcessTaskSpec,
    ProcessTaskTransport, RemoteTaskExecutor, SifImage, SlurmRecordedSubmission, SlurmSettings,
    SlurmSubmissionIntent, SlurmSubmissionObservation, SlurmSubmissionRecorder, SlurmTaskTransport,
    SshArtifactStore, SshDataHome, SshSettings, SshTarget, SshTaskTransport, TaskArtifactStore,
    TaskAttemptReceipt, TaskDispatch, TaskExecution, TaskExecutor, TaskInputBinding,
    TaskInputSource, TaskLease, TaskOutputBinding, TaskResources, TaskSpec, TaskSubmission,
    TaskTransport, TypedPodmanResourceControl, TypedPodmanSettings, TypedPodmanUserNamespace,
    TypedRemoteTaskExecutor, TypedSlurmSwapPolicy, TypedWorkerEnvironment, TypedWorkerSettings,
    dag_task_gc_age, dag_task_gc_interval, dag_task_gc_max_bytes, dag_tasks_root,
    dag_tasks_root_for_state_dir, local_task_executor, slurm_backed_executor, sweep_dag_tasks,
    sweep_dag_tasks_watermark,
};
pub use history::{DagHistory, RunRecord};
pub use port_outputs::PortOutputs;
pub use runtime::{DagErrorReport, NodeRunDetails};
pub use status::RuntimeStatus;
pub use view::{DagEdgeView, DagNodeView, DagPortView, DagTuiSnapshot};
