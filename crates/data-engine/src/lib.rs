//! Data analysis engine — aggregates the DAG core with concrete node bundles.
//!
//! All infrastructure (DagNode trait, NodeRegistry, DAG scheduler)
//! lives in [`dag_core`]. This crate re-exports those modules under their
//! familiar `data_engine::dag`, etc. paths so that existing consumers need no
//! changes.

// Re-export core infrastructure from dag-core under the original paths.
pub use dag_core::dag;
pub use dag_core::dataset;
pub use dag_core::error;
// Node-level traits and types.
pub use dag_core::arrow_util;
pub use dag_core::node;
pub use dag_core::sink;
// Registry under its original `node_registry` path for backward compat.
pub use dag_core::registry as node_registry;
pub use dag_core::value;

pub mod data_bundles;
pub mod data_engine;
pub mod default_registry;
pub mod nodes;
pub mod runtime;

// Convenience re-exports (backward compat with existing `use data_engine::*`).
pub use dag_core::{
    DagNode, FileRef, NodeCtx, NodeFactory, NodeId, NodeInput, NodePorts, NodeRegistry, NodeValue,
};
