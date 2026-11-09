//! DAG workflow engine: graph model, and async scheduler.
//!
//! Node abstractions ([`DagNode`] trait, [`NodePorts`], [`NodeInput`]) live in
//! [`crate::node`] and are re-exported here for convenience so
//! existing `use dag_core::dag::{DagNode, ...}` paths keep working.
//!
//! - [`graph`] — the [`DAG`] struct, edges, topological sort, cycle detection.
//! - [`error`] — [`DagError`].
//! - [`runtime`] — the async readiness scheduler and [`RunReport`].

pub mod channel;
pub mod error;
pub mod execution;
pub mod export;
pub mod graph;
pub mod history;
pub mod live;
pub mod logical;
pub mod node_event;
pub mod physical;
pub mod port_outputs;
pub mod runtime;
pub mod status;
pub mod utils;
pub mod view;

// Re-export node abstractions from the node module for backward compatibility
// and so that dag internals (graph/, runtime.rs) can use `super::DagNode` etc.
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
    local_task_executor, slurm_backed_executor, sweep_dag_tasks, sweep_dag_tasks_watermark,
};
pub use export::{ExportFile, ExportFormat, ExportSummary, SkippedFile};
pub use graph::{DAG, DynamicNodeBuilder};
pub use history::{DagHistory, DagManifest, RunRecord, Snapshot};
pub use logical::{
    LogicalEdge, LogicalExecutionStrategy, LogicalGraph, LogicalNode, LogicalNodeDefinition,
};
pub use physical::{DynamicFanoutNode, GatherNode, PhysicalJobRef};
pub use port_outputs::PortOutputs;
pub use runtime::{
    InputBinding, InputHashing, LogicalJobError, LogicalRunSummary, NodeRunDetails, RunReport,
    SchedulerConfig, WaveRunSummary,
};
pub use status::RuntimeStatus;
pub use view::{DagEdgeView, DagNodeView, DagPortView, DagTuiSnapshot};
