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
pub mod logical;
pub mod node_event;
pub mod physical;
pub mod runtime;
pub mod utils;
pub mod view;

// Re-export node abstractions from the node module for backward compatibility
// and so that dag internals (graph.rs, runtime.rs) can use `super::DagNode` etc.
pub use crate::node::{DagNode, NodeId, NodeInput, NodePorts};

pub use channel::{ChannelBranch, ChannelNode, ChannelOperator};
pub use error::{DagError, NodeError};
pub use execution::{
    LocalTaskExecutor, TaskAttemptReceipt, TaskDispatch, TaskExecution, TaskExecutor,
    TaskInputBinding, TaskInputSource, TaskOutputBinding, TaskResources, TaskSpec, TaskSubmission,
    local_task_executor,
};
pub use export::{ExportFile, ExportFormat, ExportSummary, SkippedFile};
pub use graph::{DAG, DynamicNodeBuilder};
pub use history::{DagHistory, DagManifest, RunRecord, Snapshot};
pub use logical::{
    LogicalEdge, LogicalExecutionStrategy, LogicalGraph, LogicalGraphBuilder, LogicalNode,
    LogicalNodeDefinition,
};
pub use physical::{
    DynamicFanoutNode, GatherNode, PhysicalEdge, PhysicalGraph, PhysicalInstallReport,
    PhysicalJobRef, PhysicalNode,
};
pub use runtime::{
    InputBinding, InputHashing, LogicalJobError, LogicalRunSummary, NodeRunDetails, RunReport,
    RuntimeStatus, SchedulerConfig,
};
pub use view::{DagEdgeView, DagNodeView, DagPortView, DagTuiSnapshot};
