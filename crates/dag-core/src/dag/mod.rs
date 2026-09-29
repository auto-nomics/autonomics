//! DAG workflow engine: graph model, and async scheduler.
//!
//! Node abstractions ([`DagNode`] trait, [`NodePorts`], [`NodeInput`]) live in
//! [`crate::node`] and are re-exported here for convenience so
//! existing `use dag_core::dag::{DagNode, ...}` paths keep working.
//!
//! - [`graph`] — the [`DAG`] struct, edges, topological sort, cycle detection.
//! - [`error`] — [`DagError`].
//! - [`runtime`] — the async readiness scheduler and [`RunReport`].

pub mod error;
pub mod export;
pub mod graph;
pub mod history;
pub mod node_event;
pub mod runtime;
pub mod utils;
pub mod view;

// Re-export node abstractions from the node module for backward compatibility
// and so that dag internals (graph.rs, runtime.rs) can use `super::DagNode` etc.
pub use crate::node::{DagNode, NodeId, NodeInput, NodePorts};

pub use error::{DagError, NodeError};
pub use export::{ExportFile, ExportFormat, ExportSummary, SkippedFile};
pub use graph::DAG;
pub use history::{DagHistory, DagManifest, RunRecord, Snapshot};
pub use runtime::{
    InputBinding, InputHashing, NodeRunDetails, RunReport, RuntimeStatus, SchedulerConfig,
};
pub use view::{DagEdgeView, DagNodeView, DagPortView, DagTuiSnapshot};
