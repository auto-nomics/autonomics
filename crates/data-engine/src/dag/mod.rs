//! DAG workflow engine: graph model, and async scheduler.
//!
//! Node abstractions ([`DagNode`] trait, [`NodePorts`], [`NodeInput`]) live in
//! [`crate::nodes`] and are re-exported here for convenience so
//! existing `use crate::dag::{DagNode, ...}` paths keep working.
//!
//! - [`graph`] — the [`DAG`] struct, edges, topological sort, cycle detection.
//! - [`error`] — [`DagError`].
//! - [`runtime`] — the async readiness scheduler and [`RunReport`].

pub mod error;
pub mod graph;
pub mod history;
pub mod node_event;
pub mod runtime;
pub mod utils;

// Re-export node abstractions from the nodes module for backward compatibility
// and so that dag internals (graph.rs, runtime.rs) can use `super::DagNode` etc.
pub use crate::nodes::{DagNode, NodeId, NodeInput, NodePorts};

pub use error::DagError;
pub use graph::DAG;
pub use history::{DagHistory, DagManifest, Snapshot};
pub use runtime::{DirtyState, RunReport, RuntimeStatus, SchedulerConfig};
